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
        }
    }
    pub fn anthropic(
        endpoint: impl Into<String>,
        key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            client: Client::new(),
            endpoint: endpoint.into(),
            api_key: key.into(),
            model: model.into(),
            format: ApiFormat::Anthropic,
        }
    }
    fn messages(&self, input: &[AgentMessage]) -> Vec<Value> {
        input.iter().filter_map(|m| match m { AgentMessage::System(s)=>Some(json!({"role":"system","content":s})), AgentMessage::User(c)=>Some(json!({"role":"user","content":text(c)})), AgentMessage::Assistant(a)=>Some(json!({"role":"assistant","content":text(&a.content),"tool_calls":a.tool_calls})), AgentMessage::ToolResult(r)=>Some(json!({"role":"tool","tool_call_id":r.tool_call_id,"content":text(&r.content)})), AgentMessage::Custom{..}=>None }).collect()
    }
}
fn text(c: &[ContentPart]) -> String {
    c.iter()
        .map(|x| match x {
            ContentPart::Text(s) => s.as_str(),
        })
        .collect::<Vec<_>>()
        .join("")
}
#[async_trait]
impl ModelProvider for HttpProvider {
    async fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelEventStream, AgentError> {
        if cancel.is_cancelled() {
            return Err(AgentError::Aborted);
        }
        let (body, response) = match self.format {
            ApiFormat::OpenAi => (
                json!({"model":self.model,"messages":self.messages(&request.messages),"tools":request.tools.iter().map(|t| json!({"type":"function","function":{"name":t.name,"description":t.description,"parameters":t.input_schema}})).collect::<Vec<_>>()}),
                self.client.post(&self.endpoint).bearer_auth(&self.api_key),
            ),
            ApiFormat::Anthropic => (
                json!({"model":self.model,"max_tokens":4096,"system":request.messages.iter().find_map(|m| if let AgentMessage::System(s)=m {Some(s)} else {None}),"messages":self.messages(&request.messages).into_iter().filter(|m|m["role"]!="system").collect::<Vec<_>>()}),
                self.client
                    .post(&self.endpoint)
                    .header("x-api-key", &self.api_key)
                    .header("anthropic-version", "2023-06-01"),
            ),
        };
        let value: Value = response
            .json(&body)
            .send()
            .await
            .map_err(|e| AgentError::Model(e.to_string()))?
            .error_for_status()
            .map_err(|e| AgentError::Model(e.to_string()))?
            .json()
            .await
            .map_err(|e| AgentError::Model(e.to_string()))?;
        let content = match self.format {
            ApiFormat::OpenAi => value["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or("")
                .to_owned(),
            ApiFormat::Anthropic => value["content"][0]["text"]
                .as_str()
                .unwrap_or("")
                .to_owned(),
        };
        let tool_calls = value["choices"][0]["message"]["tool_calls"]
            .as_array()
            .map(|xs| {
                xs.iter()
                    .filter_map(|x| {
                        Some(ToolCall {
                            id: x["id"].as_str()?.into(),
                            name: x["function"]["name"].as_str()?.into(),
                            arguments: serde_json::from_str(x["function"]["arguments"].as_str()?)
                                .ok()?,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Box::pin(stream::iter(vec![
            ModelEvent::Start,
            ModelEvent::Complete(AssistantMessage {
                content: vec![ContentPart::Text(content)],
                tool_calls,
            }),
        ])))
    }
}
