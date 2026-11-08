//! Single-owner background worker for model and conversation state.

use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use rillwright_agent::{
    Agent, AgentConfig, AgentEvent, Registry, ToolCtx,
    backend::EngineBackend,
    builtin::{
        command::RunCommand,
        read_tools,
        web::{WebSearch, secret_values},
        write_tools,
    },
    policy::{Answer, Approver, Permissions, Policy, Request},
    sandbox::{SandboxConfig, SandboxMode},
    session::{self, Session},
    web::{Brave, SearchProvider, Tavily, state},
    workspace::Workspace,
};
use rillwright_engine::{
    Engine,
    chat::{self, Message},
    verify,
};

use crate::{
    config::{AgentSettings, GenerationConfig, LoadConfig},
    protocol::{
        AppEvent, ModelGeneration, RuntimeState, SessionId, SessionKind, SessionSummary, TurnId,
    },
};

enum WorkerCommand {
    Load(ModelGeneration, LoadConfig),
    UpdateSettings(u64, GenerationConfig, AgentSettings),
    NewSession {
        id: SessionId,
        kind: SessionKind,
        workspace: Option<PathBuf>,
    },
    ListSessions(PathBuf),
    OpenSession {
        id: SessionId,
        workspace: PathBuf,
    },
    CloseSession(SessionId),
    Submit {
        session: SessionId,
        turn: TurnId,
        text: String,
    },
    InspectPrompt {
        system: String,
        user: String,
        thinking: bool,
    },
    Shutdown,
}

#[derive(Clone)]
pub struct ControllerHandle {
    tx: SyncSender<WorkerCommand>,
    cancel: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    answers: Arc<Mutex<HashMap<u64, SyncSender<Answer>>>>,
    busy: Arc<AtomicBool>,
}

impl ControllerHandle {
    pub fn load_model(
        &self,
        generation: ModelGeneration,
        config: LoadConfig,
    ) -> Result<(), String> {
        self.tx
            .try_send(WorkerCommand::Load(generation, config))
            .map_err(|e| format!("cannot queue model load: {e}"))
    }

    pub fn update_settings(
        &self,
        revision: u64,
        generation: GenerationConfig,
        agent: AgentSettings,
    ) -> Result<(), String> {
        self.tx
            .try_send(WorkerCommand::UpdateSettings(revision, generation, agent))
            .map_err(|e| format!("cannot apply settings: {e}"))
    }

    pub fn new_session(
        &self,
        id: impl Into<SessionId>,
        kind: SessionKind,
        workspace: Option<PathBuf>,
    ) -> Result<(), String> {
        self.tx
            .try_send(WorkerCommand::NewSession {
                id: id.into(),
                kind,
                workspace,
            })
            .map_err(|e| format!("cannot create session: {e}"))
    }

    pub fn list_sessions(&self, workspace: PathBuf) -> Result<(), String> {
        self.tx
            .try_send(WorkerCommand::ListSessions(workspace))
            .map_err(|e| format!("cannot list sessions: {e}"))
    }

    pub fn open_session(&self, id: impl Into<SessionId>, workspace: PathBuf) -> Result<(), String> {
        self.tx
            .try_send(WorkerCommand::OpenSession {
                id: id.into(),
                workspace,
            })
            .map_err(|e| format!("cannot open session: {e}"))
    }

    pub fn close_session(&self, id: impl Into<SessionId>) -> Result<(), String> {
        self.tx
            .try_send(WorkerCommand::CloseSession(id.into()))
            .map_err(|e| format!("cannot close session: {e}"))
    }

    pub fn inspect_prompt(
        &self,
        system: String,
        user: String,
        thinking: bool,
    ) -> Result<(), String> {
        self.tx
            .try_send(WorkerCommand::InspectPrompt {
                system,
                user,
                thinking,
            })
            .map_err(|e| format!("cannot queue prompt inspection: {e}"))
    }

    pub fn submit(
        &self,
        session: impl Into<SessionId>,
        turn: TurnId,
        text: String,
    ) -> Result<(), String> {
        self.busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| "a turn is already active".to_string())?;
        let result = self
            .tx
            .try_send(WorkerCommand::Submit {
                session: session.into(),
                turn,
                text,
            })
            .map_err(|e| format!("cannot submit turn: {e}"));
        if result.is_err() {
            self.busy.store(false, Ordering::SeqCst);
        }
        result
    }

    /// A direct cancellation path: it remains usable while the worker is in inference or a tool.
    pub fn stop(&self) {
        if let Ok(active) = self.cancel.lock() {
            if let Some(flag) = active.as_ref() {
                flag.store(true, Ordering::Relaxed);
            }
        }
    }

    pub fn answer_approval(&self, request: u64, answer: Answer) -> bool {
        self.answers
            .lock()
            .ok()
            .and_then(|mut map| map.remove(&request))
            .is_some_and(|tx| tx.try_send(answer).is_ok())
    }
}

pub struct Controller {
    pub handle: ControllerHandle,
    pub events: Receiver<AppEvent>,
    thread: Option<JoinHandle<()>>,
}

struct AppSession {
    kind: SessionKind,
    workspace: Option<PathBuf>,
    messages: Vec<Message>,
    transcript: Session,
}

fn sandbox_status(sandbox: &SandboxConfig, full_permission: bool) -> String {
    let label = if sandbox.active() {
        "bwrap active".into()
    } else if sandbox.mode == SandboxMode::None {
        "None: unconfined".into()
    } else if sandbox.mode == SandboxMode::Bwrap {
        "bwrap unavailable: commands blocked".into()
    } else {
        "Auto fallback: unconfined".into()
    };
    if full_permission && !sandbox.active() && sandbox.mode != SandboxMode::None {
        format!("{label}; Full mode blocks commands")
    } else {
        label
    }
}

fn web_status(settings: &AgentSettings) -> String {
    match (settings.web_enabled, settings.web_provider.as_str()) {
        (false, _) => "off".into(),
        (true, "brave")
            if std::env::var("BRAVE_SEARCH_API_KEY").is_ok_and(|k| !k.trim().is_empty()) =>
        {
            "brave configured".into()
        }
        (true, "brave") => "brave key missing".into(),
        (true, _) if std::env::var("TAVILY_API_KEY").is_ok_and(|k| !k.trim().is_empty()) => {
            "tavily configured".into()
        }
        (true, _) => "tavily key missing".into(),
    }
}

fn validate_agent_settings(settings: &AgentSettings) -> Result<(), String> {
    if !matches!(settings.sandbox.as_str(), "auto" | "bwrap" | "none") {
        return Err("sandbox must be auto, bwrap, or none".into());
    }
    if settings.max_rounds == 0
        || settings.max_duration.is_zero()
        || settings.command_timeout.is_zero()
    {
        return Err("agent round and time limits must be nonzero".into());
    }
    if !matches!(settings.web_provider.as_str(), "tavily" | "brave")
        || !(1..=20).contains(&settings.web_budget)
    {
        return Err("web provider or search budget is invalid".into());
    }
    for path in &settings.writable {
        if path.exists() && !path.is_dir() {
            return Err(format!(
                "writable path is not a directory: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn build_sandbox_config(settings: &AgentSettings) -> SandboxConfig {
    let mode = settings
        .sandbox
        .parse::<SandboxMode>()
        .unwrap_or(SandboxMode::Auto);
    let mut sandbox = SandboxConfig::new(mode);
    sandbox.network = settings.network;
    for path in &settings.writable {
        if let Ok(path) = path.canonicalize() {
            sandbox.writable.push(path);
        }
    }
    sandbox
}

impl Controller {
    pub fn spawn(load: LoadConfig, generation: GenerationConfig, agent: AgentSettings) -> Self {
        let (command_tx, command_rx) = mpsc::sync_channel(8);
        let (event_tx, event_rx) = mpsc::sync_channel(128);
        let cancel = Arc::new(Mutex::new(None));
        let answers = Arc::new(Mutex::new(HashMap::new()));
        let next_request = Arc::new(AtomicU64::new(1));
        let busy = Arc::new(AtomicBool::new(false));
        let handle = ControllerHandle {
            tx: command_tx,
            cancel: cancel.clone(),
            answers: answers.clone(),
            busy: busy.clone(),
        };
        let thread = thread::Builder::new()
            .name("rillwright-app".into())
            .spawn(move || {
                worker(
                    load,
                    generation,
                    agent,
                    command_rx,
                    event_tx,
                    cancel,
                    answers,
                    next_request,
                    busy,
                );
            })
            .expect("spawn application worker");
        Self {
            handle,
            events: event_rx,
            thread: Some(thread),
        }
    }

    pub fn request_shutdown(&self) {
        self.handle.stop();
        let _ = self.handle.tx.try_send(WorkerCommand::Shutdown);
    }

    pub fn join(&mut self) {
        self.request_shutdown();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        self.handle.stop();
        let _ = self.handle.tx.try_send(WorkerCommand::Shutdown);
        // Dropping JoinHandle detaches the worker. Engine destruction stays on its owner thread
        // and a slow native load can never block the window's close callback.
        self.thread.take();
    }
}

fn worker(
    mut load: LoadConfig,
    mut generation: GenerationConfig,
    mut default_agent: AgentSettings,
    commands: Receiver<WorkerCommand>,
    events: SyncSender<AppEvent>,
    active_cancel: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    answers: Arc<Mutex<HashMap<u64, SyncSender<Answer>>>>,
    next_request: Arc<AtomicU64>,
    busy: Arc<AtomicBool>,
) {
    let mut engine: Option<Engine> = None;
    let mut sessions: HashMap<SessionId, AppSession> = HashMap::new();
    let mut active_agent_session: Option<SessionId> = None;
    let mut state = RuntimeState::Unloaded;
    let mut settings_revision = 0u64;
    let mut sandbox_cache = build_sandbox_config(&default_agent);
    while let Ok(command) = commands.recv() {
        match command {
            WorkerCommand::UpdateSettings(revision, next_generation, next_agent) => {
                let validation = next_generation
                    .validate()
                    .and_then(|()| validate_agent_settings(&next_agent));
                match validation {
                    Ok(()) => {
                        generation = next_generation;
                        default_agent = next_agent;
                        settings_revision = revision;
                        sandbox_cache = build_sandbox_config(&default_agent);
                        let _ = deliver(
                            &events,
                            &active_cancel,
                            AppEvent::SettingsApplied { revision },
                        );
                        if let Some(session) = active_agent_session.as_ref()
                            && let Some(active) = sessions.get(session)
                            && let Some(workspace) = active.workspace.as_ref()
                        {
                            let sandbox = sandbox_cache.clone();
                            let status = AppEvent::AgentStatus {
                                session: session.clone(),
                                config_revision: settings_revision,
                                workspace: workspace.clone(),
                                permission: default_agent.permission.as_str().into(),
                                sandbox: sandbox_status(
                                    &sandbox,
                                    default_agent.permission
                                        == rillwright_agent::policy::Mode::Full,
                                ),
                                command_network: default_agent.network,
                                web_search: web_status(&default_agent),
                            };
                            let _ = deliver(&events, &active_cancel, status);
                        }
                    }
                    Err(message) => {
                        let _ = deliver(
                            &events,
                            &active_cancel,
                            AppEvent::SettingsRejected { revision, message },
                        );
                    }
                }
            }
            WorkerCommand::Load(id, config) => {
                if !state.can_load() {
                    continue;
                }
                load = config;
                state = RuntimeState::Verifying;
                let _ = deliver(
                    &events,
                    &active_cancel,
                    AppEvent::LoadStage {
                        model_generation: id,
                        stage: "Checking pinned fork and model checksum".into(),
                    },
                );
                let result = (|| -> Result<Engine, String> {
                    verify::verify_fork().map_err(|e| e.to_string())?;
                    verify::verify_model(&load.model).map_err(|e| e.to_string())?;
                    let options = load.engine_options()?;
                    state = RuntimeState::Unloading;
                    drop(engine.take());
                    state = RuntimeState::Loading;
                    let _ = deliver(
                        &events,
                        &active_cancel,
                        AppEvent::LoadStage {
                            model_generation: id,
                            stage: "Loading model weights and context".into(),
                        },
                    );
                    let engine = Engine::start(options).map_err(|e| e.to_string())?;
                    let template = engine.info.chat_template.clone().unwrap_or_default();
                    chat::check_template(&template).map_err(|e| e.to_string())?;
                    Ok(engine)
                })();
                match result {
                    Ok(next) => {
                        state = RuntimeState::Ready;
                        let loops = next.info.loops;
                        let details = format!(
                            "{} · context {} · loops {} · cache {} · batch {}/{}",
                            load.model.display(),
                            next.info.n_ctx,
                            next.info.loops,
                            next.info.cache_mode.as_str(),
                            next.info.n_batch,
                            next.info.n_ubatch
                        );
                        engine = Some(next);
                        let _ = deliver(
                            &events,
                            &active_cancel,
                            AppEvent::ModelReady {
                                model_generation: id,
                                details,
                                loops,
                            },
                        );
                    }
                    Err(e) => {
                        state = RuntimeState::Failed;
                        let _ = deliver(
                            &events,
                            &active_cancel,
                            AppEvent::Error {
                                model_generation: id,
                                session: None,
                                turn: None,
                                message: e,
                            },
                        );
                    }
                }
            }
            WorkerCommand::NewSession {
                id,
                kind,
                workspace,
            } => {
                let mut messages = Vec::new();
                let root = if kind == SessionKind::Agent {
                    match workspace
                        .as_deref()
                        .and_then(|p| p.canonicalize().ok())
                        .filter(|path| path.is_dir())
                    {
                        Some(path) => path,
                        None => {
                            let _ = deliver(
                                &events,
                                &active_cancel,
                                AppEvent::Error {
                                    model_generation: 0,
                                    session: Some(id),
                                    turn: None,
                                    message: "Workspace does not exist or cannot be opened.".into(),
                                },
                            );
                            continue;
                        }
                    }
                } else {
                    match chat_data_root() {
                        Ok(path) => path,
                        Err(message) => {
                            let _ = deliver(
                                &events,
                                &active_cancel,
                                AppEvent::Error {
                                    model_generation: 0,
                                    session: Some(id),
                                    turn: None,
                                    message,
                                },
                            );
                            continue;
                        }
                    }
                };
                if kind == SessionKind::Agent {
                    if let Some(root) = workspace.as_deref() {
                        let agents_md = std::fs::read_to_string(root.join("AGENTS.md")).ok();
                        messages.push(Message::new(
                            "system",
                            &rillwright_agent::prompt::system_prompt(
                                root,
                                "linux",
                                agents_md.as_deref(),
                                None,
                                false,
                            ),
                        ));
                    }
                }
                let header = serde_json::json!({"kind": if kind == SessionKind::Agent { "agent" } else { "chat" }, "model": load.model.display().to_string(), "loops": load.loops, "cache": load.cache_mode.as_str(), "permission": default_agent.permission.as_str(), "workspace": if kind == SessionKind::Agent { Some(root.display().to_string()) } else { None }});
                let mut transcript = match Session::create(&root, header) {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = deliver(
                            &events,
                            &active_cancel,
                            AppEvent::Error {
                                model_generation: 0,
                                session: Some(id),
                                turn: None,
                                message: format!("Cannot create session transcript: {e}"),
                            },
                        );
                        continue;
                    }
                };
                let mut record_error = None;
                for message in &messages {
                    if let Err(e) = transcript.record(&AgentEvent::MessageAdded(message.clone())) {
                        record_error = Some(e);
                        break;
                    }
                }
                if let Some(e) = record_error {
                    let _ = deliver(
                        &events,
                        &active_cancel,
                        AppEvent::Error {
                            model_generation: 0,
                            session: Some(id),
                            turn: None,
                            message: format!("Cannot write session transcript: {e}"),
                        },
                    );
                    continue;
                }
                let transcript_id = transcript.id.clone();
                sessions.insert(
                    id.clone(),
                    AppSession {
                        kind,
                        workspace: (kind == SessionKind::Agent).then(|| root.clone()),
                        messages: messages.clone(),
                        transcript,
                    },
                );
                if kind == SessionKind::Agent {
                    active_agent_session = Some(id.clone());
                }
                let _ = deliver(
                    &events,
                    &active_cancel,
                    AppEvent::SessionCreated {
                        session: id.clone(),
                        transcript_id,
                        kind,
                        workspace: (kind == SessionKind::Agent).then(|| root.clone()),
                        continuation_differences: Vec::new(),
                        writable: true,
                    },
                );
                if kind == SessionKind::Agent {
                    let sandbox = sandbox_cache.clone();
                    let sandbox_status = sandbox_status(
                        &sandbox,
                        default_agent.permission == rillwright_agent::policy::Mode::Full,
                    );
                    let web_search = web_status(&default_agent);
                    let _ = deliver(
                        &events,
                        &active_cancel,
                        AppEvent::AgentStatus {
                            session: id.clone(),
                            config_revision: settings_revision,
                            workspace: root.clone(),
                            permission: default_agent.permission.as_str().into(),
                            sandbox: sandbox_status,
                            command_network: default_agent.network,
                            web_search,
                        },
                    );
                }
                let _ = deliver(
                    &events,
                    &active_cancel,
                    AppEvent::SessionSnapshot {
                        session: id,
                        messages,
                    },
                );
            }
            WorkerCommand::ListSessions(root) => {
                let sessions = session::list(&root)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|s| SessionSummary {
                        id: s.id,
                        modified: s.last_modified,
                        records: s.records,
                    })
                    .collect();
                let _ = deliver(
                    &events,
                    &active_cancel,
                    AppEvent::SessionList {
                        workspace: root,
                        sessions,
                    },
                );
            }
            WorkerCommand::OpenSession { id, workspace } => match Session::open(&workspace, &id) {
                Ok((transcript, messages, header)) => {
                    let kind =
                        if header.get("kind").and_then(serde_json::Value::as_str) == Some("chat") {
                            SessionKind::Chat
                        } else {
                            SessionKind::Agent
                        };
                    let ui_id = format!("resumed-{id}");
                    let transcript_id = transcript.id.clone();
                    let mut differences = Vec::new();
                    if header
                        .get("model")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|m| m != load.model.to_string_lossy())
                    {
                        differences.push("model file differs from the saved session".into());
                    }
                    if header
                        .get("loops")
                        .and_then(serde_json::Value::as_u64)
                        .is_some_and(|n| n != u64::from(load.loops))
                    {
                        differences.push("recurrence loop count differs".into());
                    }
                    if header
                        .get("permission")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|m| m != default_agent.permission.as_str())
                    {
                        differences
                            .push("permission mode differs; current settings will be used".into());
                    }
                    sessions.insert(
                        ui_id.clone(),
                        AppSession {
                            kind,
                            workspace: (kind == SessionKind::Agent).then(|| workspace.clone()),
                            messages: messages.clone(),
                            transcript,
                        },
                    );
                    if kind == SessionKind::Agent {
                        active_agent_session = Some(ui_id.clone());
                    }
                    let _ = deliver(
                        &events,
                        &active_cancel,
                        AppEvent::SessionCreated {
                            session: ui_id.clone(),
                            transcript_id,
                            kind,
                            workspace: (kind == SessionKind::Agent).then_some(workspace),
                            continuation_differences: differences,
                            writable: true,
                        },
                    );
                    let _ = deliver(
                        &events,
                        &active_cancel,
                        AppEvent::SessionSnapshot {
                            session: ui_id,
                            messages,
                        },
                    );
                }
                Err(e) if e.to_string().contains("locked by another writer") => {
                    match Session::read_only(&workspace, &id) {
                        Ok((messages, header)) => {
                            let kind = if header.get("kind").and_then(serde_json::Value::as_str)
                                == Some("chat")
                            {
                                SessionKind::Chat
                            } else {
                                SessionKind::Agent
                            };
                            let ui_id = format!("readonly-{id}");
                            let _ = deliver(
                                &events,
                                &active_cancel,
                                AppEvent::SessionCreated {
                                    session: ui_id.clone(),
                                    transcript_id: id.clone(),
                                    kind,
                                    workspace: (kind == SessionKind::Agent).then_some(workspace),
                                    continuation_differences: Vec::new(),
                                    writable: false,
                                },
                            );
                            let _ = deliver(
                                &events,
                                &active_cancel,
                                AppEvent::SessionSnapshot {
                                    session: ui_id,
                                    messages,
                                },
                            );
                        }
                        Err(read_error) => {
                            let _ = deliver(
                                &events,
                                &active_cancel,
                                AppEvent::Error {
                                    model_generation: 0,
                                    session: None,
                                    turn: None,
                                    message: format!(
                                        "Cannot inspect locked session {id}: {read_error}"
                                    ),
                                },
                            );
                        }
                    }
                }
                Err(e) => {
                    let _ = deliver(
                        &events,
                        &active_cancel,
                        AppEvent::Error {
                            model_generation: 0,
                            session: None,
                            turn: None,
                            message: format!("Cannot open session {id}: {e}"),
                        },
                    );
                }
            },
            WorkerCommand::CloseSession(id) => {
                sessions.remove(&id);
                if active_agent_session.as_deref() == Some(&id) {
                    active_agent_session = None;
                }
            }
            WorkerCommand::InspectPrompt {
                system,
                user,
                thinking,
            } => {
                let result = (|| -> Result<_, String> {
                    let engine = engine
                        .as_ref()
                        .ok_or("Load the model before inspecting a prompt")?;
                    let mut messages = Vec::new();
                    if !system.trim().is_empty() {
                        messages.push(Message::new("system", &system));
                    }
                    messages.push(Message::new("user", &user));
                    let prompt =
                        chat::render(&messages, &[], thinking).map_err(|e| e.to_string())?;
                    let tokens = engine
                        .tokenize(&prompt, false, true)
                        .map_err(|e| e.to_string())?;
                    Ok((prompt, tokens.len(), engine.info.n_ctx, engine.info.loops))
                })();
                match result {
                    Ok((rendered_prompt, token_count, context, loops)) => {
                        let _ = deliver(
                            &events,
                            &active_cancel,
                            AppEvent::PromptInspection {
                                rendered_prompt,
                                token_count,
                                context,
                                loops,
                            },
                        );
                    }
                    Err(message) => {
                        let _ = deliver(
                            &events,
                            &active_cancel,
                            AppEvent::Error {
                                model_generation: 0,
                                session: None,
                                turn: None,
                                message,
                            },
                        );
                    }
                }
            }
            WorkerCommand::Submit {
                session,
                turn,
                text,
            } => {
                let _busy_reset = BusyReset(busy.clone());
                let Some(engine) = engine.as_ref() else {
                    let _ = deliver(
                        &events,
                        &active_cancel,
                        AppEvent::Error {
                            model_generation: 0,
                            session: Some(session.clone()),
                            turn: Some(turn),
                            message: "Load the model before sending a message.".into(),
                        },
                    );
                    let _ = deliver(
                        &events,
                        &active_cancel,
                        AppEvent::TurnFinished {
                            session,
                            turn,
                            reason: "error: model unavailable".into(),
                        },
                    );
                    continue;
                };
                if !state.can_submit() {
                    let _ = deliver(
                        &events,
                        &active_cancel,
                        AppEvent::Error {
                            model_generation: 0,
                            session: Some(session.clone()),
                            turn: Some(turn),
                            message: "The model is already handling a turn.".into(),
                        },
                    );
                    let _ = deliver(
                        &events,
                        &active_cancel,
                        AppEvent::TurnFinished {
                            session,
                            turn,
                            reason: "error: runtime busy".into(),
                        },
                    );
                    continue;
                }
                let Some(session_data) = sessions.get_mut(&session) else {
                    let _ = deliver(
                        &events,
                        &active_cancel,
                        AppEvent::Error {
                            model_generation: 0,
                            session: Some(session.clone()),
                            turn: Some(turn),
                            message: "Create a conversation before sending a message.".into(),
                        },
                    );
                    let _ = deliver(
                        &events,
                        &active_cancel,
                        AppEvent::TurnFinished {
                            session,
                            turn,
                            reason: "error: session not found".into(),
                        },
                    );
                    continue;
                };
                let AppSession {
                    kind,
                    workspace: workspace_path,
                    messages,
                    transcript,
                    ..
                } = session_data;
                if *kind == SessionKind::Agent && workspace_path.is_none() {
                    let _ = deliver(
                        &events,
                        &active_cancel,
                        AppEvent::Error {
                            model_generation: 0,
                            session: Some(session.clone()),
                            turn: Some(turn),
                            message: "Choose a workspace before starting an agent session.".into(),
                        },
                    );
                    let _ = deliver(
                        &events,
                        &active_cancel,
                        AppEvent::TurnFinished {
                            session,
                            turn,
                            reason: "error: workspace required".into(),
                        },
                    );
                    continue;
                }
                let turn_cancel = Arc::new(AtomicBool::new(false));
                if let Ok(mut active) = active_cancel.lock() {
                    *active = Some(turn_cancel.clone());
                }
                messages.push(Message::new("user", &text));
                if let Err(e) =
                    transcript.record(&AgentEvent::MessageAdded(messages.last().unwrap().clone()))
                {
                    state = RuntimeState::Ready;
                    if let Ok(mut active) = active_cancel.lock() {
                        *active = None;
                    }
                    let _ = deliver(
                        &events,
                        &active_cancel,
                        AppEvent::Error {
                            model_generation: 0,
                            session: Some(session.clone()),
                            turn: Some(turn),
                            message: format!("Cannot write session transcript: {e}"),
                        },
                    );
                    let _ = deliver(
                        &events,
                        &active_cancel,
                        AppEvent::TurnFinished {
                            session,
                            turn,
                            reason: "error: transcript write failed".into(),
                        },
                    );
                    continue;
                }
                let _ = deliver(
                    &events,
                    &active_cancel,
                    AppEvent::SessionSnapshot {
                        session: session.clone(),
                        messages: messages.clone(),
                    },
                );
                let tools_workspace = workspace_path.as_deref().unwrap_or_else(|| {
                    load.model
                        .parent()
                        .unwrap_or_else(|| std::path::Path::new("."))
                });
                let workspace = match Workspace::new(tools_workspace, &[]) {
                    Ok(w) => w,
                    Err(e) => {
                        state = RuntimeState::Ready;
                        if let Ok(mut active) = active_cancel.lock() {
                            *active = None;
                        }
                        let _ = deliver(
                            &events,
                            &active_cancel,
                            AppEvent::TurnFinished {
                                session,
                                turn,
                                reason: format!("error: {e}"),
                            },
                        );
                        continue;
                    }
                };
                let ctx = ToolCtx::with_cancel(workspace, turn_cancel.clone());
                let mut registry = Registry::new(ctx);
                let approval = GuiApprover {
                    events: events.clone(),
                    cancel: turn_cancel.clone(),
                    answers: answers.clone(),
                    next_request: next_request.clone(),
                    session: session.clone(),
                    turn,
                };
                let policy = if *kind == SessionKind::Chat {
                    Policy::allow_all()
                } else {
                    for tool in read_tools().into_iter().chain(write_tools()) {
                        let _ = registry.add(tool);
                    }
                    let sandbox = sandbox_cache.clone();
                    let sandbox_mode = sandbox.mode;
                    let sandbox_status = sandbox_status(
                        &sandbox,
                        default_agent.permission == rillwright_agent::policy::Mode::Full,
                    );
                    registry.ctx.command_sandbox = Some(sandbox_status.clone());
                    let mut web_status = web_status(&default_agent);
                    if let Some(quota_time) = state::quota_used_up_at(&state::state_path(
                        workspace_path.as_ref().unwrap(),
                    )) {
                        web_status.push_str(&format!(" · quota last exhausted {quota_time}"));
                    }
                    let _ = deliver(
                        &events,
                        &active_cancel,
                        AppEvent::AgentStatus {
                            session: session.clone(),
                            config_revision: settings_revision,
                            workspace: workspace_path.as_ref().unwrap().clone(),
                            permission: default_agent.permission.as_str().into(),
                            sandbox: sandbox_status,
                            command_network: default_agent.network,
                            web_search: web_status,
                        },
                    );
                    let _ = registry.add(Box::new(RunCommand {
                        sandbox,
                        env_pass: default_agent.env_pass.clone(),
                        require_sandbox: default_agent.permission
                            == rillwright_agent::policy::Mode::Full
                            && sandbox_mode != SandboxMode::None,
                        default_timeout_secs: default_agent.command_timeout.as_secs().min(600),
                    }));
                    if default_agent.web_enabled {
                        let (key_name, base, provider_kind) =
                            match default_agent.web_provider.as_str() {
                                "brave" => (
                                    "BRAVE_SEARCH_API_KEY",
                                    "https://api.search.brave.com",
                                    "brave",
                                ),
                                _ => ("TAVILY_API_KEY", "https://api.tavily.com", "tavily"),
                            };
                        if let Some(key) = std::env::var(key_name)
                            .ok()
                            .filter(|key| !key.trim().is_empty())
                        {
                            let base = rillwright_agent::web::base_url(base);
                            let provider: Box<dyn SearchProvider> = if provider_kind == "brave" {
                                Box::new(Brave::new(&key, &base))
                            } else {
                                Box::new(Tavily::new(&key, &base))
                            };
                            let secrets = secret_values(&registry.ctx.workspace.secret_files())
                                .into_iter()
                                .chain([key])
                                .collect::<Vec<_>>();
                            let web_tool = WebSearch::new(provider, default_agent.web_budget)
                                .with_secrets(secrets)
                                .with_state_file(state::state_path(registry.ctx.workspace.root()));
                            let _ = registry.add(Box::new(web_tool));
                        }
                    }
                    Policy::new(
                        Permissions {
                            mode: default_agent.permission,
                            commands: rillwright_agent::policy::CommandRules::new(
                                &default_agent.allow_commands,
                            ),
                            allow_web: default_agent.allow_web,
                        },
                        Box::new(approval),
                    )
                };
                let backend = EngineBackend { engine };
                let mut config = AgentConfig::new(generation.thinking, generation.params.clone());
                config.max_rounds = default_agent.max_rounds;
                config.max_duration = Some(default_agent.max_duration);
                let agent = Agent {
                    backend: &backend,
                    tools: &registry,
                    policy: &policy,
                    config,
                    cancel: turn_cancel.clone(),
                };
                let mut terminal_seen = false;
                let session_for_events = session.clone();
                let mut display_messages = messages.clone();
                let mut event_sequence = 0u64;
                let mut detected_calls: VecDeque<(String, String)> = VecDeque::new();
                let mut committed_calls: VecDeque<(String, String)> = VecDeque::new();
                let mut detection_batch_closed = false;
                let mut callback = |event: AgentEvent| {
                    if let Err(error) = transcript.record(&event) {
                        turn_cancel.store(true, Ordering::Relaxed);
                        let _ = deliver(
                            &events,
                            &active_cancel,
                            AppEvent::Error {
                                model_generation: 0,
                                session: Some(session_for_events.clone()),
                                turn: Some(turn),
                                message: format!(
                                    "Session persistence failed; the turn was stopped: {error}"
                                ),
                            },
                        );
                    }
                    event_sequence = event_sequence.wrapping_add(1);
                    let sequence = event_sequence;
                    let converted = match event {
                        AgentEvent::Content(text) => Some(AppEvent::Delta {
                            session: session_for_events.clone(),
                            turn,
                            sequence,
                            reasoning: false,
                            text,
                        }),
                        AgentEvent::Reasoning(text) => Some(AppEvent::Delta {
                            session: session_for_events.clone(),
                            turn,
                            sequence,
                            reasoning: true,
                            text,
                        }),
                        AgentEvent::ToolCall { name, arguments } => {
                            if detection_batch_closed {
                                detected_calls.clear();
                                detection_batch_closed = false;
                            }
                            let call_id = format!("{}:{turn}:{sequence}", session_for_events);
                            detected_calls.push_back((call_id.clone(), name.clone()));
                            Some(AppEvent::ToolDetected {
                                session: session_for_events.clone(),
                                turn,
                                sequence,
                                call_id,
                                name,
                                arguments,
                            })
                        }
                        AgentEvent::ToolResult {
                            name,
                            text,
                            is_error,
                            truncated,
                        } => {
                            let call_id = committed_calls
                                .pop_front()
                                .filter(|(_, queued_name)| queued_name == &name)
                                .map(|(id, _)| id);
                            Some(AppEvent::ToolResolved {
                                session: session_for_events.clone(),
                                turn,
                                sequence,
                                call_id,
                                name,
                                text,
                                is_error,
                                truncated,
                            })
                        }
                        AgentEvent::RoundStats(stats) => {
                            detection_batch_closed = true;
                            Some(AppEvent::Stats {
                                session: session_for_events.clone(),
                                turn,
                                stats,
                            })
                        }
                        AgentEvent::MessageAdded(message) => {
                            if message.role == "assistant" {
                                let mut candidates = std::mem::take(&mut detected_calls);
                                detection_batch_closed = false;
                                for call in &message.tool_calls {
                                    if let Some(position) =
                                        candidates.iter().position(|(_, name)| name == &call.name)
                                    {
                                        if let Some(bound) = candidates.remove(position) {
                                            committed_calls.push_back(bound);
                                        }
                                    }
                                }
                                // Uncommitted detections are discarded by the agent and never enter the result queue.
                                let message_position = display_messages.len();
                                for (ordinal, (call, (call_id, name))) in message
                                    .tool_calls
                                    .iter()
                                    .zip(committed_calls.iter())
                                    .enumerate()
                                {
                                    if name == &call.name {
                                        let _ = deliver(
                                            &events,
                                            &active_cancel,
                                            AppEvent::ToolBound {
                                                session: session_for_events.clone(),
                                                turn,
                                                call_id: call_id.clone(),
                                                item_id: format!(
                                                    "message:{message_position}:call:{ordinal}"
                                                ),
                                            },
                                        );
                                    }
                                }
                            }
                            display_messages.push(message);
                            Some(AppEvent::SessionSnapshot {
                                session: session_for_events.clone(),
                                messages: display_messages.clone(),
                            })
                        }
                        AgentEvent::Compacted {
                            stage,
                            before,
                            after,
                        } => Some(AppEvent::AgentNotice {
                            session: session_for_events.clone(),
                            turn,
                            sequence,
                            title: "Context compacted".into(),
                            detail: format!("stage {stage}: {before} → {after} tokens"),
                        }),
                        AgentEvent::Finished(reason) => {
                            terminal_seen = true;
                            Some(AppEvent::TurnFinished {
                                session: session_for_events.clone(),
                                turn,
                                reason: reason.as_str().into(),
                            })
                        }
                        AgentEvent::MalformedCall(raw) => Some(AppEvent::AgentNotice {
                            session: session_for_events.clone(),
                            turn,
                            sequence,
                            title: "Malformed tool call".into(),
                            detail: raw,
                        }),
                    };
                    if let Some(ev) = converted {
                        let _ = deliver(&events, &active_cancel, ev);
                    }
                };
                let result = agent.run_turn(messages, &mut callback);
                *messages = display_messages;
                if let Err(error) = result {
                    if !terminal_seen {
                        let _ = deliver(
                            &events,
                            &active_cancel,
                            AppEvent::TurnFinished {
                                session: session.clone(),
                                turn,
                                reason: format!("error: {error}"),
                            },
                        );
                    }
                }
                if let Ok(mut active) = active_cancel.lock() {
                    *active = None;
                }
                state = RuntimeState::Ready;
                let _ = deliver(
                    &events,
                    &active_cancel,
                    AppEvent::SessionSnapshot {
                        session,
                        messages: messages.clone(),
                    },
                );
            }
            WorkerCommand::Shutdown => {
                let shutdown_state = RuntimeState::ShuttingDown;
                debug_assert_eq!(shutdown_state, RuntimeState::ShuttingDown);
                drop(engine.take());
                let _ = deliver(&events, &active_cancel, AppEvent::ShutdownComplete);
                break;
            }
        }
    }
}

struct GuiApprover {
    events: SyncSender<AppEvent>,
    cancel: Arc<AtomicBool>,
    answers: Arc<Mutex<HashMap<u64, SyncSender<Answer>>>>,
    next_request: Arc<AtomicU64>,
    session: SessionId,
    turn: TurnId,
}

impl Approver for GuiApprover {
    fn ask(&self, request: &Request) -> Answer {
        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::sync_channel(1);
        if let Ok(mut answers) = self.answers.lock() {
            answers.insert(id, tx);
        } else {
            return Answer::Unavailable;
        }
        let event = AppEvent::Approval {
            session: self.session.clone(),
            turn: self.turn,
            request: id,
            tool: request.tool.clone(),
            summary: request.summary.clone(),
            risk: format!("{:?}", request.risk),
            workspace: request.workspace.clone(),
            working_directory: request.working_directory.clone(),
            arguments: request.arguments.clone(),
            command: request.command.clone(),
            sandbox: request.sandbox.clone(),
        };
        if self.events.send(event).is_err() {
            if let Ok(mut answers) = self.answers.lock() {
                answers.remove(&id);
            }
            return Answer::Unavailable;
        }
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                if let Ok(mut answers) = self.answers.lock() {
                    answers.remove(&id);
                }
                return Answer::NoWith("the user cancelled the turn".into());
            }
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(answer) => return answer,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return Answer::Unavailable,
            }
        }
    }
}

fn deliver(
    events: &SyncSender<AppEvent>,
    _cancel: &Arc<Mutex<Option<Arc<AtomicBool>>>>,
    mut event: AppEvent,
) -> Result<(), ()> {
    loop {
        match events.try_send(event) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Disconnected(_)) => return Err(()),
            Err(TrySendError::Full(returned)) => {
                event = returned;
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_status_matches_requested_enforcement() {
        let active = SandboxConfig {
            mode: SandboxMode::Auto,
            network: false,
            writable: Vec::new(),
            home: None,
            bwrap: Some(PathBuf::from("/usr/bin/bwrap")),
        };
        let fallback = SandboxConfig {
            bwrap: None,
            ..active.clone()
        };
        let required = SandboxConfig {
            mode: SandboxMode::Bwrap,
            bwrap: None,
            ..active.clone()
        };
        let none = SandboxConfig {
            mode: SandboxMode::None,
            bwrap: None,
            ..active.clone()
        };
        assert_eq!(sandbox_status(&active, false), "bwrap active");
        assert_eq!(
            sandbox_status(&fallback, false),
            "Auto fallback: unconfined"
        );
        assert_eq!(
            sandbox_status(&required, false),
            "bwrap unavailable: commands blocked"
        );
        assert_eq!(sandbox_status(&none, false), "None: unconfined");
    }

    #[test]
    fn invalid_settings_are_rejected_instead_of_falling_back() {
        let mut settings = AgentSettings::default();
        settings.sandbox = "invalid".into();
        assert!(
            validate_agent_settings(&settings)
                .unwrap_err()
                .contains("sandbox")
        );
        settings.sandbox = "bwrap".into();
        assert!(validate_agent_settings(&settings).is_ok());
    }

    #[test]
    fn legacy_chat_root_wins_even_when_both_directories_exist() {
        let temp = tempfile::tempdir().unwrap();
        let legacy = temp.path().join("dory-harness-chat");
        std::fs::create_dir(&legacy).unwrap();
        assert_eq!(chat_data_root_from(temp.path()).unwrap(), legacy);
        std::fs::create_dir(temp.path().join("rillwright-chat")).unwrap();
        assert_eq!(chat_data_root_from(temp.path()).unwrap(), legacy);
    }

    #[test]
    fn fresh_chat_root_uses_the_canonical_directory() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            chat_data_root_from(temp.path()).unwrap(),
            temp.path().join("rillwright-chat")
        );
    }
}

fn chat_data_root() -> Result<PathBuf, String> {
    static ROOT: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    ROOT.get_or_init(|| {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
            .unwrap_or_else(std::env::temp_dir);
        chat_data_root_from(&base)
    })
    .clone()
}

fn chat_data_root_from(base: &Path) -> Result<PathBuf, String> {
    let legacy = base.join("dory-harness-chat");
    let canonical = base.join("rillwright-chat");
    let exists = |path: &PathBuf| match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(true),
        Ok(_) => Err(format!(
            "Chat storage path is not a directory: {}",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!(
            "Cannot access chat storage {}: {error}",
            path.display()
        )),
    };
    let has_legacy = exists(&legacy)?;
    let has_canonical = exists(&canonical)?;
    if has_legacy && has_canonical {
        eprintln!(
            "Both chat storage roots exist; continuing to use legacy {}",
            legacy.display()
        );
    }
    Ok(if has_legacy { legacy } else { canonical })
}

struct BusyReset(Arc<AtomicBool>);
impl Drop for BusyReset {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
