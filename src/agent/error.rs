use thiserror::Error;

#[derive(Debug, Error, Clone)]
pub enum AgentError {
    #[error("model error: {0}")]
    Model(String),
    #[error("tool error: {0}")]
    Tool(String),
    #[error("plugin error: {0}")]
    Plugin(String),
    #[error("invalid tool arguments: {0}")]
    InvalidArguments(String),
    #[error("agent aborted")]
    Aborted,
    #[error("maximum turns exceeded")]
    MaxTurns,
}
