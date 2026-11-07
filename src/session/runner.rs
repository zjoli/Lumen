use super::repository::ManagedWriter;
use super::*;
use crate::{
    agent::*,
    workspace::{WorkspaceError as E, WorkspaceResult as R},
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub struct SessionRunResult {
    pub result: Option<AgentResult>,
    pub primary_error: Option<String>,
    pub cleanup_error: Option<String>,
    pub run_id: String,
}
pub struct SessionRunner;
struct RunGuard {
    handle: Arc<SessionHandle>,
    finished: bool,
}
impl Drop for RunGuard {
    fn drop(&mut self) {
        if !self.finished {
            *self.handle.phase.lock().unwrap() = SessionPhase::Faulted;
        }
        *self.handle.cancel.lock().unwrap() = None;
    }
}
impl SessionRunner {
    pub async fn run(
        handle: Arc<SessionHandle>,
        agent: AgentLoop,
        input: Vec<ContentPart>,
        core: &str,
        cancel: CancellationToken,
        events: mpsc::UnboundedSender<AgentEvent>,
        activation: u64,
    ) -> R<SessionRunResult> {
        let runtime = handle.runtime()?;
        let (metadata, _) = handle.metadata();
        if metadata.archived {
            return Err(E::SessionArchived(metadata.id));
        }
        {
            let mut phase = handle.phase.lock().unwrap();
            if *phase != SessionPhase::Ready {
                return Err(E::SessionBusy(format!("{:?}", *phase)));
            }
            *phase = SessionPhase::Running;
        }
        let run_id = format!("run_{}", uuid::Uuid::now_v7());
        *handle.cancel.lock().unwrap() = Some((run_id.clone(), cancel.clone()));
        let mut guard = RunGuard {
            handle: handle.clone(),
            finished: false,
        };
        let loaded = handle.repo.load(&metadata.id).await?;
        if loaded.revision != handle.metadata().1 {
            return Err(E::SessionUnavailable(
                "session metadata changed; reopen before sending".into(),
            ));
        }
        let report = replay(&metadata.id, &loaded.segments)?;
        if !matches!(
            report.health,
            SessionHealth::Complete | SessionHealth::Interrupted
        ) {
            return Err(E::SessionUnavailable(report.diagnostics.join("; ")));
        }
        if runtime.snapshot().await.messages
            != SessionRuntime::from_records(report.records.clone(), None)
                .map_err(super::manager::agent_error)?
                .snapshot()
                .await
                .messages
        {
            return Err(E::SessionUnavailable(
                "durable history changed; reopen session before sending".into(),
            ));
        }
        let segment = handle.repo.begin(&metadata.id).await?;
        let writer = Arc::new(ManagedWriter {
            repo: handle.repo.clone(),
            session_id: metadata.id.clone(),
            segment_id: segment.clone(),
            active: AtomicBool::new(true),
        });
        let header = SegmentOpen {
            model: Some(metadata.model.clone()),
            context: Some(metadata.context.clone()),
            schema_version: 1,
            session_id: metadata.id.clone(),
            run_id: run_id.clone(),
            segment_id: segment.clone(),
            parent: if report.legacy_prefix.last().is_some_and(|p| {
                report
                    .head
                    .as_ref()
                    .is_some_and(|head| head.segment_id == p.segment_id)
            }) {
                None
            } else {
                report.head
            },
            legacy_prefix: report.legacy_prefix,
            config_hash: content_hash(&serde_json::to_vec(&(&metadata.model, &metadata.context))?),
            started_at: now(),
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        let header_written = AtomicBool::new(false);
        let messages_before = runtime.snapshot().await.messages.len();
        let identity = handle.identity.clone();
        let event_run = run_id.clone();
        let forward = async move {
            while let Some(event) = rx.recv().await {
                let _ = events.send(AgentEvent::SessionEvent {
                    identity: identity.clone(),
                    run_id: event_run.clone(),
                    activation,
                    event: Box::new(event),
                });
            }
        };
        let execution = async {
            handle
                .repo
                .append(
                    &metadata.id,
                    &segment,
                    &AgentMessage::Custom {
                        kind: "lumen.segment_open".into(),
                        content: serde_json::to_value(header).map_err(agent_error_json)?,
                    },
                )
                .await?;
            header_written.store(true, Ordering::SeqCst);
            runtime
                .bind_managed_writer(Some(writer.clone()))
                .await
                .map_err(super::manager::agent_error)?;
            if runtime.snapshot().await.messages.is_empty() {
                runtime
                    .append(AgentMessage::System(core.into()))
                    .await
                    .map_err(super::manager::agent_error)?;
            }
            let mut state = AgentState {
                messages: runtime.snapshot().await.messages,
                turn_count: 0,
            };
            agent
                .with_session(runtime.clone())
                .with_event_sink(tx)
                .run(input, &mut state, cancel.clone())
                .await
                .map_err(super::manager::agent_error)
        };
        let (outcome, ()) = tokio::join!(execution, forward);
        let mut primary = outcome.as_ref().err().map(ToString::to_string);
        if let Ok(result) = &outcome {
            primary = result.error.as_ref().map(ToString::to_string);
        }
        let reason = outcome
            .as_ref()
            .map(|result| result.reason.clone())
            .unwrap_or(StopReason::ModelError);
        let (usage, compaction_usage) = outcome
            .as_ref()
            .map(|result| {
                let normal = result
                    .events
                    .iter()
                    .filter_map(|e| {
                        if let AgentEvent::UsageReported(u) = e {
                            Some(u.clone())
                        } else {
                            None
                        }
                    })
                    .collect();
                let compact = result
                    .events
                    .iter()
                    .filter_map(|e| {
                        if let AgentEvent::CompactionUsageReported(u) = e {
                            Some(u.clone())
                        } else {
                            None
                        }
                    })
                    .collect();
                (normal, compact)
            })
            .unwrap_or_default();
        let end = RunEnd {
            message_range: Some([messages_before, runtime.snapshot().await.messages.len()]),
            error_kind: outcome
                .as_ref()
                .ok()
                .and_then(|result| result.error.as_ref())
                .map(|error| {
                    match error {
                        AgentError::Aborted => "cancelled",
                        AgentError::Session(_) => "session",
                        AgentError::Context(_) => "context",
                        AgentError::Model(_) => "model",
                        AgentError::Tool(_) => "tool",
                        _ => "agent",
                    }
                    .to_string()
                }),
            schema_version: 1,
            run_id: run_id.clone(),
            reason,
            error: primary.clone(),
            ended_at: now(),
            usage,
            compaction_usage,
        };
        // Cleanup is independent of the cancelled run token. Always attempt finish even if run_end fails.
        let end_result = if !header_written.load(Ordering::SeqCst) || runtime.is_faulted() {
            Err(E::SessionUnavailable(
                "run end not appended after an uncertain durable write".into(),
            ))
        } else {
            handle
                .repo
                .append(
                    &metadata.id,
                    &segment,
                    &AgentMessage::Custom {
                        kind: "lumen.run_end".into(),
                        content: serde_json::to_value(end)?,
                    },
                )
                .await
        };
        writer.active.store(false, Ordering::SeqCst);
        let unbind = runtime.bind_managed_writer(None).await;
        let finish = handle.repo.finish(&metadata.id, &segment).await;
        let cleanup = end_result
            .err()
            .map(|e| e.to_string())
            .or_else(|| unbind.err().map(|e| e.to_string()))
            .or_else(|| finish.err().map(|e| e.to_string()));
        let durable_fault = runtime.is_faulted();
        *handle.phase.lock().unwrap() = if cleanup.is_some() || durable_fault {
            SessionPhase::Faulted
        } else {
            SessionPhase::Ready
        };
        guard.finished = true;
        Ok(SessionRunResult {
            result: outcome.ok(),
            primary_error: primary,
            cleanup_error: cleanup,
            run_id,
        })
    }
}
fn agent_error_json(e: serde_json::Error) -> E {
    E::InvalidFormat(e.to_string())
}
