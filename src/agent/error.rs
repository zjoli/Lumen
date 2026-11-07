use thiserror::Error;

#[derive(Debug, Error, Clone)]
pub enum AgentError {
    #[error("{0}")]
    Provider(crate::provider::ProviderError),
    #[error("{source}")]
    Compaction {
        source: Box<AgentError>,
        usage: Vec<super::TokenUsage>,
        reports: Vec<super::ModelRequestReport>,
    },
    #[error("context error: {0}")]
    Context(String),
    #[error("session error: {0}")]
    Session(String),
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
impl AgentError {
    pub fn provider_error(&self) -> Option<&crate::provider::ProviderError> {
        match self {
            Self::Provider(e) => Some(e),
            Self::Compaction { source, .. } => source.provider_error(),
            _ => None,
        }
    }
    pub fn is_aborted(&self) -> bool {
        match self {
            Self::Aborted => true,
            Self::Compaction { source, .. } => source.is_aborted(),
            _ => false,
        }
    }
    pub fn with_observations(
        self,
        usage: Vec<super::TokenUsage>,
        reports: Vec<super::ModelRequestReport>,
    ) -> Self {
        if usage.is_empty() && reports.is_empty() {
            self
        } else {
            Self::Compaction {
                source: Box::new(self),
                usage,
                reports,
            }
        }
    }
    pub fn observations(&self) -> (Vec<super::TokenUsage>, Vec<super::ModelRequestReport>) {
        match self {
            Self::Provider(e) => (
                e.observed_usage.clone().into_iter().map(|u| *u).collect(),
                vec![],
            ),
            Self::Compaction {
                source,
                usage,
                reports,
            } => {
                let (mut u, mut r) = source.observations();
                u.extend(usage.clone());
                r.extend(reports.clone());
                (u, r)
            }
            _ => (vec![], vec![]),
        }
    }
}
