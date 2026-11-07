use super::*;
use std::collections::BTreeMap;
pub(super) struct Responses {
    pub mode: TransportMode,
}
impl ProtocolAdapter for Responses {
    fn encode(
        &self,
        d: &ResolvedModelDescriptor,
        r: &ModelRequest,
    ) -> Result<EncodedRequest, ProviderError> {
        validate(d, r)?;
        let mut input = vec![];
        for m in &r.messages {
            match m {
            AgentMessage::System(s)=>input.push(json!({"role":"system","content":s})),
            AgentMessage::User(c)=>input.push(json!({"role":"user","content":text(c)})),
            AgentMessage::ToolResult(t)=>input.push(json!({"type":"function_call_output","call_id":t.tool_call_id,"output":text(&t.content)})),
            AgentMessage::Assistant(a)=>{if let Some(replay)=&a.provider_replay {let b=project(&replay.items,false)?;check_projection(a,&b)?;input.extend(replay.items.clone());}else{if !a.content.is_empty(){input.push(json!({"role":"assistant","content":text(&a.content)}));}input.extend(a.tool_calls.iter().map(|c|json!({"type":"function_call","call_id":c.id,"name":c.name,"arguments":c.arguments.to_string()})));}},
            AgentMessage::Custom{..}=>unreachable!(),
        }
        }
        let mut body = json!({"model":d.model_id,"input":input,"max_output_tokens":output(d,r),"stream":self.mode==TransportMode::Sse,"store":false,"include":["reasoning.encrypted_content"]});
        if !r.tools.is_empty() {
            body["tools"]=json!(r.tools.iter().map(|t|json!({"type":"function","name":t.name,"description":t.description,"parameters":t.input_schema})).collect::<Vec<_>>());
        }
        if let Some(choice) = &r.tool_choice {
            if !r.tools.is_empty() {
                body["tool_choice"] = match choice {
                    ToolChoice::Auto => json!("auto"),
                    ToolChoice::None => json!("none"),
                    ToolChoice::Required => json!("required"),
                    ToolChoice::Named(name) => json!({"type":"function","name":name}),
                };
            }
        }
        generation(&mut body, d);
        encoded(body, vec![])
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
        response(d, &parse(b)?)
    }
}
fn project(items: &[Value], truncated: bool) -> Result<AssistantMessage, ProviderError> {
    let mut a = AssistantMessage::default();
    let mut all = String::new();
    for item in items {
        match item["type"].as_str() {
            Some("message") => {
                if item["role"] != "assistant" {
                    return Err(ProviderError::protocol("invalid Responses output role"));
                }
                for c in item["content"]
                    .as_array()
                    .ok_or_else(|| ProviderError::protocol("missing output content"))?
                {
                    match c["type"].as_str() {
                        Some("output_text") => all.push_str(
                            c["text"]
                                .as_str()
                                .ok_or_else(|| ProviderError::protocol("missing output text"))?,
                        ),
                        Some("refusal") => all.push_str(
                            c["refusal"]
                                .as_str()
                                .ok_or_else(|| ProviderError::protocol("missing refusal text"))?,
                        ),
                        _ => {
                            return Err(ProviderError::unsupported(
                                "unsupported Responses content block",
                            ))
                        }
                    }
                }
            }
            Some("function_call") if !truncated => a.tool_calls.push(ToolCall {
                id: required(item, "call_id")?,
                name: required(item, "name")?,
                arguments: serde_json::from_str(&required(item, "arguments")?)
                    .map_err(|_| ProviderError::protocol("invalid tool argument JSON"))?,
            }),
            Some("function_call") => (),
            Some("reasoning") => {
                if !truncated {
                    required(item, "encrypted_content").map_err(|_| {
                        ProviderError::unsupported(
                            "reasoning continuation requires encrypted state",
                        )
                    })?;
                }
            }
            _ => {
                return Err(ProviderError::unsupported(
                    "unsupported Responses output item",
                ))
            }
        }
    }
    if !all.is_empty() {
        a.content.push(ContentPart::Text(all));
    }
    Ok(a)
}
fn response(d: &ResolvedModelDescriptor, v: &Value) -> Result<ModelResponse, ProviderError> {
    let items = v["output"]
        .as_array()
        .ok_or_else(|| ProviderError::protocol("missing Responses output"))?;
    let refusal = items.iter().any(|i| {
        i["content"]
            .as_array()
            .is_some_and(|c| c.iter().any(|c| c["type"] == "refusal"))
    });
    let reason = match v["status"].as_str() {
        Some("completed") if refusal => ModelFinishReason::Refusal,
        Some("completed") => {
            if items.iter().any(|i| i["type"] == "function_call") {
                ModelFinishReason::ToolCalls
            } else {
                ModelFinishReason::Stop
            }
        }
        Some("incomplete") => match v["incomplete_details"]["reason"].as_str() {
            Some("max_output_tokens") => ModelFinishReason::OutputLimit,
            Some("content_filter") => ModelFinishReason::Refusal,
            _ => ModelFinishReason::Other("incomplete".into()),
        },
        Some("failed" | "cancelled") => {
            return Err(ProviderError::new(
                ProviderErrorKind::Upstream,
                "Responses generation failed",
            ))
        }
        _ => return Err(ProviderError::protocol("invalid Responses terminal status")),
    };
    let a = project(
        items,
        matches!(
            reason,
            ModelFinishReason::OutputLimit | ModelFinishReason::Refusal
        ),
    )?;
    let replay = if items.len() > 1
        || items
            .iter()
            .any(|i| i["type"] == "reasoning" || i.get("phase").is_some())
    {
        Some(
            items
                .iter()
                .map(|i| {
                    let mut v = i.clone();
                    v.as_object_mut().unwrap().retain(|k, _| {
                        matches!(
                            k.as_str(),
                            "type"
                                | "id"
                                | "role"
                                | "content"
                                | "phase"
                                | "status"
                                | "call_id"
                                | "name"
                                | "arguments"
                                | "encrypted_content"
                                | "summary"
                        )
                    });
                    v
                })
                .collect(),
        )
    } else {
        None
    };
    complete(
        d,
        a,
        reason,
        super::super::usage::openai(&v["usage"], false),
        v["id"].as_str().map(str::to_owned),
        replay,
    )
}
#[derive(Default)]
struct Decoder {
    descriptor: ResolvedModelDescriptor,
    items: BTreeMap<u64, Value>,
    usage: TokenUsage,
    started: bool,
    done: bool,
}
impl ResponseDecoder for Decoder {
    fn push(&mut self, event: SseEvent) -> Result<Vec<ModelEvent>, ProviderError> {
        if self.done {
            return Err(ProviderError::protocol("Responses events after completion"));
        }
        let v = parse(event.data.as_bytes())?;
        let kind = required(&v, "type")?;
        if event.event.as_ref().is_some_and(|t| t != &kind) {
            return Err(ProviderError::protocol("SSE event type mismatch"));
        }
        let mut out = vec![];
        match kind.as_str() {
            "response.created" => {
                if self.started {
                    return Err(ProviderError::protocol("duplicate response.created"));
                }
                self.started = true;
            }
            "response.in_progress" => (),
            "response.output_item.added" => {
                let index = v["output_index"]
                    .as_u64()
                    .ok_or_else(|| ProviderError::protocol("missing output index"))?;
                let item = v["item"].clone();
                if self.items.contains_key(&index) {
                    return Err(ProviderError::protocol("duplicate output index"));
                }
                match item["type"].as_str() {
                    Some("function_call") => out.push(tool_start(
                        index.to_string(),
                        required(&item, "call_id")?,
                        required(&item, "name")?,
                    )),
                    Some("message" | "reasoning") => (),
                    _ => return Err(ProviderError::unsupported("unsupported Responses item")),
                };
                self.items.insert(index, item);
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                let index = v["output_index"]
                    .as_u64()
                    .ok_or_else(|| ProviderError::protocol("missing text delta index"))?;
                if !self
                    .items
                    .get(&index)
                    .is_some_and(|i| i["type"] == "message")
                {
                    return Err(ProviderError::protocol("text delta without message item"));
                }
                let s = v["delta"]
                    .as_str()
                    .ok_or_else(|| ProviderError::protocol("missing output delta"))?;
                out.push(ModelEvent::TextDelta(s.into()));
            }
            "response.function_call_arguments.delta" => {
                let index = v["output_index"]
                    .as_u64()
                    .ok_or_else(|| ProviderError::protocol("missing tool delta index"))?;
                if !self
                    .items
                    .get(&index)
                    .is_some_and(|i| i["type"] == "function_call")
                {
                    return Err(ProviderError::protocol("tool delta without call item"));
                }
                out.push(ModelEvent::ToolArgumentsDelta {
                    key: index.to_string(),
                    delta: v["delta"]
                        .as_str()
                        .ok_or_else(|| ProviderError::protocol("missing argument delta"))?
                        .into(),
                });
            }
            "response.output_item.done" => {
                let index = v["output_index"]
                    .as_u64()
                    .ok_or_else(|| ProviderError::protocol("missing output index"))?;
                if !self.items.contains_key(&index) {
                    return Err(ProviderError::protocol("item done without item start"));
                }
                self.items.insert(index, v["item"].clone());
            }
            "response.completed" | "response.incomplete" => {
                if !self.started {
                    return Err(ProviderError::protocol("response ended without start"));
                }
                let expected = if kind == "response.completed" {
                    "completed"
                } else {
                    "incomplete"
                };
                if v["response"]["status"] != expected {
                    return Err(ProviderError::protocol(
                        "Responses terminal event/status mismatch",
                    ));
                }
                self.usage = super::super::usage::openai(&v["response"]["usage"], false);
                let r = response(&self.descriptor, &v["response"])?;
                self.done = true;
                out.push(ModelEvent::completed(r));
            }
            "response.failed" | "error" => {
                self.usage = super::super::usage::openai(&v["response"]["usage"], false);
                return Err(ProviderError::new(
                    ProviderErrorKind::Upstream,
                    "Responses stream reported an error",
                ));
            }
            "response.content_part.added"
            | "response.content_part.done"
            | "response.output_text.done"
            | "response.refusal.done"
            | "response.function_call_arguments.done"
            | "response.reasoning_summary_part.added"
            | "response.reasoning_summary_part.done"
            | "response.reasoning_summary_text.delta"
            | "response.reasoning_summary_text.done"
            | "response.reasoning_text.delta"
            | "response.reasoning_text.done" => (),
            _ => return Err(ProviderError::unsupported("unsupported Responses event")),
        }
        Ok(out)
    }
    fn finish(&self) -> Result<(), ProviderError> {
        if self.done {
            Ok(())
        } else {
            Err(ProviderError::protocol(
                "Responses stream ended without terminal response",
            ))
        }
    }
    fn usage(&self) -> TokenUsage {
        self.usage.clone()
    }
}
