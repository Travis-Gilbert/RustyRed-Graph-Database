use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::errors::ThgError;
use crate::state::{ThgEdge, ThgNode};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ThgCommand {
    RunBegin,
    RunStep,
    RunGet,
    ToolSelect,
    ContextPack,
    ContextGet,
    PatchPropose,
    PatchValidate,
    PatchCommit,
    StateHash,
    CypherDebug,
}

impl ThgCommand {
    pub fn from_name(name: &str) -> Result<Self, ThgError> {
        match name.trim().to_ascii_uppercase().as_str() {
            "THG.RUN.BEGIN" => Ok(Self::RunBegin),
            "THG.RUN.STEP" => Ok(Self::RunStep),
            "THG.RUN.GET" => Ok(Self::RunGet),
            "THG.TOOL.SELECT" => Ok(Self::ToolSelect),
            "THG.CONTEXT.PACK" => Ok(Self::ContextPack),
            "THG.CONTEXT.GET" => Ok(Self::ContextGet),
            "THG.PATCH.PROPOSE" => Ok(Self::PatchPropose),
            "THG.PATCH.VALIDATE" => Ok(Self::PatchValidate),
            "THG.PATCH.COMMIT" => Ok(Self::PatchCommit),
            "THG.STATE.HASH" => Ok(Self::StateHash),
            "THG.DEBUG.CYPHER" | "THG.CYPHER" => Ok(Self::CypherDebug),
            _ => Err(ThgError::unsupported_command(name)),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::RunBegin => "THG.RUN.BEGIN",
            Self::RunStep => "THG.RUN.STEP",
            Self::RunGet => "THG.RUN.GET",
            Self::ToolSelect => "THG.TOOL.SELECT",
            Self::ContextPack => "THG.CONTEXT.PACK",
            Self::ContextGet => "THG.CONTEXT.GET",
            Self::PatchPropose => "THG.PATCH.PROPOSE",
            Self::PatchValidate => "THG.PATCH.VALIDATE",
            Self::PatchCommit => "THG.PATCH.COMMIT",
            Self::StateHash => "THG.STATE.HASH",
            Self::CypherDebug => "THG.DEBUG.CYPHER",
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ThgRequest {
    pub command: String,
    #[serde(default, alias = "payload")]
    pub args: Value,
}

impl ThgRequest {
    pub fn new(command: impl Into<String>, args: Value) -> Self {
        Self {
            command: command.into(),
            args,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ThgResponse {
    pub ok: bool,
    pub command: String,
    pub status: String,
    pub payload: Value,
    pub nodes: Vec<ThgNode>,
    pub edges: Vec<ThgEdge>,
    pub events: Vec<Value>,
    pub state_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ThgError>,
}

impl ThgResponse {
    pub fn ok(
        command: impl Into<String>,
        status: impl Into<String>,
        payload: Value,
        state_hash: impl Into<String>,
    ) -> Self {
        Self {
            ok: true,
            command: command.into(),
            status: status.into(),
            payload,
            nodes: Vec::new(),
            edges: Vec::new(),
            events: Vec::new(),
            state_hash: state_hash.into(),
            error: None,
        }
    }

    pub fn err(
        command: impl Into<String>,
        error: ThgError,
        state_hash: impl Into<String>,
    ) -> Self {
        Self {
            ok: false,
            command: command.into(),
            status: error.code.clone(),
            payload: Value::Object(Default::default()),
            nodes: Vec::new(),
            edges: Vec::new(),
            events: Vec::new(),
            state_hash: state_hash.into(),
            error: Some(error),
        }
    }
}
