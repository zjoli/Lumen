use super::*;
use crate::agent::*;
use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
#[derive(Clone)]
pub struct MockProvider {
    responses: Arc<Mutex<std::collections::VecDeque<AssistantMessage>>>,
    descriptor: ResolvedModelDescriptor,
}
impl MockProvider {
    pub fn new(responses: Vec<AssistantMessage>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into())),
            descriptor: ResolvedModelDescriptor {
                provider_ref: "mock".into(),
                model_id: "mock".into(),
                configuration_key: "mock".into(),
                ..Default::default()
            },
        }
    }
    pub fn demonstration() -> Self {
        Self::new(vec![AssistantMessage {
            content: vec![ContentPart::Text(
                "当前为演示模式，尚未连接模型。配置 .lumen/providers.yaml 后选择模型即可开始对话。"
                    .into(),
            )],
            ..Default::default()
        }])
    }
}
#[async_trait]
impl ModelProvider for MockProvider {
    fn descriptor(&self) -> &ResolvedModelDescriptor {
        &self.descriptor
    }
    fn supports_summary(&self) -> bool {
        false
    }
    async fn stream(
        &self,
        _: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelEventStream, ProviderError> {
        if cancel.is_cancelled() {
            return Err(ProviderError::cancelled());
        }
        let message = self
            .responses
            .lock()
            .map_err(|_| ProviderError::protocol("mock lock poisoned"))?
            .pop_front()
            .ok_or_else(|| ProviderError::protocol("mock responses exhausted"))?;
        Ok(Box::pin(futures_util::stream::iter(vec![
            Ok(ModelEvent::Started),
            Ok(ModelEvent::completed(message)),
        ])))
    }
}
