use super::SessionRuntime;
use crate::agent::*;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

pub struct ContextArtifactTool(pub Arc<SessionRuntime>);
#[async_trait]
impl AgentTool for ContextArtifactTool {
    fn name(&self) -> &str {
        "context_artifact_read"
    }
    fn description(&self) -> &str {
        "Read a bounded section of an archived tool result. Offset and limit count Unicode characters."
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{"id":{"type":"string"},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":4096}},"required":["id"],"additionalProperties":false})
    }
    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolResult, AgentError> {
        if ctx.cancel.is_cancelled() {
            return Err(AgentError::Aborted);
        }
        let result = self
            .0
            .read_artifact(args["id"].as_str().unwrap_or(""))
            .await?;
        let text: String = result
            .content
            .iter()
            .map(|ContentPart::Text(s)| s.as_str())
            .collect();
        let offset = args["offset"].as_u64().unwrap_or(0) as usize;
        let limit = args["limit"].as_u64().unwrap_or(2048).min(4096) as usize;
        let content = text.chars().skip(offset).take(limit).collect::<String>();
        Ok(ToolResult { content: vec![ContentPart::Text(json!({"id":args["id"],"offset":offset,"content":content,"truncated":offset.saturating_add(limit)<text.chars().count(),"original_is_error":result.is_error}).to_string())], is_error: false, terminate: false })
    }
}
