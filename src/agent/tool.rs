use super::{AgentError, ContentPart, ToolCall, ToolResultMessage};
use async_trait::async_trait;
use serde_json::Value;
use std::{collections::HashMap, sync::Arc};
use tokio_util::sync::CancellationToken;

pub struct ToolContext {
    pub cancel: CancellationToken,
}
pub struct ToolResult {
    pub content: Vec<ContentPart>,
    pub is_error: bool,
    pub terminate: bool,
}
#[async_trait]
pub trait AgentTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str {
        ""
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({"type":"object"})
    }
    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolResult, AgentError>;
}
#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn AgentTool>>,
}
impl ToolRegistry {
    pub fn register(&mut self, tool: Arc<dyn AgentTool>) {
        self.tools.insert(tool.name().to_owned(), tool);
    }
    pub async fn execute(&self, call: &ToolCall, cancel: CancellationToken) -> ToolResultMessage {
        let Some(tool) = self.tools.get(&call.name) else {
            return ToolResultMessage {
                terminate: false,
                tool_call_id: call.id.clone(),
                content: vec![ContentPart::Text(format!("unknown tool: {}", call.name))],
                is_error: true,
            };
        };
        let validation = jsonschema::validator_for(&tool.input_schema())
            .map_err(|e| e.to_string())
            .and_then(|validator| {
                validator
                    .validate(&call.arguments)
                    .map_err(|e| e.to_string())
            });
        if let Err(error) = validation {
            return ToolResultMessage {
                terminate: false,
                tool_call_id: call.id.clone(),
                content: vec![ContentPart::Text(format!("invalid arguments: {error}"))],
                is_error: true,
            };
        }
        let child = cancel.child_token();
        let outcome = tokio::select! {
            _ = cancel.cancelled() => Err(AgentError::Aborted),
            result = tokio::time::timeout(std::time::Duration::from_secs(30), tool.execute(call.arguments.clone(), ToolContext { cancel: child.clone() })) => result.unwrap_or_else(|_| Err(AgentError::Tool("tool timeout".into()))),
        };
        child.cancel();
        match outcome {
            Ok(result) => ToolResultMessage {
                terminate: result.terminate,
                tool_call_id: call.id.clone(),
                content: result.content,
                is_error: result.is_error,
            },
            Err(error) => ToolResultMessage {
                terminate: false,
                tool_call_id: call.id.clone(),
                content: vec![ContentPart::Text(error.to_string())],
                is_error: true,
            },
        }
    }
    pub fn names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }
}

impl ToolRegistry {
    pub fn declarations(&self) -> Vec<super::ToolDeclaration> {
        let mut tools: Vec<_> = self
            .tools
            .values()
            .map(|t| super::ToolDeclaration {
                name: t.name().into(),
                description: t.description().into(),
                input_schema: t.input_schema(),
            })
            .collect();
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        tools
    }
}
