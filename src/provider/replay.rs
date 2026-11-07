use super::*;
use crate::agent::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderReplay {
    pub schema_version: u32,
    pub protocol: ProtocolKind,
    pub compatibility_key: String,
    pub content_hash: String,
    pub items: Vec<Value>,
}
impl std::fmt::Debug for ProviderReplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderReplay")
            .field("protocol", &self.protocol)
            .field("items", &self.items.len())
            .finish_non_exhaustive()
    }
}
fn hash(message: &AssistantMessage) -> String {
    blake3::hash(&serde_json::to_vec(&(&message.content, &message.tool_calls)).unwrap())
        .to_hex()
        .to_string()
}
impl ProviderReplay {
    pub(crate) fn new(
        d: &ResolvedModelDescriptor,
        message: &AssistantMessage,
        items: Vec<Value>,
    ) -> Self {
        Self {
            schema_version: 1,
            protocol: d.protocol,
            compatibility_key: d.compatibility_key.clone(),
            content_hash: hash(message),
            items,
        }
    }
    pub fn validate(
        &self,
        d: &ResolvedModelDescriptor,
        message: &AssistantMessage,
    ) -> Result<(), ProviderError> {
        if self.schema_version != 1
            || self.protocol != d.protocol
            || self.compatibility_key != d.compatibility_key
        {
            return Err(ProviderError::new(ProviderErrorKind::IncompatibleState,"native state requires its original connection/model; compact there or start a new session"));
        }
        if self.content_hash != hash(message) {
            return Err(ProviderError::protocol(
                "native state does not match visible message",
            ));
        }
        Ok(())
    }
}
pub fn validate_replay(
    messages: &[AgentMessage],
    descriptor: &ResolvedModelDescriptor,
) -> Result<(), ProviderError> {
    for m in messages {
        if let AgentMessage::Assistant(a) = m {
            if let Some(r) = &a.provider_replay {
                r.validate(descriptor, a)?;
            }
        }
    }
    Ok(())
}
