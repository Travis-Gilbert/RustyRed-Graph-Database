use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::errors::RustyredError;
use crate::state::{RustyredEdge, RustyredNode};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RustyredCommand {
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
    GraphNodeUpsert,
    GraphEdgeUpsert,
    GraphNodesQuery,
    GraphNeighbors,
    GraphStats,
    GraphVerify,
    GraphRebuildIndexes,
}

impl RustyredCommand {
    pub fn from_name(name: &str) -> Result<Self, RustyredError> {
        match name.trim().to_ascii_uppercase().as_str() {
            "RUSTYRED.RUN.BEGIN" => Ok(Self::RunBegin),
            "RUSTYRED.RUN.STEP" => Ok(Self::RunStep),
            "RUSTYRED.RUN.GET" => Ok(Self::RunGet),
            "RUSTYRED.TOOL.SELECT" => Ok(Self::ToolSelect),
            "RUSTYRED.CONTEXT.PACK" => Ok(Self::ContextPack),
            "RUSTYRED.CONTEXT.GET" => Ok(Self::ContextGet),
            "RUSTYRED.PATCH.PROPOSE" => Ok(Self::PatchPropose),
            "RUSTYRED.PATCH.VALIDATE" => Ok(Self::PatchValidate),
            "RUSTYRED.PATCH.COMMIT" => Ok(Self::PatchCommit),
            "RUSTYRED.STATE.HASH" => Ok(Self::StateHash),
            "RUSTYRED.DEBUG.CYPHER" | "RUSTYRED.CYPHER" => Ok(Self::CypherDebug),
            "RUSTYRED.GRAPH.NODE.UPSERT" => Ok(Self::GraphNodeUpsert),
            "RUSTYRED.GRAPH.EDGE.UPSERT" => Ok(Self::GraphEdgeUpsert),
            "RUSTYRED.GRAPH.NODES.QUERY" => Ok(Self::GraphNodesQuery),
            "RUSTYRED.GRAPH.NEIGHBORS" => Ok(Self::GraphNeighbors),
            "RUSTYRED.GRAPH.STATS" => Ok(Self::GraphStats),
            "RUSTYRED.GRAPH.VERIFY" => Ok(Self::GraphVerify),
            "RUSTYRED.GRAPH.REBUILD_INDEXES" | "RUSTYRED.GRAPH.REBUILD" => {
                Ok(Self::GraphRebuildIndexes)
            }
            _ => Err(RustyredError::unsupported_command(name)),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::RunBegin => "RUSTYRED.RUN.BEGIN",
            Self::RunStep => "RUSTYRED.RUN.STEP",
            Self::RunGet => "RUSTYRED.RUN.GET",
            Self::ToolSelect => "RUSTYRED.TOOL.SELECT",
            Self::ContextPack => "RUSTYRED.CONTEXT.PACK",
            Self::ContextGet => "RUSTYRED.CONTEXT.GET",
            Self::PatchPropose => "RUSTYRED.PATCH.PROPOSE",
            Self::PatchValidate => "RUSTYRED.PATCH.VALIDATE",
            Self::PatchCommit => "RUSTYRED.PATCH.COMMIT",
            Self::StateHash => "RUSTYRED.STATE.HASH",
            Self::CypherDebug => "RUSTYRED.DEBUG.CYPHER",
            Self::GraphNodeUpsert => "RUSTYRED.GRAPH.NODE.UPSERT",
            Self::GraphEdgeUpsert => "RUSTYRED.GRAPH.EDGE.UPSERT",
            Self::GraphNodesQuery => "RUSTYRED.GRAPH.NODES.QUERY",
            Self::GraphNeighbors => "RUSTYRED.GRAPH.NEIGHBORS",
            Self::GraphStats => "RUSTYRED.GRAPH.STATS",
            Self::GraphVerify => "RUSTYRED.GRAPH.VERIFY",
            Self::GraphRebuildIndexes => "RUSTYRED.GRAPH.REBUILD_INDEXES",
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RustyredRequest {
    pub command: String,
    #[serde(default, alias = "payload")]
    pub args: Value,
}

impl RustyredRequest {
    pub fn new(command: impl Into<String>, args: Value) -> Self {
        Self {
            command: command.into(),
            args,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RustyredResponse {
    pub ok: bool,
    pub command: String,
    pub status: String,
    pub payload: Value,
    pub nodes: Vec<RustyredNode>,
    pub edges: Vec<RustyredEdge>,
    pub events: Vec<Value>,
    pub state_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RustyredError>,
}

impl RustyredResponse {
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
        error: RustyredError,
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
