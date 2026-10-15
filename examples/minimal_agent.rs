use lumen::agent::*;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), AgentError> {
    let provider = MockProvider::new(vec![AssistantMessage {
        content: vec![ContentPart::Text("Lumen Agent Loop is running.".into())],
        tool_calls: vec![],
    }]);
    let agent = AgentLoop::new(Arc::new(provider));
    let mut state = AgentState {
        messages: vec![],
        turn_count: 0,
    };
    let result = agent
        .run(
            vec![ContentPart::Text("hello".into())],
            &mut state,
            CancellationToken::new(),
        )
        .await?;
    for event in result.events {
        println!("{event:?}");
    }
    Ok(())
}
