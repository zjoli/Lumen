use lumen::{
    agent::*,
    commands::{self, Command},
};
use std::{
    io::{self, Write},
    path::PathBuf,
    sync::Arc,
};
use tokio_util::sync::CancellationToken;
fn show_last(state: &AgentState) {
    if let Some(AgentMessage::Assistant(a)) = state
        .messages
        .iter()
        .rev()
        .find(|m| matches!(m, AgentMessage::Assistant(_)))
    {
        for ContentPart::Text(text) in &a.content {
            println!("\n{text}");
        }
    }
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Lumen — terminal UI (type /help for commands)");
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
    let session_path = std::env::var_os("LUMEN_SESSION")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("lumen-session.jsonl"));
    let session = SessionStore::new(session_path.clone());
    let mut model = "mock".to_owned();
    let mut cancel = CancellationToken::new();
    loop {
        print!("\n> ");
        io::stdout().flush()?;
        let mut line = String::new();
        if io::stdin().read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('/') {
            match commands::parse(line) {
                Some(Command::Help) => println!("{}", commands::help()),
                Some(Command::Quit) => {
                    for m in &state.messages {
                        session.append(m).await?;
                    }
                    println!("saved session to {}", session_path.display());
                    break;
                }
                Some(Command::New) => {
                    state = AgentState {
                        messages: vec![],
                        turn_count: 0,
                    };
                    println!("started a new conversation");
                }
                Some(Command::Clear) => {
                    print!("\x1b[2J\x1b[H");
                    io::stdout().flush()?;
                }
                Some(Command::Model(name)) => match name {
                    Some(name) => {
                        model = name;
                        println!("model: {model}");
                    }
                    None => println!("model: {model}"),
                },
                Some(Command::Status) => println!(
                    "model: {model}\nmessages: {}\nturns: {}\nsession: {}",
                    state.messages.len(),
                    state.turn_count,
                    session_path.display()
                ),
                Some(Command::Stop) => {
                    cancel.cancel();
                    cancel = CancellationToken::new();
                    println!("active run cancelled");
                }
                None => println!("unknown command; type /help"),
            }
            continue;
        }
        let result = agent
            .run(
                vec![ContentPart::Text(line.into())],
                &mut state,
                cancel.clone(),
            )
            .await?;
        show_last(&state);
        if result.reason == StopReason::Aborted {
            println!("run cancelled");
        }
    }
    Ok(())
}
