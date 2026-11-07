use super::*;
use async_trait::async_trait;
use futures_util::stream;
use reqwest::Client;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug)]
pub enum ApiFormat {
    OpenAi,
    Anthropic,
}
#[derive(Clone)]
pub struct HttpProvider {
    client: Client,
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    pub format: ApiFormat,
    pub cache_enabled: bool,
    capacity: ModelCapabilities,
}
impl HttpProvider {
    pub fn openai(
        endpoint: impl Into<String>,
        key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            client: Client::new(),
            endpoint: endpoint.into(),
            api_key: key.into(),
            model: model.into(),
            format: ApiFormat::OpenAi,
            cache_enabled: true,
            capacity: ModelCapabilities::default(),
        }
    }
    pub fn anthropic(
        endpoint: impl Into<String>,
        key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            format: ApiFormat::Anthropic,
            ..Self::openai(endpoint, key, model)
        }
    }
    pub fn with_capabilities(mut self, capacity: ModelCapabilities) -> Self {
        self.capacity = capacity;
        self
    }
    fn encode(&self, request: &ModelRequest) -> Result<Value, AgentError> {
        if request
            .messages
            .iter()
            .any(|m| matches!(m, AgentMessage::Custom { .. }))
        {
            return Err(AgentError::Model("unprojected Custom message".into()));
        }
        let output = request
            .max_output_tokens
            .min(self.capacity.max_output_tokens);
        Ok(match self.format {
            ApiFormat::OpenAi => {
                let messages: Vec<Value> = request.messages.iter().map(|m| match m {
                    AgentMessage::System(s) => json!({"role":"system","content":s}),
                    AgentMessage::User(c) => json!({"role":"user","content":text(c)}),
                    AgentMessage::Assistant(a) => {
                        let mut item = json!({"role":"assistant","content":text(&a.content)});
                        if !a.tool_calls.is_empty() { item["tool_calls"] = json!(a.tool_calls.iter().map(|c| json!({"id":c.id,"type":"function","function":{"name":c.name,"arguments":c.arguments.to_string()}})).collect::<Vec<_>>()); }
                        item
                    }
                    AgentMessage::ToolResult(r) => json!({"role":"tool","tool_call_id":r.tool_call_id,"content":text(&r.content)}),
                    AgentMessage::Custom { .. } => unreachable!(),
                }).collect();
                let mut body = json!({"model":self.model,"messages":messages,"max_tokens":output});
                if !request.tools.is_empty() {
                    body["tools"] = json!(request.tools.iter().map(|t| json!({"type":"function","function":{"name":t.name,"description":t.description,"parameters":t.input_schema}})).collect::<Vec<_>>());
                }
                body
            }
            ApiFormat::Anthropic => {
                let system = request
                    .messages
                    .iter()
                    .filter_map(|m| {
                        if let AgentMessage::System(s) = m {
                            Some(s.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let mut messages: Vec<Value> = vec![];
                for message in &request.messages {
                    let (role, content) = match message {
                        AgentMessage::System(_) => continue,
                        AgentMessage::User(c) => {
                            ("user", vec![json!({"type":"text","text":text(c)})])
                        }
                        AgentMessage::Assistant(a) => {
                            let mut blocks = vec![];
                            if !text(&a.content).is_empty() {
                                blocks.push(json!({"type":"text","text":text(&a.content)}));
                            }
                            blocks.extend(a.tool_calls.iter().map(|c| json!({"type":"tool_use","id":c.id,"name":c.name,"input":c.arguments})));
                            if blocks.is_empty() {
                                blocks.push(json!({"type":"text","text":" "}));
                            }
                            ("assistant", blocks)
                        }
                        AgentMessage::ToolResult(r) => (
                            "user",
                            vec![
                                json!({"type":"tool_result","tool_use_id":r.tool_call_id,"content":text(&r.content),"is_error":r.is_error}),
                            ],
                        ),
                        AgentMessage::Custom { .. } => unreachable!(),
                    };
                    if let Some(previous) = messages.last_mut().filter(|p| p["role"] == role) {
                        previous["content"].as_array_mut().unwrap().extend(content);
                    } else {
                        messages.push(json!({"role":role,"content":content}));
                    }
                }
                let mut body = json!({"model":self.model,"max_tokens":output,"messages":messages});
                if !system.is_empty() {
                    body["system"] = json!(system);
                }
                if !request.tools.is_empty() {
                    body["tools"] = json!(request.tools.iter().map(|t| json!({"name":t.name,"description":t.description,"input_schema":t.input_schema})).collect::<Vec<_>>());
                }
                if self.cache_enabled {
                    body["cache_control"] = json!({"type":"ephemeral"});
                }
                body
            }
        })
    }
    fn decode(&self, value: Value) -> Result<ModelEventStream, AgentError> {
        let mut tool_calls = vec![];
        let mut content = vec![];
        let usage = match self.format {
            ApiFormat::OpenAi => {
                let message = value["choices"][0]
                    .get("message")
                    .ok_or_else(|| AgentError::Model("response missing message".into()))?;
                if let Some(text) = message["content"].as_str() {
                    content.push(ContentPart::Text(text.into()));
                }
                if let Some(calls) = message["tool_calls"].as_array() {
                    if value["choices"][0]["finish_reason"] == "length" {
                        return Err(AgentError::Model("truncated tool call".into()));
                    }
                    for call in calls {
                        tool_calls.push(ToolCall {
                            id: required_str(call, "id")?,
                            name: required_str(&call["function"], "name")?,
                            arguments: serde_json::from_str(&required_str(
                                &call["function"],
                                "arguments",
                            )?)
                            .map_err(|e| {
                                AgentError::Model(format!("invalid tool arguments: {e}"))
                            })?,
                        });
                    }
                }
                TokenUsage {
                    input_tokens: value["usage"]["prompt_tokens"].as_u64(),
                    output_tokens: value["usage"]["completion_tokens"].as_u64(),
                    cached_tokens: value["usage"]["prompt_tokens_details"]["cached_tokens"]
                        .as_u64(),
                    cache_write_tokens: value["usage"]["prompt_tokens_details"]
                        ["cache_write_tokens"]
                        .as_u64(),
                }
            }
            ApiFormat::Anthropic => {
                let blocks = value["content"]
                    .as_array()
                    .ok_or_else(|| AgentError::Model("response missing content blocks".into()))?;
                for block in blocks {
                    match block["type"].as_str() {
                        Some("text") => {
                            content.push(ContentPart::Text(required_str(block, "text")?))
                        }
                        Some("tool_use") => {
                            if value["stop_reason"] == "max_tokens" {
                                return Err(AgentError::Model("truncated tool call".into()));
                            }
                            tool_calls.push(ToolCall {
                                id: required_str(block, "id")?,
                                name: required_str(block, "name")?,
                                arguments: block.get("input").cloned().ok_or_else(|| {
                                    AgentError::Model("missing tool input".into())
                                })?,
                            });
                        }
                        _ => (),
                    }
                }
                let u = &value["usage"];
                let input_tokens = u["input_tokens"].as_u64().map(|n| {
                    n + u["cache_read_input_tokens"].as_u64().unwrap_or(0)
                        + u["cache_creation_input_tokens"].as_u64().unwrap_or(0)
                });
                TokenUsage {
                    input_tokens,
                    output_tokens: u["output_tokens"].as_u64(),
                    cached_tokens: u["cache_read_input_tokens"].as_u64(),
                    cache_write_tokens: u["cache_creation_input_tokens"].as_u64(),
                }
            }
        };
        Ok(Box::pin(stream::iter(vec![
            ModelEvent::Start,
            ModelEvent::Usage(usage),
            ModelEvent::Complete(AssistantMessage {
                content,
                tool_calls,
            }),
        ])))
    }
}
fn required_str(value: &Value, key: &str) -> Result<String, AgentError> {
    value[key]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| AgentError::Model(format!("missing {key}")))
}
fn text(c: &[ContentPart]) -> String {
    c.iter().map(|ContentPart::Text(s)| s.as_str()).collect()
}
#[async_trait]
impl ModelProvider for HttpProvider {
    fn capabilities(&self) -> ModelCapabilities {
        self.capacity.clone()
    }
    fn configuration_key(&self) -> String {
        format!(
            "{:?}:{}:{}:{}:{}:{}",
            self.format,
            self.endpoint,
            self.model,
            self.cache_enabled,
            self.capacity.context_tokens,
            self.capacity.max_output_tokens
        )
    }
    fn prepare(&self, request: ModelRequest) -> Result<PreparedRequest, AgentError> {
        let payload = self.encode(&request)?;
        let input_tokens = payload
            .to_string()
            .len()
            .saturating_add(request.messages.len() * 32)
            .saturating_add(128);
        Ok(PreparedRequest {
            request,
            payload: Some(payload),
            input_tokens,
            provider_key: self.configuration_key(),
        })
    }
    async fn stream_prepared(
        &self,
        request: PreparedRequest,
        cancel: CancellationToken,
    ) -> Result<ModelEventStream, AgentError> {
        if request.provider_key != self.configuration_key() {
            return Err(AgentError::Model("prepared provider mismatch".into()));
        }
        let body = request
            .payload
            .ok_or_else(|| AgentError::Model("missing encoded request".into()))?;
        let response = match self.format {
            ApiFormat::OpenAi => self.client.post(&self.endpoint).bearer_auth(&self.api_key),
            ApiFormat::Anthropic => self
                .client
                .post(&self.endpoint)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01"),
        };
        let future = async {
            response
                .json(&body)
                .send()
                .await?
                .error_for_status()?
                .json::<Value>()
                .await
        };
        let value = tokio::select! { _ = cancel.cancelled() => return Err(AgentError::Aborted), response = future => response.map_err(|e| AgentError::Model(e.to_string()))? };
        self.decode(value)
    }
    async fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelEventStream, AgentError> {
        self.stream_prepared(self.prepare(request)?, cancel).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    fn request() -> ModelRequest {
        ModelRequest {
            messages: vec![
                AgentMessage::System("core".into()),
                AgentMessage::User(vec![ContentPart::Text("question".into())]),
                AgentMessage::Assistant(AssistantMessage {
                    content: vec![],
                    tool_calls: vec![ToolCall {
                        id: "id1".into(),
                        name: "lookup".into(),
                        arguments: json!({"q":"中文"}),
                    }],
                }),
                AgentMessage::ToolResult(ToolResultMessage {
                    tool_call_id: "id1".into(),
                    content: vec![ContentPart::Text("evidence".into())],
                    is_error: false,
                    terminate: false,
                }),
            ],
            tools: vec![ToolDeclaration {
                name: "lookup".into(),
                description: "lookup".into(),
                input_schema: json!({"type":"object"}),
            }],
            max_output_tokens: 123,
        }
    }
    #[test]
    fn prepared_payload_contains_correct_tools_limits_and_stable_encoded_history() {
        let openai = HttpProvider::openai("http://localhost", "credential", "model");
        let prepared = openai.prepare(request()).unwrap();
        let body = prepared.payload.as_ref().unwrap();
        assert_eq!(body["max_tokens"], 123);
        assert_eq!(
            body["messages"][2]["tool_calls"][0]["function"]["name"],
            "lookup"
        );
        assert_eq!(
            serde_json::from_str::<Value>(
                body["messages"][2]["tool_calls"][0]["function"]["arguments"]
                    .as_str()
                    .unwrap()
            )
            .unwrap(),
            json!({"q":"中文"})
        );
        assert!(!body.to_string().contains("credential"));
        assert!(prepared.estimated_input_tokens() >= body.to_string().len());
        let anthropic = HttpProvider::anthropic("http://localhost", "credential", "model");
        let prepared = anthropic.prepare(request()).unwrap();
        let body = prepared.payload.as_ref().unwrap();
        assert_eq!(body["tools"][0]["name"], "lookup");
        assert_eq!(body["messages"][1]["content"][0]["type"], "tool_use");
        assert_eq!(body["messages"][2]["content"][0]["tool_use_id"], "id1");
        assert_eq!(body["cache_control"]["type"], "ephemeral");
    }
    #[tokio::test]
    async fn usage_and_anthropic_multiple_text_and_tool_blocks_are_preserved() {
        let p = HttpProvider::anthropic("http://localhost", "key", "model");
        let mut stream=p.decode(json!({"content":[{"type":"text","text":"one"},{"type":"text","text":"two"},{"type":"tool_use","id":"call","name":"lookup","input":{"q":"x"}}],"stop_reason":"tool_use","usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":20,"cache_creation_input_tokens":3}})).unwrap();
        assert!(matches!(stream.next().await, Some(ModelEvent::Start)));
        assert!(matches!(
            stream.next().await,
            Some(ModelEvent::Usage(TokenUsage {
                input_tokens: Some(33),
                cached_tokens: Some(20),
                ..
            }))
        ));
        let Some(ModelEvent::Complete(a)) = stream.next().await else {
            panic!()
        };
        assert_eq!(text(&a.content), "onetwo");
        assert_eq!(a.tool_calls.len(), 1);
    }
    #[test]
    fn malformed_and_truncated_calls_are_errors_instead_of_silently_disappearing() {
        let p = HttpProvider::openai("http://localhost", "key", "model");
        assert!(p.decode(json!({"choices":[{"message":{"tool_calls":[{"id":"call","function":{"name":"lookup","arguments":"{incomplete"}}]}}]})).is_err());
        let p = HttpProvider::anthropic("http://localhost", "key", "model");
        assert!(p.decode(json!({"content":[{"type":"tool_use","id":"call","name":"lookup","input":{}}],"stop_reason":"max_tokens"})).is_err());
    }
}
