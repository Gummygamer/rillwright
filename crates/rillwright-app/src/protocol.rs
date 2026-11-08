use std::path::PathBuf;

use rillwright_engine::{GenStats, chat::Message};
use serde_json::{Map, Value};
use std::time::SystemTime;

pub type ModelGeneration = u64;
pub type SessionId = String;
pub type TurnId = u64;
pub type RequestId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeState {
    Unloaded,
    Verifying,
    Loading,
    Ready,
    Running,
    AwaitingApproval,
    Cancelling,
    Unloading,
    Failed,
    ShuttingDown,
}

impl RuntimeState {
    pub fn can_load(&self) -> bool {
        matches!(self, Self::Unloaded | Self::Ready | Self::Failed)
    }
    pub fn can_submit(&self) -> bool {
        *self == Self::Ready
    }
    pub fn can_stop(&self) -> bool {
        matches!(self, Self::Running | Self::AwaitingApproval)
    }
}

#[cfg(test)]
mod tests {
    use super::RuntimeState;

    #[test]
    fn runtime_state_gates_load_submit_and_stop() {
        assert!(RuntimeState::Unloaded.can_load());
        assert!(!RuntimeState::Unloaded.can_submit());
        assert!(RuntimeState::Ready.can_submit());
        assert!(!RuntimeState::Running.can_submit());
        assert!(RuntimeState::Running.can_stop());
        assert!(RuntimeState::AwaitingApproval.can_stop());
        assert!(!RuntimeState::ShuttingDown.can_load());
    }
}

#[derive(Clone, Debug)]
pub enum Command {
    LoadModel {
        model_generation: ModelGeneration,
    },
    UnloadModel,
    NewSession {
        kind: SessionKind,
        workspace: Option<PathBuf>,
    },
    OpenSession {
        id: SessionId,
        workspace: Option<PathBuf>,
    },
    SubmitTurn {
        session: SessionId,
        turn: TurnId,
        text: String,
    },
    UpdateIdleSettings,
    Shutdown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SessionKind {
    Chat,
    Agent,
}

#[derive(Clone, Debug)]
pub enum AppEvent {
    LoadStage {
        model_generation: ModelGeneration,
        stage: String,
    },
    ModelReady {
        model_generation: ModelGeneration,
        details: String,
        loops: u32,
    },
    SessionSnapshot {
        session: SessionId,
        messages: Vec<Message>,
    },
    SessionCreated {
        session: SessionId,
        transcript_id: String,
        kind: SessionKind,
        workspace: Option<PathBuf>,
        continuation_differences: Vec<String>,
        writable: bool,
    },
    SessionList {
        workspace: PathBuf,
        sessions: Vec<SessionSummary>,
    },
    PromptInspection {
        rendered_prompt: String,
        token_count: usize,
        context: u32,
        loops: u32,
    },
    AgentStatus {
        session: SessionId,
        config_revision: u64,
        workspace: PathBuf,
        permission: String,
        sandbox: String,
        command_network: bool,
        web_search: String,
    },
    SettingsApplied {
        revision: u64,
    },
    SettingsRejected {
        revision: u64,
        message: String,
    },
    Delta {
        session: SessionId,
        turn: TurnId,
        sequence: u64,
        reasoning: bool,
        text: String,
    },
    ToolDetected {
        session: SessionId,
        turn: TurnId,
        sequence: u64,
        call_id: String,
        name: String,
        arguments: Map<String, Value>,
    },
    ToolResolved {
        session: SessionId,
        turn: TurnId,
        sequence: u64,
        call_id: Option<String>,
        name: String,
        text: String,
        is_error: bool,
        truncated: bool,
    },
    ToolBound {
        session: SessionId,
        turn: TurnId,
        call_id: String,
        item_id: String,
    },
    AgentNotice {
        session: SessionId,
        turn: TurnId,
        sequence: u64,
        title: String,
        detail: String,
    },
    Approval {
        session: SessionId,
        turn: TurnId,
        request: RequestId,
        tool: String,
        summary: String,
        risk: String,
        workspace: PathBuf,
        working_directory: PathBuf,
        arguments: Map<String, Value>,
        command: Option<String>,
        sandbox: Option<String>,
    },
    Stats {
        session: SessionId,
        turn: TurnId,
        stats: GenStats,
    },
    TurnFinished {
        session: SessionId,
        turn: TurnId,
        reason: String,
    },
    Error {
        model_generation: ModelGeneration,
        session: Option<SessionId>,
        turn: Option<TurnId>,
        message: String,
    },
    ShutdownComplete,
}

#[derive(Clone, Debug)]
pub struct SessionSummary {
    pub id: String,
    pub modified: Option<SystemTime>,
    pub records: usize,
}
