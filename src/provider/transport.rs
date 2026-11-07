use super::*;
use super::{credentials::Secret, protocols::ProtocolAdapter, sse::SseParser};
use crate::agent::*;
use async_trait::async_trait;
use futures_util::StreamExt;
use std::{sync::Arc, time::Duration};
use tokio::time::{timeout_at, Instant};
use tokio_util::sync::CancellationToken;
const RESPONSE_LIMIT: usize = 32 * 1024 * 1024;
const ERROR_LIMIT: usize = 64 * 1024;

/// HTTP boundary. Implementations must not retry or redirect a request implicitly.
#[async_trait]
pub trait HttpTransport: Send + Sync {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, ProviderError>;
}
pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Arc<[u8]>,
}
impl std::fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpRequest")
            .field("body_bytes", &self.body.len())
            .finish_non_exhaustive()
    }
}
pub type HttpBody =
    std::pin::Pin<Box<dyn futures_core::Stream<Item = Result<Vec<u8>, ProviderError>> + Send>>;
pub struct HttpResponse {
    pub status: u16,
    pub headers: std::collections::BTreeMap<String, String>,
    pub body: HttpBody,
}
pub struct ReqwestTransport {
    client: reqwest::Client,
}
impl ReqwestTransport {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}
#[async_trait]
impl HttpTransport for ReqwestTransport {
    async fn send(&self, r: HttpRequest) -> Result<HttpResponse, ProviderError> {
        let mut request = self.client.post(&r.url);
        for (k, v) in r.headers {
            request = request.header(k, v);
        }
        let mut response = request.body(r.body.to_vec()).send().await.map_err(|e| {
            let mut err = ProviderError::new(
                if e.is_timeout() {
                    ProviderErrorKind::Timeout
                } else {
                    ProviderErrorKind::Transport
                },
                "model HTTP request failed",
            );
            err.delivery = if e.is_connect() {
                DeliveryState::NotSent
            } else {
                DeliveryState::Unknown
            };
            err
        })?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(k, v)| {
                v.to_str()
                    .ok()
                    .map(|v| (k.as_str().to_owned(), v.to_owned()))
            })
            .collect();
        let body = Box::pin(
            async_stream::stream! {loop {match response.chunk().await {Ok(Some(b))=>yield Ok(b.to_vec()),Ok(None)=>break,Err(_)=>{yield Err(ProviderError::new(ProviderErrorKind::Transport,"response stream disconnected"));break;}}}},
        );
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

pub struct HttpModelProvider {
    descriptor: ResolvedModelDescriptor,
    endpoint: String,
    secret: Option<Secret>,
    transport: TransportConfig,
    adapter: Arc<dyn ProtocolAdapter>,
    http: Arc<dyn HttpTransport>,
    instance_id: String,
}
impl std::fmt::Debug for HttpModelProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpModelProvider")
            .field("descriptor", &self.descriptor)
            .finish_non_exhaustive()
    }
}
impl HttpModelProvider {
    pub(crate) fn new(
        d: ResolvedModelDescriptor,
        endpoint: String,
        secret: Option<Secret>,
        t: TransportConfig,
        o: ProtocolOptions,
        http: Arc<dyn HttpTransport>,
    ) -> Result<Self, ProviderError> {
        let adapter = super::protocols::adapter(d.protocol, t.mode, o)?;
        Ok(Self {
            descriptor: d,
            endpoint,
            secret,
            transport: t,
            adapter: Arc::from(adapter),
            http,
            instance_id: uuid::Uuid::now_v7().to_string(),
        })
    }
}
fn timeout_error() -> ProviderError {
    ProviderError::new(ProviderErrorKind::Timeout, "model request timed out")
}
fn accepted(mut e: ProviderError, id: &Option<String>, usage: TokenUsage) -> ProviderError {
    e.delivery = DeliveryState::Accepted;
    e.request_id = id.clone();
    if usage != TokenUsage::default() {
        e.observed_usage = Some(Box::new(usage));
    }
    e
}
fn identifier(value: Option<&str>, secret: &Option<Secret>) -> Option<String> {
    value
        .filter(|s| {
            s.len() <= 200
                && s.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-.:".contains(&c))
                && !secret.as_ref().is_some_and(|k| s.contains(&k.0))
        })
        .map(str::to_owned)
}
fn stamp(
    response: &mut ModelResponse,
    id: &Option<String>,
    secret: &Option<Secret>,
    started: Instant,
    first_content_ms: Option<u64>,
) {
    response.metrics = ModelMetrics {
        first_content_ms,
        elapsed_ms: started.elapsed().as_millis() as u64,
    };
    response.request_id = id.clone();
    response.response_id = identifier(response.response_id.as_deref(), secret);
    if let Some(c) = &mut response.message.completion {
        c.request_id = id.clone();
        c.response_id = response.response_id.clone();
    }
}

#[async_trait]
impl ModelProvider for HttpModelProvider {
    fn descriptor(&self) -> &ResolvedModelDescriptor {
        &self.descriptor
    }
    fn capabilities(&self) -> ModelCapabilities {
        self.descriptor.capabilities.clone()
    }
    fn configuration_key(&self) -> String {
        self.descriptor.configuration_key.clone()
    }
    fn prepare(&self, r: ModelRequest) -> Result<PreparedRequest, ProviderError> {
        let encoded = self.adapter.encode(&self.descriptor, &r)?;
        let tokens = encoded
            .body
            .len()
            .saturating_add(r.messages.len() * 32)
            .saturating_add(128);
        Ok(PreparedRequest {
            request: r,
            body: encoded.body.into(),
            semantic_headers: encoded.headers,
            provider_key: self.configuration_key(),
            instance_id: Some(self.instance_id.clone()),
            estimate: TokenEstimate {
                tokens,
                method: "byte_estimate".into(),
                exact: false,
            },
        })
    }
    async fn stream_prepared(
        &self,
        r: PreparedRequest,
        cancel: CancellationToken,
    ) -> Result<ModelEventStream, ProviderError> {
        if cancel.is_cancelled() {
            return Err(ProviderError::cancelled());
        }
        if r.provider_key != self.configuration_key()
            || r.instance_id.as_deref() != Some(&self.instance_id)
        {
            return Err(ProviderError::protocol(
                "prepared request belongs to another provider instance",
            ));
        }
        let started = Instant::now();
        let deadline = started + Duration::from_millis(self.transport.request_timeout_ms);
        let first_deadline =
            (started + Duration::from_millis(self.transport.first_byte_timeout_ms)).min(deadline);
        let mut headers = vec![
            ("content-type".into(), "application/json".into()),
            (
                "accept".into(),
                if self.transport.mode == TransportMode::Sse {
                    "text/event-stream"
                } else {
                    "application/json"
                }
                .into(),
            ),
        ];
        headers.extend(r.semantic_headers);
        if let Some(key) = &self.secret {
            headers.push(match self.descriptor.protocol {
                ProtocolKind::AnthropicMessages => ("x-api-key".into(), key.0.clone()),
                _ => ("authorization".into(), format!("Bearer {}", key.0)),
            });
        }
        let request = HttpRequest {
            url: self.endpoint.clone(),
            headers,
            body: r.body,
        };
        let response = tokio::select! {biased;_=cancel.cancelled()=>return Err(ProviderError::cancelled()),result=timeout_at(first_deadline,self.http.send(request))=>match result {Err(_)=>{let mut e=timeout_error();e.delivery=DeliveryState::Unknown;return Err(e);},Ok(result)=>result?}};
        let request_id = identifier(
            response
                .headers
                .get("x-request-id")
                .or_else(|| response.headers.get("request-id"))
                .map(String::as_str),
            &self.secret,
        );
        let status = reqwest::StatusCode::from_u16(response.status)
            .map_err(|_| ProviderError::protocol("invalid HTTP status"))?;
        let retry_after = response.headers.get("retry-after").and_then(|v| {
            v.parse::<u64>().ok().or_else(|| {
                chrono::DateTime::parse_from_rfc2822(v)
                    .ok()
                    .map(|t| (t.timestamp() - chrono::Utc::now().timestamp()).max(0) as u64)
            })
        });
        let content_type = response
            .headers
            .get("content-type")
            .map(String::as_str)
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let mut bytes = response.body;
        let t = self.transport.clone();
        let secret = self.secret.clone();
        if !status.is_success() {
            let mut body = vec![];
            let mut seen = false;
            while body.len() < ERROR_LIMIT {
                let until = if seen {
                    (Instant::now() + Duration::from_millis(t.idle_timeout_ms)).min(deadline)
                } else {
                    first_deadline
                };
                let next = tokio::select! {_=cancel.cancelled()=>return Err(ProviderError::cancelled()),r=timeout_at(until,bytes.next())=>r};
                match next {
                    Ok(Some(Ok(b))) => {
                        seen = true;
                        body.extend_from_slice(&b[..b.len().min(ERROR_LIMIT - body.len())]);
                    }
                    _ => break,
                }
            }
            let value: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
            let code = identifier(
                value["error"]["code"]
                    .as_str()
                    .or_else(|| value["error"]["type"].as_str()),
                &secret,
            );
            let kind = match status.as_u16() {
                401 => ProviderErrorKind::Authentication,
                403 => ProviderErrorKind::Permission,
                429 => ProviderErrorKind::RateLimited,
                413 => ProviderErrorKind::ContextExceeded,
                400 if code.as_ref().is_some_and(|s| {
                    s.contains("context_length") || s.contains("context_window")
                }) =>
                {
                    ProviderErrorKind::ContextExceeded
                }
                400..=499 => ProviderErrorKind::InvalidRequest,
                _ => ProviderErrorKind::Upstream,
            };
            let mut err = ProviderError::new(kind, "model endpoint rejected the request");
            err.status = Some(status.as_u16());
            err.code = code;
            err.retry_after = retry_after;
            err.request_id = request_id;
            err.delivery = if status.is_client_error() {
                DeliveryState::Rejected
            } else {
                DeliveryState::Unknown
            };
            return Err(err);
        }
        let expected = if t.mode == TransportMode::Sse {
            "text/event-stream"
        } else {
            "application/json"
        };
        if content_type != expected {
            return Err(accepted(
                ProviderError::protocol("unexpected model response content type"),
                &request_id,
                TokenUsage::default(),
            ));
        }
        let mut decoder = self.adapter.decoder(&self.descriptor);
        let adapter = self.adapter.clone();
        let descriptor = self.descriptor.clone();
        Ok(Box::pin(async_stream::stream! {
            if cancel.is_cancelled() {
                yield Err(accepted(ProviderError::cancelled(), &request_id, decoder.usage()));
                return;
            }
            yield Ok(ModelEvent::Started);
            let mut total=0usize;
            let mut seen=false;
            let mut first_content=None;
            let mut parser=SseParser::default();
            let mut json_body=vec![];
            loop {
                if Instant::now()>=deadline {
                    yield Err(accepted(timeout_error(), &request_id, decoder.usage()));
                    return;
                }
                let until=if seen {
                    (Instant::now()+Duration::from_millis(t.idle_timeout_ms)).min(deadline)
                } else { first_deadline };
                let item=tokio::select! {
                    biased;
                    _=cancel.cancelled()=>{
                        yield Err(accepted(ProviderError::cancelled(), &request_id, decoder.usage()));
                        return;
                    },
                    next=timeout_at(until,bytes.next())=>match next {
                        Err(_)=>{yield Err(accepted(timeout_error(), &request_id, decoder.usage()));return;},
                        Ok(next)=>next,
                    }
                };
                match item {
                    Some(Ok(chunk))=>{
                        seen=true;
                        total=total.saturating_add(chunk.len());
                        if total>RESPONSE_LIMIT {
                            yield Err(accepted(ProviderError::protocol("response exceeds size limit"), &request_id, decoder.usage()));
                            return;
                        }
                        if t.mode==TransportMode::Json {json_body.extend_from_slice(&chunk);continue;}
                        let frames=match parser.push(&chunk) {
                            Ok(v)=>v,
                            Err(e)=>{yield Err(accepted(e, &request_id, decoder.usage()));return;}
                        };
                        for frame in frames {
                            let events=match decoder.push(frame) {
                                Ok(v)=>v,
                                Err(e)=>{yield Err(accepted(e, &request_id, decoder.usage()));return;}
                            };
                            for mut event in events {
                                if cancel.is_cancelled() {
                                    yield Err(accepted(ProviderError::cancelled(), &request_id, decoder.usage()));
                                    return;
                                }
                                if Instant::now()>=deadline {
                                    yield Err(accepted(timeout_error(), &request_id, decoder.usage()));
                                    return;
                                }
                                if first_content.is_none() && (matches!(&event,ModelEvent::TextDelta(s) if !s.is_empty()) || matches!(event,ModelEvent::ToolCallStarted{..})) {
                                    first_content=Some(started.elapsed().as_millis() as u64);
                                }
                                if let ModelEvent::Completed(response)=&mut event {
                                    stamp(response,&request_id,&secret,started,first_content);
                                    yield Ok(event);
                                    return;
                                }
                                yield Ok(event);
                            }
                        }
                    },
                    Some(Err(_))=>{
                        yield Err(accepted(ProviderError::new(ProviderErrorKind::Transport,"response stream disconnected"), &request_id, decoder.usage()));
                        return;
                    },
                    None=>{
                        if t.mode==TransportMode::Json {
                            match adapter.decode_json(&descriptor,&json_body) {
                                Ok(mut response)=>{
                                    stamp(&mut response,&request_id,&secret,started,Some(started.elapsed().as_millis() as u64));
                                    yield Ok(ModelEvent::completed(response));
                                },
                                Err(e)=>{
                                    let value:serde_json::Value=serde_json::from_slice(&json_body).unwrap_or_default();
                                    let usage=match descriptor.protocol {
                                        ProtocolKind::AnthropicMessages=>super::usage::anthropic(&value["usage"]),
                                        p=>super::usage::openai(&value["usage"],p==ProtocolKind::OpenaiChat)
                                    };
                                    yield Err(accepted(e, &request_id, usage));
                                }
                            }
                        } else if let Err(e)=parser.finish().and_then(|_|decoder.finish()) {
                            yield Err(accepted(e, &request_id, decoder.usage()));
                        }
                        return;
                    }
                }
            }
        }))
    }
    async fn stream(
        &self,
        r: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelEventStream, ProviderError> {
        self.stream_prepared(self.prepare(r)?, cancel).await
    }
}
