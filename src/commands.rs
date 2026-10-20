#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    Quit,
    New,
    Clear,
    Model(Option<String>),
    Status,
    Stop,
}
pub fn parse(input: &str) -> Option<Command> {
    let mut parts = input.trim().splitn(2, char::is_whitespace);
    let name = parts.next()?.to_ascii_lowercase();
    let arg = parts
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    Some(match name.as_str() {
        "/help" => Command::Help,
        "/quit" | "/exit" => Command::Quit,
        "/new" => Command::New,
        "/clear" => Command::Clear,
        "/model" => Command::Model(arg),
        "/status" => Command::Status,
        "/stop" | "/cancel" => Command::Stop,
        _ => return None,
    })
}
pub fn help() -> &'static str {
    "/help show commands\n/quit save session and exit\n/new start a new conversation\n/clear clear terminal\n/model [id] show or select model\n/status show runtime status\n/stop cancel active run"
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
    }
}
