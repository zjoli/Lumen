use crate::agent::TokenUsage;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderErrorKind {
    Configuration,
    Authentication,
    Permission,
    RateLimited,
    ContextExceeded,
    UnsupportedFeature,
    IncompatibleState,
    InvalidRequest,
    Transport,
    Protocol,
    Cancelled,
    Timeout,
    Upstream,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DeliveryState {
    #[default]
    NotSent,
    Rejected,
    Accepted,
    Unknown,
}
#[derive(Clone, Debug, Serialize, Deserialize, thiserror::Error)]
#[error("{kind:?}: {message}")]
pub struct ProviderError {
    pub kind: ProviderErrorKind,
    pub message: String,
    pub status: Option<u16>,
    pub code: Option<String>,
    pub request_id: Option<String>,
    pub retry_after: Option<u64>,
    pub observed_usage: Option<Box<TokenUsage>>,
    pub delivery: DeliveryState,
}
impl ProviderError {
    pub fn new(kind: ProviderErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            status: None,
            code: None,
            request_id: None,
            retry_after: None,
            observed_usage: None,
            delivery: DeliveryState::NotSent,
        }
    }
    pub fn configuration(message: impl Into<String>) -> Self {
        Self::new(ProviderErrorKind::Configuration, message)
    }
    pub fn protocol(message: impl Into<String>) -> Self {
        Self::new(ProviderErrorKind::Protocol, message)
    }
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ProviderErrorKind::UnsupportedFeature, message)
    }
    pub fn cancelled() -> Self {
        Self::new(ProviderErrorKind::Cancelled, "request cancelled")
    }
}
impl From<ProviderError> for crate::agent::AgentError {
    fn from(e: ProviderError) -> Self {
        if e.kind == ProviderErrorKind::Cancelled {
            Self::Aborted
                .with_observations(e.observed_usage.into_iter().map(|u| *u).collect(), vec![])
        } else {
            Self::Provider(e)
        }
    }
}
