use lumen::agent::*;
use std::io::{self, Write};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Lumen — minimal terminal UI (type /quit to exit)");
    let provider = MockProvider::new(vec![AssistantMessage {
        content: vec![ContentPart::Text(
            "Mock provider ready. Configure HttpProvider in your application for a real model."
                .into(),
        )],
        tool_calls: vec![],
    }]);
    let agent = AgentLoop::new(Arc::new(provider));
    let mut state = AgentState {
        messages: vec![],
        turn_count: 0,
    };
    loop {
        print!("\n> ");
        io::stdout().flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        let line = line.trim();
        if line == "/quit" {
            break;
        }
        if line.is_empty() {
            continue;
        }
        let result = agent
            .run(
                vec![ContentPart::Text(line.into())],
                &mut state,
                CancellationToken::new(),
            )
            .await?;
        for message in result.messages.iter().rev() {
            if let AgentMessage::Assistant(a) = message {
                for part in &a.content {
                    let ContentPart::Text(text) = part;
                    println!("\n{text}");
                }
                break;
            }
        }
    }
    Ok(())
}
