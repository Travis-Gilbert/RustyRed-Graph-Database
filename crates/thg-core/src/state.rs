use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ThgState {
    pub seq: u64,
    pub runs: BTreeMap<String, RunState>,
    pub contexts: BTreeMap<String, ContextState>,
    pub patches: BTreeMap<String, PatchState>,
}

impl ThgState {
    pub fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    pub fn hash(&self) -> String {
        stable_hash(self)
    }

    pub fn graph(&self) -> (Vec<ThgNode>, Vec<ThgEdge>) {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();

        for run in self.runs.values() {
            nodes.push(run.node());
            for step in &run.steps {
                nodes.push(step.node(&run.run_id));
                edges.push(ThgEdge {
                    from_id: run.run_id.clone(),
                    edge_type: "HAS_STEP".to_string(),
                    to_id: step.step_id.clone(),
                    properties: json!({ "index": step.index }),
                });
            }
        }
        for context in self.contexts.values() {
            nodes.push(context.node());
        }
        for patch in self.patches.values() {
            nodes.push(patch.node());
            if !patch.run_id.is_empty() {
                edges.push(ThgEdge {
                    from_id: patch.run_id.clone(),
                    edge_type: "PROPOSED_PATCH".to_string(),
                    to_id: patch.patch_id.clone(),
                    properties: Value::Object(Default::default()),
                });
            }
        }

        (nodes, edges)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunState {
    pub run_id: String,
    pub task: String,
    pub actor: String,
    pub scope: Value,
    pub status: String,
    pub steps: Vec<StepState>,
}

impl RunState {
    pub fn node(&self) -> ThgNode {
        ThgNode {
            id: self.run_id.clone(),
            labels: vec!["AgentRun".to_string()],
            properties: json!({
                "task": self.task,
                "actor": self.actor,
                "scope": self.scope,
                "status": self.status,
            }),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StepState {
    pub step_id: String,
    pub kind: String,
    pub index: i64,
    pub payload: Value,
}

impl StepState {
    pub fn node(&self, run_id: &str) -> ThgNode {
        ThgNode {
            id: self.step_id.clone(),
            labels: vec!["Step".to_string()],
            properties: json!({
                "run_id": run_id,
                "kind": self.kind,
                "index": self.index,
                "payload": self.payload,
            }),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ContextState {
    pub artifact_id: String,
    pub status: String,
    pub sections: Value,
    pub token_ledger: Value,
}

impl ContextState {
    pub fn node(&self) -> ThgNode {
        ThgNode {
            id: self.artifact_id.clone(),
            labels: vec!["ContextArtifact".to_string()],
            properties: json!({
                "status": self.status,
                "sections": self.sections,
                "token_ledger": self.token_ledger,
            }),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PatchState {
    pub patch_id: String,
    pub run_id: String,
    pub status: String,
    pub patch: Value,
    pub findings: Value,
}

impl PatchState {
    pub fn node(&self) -> ThgNode {
        ThgNode {
            id: self.patch_id.clone(),
            labels: vec!["MemoryPatch".to_string()],
            properties: json!({
                "status": self.status,
                "patch": self.patch,
                "findings": self.findings,
            }),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ThgNode {
    pub id: String,
    pub labels: Vec<String>,
    pub properties: Value,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ThgEdge {
    pub from_id: String,
    #[serde(rename = "type")]
    pub edge_type: String,
    pub to_id: String,
    pub properties: Value,
}

pub fn stable_hash<T: Serialize>(value: T) -> String {
    let encoded = serde_json::to_vec(&value).unwrap_or_default();
    let digest = Sha256::digest(encoded);
    format!("sha256:{digest:x}")
}
