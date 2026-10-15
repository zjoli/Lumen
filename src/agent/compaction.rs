use super::*;
use async_trait::async_trait;
pub struct CompactionPlugin {
    pub max_messages: usize,
    pub keep_recent: usize,
}
#[async_trait]
impl AgentPlugin for CompactionPlugin {
    async fn before_request(
        &self,
        event: &mut BeforeRequest<'_>,
        ctx: &mut PluginContext<'_>,
    ) -> Result<(), AgentError> {
        if event.messages.len() <= self.max_messages {
            return Ok(());
        }
        let keep = self.keep_recent.min(event.messages.len());
        let split = event.messages.len() - keep;
        let summary = event.messages[..split]
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
        event.messages.drain(..split);
        event.messages.insert(
            0,
            AgentMessage::Custom {
                kind: "compaction_summary".into(),
                content: serde_json::json!({"text":summary}),
            },
        );
        ctx.events
            .push(AgentEvent::ContextCompactionEnd { summary });
        Ok(())
    }
}
