use super::*;
use std::collections::BTreeMap;
pub(super) struct Chat {
    pub mode: TransportMode,
    pub options: ProtocolOptions,
}
impl ProtocolAdapter for Chat {
    fn encode(
        &self,
        d: &ResolvedModelDescriptor,
        r: &ModelRequest,
    ) -> Result<EncodedRequest, ProviderError> {
        validate(d, r)?;
        let mut messages = vec![];
        for m in &r.messages {
            messages.push(match m {
            AgentMessage::System(s)=>json!({"role":"system","content":s}),
            AgentMessage::User(c)=>json!({"role":"user","content":text(c)}),
            AgentMessage::ToolResult(t)=>json!({"role":"tool","tool_call_id":t.tool_call_id,"content":text(&t.content)}),
            AgentMessage::Assistant(a)=>{
                if let Some(replay)=&a.provider_replay {let native=replay.items.first().filter(|_|replay.items.len()==1).ok_or_else(||ProviderError::protocol("invalid chat replay"))?;let b=message(native,false)?;check_projection(a,&b)?;native.clone()}else{let mut v=json!({"role":"assistant","content":text(&a.content)});if !a.tool_calls.is_empty(){v["tool_calls"]=json!(a.tool_calls.iter().map(|c|json!({"id":c.id,"type":"function","function":{"name":c.name,"arguments":c.arguments.to_string()}})).collect::<Vec<_>>());}v}
            },AgentMessage::Custom{..}=>unreachable!(),
        });
        }
        let streaming = self.mode == TransportMode::Sse;
        let mut body = json!({"model":d.model_id,"messages":messages,"stream":streaming});
        body[match self
            .options
            .output_token_field
            .unwrap_or(OutputTokenField::MaxTokens)
        {
            OutputTokenField::MaxTokens => "max_tokens",
            OutputTokenField::MaxCompletionTokens => "max_completion_tokens",
        }] = json!(output(d, r));
        if streaming && self.options.stream_usage.unwrap_or(false) {
            body["stream_options"] = json!({"include_usage":true});
        }
        if !r.tools.is_empty() {
            body["tools"]=json!(r.tools.iter().map(|t|json!({"type":"function","function":{"name":t.name,"description":t.description,"parameters":t.input_schema}})).collect::<Vec<_>>());
        }
        if let Some(c) = &r.tool_choice {
            if !r.tools.is_empty() {
                body["tool_choice"] = match c {
                    ToolChoice::Auto => json!("auto"),
                    ToolChoice::None => json!("none"),
                    ToolChoice::Required => json!("required"),
                    ToolChoice::Named(name) => json!({"type":"function","function":{"name":name}}),
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
fn reason(v: &Value, m: &Value) -> ModelFinishReason {
    if m["refusal"].as_str().is_some_and(|s| !s.is_empty()) {
        return ModelFinishReason::Refusal;
    }
    match v.as_str() {
        Some("stop") => ModelFinishReason::Stop,
        Some("tool_calls") => ModelFinishReason::ToolCalls,
        Some("length") => ModelFinishReason::OutputLimit,
        Some("content_filter") => ModelFinishReason::Refusal,
        other => ModelFinishReason::Other(other.unwrap_or("missing").into()),
    }
}
fn message(v: &Value, truncated: bool) -> Result<AssistantMessage, ProviderError> {
    if v["role"] != "assistant" {
        return Err(ProviderError::protocol("invalid chat assistant role"));
    }
    if !v["function_call"].is_null() || !v["audio"].is_null() {
        return Err(ProviderError::unsupported(
            "unsupported chat output payload",
        ));
    }
    let mut a = AssistantMessage::default();
    match &v["content"] {
        Value::String(s) => {
            if !s.is_empty() {
                a.content.push(ContentPart::Text(s.clone()));
            }
        }
        Value::Null => (),
        _ => return Err(ProviderError::unsupported("unsupported chat content type")),
    }
    if let Some(s) = v["refusal"].as_str().filter(|s| !s.is_empty()) {
        a.content.push(ContentPart::Text(s.into()));
    }
    if !truncated {
        if let Some(calls) = v.get("tool_calls").filter(|v| !v.is_null()) {
            for c in calls
                .as_array()
                .ok_or_else(|| ProviderError::protocol("invalid tool call list"))?
            {
                if c["type"] != "function" {
                    return Err(ProviderError::unsupported("non-function tool call"));
                }
                a.tool_calls.push(ToolCall {
                    id: required(c, "id")?,
                    name: required(&c["function"], "name")?,
                    arguments: serde_json::from_str(&required(&c["function"], "arguments")?)
                        .map_err(|_| ProviderError::protocol("invalid tool arguments JSON"))?,
                });
            }
        }
    }
    Ok(a)
}
fn response(d: &ResolvedModelDescriptor, v: &Value) -> Result<ModelResponse, ProviderError> {
    let choices = v["choices"]
        .as_array()
        .filter(|c| c.len() == 1)
        .ok_or_else(|| ProviderError::protocol("expected one chat choice"))?;
    let c = &choices[0];
    let m = &c["message"];
    if !m.is_object() {
        return Err(ProviderError::protocol("missing chat message"));
    }
    let reason = reason(&c["finish_reason"], m);
    let a = message(
        m,
        matches!(
            reason,
            ModelFinishReason::OutputLimit | ModelFinishReason::Refusal
        ),
    )?;
    let has_state = |key: &str| {
        m.get(key)
            .is_some_and(|v| !v.is_null() && !v.as_str().is_some_and(str::is_empty))
    };
    let native = if has_state("reasoning_content") || has_state("reasoning") {
        let mut item = m.clone();
        item.as_object_mut().unwrap().retain(|k, _| {
            matches!(
                k.as_str(),
                "role" | "content" | "tool_calls" | "reasoning_content" | "reasoning" | "refusal"
            )
        });
        Some(vec![item])
    } else {
        None
    };
    complete(
        d,
        a,
        reason,
        super::super::usage::openai(&v["usage"], true),
        v["id"].as_str().map(str::to_owned),
        native,
    )
}
#[derive(Default)]
struct Call {
    id: String,
    name: String,
    args: String,
    started: bool,
}
#[derive(Default)]
struct Decoder {
    descriptor: ResolvedModelDescriptor,
    text: String,
    refusal: String,
    reasoning: String,
    calls: BTreeMap<u64, Call>,
    finish: Option<String>,
    usage: TokenUsage,
    id: Option<String>,
    done: bool,
}
impl ResponseDecoder for Decoder {
    fn push(&mut self, event: SseEvent) -> Result<Vec<ModelEvent>, ProviderError> {
        if self.done {
            return Err(ProviderError::protocol("chat events after completion"));
        }
        if event.data.trim() == "[DONE]" {
            let finish = self
                .finish
                .as_ref()
                .ok_or_else(|| ProviderError::protocol("chat ended without finish reason"))?;
            let mut m = json!({"role":"assistant","content":self.text});
            if !self.refusal.is_empty() {
                m["refusal"] = json!(self.refusal);
            }
            if !self.reasoning.is_empty() {
                m["reasoning_content"] = json!(self.reasoning);
            }
            if !self.calls.is_empty() {
                m["tool_calls"]=json!(self.calls.values().map(|c|json!({"id":c.id,"type":"function","function":{"name":c.name,"arguments":c.args}})).collect::<Vec<_>>());
            }
            let v = json!({"id":self.id,"choices":[{"message":m,"finish_reason":finish}]});
            let mut r = response(&self.descriptor, &v)?;
            r.usage = self.usage.clone();
            self.done = true;
            return Ok(vec![ModelEvent::completed(r)]);
        }
        let v = parse(event.data.as_bytes())?;
        if v.get("error").is_some() {
            return Err(ProviderError::new(
                ProviderErrorKind::Upstream,
                "chat stream reported an error",
            ));
        }
        if v.get("usage").is_some_and(Value::is_object) {
            super::super::usage::merge(
                &mut self.usage,
                super::super::usage::openai(&v["usage"], true),
            );
        }
        if self.id.is_none() {
            self.id = v["id"].as_str().map(str::to_owned);
        }
        let choices = v["choices"]
            .as_array()
            .ok_or_else(|| ProviderError::protocol("missing chat choices"))?;
        let mut out = vec![];
        for c in choices {
            if c["index"].as_u64() != Some(0) {
                return Err(ProviderError::protocol("unexpected chat choice index"));
            }
            if self.finish.is_some() {
                return Err(ProviderError::protocol("content after chat finish"));
            }
            if let Some(f) = c["finish_reason"].as_str() {
                self.finish = Some(f.into());
            }
            let delta = &c["delta"];
            if !delta["role"].is_null() && delta["role"] != "assistant" {
                return Err(ProviderError::protocol("invalid chat delta role"));
            }
            if !delta["tool_calls"].is_null() && !delta["tool_calls"].is_array() {
                return Err(ProviderError::protocol("invalid chat tool delta list"));
            }
            if !delta["function_call"].is_null()
                || !delta["audio"].is_null()
                || !delta["reasoning"].is_null()
            {
                return Err(ProviderError::unsupported("unsupported chat delta payload"));
            }
            if let Some(s) = delta["content"].as_str() {
                self.text.push_str(s);
                out.push(ModelEvent::TextDelta(s.into()));
            } else if !delta["content"].is_null() {
                return Err(ProviderError::unsupported("unsupported chat delta content"));
            }
            if let Some(s) = delta["refusal"].as_str() {
                self.refusal.push_str(s);
                out.push(ModelEvent::TextDelta(s.into()));
            }
            if let Some(s) = delta["reasoning_content"].as_str() {
                self.reasoning.push_str(s);
            }
            if let Some(calls) = delta["tool_calls"].as_array() {
                for call in calls {
                    let index = call["index"]
                        .as_u64()
                        .ok_or_else(|| ProviderError::protocol("tool delta missing index"))?;
                    let p = self.calls.entry(index).or_default();
                    if let Some(id) = call["id"].as_str() {
                        p.id.push_str(id);
                    }
                    if let Some(name) = call["function"]["name"].as_str() {
                        p.name.push_str(name);
                    }
                    if !p.started && !p.id.is_empty() && !p.name.is_empty() {
                        out.push(tool_start(index.to_string(), p.id.clone(), p.name.clone()));
                        p.started = true;
                    }
                    if let Some(args) = call["function"]["arguments"].as_str() {
                        p.args.push_str(args);
                        out.push(ModelEvent::ToolArgumentsDelta {
                            key: index.to_string(),
                            delta: args.into(),
                        });
                    }
                }
            }
        }
        Ok(out)
    }
    fn finish(&self) -> Result<(), ProviderError> {
        if self.done {
            Ok(())
        } else {
            Err(ProviderError::protocol(
                "chat stream ended without terminal marker",
            ))
        }
    }
    fn usage(&self) -> TokenUsage {
        self.usage.clone()
    }
}
