use async_trait::async_trait;
use lumen::agent::*;
use serde_json::json;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

struct Echo;
#[async_trait]
impl AgentTool for Echo {
    fn name(&self) -> &str {
        "echo"
    }
    async fn execute(
        &self,
        args: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolResult, AgentError> {
        Ok(ToolResult {
            content: vec![ContentPart::Text(
                args["text"].as_str().unwrap_or("").to_owned(),
            )],
            is_error: false,
            terminate: false,
        })
    }
}

#[tokio::test]
async fn completes_text_turn_and_preserves_events() {
    let provider = MockProvider::new(vec![AssistantMessage {
        content: vec![ContentPart::Text("hello".into())],
        tool_calls: vec![],
    }]);
    let agent = AgentLoop::new(Arc::new(provider));
    let mut state = AgentState {
        messages: vec![],
        turn_count: 0,
    };
    let result = agent
        .run(
            vec![ContentPart::Text("hi".into())],
            &mut state,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result.reason, StopReason::Completed);
    assert!(result
        .messages
        .iter()
        .any(|m| matches!(m, AgentMessage::Assistant(_))));
    assert!(matches!(
        result.events.first(),
        Some(AgentEvent::AgentStart)
    ));
    assert!(matches!(
        result.events.last(),
        Some(AgentEvent::AgentEnd { .. })
    ));
}

#[tokio::test]
async fn executes_tool_and_requests_next_turn() {
    let provider = MockProvider::new(vec![
        AssistantMessage {
            content: vec![],
            tool_calls: vec![ToolCall {
                id: "1".into(),
                name: "echo".into(),
                arguments: json!({"text":"ok"}),
            }],
        },
        AssistantMessage {
            content: vec![ContentPart::Text("done".into())],
            tool_calls: vec![],
        },
    ]);
    let mut agent = AgentLoop::new(Arc::new(provider));
    agent.register_tool(Arc::new(Echo));
    let mut state = AgentState {
        messages: vec![],
        turn_count: 0,
    };
    let result = agent
        .run(
            vec![ContentPart::Text("run".into())],
            &mut state,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result.reason, StopReason::Completed);
    assert_eq!(
        result
            .messages
            .iter()
            .filter(|m| matches!(m, AgentMessage::ToolResult(_)))
            .count(),
        1
    );
    assert_eq!(state.turn_count, 2);
}

fn text_response() -> AssistantMessage {
    AssistantMessage {
        content: vec![ContentPart::Text("done".into())],
        tool_calls: vec![],
    }
}
fn call_response(name: &str, arguments: serde_json::Value) -> AssistantMessage {
    AssistantMessage {
        content: vec![],
        tool_calls: vec![ToolCall {
            id: "call".into(),
            name: name.into(),
            arguments,
        }],
    }
}
fn state() -> AgentState {
    AgentState {
        messages: vec![],
        turn_count: 0,
    }
}

#[tokio::test]
async fn last_allowed_turn_completes_and_state_can_be_reused() {
    let agent = AgentLoop::new(Arc::new(MockProvider::new(vec![
        text_response(),
        text_response(),
    ])))
    .with_max_turns(1);
    let mut state = state();
    for _ in 0..2 {
        assert_eq!(
            agent
                .run(vec![], &mut state, CancellationToken::new())
                .await
                .unwrap()
                .reason,
            StopReason::Completed
        );
    }
}
#[tokio::test]
async fn unknown_tool_and_invalid_arguments_are_model_visible() {
    for (name, args) in [("missing", json!({})), ("echo", json!(42))] {
        let mut agent = AgentLoop::new(Arc::new(MockProvider::new(vec![
            call_response(name, args),
            text_response(),
        ])));
        agent.register_tool(Arc::new(Echo));
        let result = agent
            .run(vec![], &mut state(), CancellationToken::new())
            .await
            .unwrap();
        assert!(result
            .messages
            .iter()
            .any(|m| matches!(m, AgentMessage::ToolResult(r) if r.is_error)));
        assert_eq!(result.reason, StopReason::Completed);
    }
}
struct Block;
#[async_trait]
impl AgentPlugin for Block {
    async fn before_tool_call(
        &self,
        _: &mut BeforeToolCall<'_>,
        _: &mut PluginContext<'_>,
    ) -> Result<ToolDecision, AgentError> {
        Ok(ToolDecision::Block {
            reason: "denied".into(),
            terminate: true,
        })
    }
}
#[tokio::test]
async fn plugin_blocks_and_ends_run() {
    let mut agent = AgentLoop::new(Arc::new(MockProvider::new(vec![call_response(
        "echo",
        json!({}),
    )])));
    agent.register_plugin(Arc::new(Block));
    let result = agent
        .run(vec![], &mut state(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.reason, StopReason::PluginRequested);
    assert!(!result
        .events
        .iter()
        .any(|e| matches!(e, AgentEvent::ToolExecutionStart(_))));
}
#[tokio::test]
async fn maximum_turns_stops_tool_cycle() {
    let agent = AgentLoop::new(Arc::new(MockProvider::new(vec![call_response(
        "missing",
        json!({}),
    )])))
    .with_max_turns(1);
    assert_eq!(
        agent
            .run(vec![], &mut state(), CancellationToken::new())
            .await
            .unwrap()
            .reason,
        StopReason::MaxTurns
    );
}
struct Hanging;
#[async_trait]
impl ModelProvider for Hanging {
    async fn stream(
        &self,
        _: ModelRequest,
        _: CancellationToken,
    ) -> Result<ModelEventStream, AgentError> {
        std::future::pending().await
    }
}
#[tokio::test]
async fn cancellation_interrupts_noncooperative_provider_and_emits_end() {
    let token = CancellationToken::new();
    let other = token.clone();
    tokio::spawn(async move {
        tokio::task::yield_now().await;
        other.cancel();
    });
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let agent = AgentLoop::new(Arc::new(Hanging)).with_event_sink(sender);
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        agent.run(vec![], &mut state(), token),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result.reason, StopReason::Aborted);
    assert!(matches!(
        receiver.recv().await,
        Some(AgentEvent::AgentStart)
    ));
    assert!(matches!(
        result.events.last(),
        Some(AgentEvent::AgentEnd {
            reason: StopReason::Aborted
        })
    ));
}
#[tokio::test]
async fn exhausted_provider_reports_error_and_final_event() {
    let agent = AgentLoop::new(Arc::new(MockProvider::new(vec![])));
    let result = agent
        .run(vec![], &mut state(), CancellationToken::new())
        .await
        .unwrap();
    assert!(result.error.is_some());
    assert!(matches!(
        result.events.last(),
        Some(AgentEvent::AgentEnd {
            reason: StopReason::ModelError
        })
    ));
}
