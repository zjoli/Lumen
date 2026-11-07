use super::*;
use async_trait::async_trait;
pub struct CompactionPlugin {
    pub max_messages: usize,
    pub keep_recent: usize,
}
#[async_trait]
impl AgentPlugin for CompactionPlugin {
    fn legacy_request_mutator(&self) -> bool {
        true
    }
    async fn before_request(
        &self,
        event: &mut BeforeRequest<'_>,
        ctx: &mut PluginContext<'_>,
    ) -> Result<(), AgentError> {
        let BeforeRequest::Legacy { messages } = event else {
            return Err(AgentError::Context(
                "legacy compaction cannot run in coordinated mode".into(),
            ));
        };
        if messages.len() <= self.max_messages {
            return Ok(());
        }
        let keep = self.keep_recent.min(messages.len());
        let split = messages.len() - keep;
        let summary = messages[..split]
            .iter()
            .filter_map(|m| {
                if let AgentMessage::User(c)
                | AgentMessage::Assistant(AssistantMessage { content: c, .. }) = m
                {
                    Some(
                        c.iter()
                            .map(|p| match p {
                                ContentPart::Text(s) => s.as_str(),
                            })
                            .collect::<String>(),
                    )
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        messages.drain(..split);
        messages.insert(
            0,
            AgentMessage::User(vec![ContentPart::Text(format!(
                "Historical conversation summary (preserve original authority):\n{summary}"
            ))]),
        );
        ctx.events
            .push(AgentEvent::ContextCompactionEnd { summary });
        Ok(())
    }
}
