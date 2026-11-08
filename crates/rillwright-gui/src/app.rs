use std::{
    collections::HashMap,
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::conversation::{
    ConversationItem, ToolCallView, ToolState, project_messages, summary as tool_summary,
};
use crate::settings::UserSettings;
use crate::startup::{LaunchOptions, ModelSource, resolve};
use eframe::egui::{self, Align, Layout, RichText, ScrollArea, TextEdit};
use rillwright_agent::policy::Answer;
use rillwright_app::{
    controller::Controller,
    protocol::{AppEvent, SessionKind},
};

#[derive(Clone)]
struct ModeSession {
    id: String,
    messages: Vec<ConversationItem>,
    next_turn: u64,
    transcript_id: String,
    writable: bool,
    agent_status: String,
    workspace: Option<PathBuf>,
}

enum PickerResult {
    Model(Option<PathBuf>),
    Workspace(Option<PathBuf>),
    Diagnostic(Option<PathBuf>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NavigationAction {
    ShowCurrent,
    Switch,
    Block,
}

fn navigation_action(
    target: SessionKind,
    current: Option<SessionKind>,
    pending: Option<SessionKind>,
    busy: bool,
) -> NavigationAction {
    if current == Some(target) || pending == Some(target) {
        NavigationAction::ShowCurrent
    } else if busy {
        NavigationAction::Block
    } else {
        NavigationAction::Switch
    }
}

fn acknowledge_model_load(
    current_generation: u64,
    event_generation: u64,
    loops: u32,
    loaded_loops: &mut Option<u32>,
    loading_loops: &mut Option<u32>,
) -> bool {
    if current_generation != event_generation {
        return false;
    }
    *loaded_loops = Some(loops);
    *loading_loops = None;
    true
}

pub struct RillwrightApp {
    demo: bool,
    model_path: String,
    model_file: Option<PathBuf>,
    model_source: ModelSource,
    status: String,
    error: Option<String>,
    messages: Vec<ConversationItem>,
    composer: String,
    running: bool,
    pending_turn_text: Option<(String, u64, String)>,
    demo_due: Option<Instant>,
    demo_phase: usize,
    approval: bool,
    selected: &'static str,
    controller: Controller,
    model_generation: u64,
    loaded_loops: Option<u32>,
    loading_loops: Option<u32>,
    session_id: String,
    next_turn: u64,
    workspace: String,
    activity: Vec<String>,
    runtime_ready: bool,
    model_loading: bool,
    deferred_model_load: bool,
    pending_approval: Option<u64>,
    approval_reason: String,
    approval_summary: String,
    approval_tool: String,
    approval_risk: String,
    approval_workspace: String,
    approval_cwd: String,
    approval_arguments: String,
    approval_command: String,
    approval_sandbox: String,
    agent_session_started: bool,
    session_pending: bool,
    pending_session_id: Option<String>,
    session_kind_pending: Option<SessionKind>,
    current_session_kind: Option<SessionKind>,
    active_workspace: Option<PathBuf>,
    mode_sessions: HashMap<SessionKind, ModeSession>,
    workspace_error: Option<String>,
    agent_status: String,
    settings_revision: u64,
    pending_settings_revision: Option<u64>,
    agent_status_revision: u64,
    picker_tx: mpsc::Sender<PickerResult>,
    picker_rx: Receiver<PickerResult>,
    picker_active: bool,
    settings: UserSettings,
    invalid_saved_loops: bool,
    transcript_id: String,
    history_workspace: String,
    history: Vec<rillwright_app::protocol::SessionSummary>,
    continuation_differences: Vec<String>,
    resume_pending: bool,
    close_dialog: bool,
    close_armed: bool,
    session_writable: bool,
    inspect_system: String,
    inspect_user: String,
    inspected_prompt: String,
    inspected_prompt_tokens: Option<usize>,
}

impl RillwrightApp {
    pub fn new(_cc: &eframe::CreationContext<'_>, launch: LaunchOptions) -> Self {
        let (picker_tx, picker_rx) = mpsc::channel();
        let (settings, settings_warning) = UserSettings::load_with_warning();
        let cwd = std::env::current_dir().ok();
        let selection = resolve(
            &launch,
            rillwright_app::config::model_env()
                .filter(|p| !p.is_empty())
                .map(PathBuf::from),
            (!settings.model_path.is_empty()).then(|| PathBuf::from(&settings.model_path)),
            cwd,
            (!settings.workspace_path.is_empty()).then(|| PathBuf::from(&settings.workspace_path)),
        );
        let model_file = selection.model.clone();
        let model_path = model_file
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let demo = launch.demo;
        let load = settings.load_config(model_file.clone().unwrap_or_default());
        let invalid_saved_loops = !(1..=4).contains(&settings.loops);
        let generation_result = settings.generation_config();
        let agent_result = settings.agent_settings(selection.workspace.clone().unwrap_or_default());
        let startup_settings_error = generation_result
            .as_ref()
            .err()
            .cloned()
            .or_else(|| agent_result.as_ref().err().cloned());
        let generation = generation_result.unwrap_or_default();
        let agent = agent_result.unwrap_or_default();
        let controller = Controller::spawn(load, generation, agent);
        let mut app = Self {
            demo,
            model_path,
            model_file,
            model_source: selection.model_source,
            status: if demo {
                "Demo mode — no model loaded"
            } else if selection.model.is_some() {
                "Checking configured model…"
            } else {
                "Choose a model and project workspace to begin"
            }
            .into(),
            error: settings_warning.or(startup_settings_error),
            messages: Vec::new(),
            composer: String::new(),
            running: false,
            pending_turn_text: None,
            demo_due: None,
            demo_phase: 0,
            approval: false,
            selected: "Agent",
            controller,
            model_generation: 0,
            loaded_loops: None,
            loading_loops: None,
            session_id: String::new(),
            next_turn: 1,
            workspace: selection
                .workspace
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            activity: Vec::new(),
            runtime_ready: false,
            model_loading: false,
            deferred_model_load: false,
            pending_approval: None,
            approval_reason: String::new(),
            approval_summary: String::new(),
            approval_tool: String::new(),
            approval_risk: String::new(),
            approval_workspace: String::new(),
            approval_cwd: String::new(),
            approval_arguments: String::new(),
            approval_command: String::new(),
            approval_sandbox: String::new(),
            agent_session_started: false,
            session_pending: false,
            pending_session_id: None,
            session_kind_pending: None,
            current_session_kind: None,
            active_workspace: None,
            mode_sessions: HashMap::new(),
            workspace_error: selection.workspace_error,
            agent_status: String::new(),
            settings_revision: 0,
            pending_settings_revision: None,
            agent_status_revision: 0,
            picker_tx,
            picker_rx,
            picker_active: false,
            settings,
            invalid_saved_loops,
            transcript_id: String::new(),
            history_workspace: selection
                .workspace
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            history: Vec::new(),
            continuation_differences: Vec::new(),
            resume_pending: false,
            close_dialog: false,
            close_armed: false,
            session_writable: true,
            inspect_system: String::new(),
            inspect_user: String::new(),
            inspected_prompt: String::new(),
            inspected_prompt_tokens: None,
        };
        if !demo && app.model_file.is_some() {
            app.start_model_load();
        }
        app
    }

    fn start_demo(&mut self) {
        if self.running {
            return;
        }
        let prompt = std::mem::take(&mut self.composer);
        if prompt.trim().is_empty() {
            return;
        }
        self.messages.push(ConversationItem::Text {
            id: format!("demo:{}:user", self.demo_phase),
            role: "You".into(),
            body: prompt,
        });
        self.messages.push(ConversationItem::Text {
            id: format!("demo:{}:assistant", self.demo_phase),
            role: "Dory".into(),
            body: String::new(),
        });
        self.running = true;
        self.demo_phase = 0;
        self.demo_due = Some(Instant::now() + Duration::from_millis(400));
        self.status = "Demo is streaming a scripted response".into();
    }

    fn start_model_load(&mut self) {
        if self.demo {
            self.status = "Demo mode does not load a model".into();
            return;
        }
        if self.model_loading {
            self.deferred_model_load = true;
            self.status = "Model change queued until the current load finishes".into();
            return;
        }
        if self.running {
            self.deferred_model_load = true;
            self.status = "Model change will load after this turn finishes".into();
            return;
        }
        let path = self
            .model_file
            .clone()
            .unwrap_or_else(|| PathBuf::from(&self.model_path));
        if path.as_os_str().is_empty() {
            self.error = Some("Select the pinned Dory GGUF model file.".into());
            return;
        }
        if !(1..=4).contains(&self.settings.loops) {
            self.error = Some(
                "Model loops must be between 1 and 4. Correct the selection before loading.".into(),
            );
            return;
        }
        self.model_generation = self.model_generation.wrapping_add(1).max(1);
        let requested_loops = self.settings.loops;
        let config = self.settings.load_config(path);
        match self
            .controller
            .handle
            .load_model(self.model_generation, config)
        {
            Ok(()) => {
                self.status = "Verifying pinned model and fork…".into();
                self.model_loading = true;
                self.loading_loops = Some(requested_loops);
                self.runtime_ready = false;
                self.error = None;
            }
            Err(e) => {
                self.model_loading = false;
                self.error = Some(e);
            }
        }
    }

    fn maybe_create_agent_session(&mut self) {
        if self.demo
            || !self.runtime_ready
            || self.workspace.is_empty()
            || self.session_pending
            || self.selected != "Agent"
        {
            return;
        }
        let entered = PathBuf::from(&self.workspace);
        let workspace = match entered.canonicalize() {
            Ok(path) if path.is_dir() => path,
            Ok(_) => {
                self.workspace_error = Some(format!(
                    "Workspace is not a directory: {}",
                    entered.display()
                ));
                return;
            }
            Err(e) => {
                self.workspace_error =
                    Some(format!("Cannot open workspace {}: {e}", entered.display()));
                return;
            }
        };
        if self.current_session_kind == Some(SessionKind::Agent)
            && self.agent_session_started
            && self.active_workspace.as_ref() == Some(&workspace)
        {
            return;
        }
        self.workspace = workspace.to_string_lossy().into_owned();
        let id = unique_session_id("agent");
        match self
            .controller
            .handle
            .new_session(id.clone(), SessionKind::Agent, Some(workspace))
        {
            Ok(()) => {
                self.pending_session_id = Some(id);
                self.session_pending = true;
                self.session_kind_pending = Some(SessionKind::Agent);
                self.status = "Creating Agent session…".into();
                self.workspace_error = None;
            }
            Err(e) => {
                self.error = Some(format!("Could not create Agent session: {e}"));
            }
        }
    }

    fn new_conversation(&mut self) {
        if self.running
            || self.model_loading
            || self.session_pending
            || self.pending_settings_revision.is_some()
            || self.demo
        {
            return;
        }
        if !self.runtime_ready {
            self.error =
                Some("Wait for the model to be ready before creating a conversation.".into());
            return;
        }
        let kind = if self.selected == "Agent" {
            SessionKind::Agent
        } else {
            SessionKind::Chat
        };
        self.mode_sessions.remove(&kind);
        if kind == SessionKind::Agent && self.workspace.is_empty() {
            self.error =
                Some("Choose a project workspace before creating an Agent conversation.".into());
            return;
        }
        let id = unique_session_id(if kind == SessionKind::Agent {
            "agent"
        } else {
            "chat"
        });
        let root = (kind == SessionKind::Agent).then(|| PathBuf::from(&self.workspace));
        match self.controller.handle.new_session(id.clone(), kind, root) {
            Ok(()) => {
                self.pending_session_id = Some(id);
                self.session_pending = true;
                self.session_kind_pending = Some(kind);
                self.status = "Creating conversation…".into();
            }
            Err(e) => self.error = Some(format!("Could not create conversation: {e}")),
        }
    }

    fn switch_mode(&mut self, kind: SessionKind) {
        let action = navigation_action(
            kind,
            self.current_session_kind,
            self.session_pending
                .then_some(self.session_kind_pending)
                .flatten(),
            self.running
                || self.model_loading
                || self.session_pending
                || self.pending_settings_revision.is_some(),
        );
        if action == NavigationAction::Block {
            return;
        }
        self.selected = if kind == SessionKind::Agent {
            "Agent"
        } else {
            "Chat"
        };
        if action == NavigationAction::ShowCurrent {
            return;
        }
        if let Some(current_kind) = self.current_session_kind {
            if !self.session_id.is_empty() {
                self.mode_sessions.insert(
                    current_kind,
                    ModeSession {
                        id: self.session_id.clone(),
                        messages: self.messages.clone(),
                        next_turn: self.next_turn,
                        transcript_id: self.transcript_id.clone(),
                        writable: self.session_writable,
                        agent_status: self.agent_status.clone(),
                        workspace: self.active_workspace.clone(),
                    },
                );
            }
        }
        if let Some(saved) = self.mode_sessions.get(&kind).cloned() {
            self.session_id = saved.id;
            self.messages = saved.messages;
            self.next_turn = saved.next_turn;
            self.transcript_id = saved.transcript_id;
            self.session_writable = saved.writable;
            self.agent_status = saved.agent_status;
            self.active_workspace = saved.workspace;
            self.current_session_kind = Some(kind);
            self.agent_session_started = kind == SessionKind::Agent;
            self.session_pending = false;
        } else {
            self.new_conversation();
        }
    }

    fn navigation_enabled(&self, item: &str) -> bool {
        if !matches!(item, "Agent" | "Chat") {
            return true;
        }
        let kind = if item == "Agent" {
            SessionKind::Agent
        } else {
            SessionKind::Chat
        };
        navigation_action(
            kind,
            self.current_session_kind,
            self.session_pending
                .then_some(self.session_kind_pending)
                .flatten(),
            self.running
                || self.model_loading
                || self.session_pending
                || self.pending_settings_revision.is_some(),
        ) != NavigationAction::Block
    }

    fn approval_answered(&mut self, request: u64) {
        for item in &mut self.messages {
            if let Some(tool) = item.tool_mut() {
                if tool.approval_request == Some(request) {
                    tool.state = ToolState::Processing;
                }
            }
        }
    }

    fn terminalize_tool_cards(&mut self) {
        self.approval = false;
        self.pending_approval = None;
        for item in &mut self.messages {
            if let Some(tool) = item.tool_mut() {
                if tool.result.is_none() {
                    tool.state = match tool.state {
                        ToolState::Pending => ToolState::NotExecuted,
                        ToolState::Processing | ToolState::WaitingForApproval => {
                            ToolState::OutcomeUnknown
                        }
                        _ => tool.state.clone(),
                    };
                }
            }
        }
    }

    fn reload_block_reason(&self) -> Option<&'static str> {
        if self.demo {
            Some("Demo mode does not load a model")
        } else if self.model_file.is_none() && self.model_path.is_empty() {
            Some("Choose a model first")
        } else if self.model_loading {
            Some("A model load is already in progress")
        } else if self.running || self.pending_approval.is_some() {
            Some("Wait for the active turn or approval to finish")
        } else if self.session_pending {
            Some("Wait for conversation initialization to finish")
        } else if self.pending_settings_revision.is_some() {
            Some("Wait for next-turn settings to be acknowledged")
        } else {
            None
        }
    }

    fn can_submit(&self) -> bool {
        if self.running
            || self.session_pending
            || !self.session_writable
            || self.resume_pending
            || self.pending_settings_revision.is_some()
        {
            return false;
        }
        if self.demo {
            return true;
        }
        if !self.runtime_ready || self.session_id.is_empty() {
            return false;
        }
        if self.current_session_kind
            != Some(if self.selected == "Agent" {
                SessionKind::Agent
            } else {
                SessionKind::Chat
            })
        {
            return false;
        }
        if self.selected == "Agent"
            && (!self.agent_session_started
                || self.workspace.is_empty()
                || self.active_workspace.as_deref()
                    != PathBuf::from(&self.workspace)
                        .canonicalize()
                        .ok()
                        .as_deref())
        {
            return false;
        }
        self.settings.generation_config().is_ok()
            && self
                .settings
                .agent_settings(PathBuf::from(&self.workspace))
                .is_ok()
    }

    fn send_reason(&self) -> &'static str {
        if self.running {
            "A turn is running"
        } else if self.model_loading {
            "Model is loading"
        } else if self.session_pending {
            "Creating conversation…"
        } else if !self.runtime_ready && !self.demo {
            "Waiting for model"
        } else if self.selected == "Agent" && self.workspace.is_empty() {
            "Choose a project workspace"
        } else if self.selected == "Agent" && !self.agent_session_started {
            "Creating Agent session…"
        } else if self.selected == "Agent"
            && self.active_workspace.as_deref()
                != PathBuf::from(&self.workspace)
                    .canonicalize()
                    .ok()
                    .as_deref()
        {
            "Apply the selected workspace first"
        } else if self.resume_pending {
            "Review session settings before continuing"
        } else if !self.session_writable {
            "Conversation is read-only"
        } else if !self.settings.generation_config().is_ok() {
            "Generation settings need correction"
        } else if !self
            .settings
            .agent_settings(PathBuf::from(&self.workspace))
            .is_ok()
        {
            "Agent settings need correction"
        } else {
            "Ready · Ctrl+Enter to send"
        }
    }

    fn apply_next_turn_settings(&mut self) {
        let workspace = if self.selected == "Agent" {
            PathBuf::from(&self.workspace)
        } else {
            PathBuf::new()
        };
        match (
            self.settings.generation_config(),
            self.settings.agent_settings(workspace),
        ) {
            (Ok(generation), Ok(agent)) => {
                self.queue_settings(generation, agent);
            }
            (Err(e), _) | (_, Err(e)) => self.error = Some(e),
        }
    }

    fn queue_settings(
        &mut self,
        generation: rillwright_app::config::GenerationConfig,
        agent: rillwright_app::config::AgentSettings,
    ) -> bool {
        self.settings_revision = self.settings_revision.wrapping_add(1).max(1);
        let revision = self.settings_revision;
        match self
            .controller
            .handle
            .update_settings(revision, generation, agent)
        {
            Ok(()) => {
                self.pending_settings_revision = Some(revision);
                self.settings.workspace_path = self
                    .active_workspace
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.status = format!("Applying settings revision {revision}…");
                true
            }
            Err(e) => {
                self.error = Some(e);
                false
            }
        }
    }

    fn open_picker(&mut self, folder: bool) {
        if self.picker_active {
            return;
        }
        self.picker_active = true;
        let tx = self.picker_tx.clone();
        let initial = PathBuf::from(if folder {
            &self.workspace
        } else {
            &self.model_path
        });
        std::thread::spawn(move || {
            let mut dialog = rfd::FileDialog::new();
            if initial.exists() {
                dialog = dialog.set_directory(&initial);
            }
            if !folder {
                dialog = dialog.add_filter("Dory GGUF model", &["gguf"]);
            }
            let selected = if folder {
                dialog.pick_folder()
            } else {
                dialog.pick_file()
            };
            let _ = tx.send(if folder {
                PickerResult::Workspace(selected)
            } else {
                PickerResult::Model(selected)
            });
        });
    }

    fn export_diagnostics(&mut self) {
        if self.picker_active {
            return;
        }
        self.picker_active = true;
        let tx = self.picker_tx.clone();
        std::thread::spawn(move || {
            let path = rfd::FileDialog::new()
                .set_file_name("rillwright-diagnostics.json")
                .add_filter("JSON", &["json"])
                .save_file();
            let _ = tx.send(PickerResult::Diagnostic(path));
        });
    }

    fn poll_picker(&mut self, ctx: &egui::Context) {
        loop {
            match self.picker_rx.try_recv() {
                Ok(PickerResult::Workspace(Some(path))) => {
                    self.workspace = path.to_string_lossy().into_owned();
                    self.workspace_error = None;
                    self.picker_active = false;
                    if self.selected == "Agent" {
                        self.maybe_create_agent_session();
                    } else if self.current_session_kind.is_none() {
                        self.new_conversation();
                    }
                }
                Ok(PickerResult::Model(Some(path))) => {
                    self.model_file = Some(path.clone());
                    self.model_path = path.to_string_lossy().into_owned();
                    self.model_source = ModelSource::Picker;
                    self.picker_active = false;
                    self.start_model_load();
                }
                Ok(PickerResult::Diagnostic(Some(path))) => {
                    self.picker_active = false;
                    let report = serde_json::json!({
                        "application_version": env!("DORY_VERSION"),
                        "pinned_fork": rillwright_engine::verify::PINNED_FORK_COMMIT,
                        "built_fork": rillwright_sys::FORK_COMMIT,
                        "pinned_model_sha256": rillwright_engine::verify::PINNED_MODEL_SHA256,
                        "selected_model_path": self.model_path,
                        "status": self.status,
                        "settings": { "context": self.settings.context, "loops": self.settings.loops,
                            "cache_mode": self.settings.cache_mode, "batch": self.settings.batch,
                            "micro_batch": self.settings.micro_batch, "gpu_layers": self.settings.gpu_layers,
                            "threads": self.settings.threads, "sandbox": self.settings.sandbox }
                    });
                    match std::fs::write(
                        path,
                        serde_json::to_vec_pretty(&report).unwrap_or_default(),
                    ) {
                        Ok(()) => self.status = "Redacted diagnostics exported".into(),
                        Err(e) => self.error = Some(format!("Could not export diagnostics: {e}")),
                    }
                }
                Ok(
                    PickerResult::Workspace(None)
                    | PickerResult::Model(None)
                    | PickerResult::Diagnostic(None),
                ) => self.picker_active = false,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.picker_active = false;
                    break;
                }
            }
        }
        if self.picker_active {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn advance_demo(&mut self) {
        let Some(due) = self.demo_due else {
            return;
        };
        if Instant::now() < due {
            return;
        }
        match self.demo_phase {
            0 => {
                if let Some(ConversationItem::Text { body, .. }) = self.messages.last_mut() {
                    body.push_str("I’ll inspect the requested change and run a safe example. ");
                }
                self.messages.push(ConversationItem::Tool(ToolCallView {
                    id: "demo:read".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path":"src/main.rs"})
                        .as_object()
                        .cloned()
                        .unwrap_or_default(),
                    result: Some("fn main() { /* demo only */ }".into()),
                    state: ToolState::ResultAvailable,
                    is_error: None,
                    truncated: Some(false),
                    approval_request: None,
                }));
                self.messages.push(ConversationItem::Tool(ToolCallView {
                    id: "demo:command".into(),
                    name: "run_command".into(),
                    arguments: serde_json::json!({"command":"cargo check -p rillwright-gui"})
                        .as_object()
                        .cloned()
                        .unwrap_or_default(),
                    result: None,
                    state: ToolState::WaitingForApproval,
                    is_error: None,
                    truncated: None,
                    approval_request: None,
                }));
                self.demo_phase = 1;
                self.approval = true;
                self.status = "Waiting for your approval".into();
                self.demo_due = None;
            }
            1 => {
                if let Some(tool) = self
                    .messages
                    .iter_mut()
                    .find_map(ConversationItem::tool_mut)
                    .filter(|tool| tool.id == "demo:command")
                {
                    tool.state = ToolState::Completed;
                    tool.result = Some("Demo only: command execution was not attempted.".into());
                }
                if let Some(ConversationItem::Text { body, .. }) = self.messages.last_mut() {
                    body.push_str("The example action completed successfully. Here is the final scripted answer.");
                }
                self.running = false;
                self.demo_due = None;
                self.status = "Demo turn finished".into();
            }
            2 => {
                let args = serde_json::json!({"path":"same.txt"})
                    .as_object()
                    .cloned()
                    .unwrap_or_default();
                self.messages.push(ConversationItem::Tool(ToolCallView {
                    id: "demo:failed-read-a".into(),
                    name: "read_file".into(),
                    arguments: args.clone(),
                    result: Some("λ".repeat(3000)),
                    state: ToolState::Failed,
                    is_error: Some(true),
                    truncated: Some(false),
                    approval_request: None,
                }));
                self.messages.push(ConversationItem::Tool(ToolCallView {
                    id: "demo:unknown-read-b".into(),
                    name: "read_file".into(),
                    arguments: args,
                    result: None,
                    state: ToolState::OutcomeUnknown,
                    is_error: None,
                    truncated: None,
                    approval_request: None,
                }));
                self.running = false;
                self.demo_due = None;
                self.error =
                    Some("Scripted demo failure: this illustrates the error panel.".into());
                self.status = "Demo turn failed".into();
            }
            _ => {
                self.running = false;
                self.demo_due = None;
            }
        }
    }

    fn drain_events(&mut self, ctx: &egui::Context) {
        use std::sync::mpsc::TryRecvError;
        for _ in 0..256 {
            let event = match self.controller.events.try_recv() {
                Ok(event) => event,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if self.running
                        || self.model_loading
                        || self.session_pending
                        || self.pending_settings_revision.is_some()
                    {
                        self.model_loading = false;
                        self.loading_loops = None;
                        self.runtime_ready = false;
                        self.pending_session_id = None;
                        self.session_pending = false;
                        self.session_kind_pending = None;
                        self.pending_settings_revision = None;
                        self.loaded_loops = None;
                        self.error = Some("The application worker disconnected. Navigation remains available; restart the app to retry.".into());
                    }
                    if self.running {
                        self.running = false;
                        self.terminalize_tool_cards();
                    }
                    break;
                }
            };
            match event {
                AppEvent::LoadStage {
                    model_generation,
                    stage,
                } if model_generation == self.model_generation => {
                    self.model_loading = true;
                    self.status = stage;
                }
                AppEvent::ModelReady {
                    model_generation,
                    details,
                    loops,
                } if acknowledge_model_load(
                    self.model_generation,
                    model_generation,
                    loops,
                    &mut self.loaded_loops,
                    &mut self.loading_loops,
                ) =>
                {
                    self.runtime_ready = true;
                    self.model_loading = false;
                    if self.deferred_model_load {
                        self.deferred_model_load = false;
                        self.start_model_load();
                        continue;
                    }
                    self.status = "Ready".into();
                    self.activity.push(details);
                    self.maybe_create_agent_session();
                }
                AppEvent::SessionSnapshot { session, messages } if session == self.session_id => {
                    if self
                        .pending_turn_text
                        .as_ref()
                        .is_some_and(|(pending_session, _, _)| pending_session == &session)
                    {
                        self.pending_turn_text = None;
                    }
                    let prior = std::mem::take(&mut self.messages);
                    let mut projected = project_messages(&messages, self.settings.show_reasoning);
                    for item in &mut projected {
                        let Some(tool) = item.tool_mut() else {
                            continue;
                        };
                        let exact = prior
                            .iter()
                            .position(|old| old.tool().is_some_and(|t| t.id == tool.id));
                        if let Some(idx) = exact {
                            if let Some(previous) = prior[idx].tool() {
                                if tool.result.is_none() {
                                    tool.result = previous.result.clone();
                                }
                                if previous.is_error.is_some() {
                                    tool.is_error = previous.is_error;
                                    tool.truncated = previous.truncated;
                                }
                                if previous.result.is_some()
                                    || matches!(
                                        previous.state,
                                        ToolState::NotExecuted
                                            | ToolState::OutcomeUnknown
                                            | ToolState::Failed
                                    )
                                {
                                    tool.state = previous.state.clone();
                                }
                                tool.approval_request = previous.approval_request;
                            }
                        }
                    }
                    // Streamed assistant text remains visible until its committed snapshot arrives.
                    for old in prior {
                        if let ConversationItem::Text { id, role, body } = &old {
                            if role == "Dory" {
                                let committed = projected.iter().any(|item| {
                                    matches!(item,
                                    ConversationItem::Text { role: new_role, body: new_body, .. }
                                    if new_role == "Dory" && new_body.starts_with(body))
                                });
                                let same_draft = projected.iter().any(|item| {
                                    matches!(item,
                                    ConversationItem::Text { id: new_id, .. } if new_id == id)
                                });
                                if !committed && !same_draft {
                                    projected.push(old);
                                }
                            }
                        }
                    }
                    self.messages = projected;
                }
                AppEvent::SessionCreated {
                    session,
                    transcript_id,
                    kind,
                    workspace,
                    continuation_differences,
                    writable,
                } => {
                    if self.pending_session_id.as_deref().is_some_and(|expected| {
                        expected == session
                            || expected
                                .strip_prefix("resumed-")
                                .is_some_and(|id| session == format!("readonly-{id}"))
                    }) {
                        let previous_session = self.session_id.clone();
                        if self
                            .current_session_kind
                            .is_some_and(|old_kind| old_kind != kind)
                            && !previous_session.is_empty()
                        {
                            self.mode_sessions.insert(
                                self.current_session_kind.unwrap(),
                                ModeSession {
                                    id: previous_session.clone(),
                                    messages: self.messages.clone(),
                                    next_turn: self.next_turn,
                                    transcript_id: self.transcript_id.clone(),
                                    writable: self.session_writable,
                                    agent_status: self.agent_status.clone(),
                                    workspace: self.active_workspace.clone(),
                                },
                            );
                        }
                        self.session_id = session.clone();
                        self.pending_session_id = None;
                        self.messages.clear();
                        self.next_turn = 1;
                        self.session_id = session;
                        if !previous_session.is_empty()
                            && previous_session != self.session_id
                            && !self
                                .mode_sessions
                                .values()
                                .any(|saved| saved.id == previous_session)
                        {
                            let _ = self.controller.handle.close_session(previous_session);
                        }
                        self.session_pending = false;
                        self.session_writable = writable;
                        if !writable {
                            self.status =
                                "Read-only view; this session is active in another process".into();
                        }
                        self.selected = if kind == SessionKind::Agent {
                            "Agent"
                        } else {
                            "Chat"
                        };
                        self.transcript_id = transcript_id;
                        if let Some(path) = workspace {
                            self.active_workspace = Some(path.clone());
                            self.history_workspace = path.display().to_string();
                            self.workspace = path.display().to_string();
                            if kind == SessionKind::Agent {
                                self.settings.workspace_path = self.workspace.clone();
                                if let Err(e) = self.settings.save() {
                                    self.error = Some(format!(
                                        "Workspace is active, but settings could not be saved: {e}"
                                    ));
                                }
                            }
                        } else if kind == SessionKind::Chat {
                            self.active_workspace = None;
                        }
                        self.agent_session_started = kind == SessionKind::Agent;
                        self.session_kind_pending = None;
                        self.current_session_kind = Some(kind);
                        self.status = if writable {
                            "Conversation ready".into()
                        } else {
                            "Read-only conversation".into()
                        };
                        self.continuation_differences = continuation_differences;
                        self.resume_pending = !self.continuation_differences.is_empty();
                    }
                }
                AppEvent::SessionList { sessions, .. } => self.history = sessions,
                AppEvent::SettingsApplied { revision }
                    if self.pending_settings_revision == Some(revision) =>
                {
                    self.pending_settings_revision = None;
                    self.status = format!("Settings revision {revision} applied to upcoming turns");
                    if let Err(e) = self.settings.save() {
                        self.error =
                            Some(format!("Settings were applied but could not be saved: {e}"));
                    }
                }
                AppEvent::SettingsRejected { revision, message }
                    if self.pending_settings_revision == Some(revision) =>
                {
                    self.pending_settings_revision = None;
                    self.error = Some(format!("Settings revision {revision} rejected: {message}"));
                }
                AppEvent::PromptInspection {
                    rendered_prompt,
                    token_count,
                    context,
                    loops,
                } => {
                    self.inspected_prompt = rendered_prompt;
                    self.inspected_prompt_tokens = Some(token_count);
                    self.activity.push(format!("Prompt inspection: {token_count} tokens · context {context} · loops {loops}"));
                }
                AppEvent::AgentStatus {
                    session,
                    config_revision,
                    workspace,
                    permission,
                    sandbox,
                    command_network,
                    web_search,
                    ..
                } => {
                    let status = format!(
                        "Permission {permission} · {sandbox} · command network {} · web search {web_search}",
                        if command_network { "on" } else { "off" }
                    );
                    if session == self.session_id && config_revision >= self.agent_status_revision {
                        self.agent_status_revision = config_revision;
                        self.agent_status = status.clone();
                        self.activity.push(format!(
                            "Agent workspace {} · {}",
                            workspace.display(),
                            status
                        ));
                    } else if let Some(saved) =
                        self.mode_sessions.values_mut().find(|s| s.id == session)
                    {
                        saved.agent_status = status;
                    }
                }
                AppEvent::Delta {
                    session,
                    turn,
                    reasoning,
                    text,
                    ..
                } if session == self.session_id && turn + 1 == self.next_turn => {
                    if reasoning && !self.settings.show_reasoning {
                        continue;
                    }
                    let role = if reasoning { "Reasoning" } else { "Dory" };
                    let matches_role = self.messages.last().is_some_and(|item| {
                        matches!(item,
                        ConversationItem::Text { role: r, .. } if r == role)
                            || matches!(item, ConversationItem::Reasoning { .. }) && reasoning
                    });
                    if !matches_role {
                        if reasoning {
                            self.messages.push(ConversationItem::Reasoning {
                                id: format!("live:reasoning:{}", self.next_turn),
                                body: String::new(),
                            });
                        } else {
                            self.messages.push(ConversationItem::Text {
                                id: format!("live:assistant:{}", self.next_turn),
                                role: "Dory".into(),
                                body: String::new(),
                            });
                        }
                    }
                    if let Some(last) = self.messages.last_mut() {
                        match last {
                            ConversationItem::Text { body, .. }
                            | ConversationItem::Reasoning { body, .. } => body.push_str(&text),
                            _ => {}
                        }
                    }
                }
                AppEvent::ToolDetected {
                    session,
                    turn,
                    call_id,
                    name,
                    arguments,
                    ..
                } if session == self.session_id && turn + 1 == self.next_turn => {
                    self.messages.push(ConversationItem::Tool(ToolCallView {
                        id: format!("live:{call_id}"),
                        name,
                        arguments,
                        result: None,
                        state: ToolState::Pending,
                        is_error: None,
                        truncated: None,
                        approval_request: None,
                    }));
                }
                AppEvent::ToolResolved {
                    session,
                    turn,
                    call_id,
                    name,
                    text,
                    is_error,
                    truncated,
                    sequence,
                    ..
                } if session == self.session_id && turn + 1 == self.next_turn => {
                    let item = call_id.as_ref().and_then(|id| {
                        self.messages.iter_mut().find(|item| {
                            item.tool()
                                .is_some_and(|tool| tool.id == format!("live:{id}"))
                        })
                    });
                    if let Some(item) = item {
                        if let Some(tool) = item.tool_mut() {
                            tool.result = Some(text.clone());
                            tool.is_error = Some(is_error);
                            tool.truncated = Some(truncated);
                            tool.state = if is_error {
                                ToolState::Failed
                            } else {
                                ToolState::Completed
                            };
                        }
                    } else {
                        self.messages.push(ConversationItem::Notice {
                            id: format!("live:orphan-result:{sequence}"),
                            title: format!("Unmatched result: {name}"),
                            body: text.clone(),
                        });
                    }
                    let shown = if truncated {
                        format!("{text}\n[output truncated]")
                    } else {
                        text
                    };
                    self.activity.push(format!(
                        "{}: {name} — {shown}",
                        if is_error { "failed" } else { "completed" }
                    ));
                }
                AppEvent::ToolBound {
                    session,
                    turn,
                    call_id,
                    item_id,
                } if session == self.session_id && turn + 1 == self.next_turn => {
                    if let Some(tool) = self
                        .messages
                        .iter_mut()
                        .filter_map(ConversationItem::tool_mut)
                        .find(|tool| tool.id == format!("live:{call_id}"))
                    {
                        tool.id = item_id;
                    }
                }
                AppEvent::AgentNotice {
                    session,
                    turn,
                    title,
                    detail,
                    sequence,
                    ..
                } if session == self.session_id && turn + 1 == self.next_turn => {
                    let detail = crate::conversation::bounded(&detail, 4000);
                    self.messages.push(ConversationItem::Notice {
                        id: format!("live:notice:{sequence}"),
                        title: title.clone(),
                        body: detail.clone(),
                    });
                    self.activity.push(format!("{title}: {detail}"));
                }
                AppEvent::Stats {
                    session,
                    turn,
                    stats,
                } if session == self.session_id && turn + 1 == self.next_turn => {
                    self.activity.push(format!(
                        "{} generated / {} prompt tokens in {:.0} ms",
                        stats.generated, stats.prompt_tokens, stats.gen_ms
                    ))
                }
                AppEvent::Approval {
                    session,
                    turn,
                    request,
                    tool: tool_name,
                    summary,
                    risk,
                    workspace,
                    working_directory,
                    arguments,
                    command,
                    sandbox,
                    ..
                } if session == self.session_id && turn + 1 == self.next_turn => {
                    if let Some(item) = self.messages.iter_mut().rev().find(|item| {
                        item.tool()
                            .is_some_and(|card| card.name == tool_name && card.result.is_none())
                    }) {
                        if let Some(card) = item.tool_mut() {
                            card.state = ToolState::WaitingForApproval;
                            card.approval_request = Some(request);
                        }
                    }
                    self.approval = true;
                    self.approval_tool = tool_name.clone();
                    self.approval_summary = summary.clone();
                    self.approval_risk = risk;
                    self.approval_workspace = workspace.display().to_string();
                    self.approval_cwd = working_directory.display().to_string();
                    self.approval_arguments =
                        serde_json::to_string_pretty(&arguments).unwrap_or_default();
                    self.approval_command = command.unwrap_or_default();
                    self.approval_sandbox = sandbox.unwrap_or_else(|| "not applicable".into());
                    self.activity
                        .push(format!("Approval requested for {tool_name}: {summary}"));
                    self.pending_approval = Some(request);
                }
                AppEvent::TurnFinished {
                    session,
                    turn,
                    reason,
                } if session == self.session_id && turn + 1 == self.next_turn => {
                    self.running = false;
                    self.terminalize_tool_cards();
                    self.status = format!("Turn finished: {reason}");
                    self.runtime_ready = true;
                    if self.deferred_model_load {
                        self.deferred_model_load = false;
                        self.start_model_load();
                    }
                }
                AppEvent::Error {
                    model_generation,
                    session,
                    turn,
                    message,
                } => {
                    let model_failure =
                        session.is_none() && model_generation == self.model_generation;
                    if model_failure {
                        self.model_loading = false;
                        self.loading_loops = None;
                        self.loaded_loops = None;
                        self.runtime_ready = false;
                        self.status = "Model load failed".into();
                        self.error = Some(message.clone());
                    }
                    if session.as_deref() == Some(&self.session_id) && turn.is_some() {
                        self.running = false;
                        self.terminalize_tool_cards();
                        if let (Some(turn), Some((pending_session, pending_turn, draft))) =
                            (turn, self.pending_turn_text.take())
                        {
                            if pending_session == self.session_id
                                && pending_turn == turn
                                && self.composer.is_empty()
                            {
                                self.composer = draft;
                            }
                        }
                        self.error = Some(message.clone());
                    }
                    if session.as_deref() == self.pending_session_id.as_deref() {
                        self.pending_session_id = None;
                        self.session_pending = false;
                        self.session_kind_pending = None;
                        self.selected = match self.current_session_kind {
                            Some(SessionKind::Agent) => "Agent",
                            Some(SessionKind::Chat) => "Chat",
                            None => "Agent",
                        };
                        self.agent_session_started =
                            self.current_session_kind == Some(SessionKind::Agent);
                        self.error = Some(message.clone());
                    } else if !model_failure && session.is_none() {
                        self.error = Some(message.clone());
                    }
                    if model_failure
                        && model_generation == self.model_generation
                        && self.deferred_model_load
                    {
                        self.deferred_model_load = false;
                        self.start_model_load();
                    }
                }
                AppEvent::ShutdownComplete => self.status = "Stopped".into(),
                _ => {}
            }
            ctx.request_repaint();
        }
    }

    fn submit_turn(&mut self) {
        if self.demo {
            self.start_demo();
            return;
        }
        if !self.can_submit() {
            return;
        }
        if !self.session_writable {
            self.error = Some("This session is open read-only in another process. Start a new session to continue working.".into());
            return;
        }
        if self.resume_pending {
            self.error = Some("Review the session differences before continuing.".into());
            return;
        }
        let text = std::mem::take(&mut self.composer);
        if text.trim().is_empty() {
            return;
        }
        let Ok(generation) = self.settings.generation_config() else {
            self.error = Some("Generation settings are invalid.".into());
            self.composer = text;
            return;
        };
        let workspace = if self.selected == "Agent" {
            PathBuf::from(&self.workspace)
        } else {
            PathBuf::new()
        };
        let Ok(agent) = self.settings.agent_settings(workspace) else {
            self.error = Some("Agent settings are invalid.".into());
            self.composer = text;
            return;
        };
        if !self.queue_settings(generation, agent) {
            self.composer = text;
            return;
        }
        if self.selected == "Agent" {
            if !self.agent_session_started {
                self.composer = text;
                self.error = Some("Choose a workspace and start an agent session first.".into());
                return;
            }
        }
        match self
            .controller
            .handle
            .submit(self.session_id.clone(), self.next_turn, text.clone())
        {
            Ok(()) => {
                self.pending_turn_text = Some((self.session_id.clone(), self.next_turn, text));
                self.running = true;
                self.status = "Running".into();
                self.next_turn += 1;
            }
            Err(e) => {
                self.composer = text;
                self.error = Some(e);
            }
        }
    }

    fn settings_view(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        ui.label("Model loading options require a reload. Generation and agent options apply to the next turn.");
        ui.separator();
        ui.group(|ui| {
            ui.heading("Model configuration");
            ui.horizontal(|ui| {
                egui::ComboBox::from_label("Model loops")
                    .selected_text(if (1..=4).contains(&self.settings.loops) {
                        self.settings.loops.to_string()
                    } else {
                        format!("{} · invalid", self.settings.loops)
                    })
                    .show_ui(ui, |ui| {
                        for value in 1..=4 {
                            ui.selectable_value(&mut self.settings.loops, value, value.to_string());
                        }
                    });
                ui.label("Number of model recurrence loops. Default: 2. Changes require a model reload.");
            });
            if self.invalid_saved_loops {
                ui.colored_label(egui::Color32::LIGHT_RED, "Saved loop count is outside 1–4. Choose a valid value before saving or loading.");
                if (1..=4).contains(&self.settings.loops) {
                    self.invalid_saved_loops = false;
                }
            }
            if let Some(requested) = self.loading_loops {
                ui.label(format!("Loading model with {requested} loops…"));
            }
            let loaded = self.loaded_loops.map_or_else(|| "No model loaded".to_owned(), |n| format!("Loaded: {n}"));
            let pending = self.loaded_loops.is_some_and(|n| n != self.settings.loops);
            if pending {
                ui.label(format!("{loaded} · Selected: {} · Reload required", self.settings.loops));
            } else if self.loading_loops.is_none() {
                ui.label(format!("{loaded} · Selected: {}", self.settings.loops));
            }
            let disabled_reason = self.reload_block_reason();
            if ui.add_enabled(disabled_reason.is_none(), egui::Button::new("Reload model with selected settings")).clicked() {
                self.start_model_load();
            }
            if let Some(reason) = disabled_reason {
                ui.label(reason);
            }
            ui.label("Saving stores this choice for future loads. Apply next turn settings does not reload the model.");
        });
        ui.add_space(6.0);
        egui::ComboBox::from_label("Theme")
            .selected_text(&self.settings.theme)
            .show_ui(ui, |ui| {
                for value in ["system", "light", "dark"] {
                    ui.selectable_value(&mut self.settings.theme, value.to_owned(), value);
                }
            });
        ui.collapsing("Model loading · requires reload", |ui| {
            egui::ComboBox::from_label("Cache mode")
                .selected_text(&self.settings.cache_mode)
                .show_ui(ui, |ui| {
                    for value in ["last_loop", "per_loop"] {
                        ui.selectable_value(&mut self.settings.cache_mode, value.to_owned(), value);
                    }
                });
            egui::ComboBox::from_label("Flash attention")
                .selected_text(&self.settings.flash_attention)
                .show_ui(ui, |ui| {
                    for value in ["auto", "on", "off"] {
                        ui.selectable_value(
                            &mut self.settings.flash_attention,
                            value.to_owned(),
                            value,
                        );
                    }
                });
            ui.checkbox(&mut self.settings.prefix_reuse, "Reuse prompt prefix cache");
            ui.checkbox(&mut self.settings.swa_full, "Full sliding-window cache");
            ui.horizontal(|ui| {
                ui.label("Context");
                ui.add(egui::DragValue::new(&mut self.settings.context).range(1024..=131072));
            });
            ui.horizontal(|ui| {
                ui.label("Batch");
                ui.add(egui::DragValue::new(&mut self.settings.batch).range(1..=32768));
                ui.label("Micro-batch");
                ui.add(egui::DragValue::new(&mut self.settings.micro_batch).range(1..=8192));
            });
            ui.horizontal(|ui| {
                ui.label("GPU layers");
                ui.add(egui::DragValue::new(&mut self.settings.gpu_layers).range(-1..=128));
                ui.label("Threads");
                ui.add(egui::DragValue::new(&mut self.settings.threads).range(1..=256));
            });
        });
        ui.collapsing("Generation · next turn", |ui| {
            ui.checkbox(&mut self.settings.thinking, "Enable model reasoning");
            ui.horizontal(|ui| {
                ui.label("Seed");
                ui.add(egui::DragValue::new(&mut self.settings.seed));
                ui.label("Temperature");
                ui.add(egui::DragValue::new(&mut self.settings.temperature).speed(0.05));
            });
            ui.horizontal(|ui| {
                ui.label("Top-p");
                ui.add(egui::DragValue::new(&mut self.settings.top_p).speed(0.01));
                ui.label("Max tokens");
                ui.add(egui::DragValue::new(&mut self.settings.max_tokens).range(1..=32768));
            });
            ui.checkbox(&mut self.settings.show_reasoning, "Show reasoning messages");
        });
        ui.collapsing("Workspace permissions · next turn", |ui| {
            egui::ComboBox::from_label("Permission mode")
                .selected_text(&self.settings.permission)
                .show_ui(ui, |ui| {
                    for v in ["read-only", "ask", "auto-edit", "full"] {
                        ui.selectable_value(&mut self.settings.permission, v.to_owned(), v);
                    }
                });
            egui::ComboBox::from_label("Sandbox")
                .selected_text(&self.settings.sandbox)
                .show_ui(ui, |ui| {
                    for v in ["auto", "bwrap", "none"] {
                        ui.selectable_value(&mut self.settings.sandbox, v.to_owned(), v);
                    }
                });
            ui.checkbox(
                &mut self.settings.network,
                "Allow network inside sandboxed commands",
            );
            ui.label("This setting controls command network access only when bwrap is active. Web search has separate approval settings.");
            ui.horizontal(|ui| {
                ui.label("Agent round limit");
                ui.add(egui::DragValue::new(&mut self.settings.max_rounds).range(1..=1000));
                ui.label("Task seconds");
                ui.add(egui::DragValue::new(&mut self.settings.max_seconds).range(1..=86400));
            });
            ui.label("Agent round limit is the maximum agent rounds for one task; it is separate from model loops.");
            ui.horizontal(|ui| {
                ui.label("Command timeout seconds");
                ui.add(egui::DragValue::new(&mut self.settings.command_timeout).range(1..=600));
            });
            ui.label("Allow-command rules (one per line)");
            ui.add(TextEdit::multiline(&mut self.settings.allow_commands).desired_rows(2));
            ui.label("Writable directories (one per line)");
            ui.add(TextEdit::multiline(&mut self.settings.writable_dirs).desired_rows(2));
            ui.label("Environment variable names passed to commands (one per line)");
            ui.add(TextEdit::multiline(&mut self.settings.env_names).desired_rows(2));
        });
        ui.collapsing("Web search · environment key only", |ui| {
            egui::ComboBox::from_label("Provider")
                .selected_text(&self.settings.web_provider)
                .show_ui(ui, |ui| {
                    for v in ["tavily", "brave"] {
                        ui.selectable_value(&mut self.settings.web_provider, v.to_owned(), v);
                    }
                });
            ui.checkbox(
                &mut self.settings.web_enabled,
                "Enable web search for agent sessions",
            );
            ui.add(
                egui::DragValue::new(&mut self.settings.web_budget)
                    .range(1..=20)
                    .prefix("Search budget: "),
            );
            ui.checkbox(
                &mut self.settings.allow_web,
                "Allow search without approval",
            );
            let key = if self.settings.web_provider == "brave" {
                "BRAVE_SEARCH_API_KEY"
            } else {
                "TAVILY_API_KEY"
            };
            ui.label(format!(
                "{key}: {}",
                if std::env::var(key).is_ok_and(|v| !v.trim().is_empty()) {
                    "configured"
                } else {
                    "missing"
                }
            ));
            ui.label("Keys are never stored in settings or transcripts.");
        });
        ui.horizontal(|ui| {
            ui.label("Font scale");
            ui.add(egui::Slider::new(
                &mut self.settings.font_scale,
                0.75..=1.75,
            ));
            if ui.button("Save settings").clicked() {
                if !(1..=4).contains(&self.settings.loops) {
                    self.error =
                        Some("Model loops must be between 1 and 4 before saving settings.".into());
                    return;
                }
                self.settings.model_path = self.model_path.clone();
                self.settings.workspace_path = self.workspace.clone();
                match self.settings.save() {
                    Ok(()) => self.status = "Settings saved".into(),
                    Err(e) => self.error = Some(e),
                }
            }
            if ui.button("Apply next turn settings").clicked() {
                match (
                    self.settings.generation_config(),
                    self.settings.agent_settings(self.workspace.clone().into()),
                ) {
                    (Ok(g), Ok(a)) => {
                        self.queue_settings(g, a);
                    }
                    _ => {
                        self.error = Some(
                            "Settings are invalid; check the values and permission mode.".into(),
                        )
                    }
                }
            }
        });
    }

    fn chat(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.heading(if self.selected == "Agent" {
                "Workspace agent"
            } else {
                "Chat"
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.running && ui.button("Stop  Esc").clicked() {
                    if self.demo {
                        self.demo_due = None;
                        self.running = false;
                        self.status = "Turn cancelled".into();
                    } else {
                        self.controller.handle.stop();
                        self.status = "Cancelling…".into();
                    }
                }
                if ui
                    .add_enabled(
                        !self.running
                            && !self.session_pending
                            && self.pending_settings_revision.is_none(),
                        egui::Button::new("New conversation"),
                    )
                    .clicked()
                {
                    self.new_conversation();
                }
            });
        });
        if self.selected == "Agent" {
            ui.horizontal_wrapped(|ui| {
                ui.label("Workspace");
                let available = (ui.available_width() - 100.0).max(120.0);
                let path = ui.add(
                    TextEdit::singleline(&mut self.workspace)
                        .desired_width(available)
                        .hint_text("/path/to/project"),
                );
                path.on_hover_text(&self.workspace);
                if ui.button("Browse…").clicked() {
                    self.open_picker(true);
                }
                if ui.button("Use workspace").clicked() {
                    self.maybe_create_agent_session();
                }
            });
            if let Some(err) = &self.workspace_error {
                ui.colored_label(egui::Color32::LIGHT_RED, err);
            }
            ui.horizontal_wrapped(|ui| {
                ui.label("Permission");
                let old_permission = self.settings.permission.clone();
                egui::ComboBox::from_id_salt("agent-permission")
                    .selected_text(match self.settings.permission.as_str() {
                        "read-only" => "Read only",
                        "auto-edit" => "Auto edit",
                        "full" => "Full",
                        _ => "Ask",
                    })
                    .show_ui(ui, |ui| {
                        for (value, label) in [
                            ("read-only", "Read only"),
                            ("ask", "Ask"),
                            ("auto-edit", "Auto edit"),
                            ("full", "Full"),
                        ] {
                            ui.selectable_value(&mut self.settings.permission, value.into(), label);
                        }
                    });
                ui.label("Sandbox");
                let old_sandbox = self.settings.sandbox.clone();
                egui::ComboBox::from_id_salt("agent-sandbox")
                    .selected_text(self.settings.sandbox.clone())
                    .show_ui(ui, |ui| {
                        for value in ["auto", "bwrap", "none"] {
                            ui.selectable_value(&mut self.settings.sandbox, value.into(), value);
                        }
                    });
                if old_permission != self.settings.permission
                    || old_sandbox != self.settings.sandbox
                {
                    self.apply_next_turn_settings();
                }
                ui.label(format!(
                    "{} · {}",
                    self.settings.permission, self.settings.sandbox
                ));
            });
            ui.label("Read only: reads only. Ask: approve writes and commands. Auto edit: writes allowed, commands normally ask. Full: tools run subject to sandbox rules.");
            ui.label("Command network is isolated only while bwrap is active. Web search permission is separate.");
            if self.agent_status.is_empty() {
                ui.label("Sandbox status will be checked in the background when the Agent session starts.");
            } else {
                ui.label(&self.agent_status);
            }
        } else {
            ui.label("Plain chat has no workspace tools.");
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!(
                    "Model: {} · source: {}",
                    if self.model_file.is_some() {
                        "selected"
                    } else {
                        "missing"
                    },
                    self.model_source.label()
                ))
                .strong(),
            );
            ui.label(&self.status);
            if ui.button("Choose model…").clicked() {
                self.open_picker(false);
            }
            if !self.demo && self.model_file.is_some() && !self.runtime_ready {
                if ui.button("Retry load").clicked() {
                    self.start_model_load();
                }
            }
        });
        if let Some(path) = &self.model_file {
            ui.label(RichText::new(path.to_string_lossy()).small().weak());
        }
        ui.separator();
        let transcript_height = (ui.available_height() - 160.0).max(80.0);
        let scroll = ScrollArea::vertical()
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .max_height(transcript_height);
        scroll.show(ui, |ui| {
            for idx in 0..self.messages.len() {
                let item = &mut self.messages[idx];
                match item {
                    ConversationItem::Tool(tool) => {
                        ui.push_id(&tool.id, |ui| {
                            ui.group(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(&tool.name).strong());
                                    ui.label(format!(
                                        "{} · {}",
                                        tool_summary(tool),
                                        tool.state.label()
                                    ));
                                });
                                ui.horizontal(|ui| {
                                    if ui.small_button("Copy arguments").clicked() {
                                        ui.ctx().copy_text(
                                            serde_json::to_string_pretty(&tool.arguments)
                                                .unwrap_or_default(),
                                        );
                                    }
                                    if ui
                                        .add_enabled(
                                            tool.result.is_some(),
                                            egui::Button::new("Copy result"),
                                        )
                                        .clicked()
                                    {
                                        if let Some(result) = &tool.result {
                                            ui.ctx().copy_text(result.clone());
                                        }
                                    }
                                });
                                ui.collapsing("Arguments", |ui| {
                                    let args = serde_json::to_string_pretty(&tool.arguments)
                                        .unwrap_or_default();
                                    ui.add(
                                        egui::Label::new(RichText::new(args).monospace())
                                            .wrap()
                                            .selectable(true),
                                    );
                                });
                                if let Some(result) = &tool.result {
                                    ui.collapsing("Result", |ui| {
                                        if result.len() > 4000 {
                                            ui.small("Preview limited to 4,000 bytes");
                                        }
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(crate::conversation::bounded(
                                                    result, 4000,
                                                ))
                                                .monospace(),
                                            )
                                            .wrap()
                                            .selectable(true),
                                        );
                                        if result.len() > 4000 {
                                            ui.collapsing("Show full retained output", |ui| {
                                                ui.add(
                                                    egui::Label::new(
                                                        RichText::new(result).monospace(),
                                                    )
                                                    .wrap()
                                                    .selectable(true),
                                                );
                                            });
                                        }
                                        if tool.truncated == Some(true) {
                                            ui.label("[tool output truncated]");
                                        }
                                    });
                                }
                            });
                        });
                    }
                    ConversationItem::Text { id, role, body } => {
                        let _ = id;
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&*role).strong());
                                if role == "Dory" && ui.small_button("Copy").clicked() {
                                    ui.ctx().copy_text(body.clone());
                                }
                            });
                            let code_or_tool = body.contains("```");
                            let text = if code_or_tool {
                                RichText::new(&*body).monospace().size(14.0)
                            } else {
                                RichText::new(&*body).size(15.0)
                            };
                            ui.add(egui::Label::new(text).wrap().selectable(true));
                        });
                    }
                    ConversationItem::Reasoning { id, body } => {
                        if self.settings.show_reasoning {
                            ui.push_id(id.clone(), |ui| {
                                ui.group(|ui| {
                                    ui.label(RichText::new("Reasoning").strong());
                                    ui.add(
                                        egui::Label::new(RichText::new(&*body).weak())
                                            .wrap()
                                            .selectable(true),
                                    );
                                });
                            });
                        }
                    }
                    ConversationItem::Notice { id, title, body } => {
                        ui.push_id(id.clone(), |ui| {
                            ui.group(|ui| {
                                ui.label(RichText::new(&*title).strong());
                                ui.add(
                                    egui::Label::new(RichText::new(&*body).monospace())
                                        .wrap()
                                        .selectable(true),
                                );
                            });
                        });
                    }
                }
                ui.add_space(6.0);
            }
        });
        ui.separator();
        ui.add(
            TextEdit::multiline(&mut self.composer)
                .desired_width(f32::INFINITY)
                .desired_rows(4)
                .hint_text("Message Rillwright…  Ctrl+Enter to send"),
        );
        ui.horizontal(|ui| {
            let can_send = self.can_submit();
            if ui
                .add_enabled(can_send, egui::Button::new("Send"))
                .clicked()
            {
                self.submit_turn();
            }
            ui.label(self.send_reason());
            if self.demo && !self.running && ui.button("Simulate failure").clicked() {
                self.error = None;
                self.running = true;
                self.demo_phase = 2;
                self.demo_due = Some(Instant::now() + Duration::from_millis(300));
                self.status = "Running scripted failure case".into();
            }
        });
        let send = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Enter));
        if send && !self.running {
            self.submit_turn();
        }
        ui.collapsing(
            format!("Activity and generation details ({})", self.activity.len()),
            |ui| {
                ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                    for row in &self.activity {
                        ui.add(
                            TextEdit::singleline(&mut row.clone())
                                .desired_width(f32::INFINITY)
                                .interactive(false),
                        );
                    }
                });
            },
        );
    }
}

impl eframe::App for RillwrightApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ctx.set_zoom_factor(self.settings.font_scale.clamp(0.75, 1.75));
        match self.settings.theme.as_str() {
            "light" => ctx.set_visuals(egui::Visuals::light()),
            "dark" => ctx.set_visuals(egui::Visuals::dark()),
            _ => {}
        }
        let close_requested = ctx.input(|i| i.viewport().close_requested());
        if close_requested && !self.close_armed {
            if self.running {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.close_dialog = true;
            } else {
                self.controller.request_shutdown();
            }
        }
        self.drain_events(&ctx);
        self.poll_picker(&ctx);
        self.advance_demo();
        if self.demo_due.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        if self.model_loading
            || self.session_pending
            || self.running
            || self.pending_settings_revision.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let available = ui.available_size();
        ui.allocate_ui_with_layout(available, Layout::top_down(Align::Min), |root| {
          root.set_min_size(available);
          root.set_max_size(available);
          egui::Frame::central_panel(root.style()).show(root, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(150.0);
                    ui.heading("Rillwright");
                    if self.demo {
                        ui.label(
                            RichText::new("DEMO MODE")
                                .strong()
                                .color(egui::Color32::YELLOW),
                        );
                    }
                    ui.separator();
                    for item in ["Chat", "Agent", "History", "Settings", "Diagnostics", "Prompt Inspector"] {
                        if ui
                            .add_enabled(
                                self.navigation_enabled(item),
                                egui::Button::selectable(self.selected == item, item),
                            )
                            .clicked()
                        {
                            if item == "Agent" {
                                self.switch_mode(SessionKind::Agent);
                            } else if item == "Chat" {
                                self.switch_mode(SessionKind::Chat);
                            } else {
                                self.selected = item;
                                if item == "History" {
                                    self.history_workspace = self.workspace.clone();
                                }
                            }
                        }
                    }
                    ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                        ui.separator();
                        ui.label(RichText::new(&self.status).small());
                    });
                });
                ui.separator();
                ui.vertical(|ui| {
                    ui.set_min_width((available.x - 170.0).max(300.0));
                    ui.set_min_height(available.y);
                    if let Some(err) = self.error.clone() {
                        ui.horizontal_wrapped(|ui| {
                            ui.colored_label(egui::Color32::LIGHT_RED, format!("Error: {err}"));
                            if ui.button("Dismiss").clicked() { self.error = None; }
                        });
                    }
                    match self.selected {
                        "Settings" => { ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.settings_view(ui)); }
                        "History" => {
                            ui.heading("Session history");
                            ui.horizontal(|ui| {
                                ui.label("Project folder");
                                ui.add(
                                    TextEdit::singleline(&mut self.history_workspace)
                                        .desired_width(360.0),
                                );
                                if ui.button("List sessions").clicked() {
                                    if let Err(e) = self
                                        .controller
                                        .handle
                                        .list_sessions(self.history_workspace.clone().into())
                                    {
                                        self.error = Some(e);
                                    }
                                }
                            });
                            ScrollArea::vertical().show(ui, |ui| {
                                for entry in &self.history {
                                    ui.horizontal(|ui| {
                                        ui.label(format!(
                                            "{} · {} records · {:?}",
                                            entry.id, entry.records, entry.modified
                                        ));
                                        if ui.add_enabled(!self.running && !self.session_pending, egui::Button::new("Open and continue")).clicked() {
                                            if let Err(e) = self.controller.handle.open_session(
                                                entry.id.clone(),
                                                self.history_workspace.clone().into(),
                                            ) {
                                                self.error = Some(e);
                                            } else {
                                                self.pending_session_id = Some(format!("resumed-{}", entry.id));
                                                self.session_pending = true;
                                            }
                                        }
                                    });
                                }
                            });
                            ui.label(format!("Current transcript: {}", self.transcript_id));
                        }
                        "Diagnostics" => {
                            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                            ui.heading("Diagnostics");
                            ui.label(format!("Application version: {}", env!("DORY_VERSION")));
                            ui.label(format!("Pinned fork: {}", rillwright_engine::verify::PINNED_FORK_COMMIT));
                            ui.label(format!("Built fork: {}", rillwright_sys::FORK_COMMIT));
                            ui.label(format!("Pinned model SHA-256: {}", rillwright_engine::verify::PINNED_MODEL_SHA256));
                            ui.label(format!(
                                "Selected model: {}",
                                if self.model_path.is_empty() {
                                    "none"
                                } else {
                                    &self.model_path
                                }
                            ));
                            ui.label(&self.status);
                            ui.collapsing("Recent operational details", |ui| {
                                for row in &self.activity {
                                    ui.label(row);
                                }
                            });
                            ui.label("Export contains version, pin identifiers, model path, and selected load settings. It excludes transcripts, activity output, and provider keys.");
                            if ui.add_enabled(!self.picker_active, egui::Button::new("Export diagnostics…")).clicked() {
                                self.export_diagnostics();
                            }
                            });
                        }
                        "Prompt Inspector" => {
                            ui.heading("Prompt and token inspection");
                            ui.label("Renders a system/user prompt with the loaded Dory template and no tools, then tokenizes it with the loaded model.");
                            ui.label("System prompt (optional)");
                            ui.add(TextEdit::multiline(&mut self.inspect_system).desired_rows(4).desired_width(f32::INFINITY));
                            ui.label("User message");
                            ui.add(TextEdit::multiline(&mut self.inspect_user).desired_rows(5).desired_width(f32::INFINITY));
                            if ui.add_enabled(!self.running && self.runtime_ready, egui::Button::new("Render and tokenize")).clicked() {
                                if let Err(e) = self.controller.handle.inspect_prompt(self.inspect_system.clone(), self.inspect_user.clone(), self.settings.thinking) {
                                    self.error = Some(e);
                                }
                            }
                            if let Some(count) = self.inspected_prompt_tokens {
                                ui.label(format!("Token count: {count}"));
                            }
                            ui.add(TextEdit::multiline(&mut self.inspected_prompt).desired_rows(16).desired_width(f32::INFINITY).interactive(false));
                        }
                        _ => {
                            self.chat(ui, &ctx);
                        }
                    }
                });
            });
          });
        });
        if self.approval || self.pending_approval.is_some() {
            egui::Window::new("Approval required")
                .collapsible(false)
                .resizable(true)
                .show(&ctx, |ui| {
                    if self.demo {
                        ui.label("The scripted demo proposes to run a harmless example tool.");
                        ui.label("This approval is simulated. No command will be executed.");
                    } else {
                        ui.label(RichText::new(&self.approval_tool).strong());
                        ui.label(format!(
                            "Risk: {} · sandbox: {}",
                            self.approval_risk, self.approval_sandbox
                        ));
                        ui.label(format!("Workspace: {}", self.approval_workspace));
                        ui.label(format!("Working directory: {}", self.approval_cwd));
                        if !self.approval_command.is_empty() {
                            ui.label("Command");
                            ui.add(
                                TextEdit::singleline(&mut self.approval_command.clone())
                                    .desired_width(520.0)
                                    .interactive(false),
                            );
                        }
                        let shown_arguments = bounded(&self.approval_arguments, 16000);
                        ui.label("Arguments");
                        ui.add(
                            TextEdit::multiline(&mut shown_arguments.clone())
                                .desired_width(520.0)
                                .desired_rows(6)
                                .interactive(false),
                        );
                        if self.approval_arguments.len() > shown_arguments.len() {
                            ui.label("Arguments preview truncated in the approval panel.");
                        }
                        ui.label("Execution summary and proposed file preview. Text is inert.");
                        ui.add(
                            TextEdit::multiline(&mut self.approval_summary.clone())
                                .desired_width(520.0)
                                .desired_rows(8)
                                .interactive(false),
                        );
                        ui.label("Decline reason (optional)");
                        ui.add(
                            TextEdit::multiline(&mut self.approval_reason)
                                .desired_width(520.0)
                                .desired_rows(2),
                        );
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Allow once").clicked() {
                            self.approval = false;
                            if self.demo {
                                self.demo_phase = 1;
                                self.demo_due = Some(Instant::now() + Duration::from_millis(500));
                            } else if let Some(id) = self.pending_approval.take() {
                                let _ = self.controller.handle.answer_approval(id, Answer::Yes);
                                self.approval_answered(id);
                            }
                        }
                        if ui
                            .button("Allow this exact call for this session")
                            .clicked()
                        {
                            self.approval = false;
                            if let Some(id) = self.pending_approval.take() {
                                let _ = self.controller.handle.answer_approval(id, Answer::Always);
                                self.approval_answered(id);
                            }
                        }
                        if ui.button("Decline").clicked() {
                            self.approval = false;
                            if self.demo {
                                self.running = false;
                                self.status = "Demo action declined".into();
                            } else if let Some(id) = self.pending_approval.take() {
                                let answer = if self.approval_reason.trim().is_empty() {
                                    Answer::No
                                } else {
                                    Answer::NoWith(self.approval_reason.clone())
                                };
                                let _ = self.controller.handle.answer_approval(id, answer);
                                self.approval_answered(id);
                            }
                        }
                    });
                });
        }
        if self.resume_pending {
            egui::Window::new("Session settings differ").collapsible(false).resizable(false).show(&ctx, |ui| {
                ui.label("The continuation will use the currently loaded model and current permission settings.");
                for difference in &self.continuation_differences { ui.label(format!("• {difference}")); }
                ui.horizontal(|ui| {
                    if ui.button("Continue with current settings").clicked() { self.resume_pending = false; }
                    if ui.button("Close session").clicked() {
                        let id = self.session_id.clone();
                        let _ = self.controller.handle.close_session(id);
                        self.resume_pending = false;
                        self.continuation_differences.clear();
                        self.messages.clear();
                    }
                });
            });
        }
        if self.close_dialog {
            egui::Window::new("Active work is running")
                .collapsible(false)
                .resizable(false)
                .show(&ctx, |ui| {
                    ui.label("Keep the current turn running, or stop it and close Rillwright?");
                    ui.horizontal(|ui| {
                        if ui.button("Keep running").clicked() {
                            self.close_dialog = false;
                        }
                        if ui.button("Stop and close").clicked() {
                            self.controller.handle.stop();
                            self.controller.request_shutdown();
                            self.close_dialog = false;
                            self.close_armed = true;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                });
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && self.running {
            if self.demo {
                self.demo_due = None;
                self.running = false;
                self.status = "Turn cancelled".into();
            } else {
                self.controller.handle.stop();
                self.status = "Cancelling…".into();
            }
        }
    }
}

fn bounded(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[preview truncated]", &text[..end])
}

fn unique_session_id(prefix: &str) -> String {
    let ticks = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{prefix}-{}-{ticks}", std::process::id())
}

#[cfg(test)]
mod navigation_tests {
    use super::{NavigationAction, acknowledge_model_load, navigation_action};
    use rillwright_app::protocol::SessionKind;

    #[test]
    fn returning_to_current_or_initializing_view_is_view_only_while_busy() {
        for kind in [SessionKind::Agent, SessionKind::Chat] {
            for busy in [true, false] {
                assert_eq!(
                    navigation_action(kind, Some(kind), None, busy),
                    NavigationAction::ShowCurrent
                );
                assert_eq!(
                    navigation_action(kind, None, Some(kind), busy),
                    NavigationAction::ShowCurrent
                );
            }
        }
        assert_eq!(
            navigation_action(
                SessionKind::Chat,
                Some(SessionKind::Agent),
                Some(SessionKind::Chat),
                true
            ),
            NavigationAction::ShowCurrent
        );
    }

    #[test]
    fn changing_modes_is_blocked_while_busy_and_allowed_when_idle() {
        assert_eq!(
            navigation_action(SessionKind::Chat, Some(SessionKind::Agent), None, true),
            NavigationAction::Block
        );
        assert_eq!(
            navigation_action(SessionKind::Chat, Some(SessionKind::Agent), None, false),
            NavigationAction::Switch
        );
    }

    #[test]
    fn only_the_current_model_generation_updates_the_loaded_loop_count() {
        let mut loaded = Some(2);
        let mut loading = Some(3);
        assert!(!acknowledge_model_load(8, 7, 3, &mut loaded, &mut loading));
        assert_eq!(loaded, Some(2));
        assert_eq!(loading, Some(3));

        assert!(acknowledge_model_load(8, 8, 3, &mut loaded, &mut loading));
        assert_eq!(loaded, Some(3));
        assert_eq!(loading, None);
    }
}
