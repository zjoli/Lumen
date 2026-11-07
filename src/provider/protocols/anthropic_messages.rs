use super::*;
use std::collections::{BTreeMap, BTreeSet};
pub(super) struct Messages {
    pub mode: TransportMode,
    pub options: ProtocolOptions,
}
impl ProtocolAdapter for Messages {
    fn encode(
        &self,
        d: &ResolvedModelDescriptor,
        r: &ModelRequest,
    ) -> Result<EncodedRequest, ProviderError> {
        validate(d, r)?;
        let mut system = vec![];
        let mut messages: Vec<Value> = vec![];
        for m in &r.messages {
            let (role, content) = match m {
                AgentMessage::System(s) => {
                    system.push(json!({"type":"text","text":s}));
                    continue;
                }
                AgentMessage::User(c) => ("user", vec![json!({"type":"text","text":text(c)})]),
                AgentMessage::ToolResult(t) => (
                    "user",
                    vec![
                        json!({"type":"tool_result","tool_use_id":t.tool_call_id,"content":text(&t.content),"is_error":t.is_error}),
                    ],
                ),
                AgentMessage::Assistant(a) => {
                    let blocks = if let Some(replay) = &a.provider_replay {
                        let projected = project(&replay.items, false)?;
                        check_projection(a, &projected)?;
                        replay.items.clone()
                    } else {
                        let mut blocks = vec![];
                        if !a.content.is_empty() {
                            blocks.push(json!({"type":"text","text":text(&a.content)}));
                        }
                        blocks.extend(a.tool_calls.iter().map(|c|json!({"type":"tool_use","id":c.id,"name":c.name,"input":c.arguments})));
                        blocks
                    };
                    if blocks.is_empty() {
                        return Err(ProviderError::new(
                            ProviderErrorKind::InvalidRequest,
                            "empty Assistant message",
                        ));
                    }
                    ("assistant", blocks)
                }
                AgentMessage::Custom { .. } => unreachable!(),
            };
            if let Some(prev) = messages.last_mut().filter(|v| v["role"] == role) {
                prev["content"].as_array_mut().unwrap().extend(content);
            } else {
                messages.push(json!({"role":role,"content":content}));
            }
        }
        let mut body = json!({"model":d.model_id,"messages":messages,"max_tokens":output(d,r),"stream":self.mode==TransportMode::Sse});
        if !system.is_empty() {
            body["system"] = json!(system);
        }
        if !r.tools.is_empty() {
            body["tools"]=json!(r.tools.iter().map(|t|json!({"name":t.name,"description":t.description,"input_schema":t.input_schema})).collect::<Vec<_>>());
        }
        if let Some(choice) = &r.tool_choice {
            if !r.tools.is_empty() {
                body["tool_choice"] = match choice {
                    ToolChoice::Auto => json!({"type":"auto"}),
                    ToolChoice::None => json!({"type":"none"}),
                    ToolChoice::Required => json!({"type":"any"}),
                    ToolChoice::Named(name) => json!({"type":"tool","name":name}),
                };
            }
        }
        if r.cache_intent.unwrap_or(d.generation.cache) == CacheIntent::ProviderDefault
            && self.options.cache_control.unwrap_or(true)
            && d.capabilities
                .cache_modes
                .contains(&CacheIntent::ProviderDefault)
        {
            body["cache_control"] = json!({"type":"ephemeral"});
        }
        generation(&mut body, d);
        encoded(
            body,
            vec![("anthropic-version".into(), "2023-06-01".into())],
        )
    }
    fn decoder(&self, d: &ResolvedModelDescriptor) -> Box<dyn ResponseDecoder> {
        Box::new(Decoder {
            descriptor: d.clone(),
            ..Default::default()
        })
    }
    fn decode_json(
        &self,
        d: &ResolvedModelDescriptor,
        b: &[u8],
    ) -> Result<ModelResponse, ProviderError> {
        let v = parse(b)?;
        if v["role"] != "assistant" {
            return Err(ProviderError::protocol("invalid Messages role"));
        }
        let blocks = v["content"]
            .as_array()
            .ok_or_else(|| ProviderError::protocol("missing message blocks"))?;
        response(
            d,
            blocks,
            v["stop_reason"].as_str(),
            super::super::usage::anthropic(&v["usage"]),
            v["id"].as_str().map(str::to_owned),
        )
    }
}
fn project(blocks: &[Value], truncated: bool) -> Result<AssistantMessage, ProviderError> {
    let mut a = AssistantMessage::default();
    let mut text_value = String::new();
    for b in blocks {
        match b["type"].as_str() {
            Some("text") => text_value.push_str(
                b["text"]
                    .as_str()
                    .ok_or_else(|| ProviderError::protocol("missing text block content"))?,
            ),
            Some("tool_use") if !truncated => a.tool_calls.push(ToolCall {
                id: required(b, "id")?,
                name: required(b, "name")?,
                arguments: b
                    .get("input")
                    .cloned()
                    .ok_or_else(|| ProviderError::protocol("missing tool input"))?,
            }),
            Some("tool_use") => (),
            Some("thinking") => {
                if truncated {
                    continue;
                }
                required(b, "signature")?;
                if !b["thinking"].is_string() {
                    return Err(ProviderError::protocol("invalid thinking block"));
                }
            }
            Some("redacted_thinking") => {
                if truncated {
                    continue;
                }
                required(b, "data")?;
            }
            _ => {
                return Err(ProviderError::unsupported(
                    "unsupported Messages content block",
                ))
            }
        }
    }
    if !text_value.is_empty() {
        a.content.push(ContentPart::Text(text_value));
    }
    Ok(a)
}
fn response(
    d: &ResolvedModelDescriptor,
    blocks: &[Value],
    stop: Option<&str>,
    usage: TokenUsage,
    id: Option<String>,
) -> Result<ModelResponse, ProviderError> {
    let reason = match stop {
        Some("end_turn" | "stop_sequence") => ModelFinishReason::Stop,
        Some("tool_use") => ModelFinishReason::ToolCalls,
        Some("max_tokens" | "model_context_window_exceeded") => ModelFinishReason::OutputLimit,
        Some("refusal") => ModelFinishReason::Refusal,
        other => ModelFinishReason::Other(other.unwrap_or("missing").into()),
    };
    let truncated = matches!(
        reason,
        ModelFinishReason::OutputLimit | ModelFinishReason::Refusal
    );
    let a = project(blocks, truncated)?;
    let native = if blocks
        .iter()
        .any(|b| matches!(b["type"].as_str(), Some("thinking" | "redacted_thinking")))
        || (!a.content.is_empty() && !a.tool_calls.is_empty())
    {
        Some(blocks.to_vec())
    } else {
        None
    };
    complete(d, a, reason, usage, id, native)
}
#[derive(Default)]
struct Decoder {
    descriptor: ResolvedModelDescriptor,
    blocks: BTreeMap<u64, Value>,
    args: BTreeMap<u64, String>,
    closed: BTreeSet<u64>,
    usage: TokenUsage,
    id: Option<String>,
    stop: Option<String>,
    started: bool,
    done: bool,
}
impl ResponseDecoder for Decoder {
    fn push(&mut self, event: SseEvent) -> Result<Vec<ModelEvent>, ProviderError> {
        if self.done {
            return Err(ProviderError::protocol("Messages events after completion"));
        }
        let v = parse(event.data.as_bytes())?;
        let kind = required(&v, "type")?;
        if event.event.as_ref().is_some_and(|t| t != &kind) {
            return Err(ProviderError::protocol("SSE event type mismatch"));
        }
        let mut out = vec![];
        match kind.as_str() {
            "ping" => (),
            "error" => {
                return Err(ProviderError::new(
                    ProviderErrorKind::Upstream,
                    "Messages stream reported an error",
                ))
            }
            "message_start" => {
                if self.started {
                    return Err(ProviderError::protocol("duplicate message_start"));
                }
                self.started = true;
                if v["message"]["role"] != "assistant"
                    || !v["message"]["content"]
                        .as_array()
                        .is_some_and(Vec::is_empty)
                {
                    return Err(ProviderError::protocol("invalid Messages start payload"));
                }
                self.id = v["message"]["id"].as_str().map(str::to_owned);
                self.usage = super::super::usage::anthropic(&v["message"]["usage"]);
            }
            "content_block_start" => {
                if !self.started {
                    return Err(ProviderError::protocol("block before message start"));
                }
                let index = v["index"]
                    .as_u64()
                    .ok_or_else(|| ProviderError::protocol("missing block index"))?;
                let b = v["content_block"].clone();
                if self.blocks.contains_key(&index) {
                    return Err(ProviderError::protocol("duplicate block index"));
                }
                match b["type"].as_str() {
                    Some("tool_use") => out.push(tool_start(
                        index.to_string(),
                        required(&b, "id")?,
                        required(&b, "name")?,
                    )),
                    Some("text") => {
                        if let Some(s) = b["text"].as_str().filter(|s| !s.is_empty()) {
                            out.push(ModelEvent::TextDelta(s.into()));
                        }
                    }
                    Some("thinking" | "redacted_thinking") => (),
                    _ => return Err(ProviderError::unsupported("unsupported Messages block")),
                }
                self.blocks.insert(index, b);
            }
            "content_block_delta" => {
                let index = v["index"]
                    .as_u64()
                    .ok_or_else(|| ProviderError::protocol("missing delta index"))?;
                if self.closed.contains(&index) {
                    return Err(ProviderError::protocol("delta after block stop"));
                }
                let b = self
                    .blocks
                    .get_mut(&index)
                    .ok_or_else(|| ProviderError::protocol("delta without block"))?;
                let delta = &v["delta"];
                match delta["type"].as_str() {
                    Some("text_delta") if b["type"] == "text" => {
                        let s = delta["text"]
                            .as_str()
                            .ok_or_else(|| ProviderError::protocol("invalid text delta"))?;
                        let text = b["text"].as_str().unwrap_or("").to_owned() + s;
                        b["text"] = json!(text);
                        out.push(ModelEvent::TextDelta(s.into()));
                    }
                    Some("input_json_delta") if b["type"] == "tool_use" => {
                        let s = delta["partial_json"]
                            .as_str()
                            .ok_or_else(|| ProviderError::protocol("invalid parameter delta"))?;
                        self.args.entry(index).or_default().push_str(s);
                        out.push(ModelEvent::ToolArgumentsDelta {
                            key: index.to_string(),
                            delta: s.into(),
                        });
                    }
                    Some("thinking_delta") if b["type"] == "thinking" => {
                        let s = delta["thinking"]
                            .as_str()
                            .ok_or_else(|| ProviderError::protocol("invalid thinking delta"))?;
                        b["thinking"] = json!(b["thinking"].as_str().unwrap_or("").to_owned() + s);
                    }
                    Some("signature_delta") if b["type"] == "thinking" => {
                        let s = delta["signature"]
                            .as_str()
                            .ok_or_else(|| ProviderError::protocol("invalid signature delta"))?;
                        b["signature"] =
                            json!(b["signature"].as_str().unwrap_or("").to_owned() + s);
                    }
                    _ => return Err(ProviderError::unsupported("unsupported Messages delta")),
                }
            }
            "content_block_stop" => {
                let index = v["index"]
                    .as_u64()
                    .ok_or_else(|| ProviderError::protocol("missing stopped block index"))?;
                if !self.blocks.contains_key(&index) || !self.closed.insert(index) {
                    return Err(ProviderError::protocol("invalid block stop"));
                }
            }
            "message_delta" => {
                if !self.started || self.stop.is_some() {
                    return Err(ProviderError::protocol("invalid message delta"));
                }
                self.stop = v["delta"]["stop_reason"].as_str().map(str::to_owned);
                super::super::usage::merge(
                    &mut self.usage,
                    super::super::usage::anthropic(&v["usage"]),
                );
            }
            "message_stop" => {
                if !self.started || self.closed.len() != self.blocks.len() {
                    return Err(ProviderError::protocol(
                        "message ended before block completion",
                    ));
                }
                if !matches!(
                    self.stop.as_deref(),
                    Some("max_tokens" | "model_context_window_exceeded" | "refusal")
                ) {
                    for (index, args) in &self.args {
                        self.blocks.get_mut(index).unwrap()["input"] =
                            serde_json::from_str(args)
                                .map_err(|_| ProviderError::protocol("invalid tool JSON"))?;
                    }
                }
                let blocks = self.blocks.values().cloned().collect::<Vec<_>>();
                let r = response(
                    &self.descriptor,
                    &blocks,
                    self.stop.as_deref(),
                    self.usage.clone(),
                    self.id.clone(),
                )?;
                self.done = true;
                out.push(ModelEvent::completed(r));
            }
            _ => return Err(ProviderError::unsupported("unsupported Messages event")),
        }
        Ok(out)
    }
    fn finish(&self) -> Result<(), ProviderError> {
        if self.done {
            Ok(())
        } else {
            Err(ProviderError::protocol(
                "Messages stream ended without message_stop",
            ))
        }
    }
    fn usage(&self) -> TokenUsage {
        self.usage.clone()
    }
}
