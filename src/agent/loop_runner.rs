use super::*;
use crate::context::{ContextAssembly, ContextCollector, SessionRuntime};
use futures_util::StreamExt;
use std::{collections::BTreeSet, sync::Arc};
use tokio_util::sync::CancellationToken;

pub struct AgentState {
    pub messages: Vec<AgentMessage>,
    pub turn_count: usize,
}
pub struct AgentResult {
    pub messages: Vec<AgentMessage>,
    pub events: Vec<AgentEvent>,
    pub reason: StopReason,
    pub error: Option<AgentError>,
}
pub struct AgentLoop {
    provider: Arc<dyn ModelProvider>,
    tools: ToolRegistry,
    plugins: Vec<Arc<dyn AgentPlugin>>,
    max_turns: usize,
    event_sink: Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>,
    session: Option<Arc<SessionRuntime>>,
}
impl AgentLoop {
    pub fn new(provider: Arc<dyn ModelProvider>) -> Self {
        Self {
            provider,
            tools: ToolRegistry::default(),
            plugins: vec![],
            max_turns: 32,
            event_sink: None,
            session: None,
        }
    }
    pub fn with_event_sink(mut self, sink: tokio::sync::mpsc::UnboundedSender<AgentEvent>) -> Self {
        self.event_sink = Some(sink);
        self
    }
    pub fn with_max_turns(mut self, max: usize) -> Self {
        self.max_turns = max;
        self
    }
    pub fn with_session(mut self, session: Arc<SessionRuntime>) -> Self {
        self.session = Some(session);
        self
    }
    pub fn register_tool(&mut self, tool: Arc<dyn AgentTool>) {
        self.tools.register(tool);
    }
    pub fn register_plugin(&mut self, plugin: Arc<dyn AgentPlugin>) {
        self.plugins.push(plugin);
    }
    async fn commit(
        &self,
        message: AgentMessage,
        state: &mut AgentState,
        events: &mut EventLog,
    ) -> Result<(), AgentError> {
        if let Some(session) = &self.session {
            session.append(message.clone()).await?;
        }
        state.messages.push(message.clone());
        events.push(AgentEvent::MessageCommitted(message));
        Ok(())
    }
    pub async fn run(
        &self,
        input: Vec<ContentPart>,
        state: &mut AgentState,
        cancel: CancellationToken,
    ) -> Result<AgentResult, AgentError> {
        let _lease = if let Some(s) = &self.session {
            Some(
                tokio::select! { _ = cancel.cancelled() => return Err(AgentError::Aborted), lock = s.run_lock.lock() => lock },
            )
        } else {
            None
        };
        let mut events = EventLog::new(self.event_sink.clone());
        events.push(AgentEvent::AgentStart);
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
        let outcome = self
            .run_inner(input, state, &cancel, &mut events, deadline)
            .await;
        let reason = match &outcome {
            Ok(r) => r.clone(),
            Err(AgentError::Aborted) => StopReason::Aborted,
            Err(_) => StopReason::ModelError,
        };
        if let Err(error) = &outcome {
            events.push(AgentEvent::Error(error.to_string()));
        }
        if let Some(s) = &self.session {
            state.messages = s.snapshot().await.messages;
        }
        for p in &self.plugins {
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                p.on_agent_end(
                    &reason,
                    &mut PluginContext {
                        state,
                        events: &mut events,
                        cancel: cancel.child_token(),
                    },
                ),
            )
            .await;
            if let Err(error) =
                result.unwrap_or_else(|_| Err(AgentError::Plugin("shutdown timeout".into())))
            {
                events.push(AgentEvent::Error(error.to_string()));
            }
        }
        events.push(AgentEvent::AgentEnd {
            reason: reason.clone(),
        });
        Ok(AgentResult {
            messages: state.messages.clone(),
            events: events.items,
            reason,
            error: outcome.err(),
        })
    }
    async fn run_inner(
        &self,
        input: Vec<ContentPart>,
        state: &mut AgentState,
        cancel: &CancellationToken,
        events: &mut EventLog,
        deadline: tokio::time::Instant,
    ) -> Result<StopReason, AgentError> {
        let coordinators: Vec<_> = self
            .plugins
            .iter()
            .filter(|p| p.role() == PluginRole::ContextCoordinator)
            .collect();
        let standard: Vec<_> = self
            .plugins
            .iter()
            .filter(|p| p.role() == PluginRole::Standard)
            .collect();
        if coordinators.len() > 1 {
            return Err(AgentError::Context("multiple context coordinators".into()));
        }
        let coordinator = coordinators.first().copied();
        if coordinator.is_some() && self.session.is_none() {
            return Err(AgentError::Context(
                "context coordinator requires a session runtime".into(),
            ));
        }
        if let Some(p) = coordinator {
            if !p
                .context_session()
                .as_ref()
                .zip(self.session.as_ref())
                .map(|(a, b)| Arc::ptr_eq(a, b))
                .unwrap_or(false)
            {
                return Err(AgentError::Context(
                    "coordinator session does not match Loop session".into(),
                ));
            }
        }
        if coordinator.is_some() && standard.iter().any(|p| p.legacy_request_mutator()) {
            return Err(AgentError::Context(
                "legacy request mutator cannot join coordinated mode".into(),
            ));
        }
        if let Some(s) = &self.session {
            let snapshot = s.snapshot().await;
            if !state.messages.is_empty() && state.messages != snapshot.messages {
                return Err(AgentError::Session(
                    "AgentState does not match session transcript".into(),
                ));
            }
            state.messages = snapshot.messages;
            // Interrupted calls are never silently replayed.
            let mut pending = Vec::new();
            for message in &state.messages {
                match message {
                    AgentMessage::Assistant(a) => {
                        pending.extend(a.tool_calls.iter().map(|c| c.id.clone()))
                    }
                    AgentMessage::ToolResult(r) => {
                        pending.retain(|id| id != &r.tool_call_id);
                    }
                    _ => (),
                }
            }
            for id in pending {
                self.commit(AgentMessage::ToolResult(ToolResultMessage { tool_call_id: id, content: vec![ContentPart::Text("Previous execution was interrupted; outcome unknown. Do not automatically repeat operations with side effects.".into())], is_error: true, terminate: false }), state, events).await?;
            }
        }
        if cancel.is_cancelled() {
            return Err(AgentError::Aborted);
        }
        self.commit(AgentMessage::User(input), state, events)
            .await?;
        if let Some(p) = coordinator {
            p.before_agent_start(&mut PluginContext {
                state,
                events,
                cancel: cancel.child_token(),
            })
            .await?;
        }
        for p in &standard {
            bounded(
                p.before_agent_start(&mut PluginContext {
                    state,
                    events,
                    cancel: cancel.child_token(),
                }),
                cancel,
                deadline,
            )
            .await?;
        }
        let tools = self.tools.declarations();
        for _ in 0..self.max_turns {
            if cancel.is_cancelled() {
                return Err(AgentError::Aborted);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(AgentError::Model("run timeout".into()));
            }
            if let Some(s) = &self.session {
                state.messages = s.snapshot().await.messages;
            }
            state.turn_count += 1;
            events.push(AgentEvent::TurnStart);
            let prepared = if let Some(p) = coordinator {
                let snapshot = self.session.as_ref().unwrap().snapshot().await;
                let cursor = snapshot
                    .messages
                    .iter()
                    .rposition(|m| matches!(m, AgentMessage::User(_)))
                    .unwrap_or(0);
                let mut collector = ContextCollector {
                    items: vec![],
                    seen: snapshot.window.keys,
                    raw_cursor: cursor,
                    marks: BTreeSet::new(),
                };
                for plugin in &standard {
                    bounded(
                        plugin.before_request(
                            &mut BeforeRequest::Collect(&mut collector),
                            &mut PluginContext {
                                state,
                                events,
                                cancel: cancel.child_token(),
                            },
                        ),
                        cancel,
                        deadline,
                    )
                    .await?;
                }
                let mut assembly = ContextAssembly {
                    collector,
                    tools: tools.clone(),
                    prepared: None,
                };
                // Coordinator cancellation is checked internally; durable commits are not dropped.
                p.before_request(
                    &mut BeforeRequest::Assemble(&mut assembly),
                    &mut PluginContext {
                        state,
                        events,
                        cancel: cancel.child_token(),
                    },
                )
                .await?;
                assembly.prepared.ok_or_else(|| {
                    AgentError::Context("coordinator did not seal a request".into())
                })?
            } else {
                let mut messages = state.messages.clone();
                for p in &standard {
                    bounded(
                        p.before_request(
                            &mut BeforeRequest::Legacy {
                                messages: &mut messages,
                            },
                            &mut PluginContext {
                                state,
                                events,
                                cancel: cancel.child_token(),
                            },
                        ),
                        cancel,
                        deadline,
                    )
                    .await?;
                }
                messages.retain(|m| !matches!(m, AgentMessage::Custom { .. }));
                self.provider.prepare(ModelRequest {
                    messages,
                    tools: tools.clone(),
                    max_output_tokens: self.provider.capabilities().max_output_tokens,
                })?
            };
            if prepared.provider_key != self.provider.configuration_key()
                || serde_json::to_value(&prepared.request.tools).unwrap()
                    != serde_json::to_value(&tools).unwrap()
            {
                return Err(AgentError::Context(
                    "sealed request configuration mismatch".into(),
                ));
            }
            if cancel.is_cancelled() {
                return Err(AgentError::Aborted);
            }
            let mut stream = bounded(
                self.provider.stream_prepared(prepared, cancel.clone()),
                cancel,
                deadline,
            )
            .await?;
            let mut assistant = None;
            loop {
                let event = bounded(async { Ok(stream.next().await) }, cancel, deadline).await?;
                let Some(event) = event else {
                    break;
                };
                match event {
                    ModelEvent::TextDelta(delta) => events.push(AgentEvent::MessageDelta { delta }),
                    ModelEvent::ToolCall(call) => events.push(AgentEvent::ToolCallStart(call)),
                    ModelEvent::Complete(message) => {
                        assistant = Some(message);
                        break;
                    }
                    ModelEvent::Error(error) => return Err(AgentError::Model(error)),
                    ModelEvent::Usage(usage) => events.push(AgentEvent::UsageReported(usage)),
                    ModelEvent::Start => events.push(AgentEvent::MessageStart(
                        AgentMessage::Assistant(AssistantMessage {
                            content: vec![],
                            tool_calls: vec![],
                        }),
                    )),
                }
            }
            let assistant = assistant
                .ok_or_else(|| AgentError::Model("provider ended without a response".into()))?;
            let mut call_ids = BTreeSet::new();
            if assistant
                .tool_calls
                .iter()
                .any(|call| !call_ids.insert(&call.id))
            {
                return Err(AgentError::Model(
                    "duplicate tool call IDs in Complete".into(),
                ));
            }
            self.commit(AgentMessage::Assistant(assistant.clone()), state, events)
                .await?;
            events.push(AgentEvent::MessageEnd(AgentMessage::Assistant(
                assistant.clone(),
            )));
            let mut results = vec![];
            let mut terminate = !assistant.tool_calls.is_empty();
            for call in &assistant.tool_calls {
                if cancel.is_cancelled() {
                    return Err(AgentError::Aborted);
                }
                let mut decision = ToolDecision::Allow;
                for p in &standard {
                    decision = bounded(
                        p.before_tool_call(
                            &mut BeforeToolCall {
                                assistant: &assistant,
                                call,
                            },
                            &mut PluginContext {
                                state,
                                events,
                                cancel: cancel.child_token(),
                            },
                        ),
                        cancel,
                        deadline,
                    )
                    .await?;
                    if matches!(decision, ToolDecision::Block { .. }) {
                        break;
                    }
                }
                let mut result = match decision {
                    ToolDecision::Allow => {
                        events.push(AgentEvent::ToolExecutionStart(call.clone()));
                        self.tools.execute(call, cancel.clone()).await
                    }
                    ToolDecision::Block { reason, terminate } => ToolResultMessage {
                        tool_call_id: call.id.clone(),
                        content: vec![ContentPart::Text(reason)],
                        is_error: true,
                        terminate,
                    },
                };
                let original = result.clone();
                let mut post_error = None;
                for p in &standard {
                    if let Err(error) = bounded(
                        p.after_tool_call(
                            &mut AfterToolCall {
                                call,
                                result: &mut result,
                                original: &original,
                            },
                            &mut PluginContext {
                                state,
                                events,
                                cancel: cancel.child_token(),
                            },
                        ),
                        cancel,
                        deadline,
                    )
                    .await
                    {
                        // Preserve an already completed result even when cancellation or a plugin fails.
                        result = original.clone();
                        post_error = Some(error);
                        break;
                    }
                }
                if let Some(p) = coordinator {
                    let flags = (
                        result.tool_call_id.clone(),
                        result.is_error,
                        result.terminate,
                    );
                    p.after_tool_call(
                        &mut AfterToolCall {
                            call,
                            result: &mut result,
                            original: &original,
                        },
                        &mut PluginContext {
                            state,
                            events,
                            cancel: cancel.child_token(),
                        },
                    )
                    .await?;
                    if flags
                        != (
                            result.tool_call_id.clone(),
                            result.is_error,
                            result.terminate,
                        )
                    {
                        return Err(AgentError::Context(
                            "coordinator changed tool result identity/status".into(),
                        ));
                    }
                }
                if result.tool_call_id != call.id {
                    result = original;
                    post_error = Some(AgentError::Plugin("plugin changed tool call ID".into()));
                }
                terminate &= result.terminate;
                if let Some(s) = &self.session {
                    state.messages = s.snapshot().await.messages;
                }
                self.commit(AgentMessage::ToolResult(result.clone()), state, events)
                    .await?;
                events.push(AgentEvent::ToolExecutionEnd(result.clone()));
                results.push(result);
                if let Some(error) = post_error {
                    return Err(error);
                }
            }
            let mut continuation = Continuation::Default;
            for p in &standard {
                let c = bounded(
                    p.after_turn(
                        &mut TurnEnd {
                            assistant: &assistant,
                            results: &results,
                        },
                        &mut PluginContext {
                            state,
                            events,
                            cancel: cancel.child_token(),
                        },
                    ),
                    cancel,
                    deadline,
                )
                .await?;
                if !matches!(c, Continuation::Default) {
                    continuation = c;
                }
                if matches!(continuation, Continuation::End) {
                    break;
                }
            }
            events.push(AgentEvent::TurnEnd {
                assistant,
                tool_results: results.clone(),
            });
            if terminate || matches!(continuation, Continuation::End) {
                return Ok(StopReason::PluginRequested);
            }
            if results.is_empty() && !matches!(continuation, Continuation::Continue) {
                return Ok(StopReason::Completed);
            }
        }
        Ok(StopReason::MaxTurns)
    }
}
async fn bounded<T>(
    future: impl std::future::Future<Output = Result<T, AgentError>>,
    cancel: &CancellationToken,
    deadline: tokio::time::Instant,
) -> Result<T, AgentError> {
    tokio::select! { _ = cancel.cancelled() => Err(AgentError::Aborted), result = tokio::time::timeout_at(deadline, future) => result.unwrap_or_else(|_| Err(AgentError::Model("run timeout".into()))) }
}

/// Emits immediately and retains a trace for deterministic inspection.
pub struct EventLog {
    pub(crate) items: Vec<AgentEvent>,
    sink: Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>,
}
impl EventLog {
    fn new(sink: Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>) -> Self {
        Self {
            items: vec![],
            sink,
        }
    }
    pub fn push(&mut self, event: AgentEvent) {
        if let Some(sink) = &self.sink {
            let _ = sink.send(event.clone());
        }
        self.items.push(event);
    }
}
