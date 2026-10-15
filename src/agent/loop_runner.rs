use super::*;
use std::sync::Arc;
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
}
impl AgentLoop {
    pub fn new(provider: Arc<dyn ModelProvider>) -> Self {
        Self {
            provider,
            tools: ToolRegistry::default(),
            plugins: vec![],
            max_turns: 32,
            event_sink: None,
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
    pub fn register_tool(&mut self, tool: Arc<dyn AgentTool>) {
        self.tools.register(tool);
    }
    pub fn register_plugin(&mut self, plugin: Arc<dyn AgentPlugin>) {
        self.plugins.push(plugin);
    }
    pub async fn run(
        &self,
        input: Vec<ContentPart>,
        state: &mut AgentState,
        cancel: CancellationToken,
    ) -> Result<AgentResult, AgentError> {
        state.messages.push(AgentMessage::User(input));
        let mut events = EventLog::new(self.event_sink.clone());
        events.push(AgentEvent::AgentStart);
        let outcome = async {
            for p in &self.plugins {
                p.before_agent_start(&mut PluginContext {
                    state,
                    events: &mut events,
                })
                .await?;
            }
            let mut reason = StopReason::MaxTurns;
            for _ in 0..self.max_turns {
                if cancel.is_cancelled() {
                    reason = StopReason::Aborted;
                    break;
                }
                state.turn_count += 1;
                events.push(AgentEvent::TurnStart);
                let mut request_messages = state.messages.clone();
                {
                    let mut ctx = PluginContext {
                        state,
                        events: &mut events,
                    };
                    let mut req = BeforeRequest {
                        messages: &mut request_messages,
                    };
                    for p in &self.plugins {
                        p.before_request(&mut req, &mut ctx).await?;
                    }
                }
                request_messages.retain(|m| !matches!(m, AgentMessage::Custom { .. }));
                let stream = self
                    .provider
                    .stream(
                        ModelRequest {
                            messages: request_messages,
                            tools: self.tools.declarations(),
                        },
                        cancel.clone(),
                    )
                    .await?;
                let mut assistant = None;
                futures_util::pin_mut!(stream);
                while let Some(event) = futures_util::StreamExt::next(&mut stream).await {
                    match event {
                        ModelEvent::TextDelta(delta) => {
                            events.push(AgentEvent::MessageDelta { delta })
                        }
                        ModelEvent::ToolCall(call) => events.push(AgentEvent::ToolCallStart(call)),
                        ModelEvent::Complete(message) => assistant = Some(message),
                        ModelEvent::Error(error) => return Err(AgentError::Model(error)),
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
                state
                    .messages
                    .push(AgentMessage::Assistant(assistant.clone()));
                events.push(AgentEvent::MessageEnd(AgentMessage::Assistant(
                    assistant.clone(),
                )));
                let mut results = Vec::new();
                let mut terminate = !assistant.tool_calls.is_empty();
                for mut call in assistant.tool_calls.clone() {
                    let decision = {
                        let mut ctx = PluginContext {
                            state,
                            events: &mut events,
                        };
                        let mut e = BeforeToolCall {
                            assistant: &assistant,
                            call: &mut call,
                        };
                        let mut d = ToolDecision::Allow;
                        for p in &self.plugins {
                            d = p.before_tool_call(&mut e, &mut ctx).await?;
                            if matches!(d, ToolDecision::Block { .. }) {
                                break;
                            }
                        }
                        d
                    };
                    let mut result = match decision {
                        ToolDecision::Allow => {
                            events.push(AgentEvent::ToolExecutionStart(call.clone()));
                            self.tools.execute(&call, cancel.clone()).await
                        }
                        ToolDecision::Block {
                            reason,
                            terminate: t,
                        } => ToolResultMessage {
                            terminate: t,
                            tool_call_id: call.id.clone(),
                            content: vec![ContentPart::Text(reason)],
                            is_error: true,
                        },
                    };
                    for p in &self.plugins {
                        let mut ctx = PluginContext {
                            state,
                            events: &mut events,
                        };
                        let mut e = AfterToolCall {
                            call: &call,
                            result: &mut result,
                        };
                        p.after_tool_call(&mut e, &mut ctx).await?;
                    }
                    terminate &= result.terminate;
                    events.push(AgentEvent::ToolExecutionEnd(result.clone()));
                    state
                        .messages
                        .push(AgentMessage::ToolResult(result.clone()));
                    results.push(result);
                }
                let continuation = {
                    let mut ctx = PluginContext {
                        state,
                        events: &mut events,
                    };
                    let mut e = TurnEnd {
                        assistant: &assistant,
                        results: &results,
                    };
                    let mut c = Continuation::Default;
                    for p in &self.plugins {
                        let decision = p.after_turn(&mut e, &mut ctx).await?;
                        if !matches!(decision, Continuation::Default) {
                            c = decision;
                        }
                        if matches!(c, Continuation::End) {
                            break;
                        }
                    }
                    c
                };
                events.push(AgentEvent::TurnEnd {
                    assistant,
                    tool_results: results.clone(),
                });
                if terminate || matches!(continuation, Continuation::End) {
                    reason = StopReason::PluginRequested;
                    break;
                }
                if results.is_empty() && !matches!(continuation, Continuation::Continue) {
                    reason = StopReason::Completed;
                    break;
                }
            }
            Ok::<_, AgentError>(reason)
        };
        let outcome = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(AgentError::Aborted),
            result = tokio::time::timeout(std::time::Duration::from_secs(300), outcome) => result.unwrap_or_else(|_| Err(AgentError::Model("run timeout".into()))),
        };
        let reason = match &outcome {
            Ok(reason) => reason.clone(),
            Err(AgentError::Aborted) => StopReason::Aborted,
            Err(_) => StopReason::ModelError,
        };
        if let Err(error) = &outcome {
            events.push(AgentEvent::Error(error.to_string()));
        }
        for p in &self.plugins {
            if let Err(error) = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                p.on_agent_end(
                    &reason,
                    &mut PluginContext {
                        state,
                        events: &mut events,
                    },
                ),
            )
            .await
            .unwrap_or_else(|_| Err(AgentError::Plugin("shutdown timeout".into())))
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
