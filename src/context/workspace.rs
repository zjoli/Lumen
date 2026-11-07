use super::*;
use crate::{agent::*, knowledge::ObjectId, workspace::WorkspaceHandle};
use async_trait::async_trait;

/// Workspace rules and evidence-backed learning state, supplied as candidates only.
pub struct WorkspaceContextPlugin(pub WorkspaceHandle);
#[async_trait]
impl AgentPlugin for WorkspaceContextPlugin {
    async fn before_request(
        &self,
        event: &mut BeforeRequest<'_>,
        ctx: &mut PluginContext<'_>,
    ) -> Result<(), AgentError> {
        let BeforeRequest::Collect(collector) = event else {
            return Ok(());
        };
        if let Err(error) = self
            .0
            .call(ctx.cancel.clone(), |state, _| state.ensure_writable())
            .await
        {
            if matches!(error, crate::workspace::WorkspaceError::Cancelled) {
                return Err(AgentError::Aborted);
            }
            ctx.events.push(AgentEvent::ContextDegraded {
                reason: format!("workspace context disabled: {error}"),
            });
            return Ok(());
        }
        for name in ["LUMEN.md", "AGENTS.md"] {
            let id = format!("workspace-rule:{name}");
            let path = self.0.root().join(name);
            if path.is_file() {
                let canonical = path
                    .canonicalize()
                    .map_err(|e| AgentError::Context(e.to_string()))?;
                if !canonical.starts_with(self.0.root()) {
                    return Err(AgentError::Context(
                        "workspace rule points outside workspace".into(),
                    ));
                }
                let content = tokio::fs::read_to_string(&canonical)
                    .await
                    .map_err(|e| AgentError::Context(e.to_string()))?;
                collector.offer(ContextContribution {
                    id,
                    region: ContextRegion::Workspace,
                    source: canonical.display().to_string(),
                    revision: blake3::hash(content.as_bytes()).to_hex().to_string(),
                    content,
                    required: true,
                    retire: false,
                });
            } else {
                collector.offer(ContextContribution { id, region: ContextRegion::Workspace, source: name.into(), revision: "removed".into(), content: format!("Workspace guidance {name} removed; previous contents are no longer active."), required: false, retire: true });
            }
        }
        let key = format!(
            "learning:{}:{}",
            collector.latest_user_cursor(),
            ctx.state
                .messages
                .iter()
                .filter_map(|m| if let AgentMessage::Assistant(a) = m {
                    Some(a)
                } else {
                    None
                })
                .flat_map(|a| &a.tool_calls)
                .filter(|call| call.name == "learning_record")
                .count()
        );
        if collector.contains(&key) {
            return Ok(());
        }
        let query = ctx
            .state
            .messages
            .iter()
            .rev()
            .find_map(|m| {
                if let AgentMessage::User(parts) = m {
                    Some(
                        parts
                            .iter()
                            .map(|ContentPart::Text(s)| s.as_str())
                            .collect::<String>(),
                    )
                } else {
                    None
                }
            })
            .unwrap_or_default();
        // Resolve only IDs present in the user's question or already selected references.
        // Mastery always comes from the event-derived state, never from the summary model.
        let mut relevant = query;
        for item in &collector.items {
            relevant.push('\n');
            relevant.push_str(&item.content);
        }
        let mut ids: Vec<ObjectId> = relevant
            .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-')
            .filter_map(|token| ObjectId::parse(token).ok())
            .collect();
        ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        ids.dedup();
        ids.truncate(8);
        if !ids.is_empty() {
            match self.0.learning().state(ids, ctx.cancel.clone()).await {
                Ok(states) => {
                    for state in states {
                        if state.evidence_ids.is_empty() {
                            continue;
                        }
                        let content = serde_json::to_string(&state).unwrap();
                        collector.offer(ContextContribution {
                            id: format!("learning:{}", state.object_id),
                            region: ContextRegion::Task,
                            source: "learning-events".into(),
                            revision: blake3::hash(content.as_bytes()).to_hex().to_string(),
                            content,
                            required: true,
                            retire: false,
                        });
                    }
                }
                Err(crate::workspace::WorkspaceError::Cancelled) => {
                    return Err(AgentError::Aborted)
                }
                Err(error) => ctx.events.push(AgentEvent::ContextDegraded {
                    reason: format!("learning state unavailable: {error}"),
                }),
            }
        }
        collector.mark(key);
        Ok(())
    }
}
