use lumen::{
    agent::*,
    commands::{self, Command, IndexCommand, SyncCommand, WorkspaceCommand},
    knowledge::*,
    retrieval::*,
    versioning::*,
    workspace::*,
};
use std::{
    collections::VecDeque,
    io::{self, Write},
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
struct Application {
    workspace: Option<WorkspaceHandle>,
    state: AgentState,
    session_id: String,
    model: String,
    standalone: SessionStore,
}
fn token() -> CancellationToken {
    CancellationToken::new()
}
fn show_event(event: AgentEvent) {
    match event {
        AgentEvent::MessageEnd(AgentMessage::Assistant(a)) => {
            for ContentPart::Text(text) in a.content {
                println!("{text}")
            }
        }
        AgentEvent::Error(error) => eprintln!("{error}"),
        _ => (),
    }
}
fn display<T: serde::Serialize>(v: T) -> WorkspaceResult<String> {
    Ok(serde_json::to_string_pretty(&v)?)
}
impl Application {
    fn workspace(&self) -> WorkspaceResult<WorkspaceHandle> {
        self.workspace
            .clone()
            .ok_or_else(|| WorkspaceError::Configuration("open a workspace first".into()))
    }
    async fn command(&mut self, command: Command, c: CancellationToken) -> WorkspaceResult<String> {
        match command {
            Command::Help => Ok(commands::help().into()),
            Command::Clear => Ok("\x1b[2J\x1b[H".into()),
            Command::New => {
                self.state.messages.clear();
                self.state.turn_count = 0;
                self.session_id = format!("session_{}", uuid::Uuid::now_v7());
                Ok("started a new conversation".into())
            }
            Command::Model(model) => {
                if let Some(model) = model {
                    self.model = model
                }
                Ok(self.model.clone())
            }
            Command::Status => {
                if let Some(w) = &self.workspace {
                    display(w.status(c).await?)
                } else {
                    Ok(format!(
                        "model: {}\nmessages: {}\nno workspace",
                        self.model,
                        self.state.messages.len()
                    ))
                }
            }
            Command::Workspace(cmd) => {
                if let WorkspaceCommand::Config { key, value } = cmd {
                    let w = self.workspace()?;
                    return match (key, value) {
                        (None, None) => display(w.config(c).await?),
                        (Some(key), None) => {
                            let config = serde_json::to_value(w.config(c).await?)?;
                            display(config.get(&key).ok_or_else(|| {
                                WorkspaceError::Configuration("unknown config key".into())
                            })?)
                        }
                        (Some(key), Some(value)) => {
                            let value = serde_json::from_str(&value)
                                .unwrap_or(serde_json::Value::String(value));
                            let mut patch = serde_json::Map::new();
                            patch.insert(key, value);
                            display(w.configure(patch, c).await?)
                        }
                        _ => Err(WorkspaceError::Configuration(
                            "invalid config arguments".into(),
                        )),
                    };
                }
                let name = std::env::var("LUMEN_AUTHOR_NAME")
                    .or_else(|_| std::env::var("USER"))
                    .unwrap_or_else(|_| "Lumen User".into());
                let email = std::env::var("LUMEN_AUTHOR_EMAIL")
                    .unwrap_or_else(|_| "lumen@localhost".into());
                let next = match cmd {
                    WorkspaceCommand::Init(root) => {
                        WorkspaceService::init(
                            InitRequest {
                                root,
                                author_name: name,
                                author_email: email,
                            },
                            c.clone(),
                        )
                        .await?
                    }
                    WorkspaceCommand::Open(path) => WorkspaceService::open(path, c.clone()).await?,
                    WorkspaceCommand::Clone { url, path } => {
                        WorkspaceService::clone(
                            CloneRequest {
                                root: path,
                                url,
                                config: LocalConfig {
                                    author_name: name,
                                    author_email: email,
                                    ..Default::default()
                                },
                            },
                            c.clone(),
                        )
                        .await?
                    }
                    _ => unreachable!(),
                };
                if let Some(old) = &self.workspace {
                    old.close(token()).await?;
                }
                let info = next.info(c).await?;
                self.workspace = Some(next);
                self.state.messages.clear();
                self.session_id = format!("session_{}", uuid::Uuid::now_v7());
                display(info)
            }
            Command::Search(query) => display(
                self.workspace()?
                    .retrieval()
                    .search(
                        SearchRequest {
                            query,
                            ..Default::default()
                        },
                        c,
                    )
                    .await?,
            ),
            Command::Commit(message) => display(
                self.workspace()?
                    .versions()
                    .commit(CommitRequest { message }, c)
                    .await?,
            ),
            Command::Sync(cmd) => {
                let v = self.workspace()?.versions();
                match cmd {
                    SyncCommand::All(message) => display(v.sync(SyncRequest { message }, c).await?),
                    SyncCommand::Pull => display(v.pull(c).await?),
                    SyncCommand::Push => display(v.push(c).await?),
                    SyncCommand::Fetch => display(v.fetch(c).await?),
                    SyncCommand::Remote(None) => display(v.remote(c).await?),
                    SyncCommand::Remote(Some(url)) => display(
                        v.set_remote(if url == "none" { None } else { Some(url) }, c)
                            .await?,
                    ),
                    SyncCommand::Conflicts => display(v.conflicts(c).await?),
                    SyncCommand::Resolve { path, file } => {
                        let content = match file {
                            Some(file) => Some(tokio::fs::read(file).await?),
                            None => None,
                        };
                        display(v.resolve(ResolveRequest { path, content }, c).await?)
                    }
                    SyncCommand::Continue => display(v.continue_merge(c).await?),
                    SyncCommand::Abort => display(v.abort_merge(c).await?),
                }
            }
            Command::History(id) => display(
                self.workspace()?
                    .versions()
                    .history(
                        HistoryQuery {
                            object_id: id.map(ObjectId::parse).transpose()?,
                        },
                        PageRequest::default(),
                        c,
                    )
                    .await?,
            ),
            Command::Diff(id) => {
                let diff = self
                    .workspace()?
                    .versions()
                    .diff(
                        DiffRequest {
                            object_id: id.map(ObjectId::parse).transpose()?,
                            ..Default::default()
                        },
                        c,
                    )
                    .await?;
                Ok(diff
                    .files
                    .into_iter()
                    .map(|f| {
                        format!(
                            "{}\n{}{}",
                            f.path,
                            f.patch,
                            if f.truncated { "\n[truncated]" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
            Command::Restore { id, commit } => {
                let w = self.workspace()?;
                let id = ObjectId::parse(id)?;
                let revision = match w
                    .knowledge()
                    .read(id.clone(), ReadView::Full, c.clone())
                    .await
                {
                    Ok(view) => ExpectedRevision::Match(view.revision),
                    Err(WorkspaceError::NotFound(_)) => ExpectedRevision::Absent,
                    Err(e) => return Err(e),
                };
                display(
                    w.versions()
                        .restore(
                            RestoreRequest {
                                commit: CommitId(commit),
                                items: vec![RestoreItem {
                                    id,
                                    expected_revision: revision,
                                }],
                            },
                            c,
                        )
                        .await?,
                )
            }
            Command::Index(cmd) => {
                let index = self.workspace()?.index();
                match cmd {
                    IndexCommand::Status => display(index.status(c).await?),
                    IndexCommand::Refresh => {
                        display(index.refresh(ChangeScope::default(), c).await?)
                    }
                    IndexCommand::Rebuild(selection) => {
                        let selected =
                            serde_json::from_value(serde_json::Value::String(selection))?;
                        display(index.rebuild(selected, c).await?)
                    }
                }
            }
            Command::Validate => display(
                self.workspace()?
                    .validate(ValidationScope::WorkingTree, c)
                    .await?,
            ),
            Command::Invalid(e) => Err(WorkspaceError::Configuration(e)),
            Command::Stop => Ok("no active operation".into()),
            Command::Quit => Ok("closing".into()),
        }
    }
    async fn chat(
        &mut self,
        line: String,
        c: CancellationToken,
        events: mpsc::UnboundedSender<AgentEvent>,
    ) -> WorkspaceResult<String> {
        let provider:Arc<dyn ModelProvider>=match(std::env::var("LUMEN_MODEL_ENDPOINT"),std::env::var("LUMEN_MODEL_KEY")){(Ok(url),Ok(key))=>Arc::new(HttpProvider::openai(url,key,self.model.clone())),_=>Arc::new(MockProvider::new(vec![AssistantMessage{content:vec![ContentPart::Text("Configure LUMEN_MODEL_ENDPOINT and LUMEN_MODEL_KEY to use a model. Local workspace commands are available.".into())],tool_calls:vec![]}]))};
        let mut agent = AgentLoop::new(provider).with_event_sink(events);
        if let Some(w) = &self.workspace {
            register_knowledge_tools(&mut agent, w.clone());
            agent.register_plugin(Arc::new(RetrievalPlugin::new(w.clone())));
        }
        let segment = if let Some(w) = &self.workspace {
            Some(
                w.sessions()
                    .begin(self.session_id.clone(), c.clone())
                    .await?,
            )
        } else {
            None
        };
        let before = self.state.messages.len();
        let result = agent
            .run(vec![ContentPart::Text(line)], &mut self.state, c)
            .await;
        // Persist only messages added by this run, including a cancelled run's completed messages.
        for message in &self.state.messages[before..] {
            if let (Some(w), Some(segment)) = (&self.workspace, &segment) {
                w.sessions()
                    .append(segment.segment_id.clone(), message.clone(), token())
                    .await?;
            } else {
                self.standalone.append(message).await?;
            }
        }
        if let (Some(w), Some(segment)) = (&self.workspace, segment) {
            w.sessions().finish(segment.segment_id, token()).await?;
        }
        let result = result.map_err(|e| WorkspaceError::Io(e.to_string()))?;
        if let Some(error) = result.error {
            return Err(WorkspaceError::Io(error.to_string()));
        }
        Ok(String::new())
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // SAFETY: no runtime or worker thread exists at process startup.
    unsafe {
        configure_transport_timeouts(15_000, 60_000)?;
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let session_path = std::env::var_os("LUMEN_SESSION")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("lumen-session.jsonl"));
    let mut app = Some(Application {
        workspace: None,
        state: AgentState {
            messages: vec![],
            turn_count: 0,
        },
        session_id: format!("session_{}", uuid::Uuid::now_v7()),
        model: std::env::var("LUMEN_MODEL").unwrap_or_else(|_| "mock".into()),
        standalone: SessionStore::new(session_path),
    });
    let (input_tx, mut input_rx) = mpsc::channel::<String>(64);
    std::thread::spawn(move || {
        let stdin = io::stdin();
        loop {
            let mut line = String::new();
            match stdin.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if input_tx.blocking_send(line).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let mut running: Option<tokio::task::JoinHandle<(Application, WorkspaceResult<String>)>> = None;
    let mut cancel = token();
    let mut pending: VecDeque<String> = VecDeque::new();
    let mut quit = false;
    let mut input_closed = false;
    let mut workspace_events: Option<tokio::sync::broadcast::Receiver<WorkspaceEvent>> = None;
    println!("Lumen — /help for commands");
    print!("> ");
    io::stdout().flush()?;
    loop {
        if running.is_none() {
            if quit || (input_closed && pending.is_empty()) {
                break;
            }
            if let Some(line) = pending.pop_front() {
                let mut state = app.take().unwrap();
                let events = event_tx.clone();
                cancel = token();
                let c = cancel.clone();
                running = Some(tokio::spawn(async move {
                    let result = if line.trim_start().starts_with('/') {
                        match commands::parse(&line) {
                            Some(command) => state.command(command, c).await,
                            None => Err(WorkspaceError::Configuration(
                                "unknown command; see /help".into(),
                            )),
                        }
                    } else {
                        state.chat(line, c, events).await
                    };
                    (state, result)
                }));
            }
        }
        tokio::select! {
            line=input_rx.recv(),if !quit && !input_closed=>{match line{Some(line)=>{let line=line.trim().to_string();if line.is_empty(){continue}match commands::parse(&line){Some(Command::Quit)=>{quit=true;pending.clear();cancel.cancel();},Some(Command::Stop)if running.is_some()=>{cancel.cancel();println!("cancellation requested");},_=>pending.push_back(line)}},None=>{input_closed=true;}}},
            event=event_rx.recv()=>{if let Some(event)=event{show_event(event)}},
            event=async{match workspace_events.as_mut(){Some(events)=>Some(events.recv().await),None=>std::future::pending().await}}=>{if let Some(Ok(event))=event{println!("[workspace] {:?}",event.kind);}},
            completed=async{match running.as_mut(){Some(task)=>Some(task.await),None=>std::future::pending().await}}=>{if let Some(completed)=completed{let(state,result)=completed?;workspace_events=state.workspace.as_ref().map(|w|w.subscribe());app=Some(state);running=None;match result{Ok(text)if !text.is_empty()=>println!("{text}"),Err(e)=>eprintln!("{e}"),_=>()}print!("> ");io::stdout().flush()?;}},
        }
    }
    while let Ok(event) = event_rx.try_recv() {
        show_event(event);
    }
    if let Some(w) = app.and_then(|a| a.workspace) {
        w.close(token()).await?;
    }
    Ok(())
}
