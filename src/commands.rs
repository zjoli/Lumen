use std::path::PathBuf;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    Quit,
    New,
    Clear,
    Model(Option<String>),
    Status,
    Stop,
    Workspace(WorkspaceCommand),
    Search(String),
    Commit(String),
    Sync(SyncCommand),
    History(Option<String>),
    Diff(Option<String>),
    Restore { id: String, commit: String },
    Index(IndexCommand),
    Validate,
    Invalid(String),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceCommand {
    Init(PathBuf),
    Open(PathBuf),
    Clone {
        url: String,
        path: PathBuf,
    },
    Config {
        key: Option<String>,
        value: Option<String>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncCommand {
    All(Option<String>),
    Pull,
    Push,
    Fetch,
    Remote(Option<String>),
    Conflicts,
    Resolve { path: String, file: Option<PathBuf> },
    Continue,
    Abort,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IndexCommand {
    Status,
    Refresh,
    Rebuild(String),
}
fn words(input: &str) -> Result<Vec<String>, String> {
    let mut out = vec![];
    let mut word = String::new();
    let mut quote = None;
    let mut active = false;
    for c in input.chars() {
        if let Some(q) = quote {
            if c == q {
                quote = None
            } else {
                word.push(c)
            }
            active = true;
        } else if c == '\'' || c == '"' {
            quote = Some(c);
            active = true;
        } else if c.is_whitespace() {
            if active {
                out.push(std::mem::take(&mut word));
                active = false;
            }
        } else {
            word.push(c);
            active = true;
        }
    }
    if quote.is_some() {
        return Err("unclosed quote".into());
    }
    if active {
        out.push(word)
    }
    Ok(out)
}
pub fn parse(input: &str) -> Option<Command> {
    if !input.trim_start().starts_with('/') {
        return None;
    }
    let parts = match words(input) {
        Ok(p) => p,
        Err(e) => return Some(Command::Invalid(e)),
    };
    let name = parts.first()?.to_ascii_lowercase();
    let args = &parts[1..];
    let rest = || (!args.is_empty()).then(|| args.join(" "));
    let invalid = || Command::Invalid(format!("invalid arguments for {name}; see /help"));
    Some(match name.as_str() {
        "/help" => Command::Help,
        "/quit" | "/exit" => Command::Quit,
        "/new" => Command::New,
        "/clear" => Command::Clear,
        "/model" => Command::Model(rest()),
        "/status" => Command::Status,
        "/stop" | "/cancel" => Command::Stop,
        "/search" => rest().map(Command::Search).unwrap_or_else(invalid),
        "/commit" => rest().map(Command::Commit).unwrap_or_else(invalid),
        "/history" if args.len() <= 1 => Command::History(args.first().cloned()),
        "/diff" if args.len() <= 1 => Command::Diff(args.first().cloned()),
        "/restore" if args.len() == 2 => Command::Restore {
            id: args[0].clone(),
            commit: args[1].clone(),
        },
        "/validate" if args.is_empty() => Command::Validate,
        "/workspace" => match args.first().map(String::as_str) {
            Some("init") if args.len() == 2 => {
                Command::Workspace(WorkspaceCommand::Init(args[1].clone().into()))
            }
            Some("open") if args.len() == 2 => {
                Command::Workspace(WorkspaceCommand::Open(args[1].clone().into()))
            }
            Some("clone") if args.len() == 3 => Command::Workspace(WorkspaceCommand::Clone {
                url: args[1].clone(),
                path: args[2].clone().into(),
            }),
            Some("config") if args.len() <= 3 => Command::Workspace(WorkspaceCommand::Config {
                key: args.get(1).cloned(),
                value: args.get(2).cloned(),
            }),
            _ => invalid(),
        },
        "/sync" => Command::Sync(match args.first().map(String::as_str) {
            None => SyncCommand::All(None),
            Some("pull") if args.len() == 1 => SyncCommand::Pull,
            Some("push") if args.len() == 1 => SyncCommand::Push,
            Some("fetch") if args.len() == 1 => SyncCommand::Fetch,
            Some("remote") if args.len() <= 2 => SyncCommand::Remote(args.get(1).cloned()),
            Some("conflicts") if args.len() == 1 => SyncCommand::Conflicts,
            Some("continue") if args.len() == 1 => SyncCommand::Continue,
            Some("abort") if args.len() == 1 => SyncCommand::Abort,
            Some("resolve") if args.len() == 3 => SyncCommand::Resolve {
                path: args[1].clone(),
                file: if args[2] == "delete" {
                    None
                } else {
                    Some(args[2].clone().into())
                },
            },
            Some("--message") if args.len() > 1 => SyncCommand::All(Some(args[1..].join(" "))),
            Some(
                "pull" | "push" | "fetch" | "remote" | "conflicts" | "continue" | "abort"
                | "resolve" | "--message",
            ) => return Some(invalid()),
            _ => SyncCommand::All(rest()),
        }),
        "/index" => match args.first().map(String::as_str) {
            Some("status") if args.len() == 1 => Command::Index(IndexCommand::Status),
            Some("refresh") if args.len() == 1 => Command::Index(IndexCommand::Refresh),
            Some("rebuild") if args.len() == 2 => {
                Command::Index(IndexCommand::Rebuild(args[1].clone()))
            }
            _ => invalid(),
        },
        _ => return None,
    })
}
pub fn help() -> &'static str {
    "/help | /quit | /new | /clear | /model [id] | /status | /stop\n/workspace init <path> | open <path> | clone <url> <path> | config [key value]\n/search <query> | /commit <message> | /sync [message]\n/sync pull | push | fetch | remote [url|none] | conflicts | resolve <path> <file|delete> | continue | abort\n/history [id] | /diff [id] | /restore <id> <commit>\n/index status | refresh | rebuild <all|lexical|relations|learning_state|vectors|summaries>\n/validate"
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses() {
        assert_eq!(parse("/new"), Some(Command::New));
        assert_eq!(
            parse("/model gpt"),
            Some(Command::Model(Some("gpt".into())))
        );
        assert!(parse("hello").is_none());
        assert_eq!(
            parse("/workspace open '/tmp/my notes'"),
            Some(Command::Workspace(WorkspaceCommand::Open(
                "/tmp/my notes".into()
            )))
        );
        assert_eq!(
            parse("/sync --message pull"),
            Some(Command::Sync(SyncCommand::All(Some("pull".into()))))
        );
        assert!(matches!(
            parse("/sync resolve x"),
            Some(Command::Invalid(_))
        ));
    }
}
