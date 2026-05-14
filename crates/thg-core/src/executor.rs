use serde_json::{json, Value};

use crate::commands::{ThgCommand, ThgRequest, ThgResponse};
use crate::errors::{ThgError, ThgResult};
use crate::state::{
    stable_hash, ContextState, PatchState, RunState, StepState, ThgEdge, ThgNode, ThgState,
};
use crate::store::ThgStore;

pub trait ThgExecutor {
    fn execute(&mut self, command: ThgCommand, args: Value) -> ThgResult<ThgResponse>;
    fn execute_request(&mut self, request: ThgRequest) -> ThgResponse;
    fn state(&self) -> &ThgState;
}

#[derive(Clone, Debug, Default)]
pub struct InMemoryThgExecutor {
    state: ThgState,
}

impl InMemoryThgExecutor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn state_hash(&self) -> String {
        self.state.hash()
    }

    pub fn execute_json(&mut self, request_json: &str) -> String {
        match serde_json::from_str::<ThgRequest>(request_json) {
            Ok(request) => serde_json::to_string(&self.execute_request(request)).unwrap(),
            Err(exc) => {
                let response = ThgResponse::err(
                    "THG.UNKNOWN",
                    ThgError::invalid_json(exc.to_string()),
                    self.state_hash(),
                );
                serde_json::to_string(&response).unwrap()
            }
        }
    }

    fn run_begin(&mut self, args: Value) -> ThgResponse {
        self.state.next_seq();
        let run_id =
            string_arg(&args, "run_id").unwrap_or_else(|| generated_id("run", self.state.seq));
        let run = RunState {
            run_id: run_id.clone(),
            task: string_arg(&args, "task").unwrap_or_default(),
            actor: string_arg(&args, "actor").unwrap_or_else(|| "agent".to_string()),
            scope: args.get("scope").cloned().unwrap_or_else(|| json!({})),
            status: "running".to_string(),
            steps: Vec::new(),
        };
        let node = run.node();
        self.state.runs.insert(run_id.clone(), run);
        let mut response = ThgResponse::ok(
            ThgCommand::RunBegin.name(),
            "ok",
            json!({ "run_id": run_id, "status": "running" }),
            self.state_hash(),
        );
        response.nodes.push(node);
        response
            .events
            .push(json!({ "event": "run_begin", "run_id": run_id }));
        response
    }

    fn run_step(&mut self, args: Value) -> ThgResponse {
        self.state.next_seq();
        let run_id = string_arg(&args, "run_id").unwrap_or_default();
        let step_id =
            string_arg(&args, "step_id").unwrap_or_else(|| generated_id("step", self.state.seq));
        let index = int_arg(&args, "index").unwrap_or_else(|| {
            self.state
                .runs
                .get(&run_id)
                .map(|run| run.steps.len() as i64 + 1)
                .unwrap_or(1)
        });
        let step = StepState {
            step_id: step_id.clone(),
            kind: string_arg(&args, "kind").unwrap_or_else(|| "observation".to_string()),
            index,
            payload: args.get("payload").cloned().unwrap_or_else(|| json!({})),
        };
        let node = step.node(&run_id);
        if let Some(run) = self.state.runs.get_mut(&run_id) {
            run.steps.push(step);
        }
        let edge = ThgEdge {
            from_id: run_id.clone(),
            edge_type: "HAS_STEP".to_string(),
            to_id: step_id.clone(),
            properties: json!({ "index": index }),
        };
        let mut response = ThgResponse::ok(
            ThgCommand::RunStep.name(),
            "ok",
            json!({ "run_id": run_id, "step_id": step_id }),
            self.state_hash(),
        );
        response.nodes.push(node);
        response.edges.push(edge);
        response
            .events
            .push(json!({ "event": "run_step", "run_id": run_id, "step_id": step_id }));
        response
    }

    fn run_get(&mut self, args: Value) -> ThgResponse {
        let run_id = string_arg(&args, "run_id").unwrap_or_default();
        let run = self.state.runs.get(&run_id).cloned();
        ThgResponse::ok(
            ThgCommand::RunGet.name(),
            if run.is_some() { "ok" } else { "not_found" },
            json!({ "run": run }),
            self.state_hash(),
        )
    }

    fn tool_select(&mut self, args: Value) -> ThgResponse {
        let task_type = string_arg(&args, "task_type").unwrap_or_else(|| "other".to_string());
        let required = string_vec_arg(&args, "required_skills");
        let toolkit = compile_toolkit(&task_type, &required);
        let mut response = ThgResponse::ok(
            ThgCommand::ToolSelect.name(),
            "ok",
            json!({ "task_type": task_type, "toolkit": toolkit }),
            self.state_hash(),
        );
        response.nodes.push(ThgNode {
            id: format!("tasktype:{task_type}"),
            labels: vec!["TaskType".to_string()],
            properties: json!({ "task_type": task_type }),
        });
        for tool in toolkit {
            let tool_id = tool
                .get("tool_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            response.nodes.push(ThgNode {
                id: tool_id.clone(),
                labels: vec!["Tool".to_string()],
                properties: tool,
            });
            response.edges.push(ThgEdge {
                from_id: format!("tasktype:{task_type}"),
                edge_type: "COMPILES_TOOL".to_string(),
                to_id: tool_id,
                properties: json!({}),
            });
        }
        response
    }

    fn context_pack(&mut self, args: Value) -> ThgResponse {
        self.state.next_seq();
        let artifact_id = string_arg(&args, "artifact_id")
            .unwrap_or_else(|| generated_id("artifact", self.state.seq));
        let context = ContextState {
            artifact_id: artifact_id.clone(),
            status: "packed".to_string(),
            sections: args.get("sections").cloned().unwrap_or_else(|| json!([])),
            token_ledger: args
                .get("token_ledger")
                .cloned()
                .unwrap_or_else(|| json!({})),
        };
        let node = context.node();
        self.state.contexts.insert(artifact_id.clone(), context);
        let mut response = ThgResponse::ok(
            ThgCommand::ContextPack.name(),
            "ok",
            json!({
                "artifact_id": artifact_id,
                "sections": args.get("sections").cloned().unwrap_or_else(|| json!([])),
                "token_ledger": args.get("token_ledger").cloned().unwrap_or_else(|| json!({})),
            }),
            self.state_hash(),
        );
        response.nodes.push(node);
        response
    }

    fn context_get(&mut self, args: Value) -> ThgResponse {
        let artifact_id = string_arg(&args, "artifact_id").unwrap_or_default();
        let context = self.state.contexts.get(&artifact_id).cloned();
        ThgResponse::ok(
            ThgCommand::ContextGet.name(),
            if context.is_some() { "ok" } else { "not_found" },
            json!({ "context": context }),
            self.state_hash(),
        )
    }

    fn patch_propose(&mut self, args: Value) -> ThgResponse {
        self.state.next_seq();
        let patch_id =
            string_arg(&args, "patch_id").unwrap_or_else(|| generated_id("patch", self.state.seq));
        let run_id = string_arg(&args, "run_id").unwrap_or_default();
        let patch = PatchState {
            patch_id: patch_id.clone(),
            run_id: run_id.clone(),
            status: "proposed".to_string(),
            patch: args.get("patch").cloned().unwrap_or_else(|| json!({})),
            findings: json!([]),
        };
        let node = patch.node();
        self.state.patches.insert(patch_id.clone(), patch);
        let mut response = ThgResponse::ok(
            ThgCommand::PatchPropose.name(),
            "ok",
            json!({ "patch_id": patch_id, "status": "proposed" }),
            self.state_hash(),
        );
        response.nodes.push(node);
        if !run_id.is_empty() {
            response.edges.push(ThgEdge {
                from_id: run_id,
                edge_type: "PROPOSED_PATCH".to_string(),
                to_id: patch_id,
                properties: json!({}),
            });
        }
        response
    }

    fn patch_validate(&mut self, args: Value) -> ThgResponse {
        let patch_id = string_arg(&args, "patch_id").unwrap_or_default();
        let findings = args
            .get("findings")
            .cloned()
            .unwrap_or_else(|| json!([{ "code": "human_review_required" }]));
        if let Some(patch) = self.state.patches.get_mut(&patch_id) {
            patch.status = "needs_review".to_string();
            patch.findings = findings.clone();
        }
        ThgResponse::ok(
            ThgCommand::PatchValidate.name(),
            "ok",
            json!({ "patch_id": patch_id, "status": "needs_review", "findings": findings }),
            self.state_hash(),
        )
    }

    fn patch_commit(&mut self, args: Value) -> ThgResponse {
        let patch_id = string_arg(&args, "patch_id").unwrap_or_default();
        if let Some(patch) = self.state.patches.get_mut(&patch_id) {
            patch.status = "committed".to_string();
        }
        ThgResponse::ok(
            ThgCommand::PatchCommit.name(),
            "ok",
            json!({ "patch_id": patch_id, "status": "committed" }),
            self.state_hash(),
        )
    }

    fn state_hash_command(&mut self, args: Value) -> ThgResponse {
        let hash = match args.get("state") {
            Some(value) => stable_hash(value),
            None => self.state_hash(),
        };
        ThgResponse::ok(
            ThgCommand::StateHash.name(),
            "ok",
            json!({ "hash": hash }),
            self.state_hash(),
        )
    }

    fn cypher_debug(&mut self, args: Value) -> ThgResponse {
        let graph = args.get("graph").cloned().unwrap_or_else(|| {
            let (nodes, edges) = self.state.graph();
            json!({ "nodes": nodes, "edges": edges })
        });
        let query = string_arg(&args, "query").unwrap_or_default();
        let rows = debug_cypher_rows(&query, &graph);
        ThgResponse::ok(
            ThgCommand::CypherDebug.name(),
            "ok",
            json!({ "rows": rows, "row_count": rows.as_array().map(Vec::len).unwrap_or(0) }),
            self.state_hash(),
        )
    }
}

#[derive(Clone, Debug)]
pub struct StoreBackedThgExecutor<S: ThgStore> {
    store: S,
    inner: InMemoryThgExecutor,
}

impl<S: ThgStore> StoreBackedThgExecutor<S> {
    pub fn new(store: S) -> Self {
        let state = store.load();
        Self {
            store,
            inner: InMemoryThgExecutor { state },
        }
    }

    pub fn store(&self) -> &S {
        &self.store
    }

    pub fn state_hash(&self) -> String {
        self.inner.state_hash()
    }

    pub fn execute_json(&mut self, request_json: &str) -> String {
        match serde_json::from_str::<ThgRequest>(request_json) {
            Ok(request) => serde_json::to_string(&self.execute_request(request)).unwrap(),
            Err(exc) => {
                let response = ThgResponse::err(
                    "THG.UNKNOWN",
                    ThgError::invalid_json(exc.to_string()),
                    self.state_hash(),
                );
                serde_json::to_string(&response).unwrap()
            }
        }
    }

    fn persist(&mut self) {
        self.store.save(self.inner.state());
    }
}

impl ThgExecutor for InMemoryThgExecutor {
    fn execute(&mut self, command: ThgCommand, args: Value) -> ThgResult<ThgResponse> {
        Ok(match command {
            ThgCommand::RunBegin => self.run_begin(args),
            ThgCommand::RunStep => self.run_step(args),
            ThgCommand::RunGet => self.run_get(args),
            ThgCommand::ToolSelect => self.tool_select(args),
            ThgCommand::ContextPack => self.context_pack(args),
            ThgCommand::ContextGet => self.context_get(args),
            ThgCommand::PatchPropose => self.patch_propose(args),
            ThgCommand::PatchValidate => self.patch_validate(args),
            ThgCommand::PatchCommit => self.patch_commit(args),
            ThgCommand::StateHash => self.state_hash_command(args),
            ThgCommand::CypherDebug => self.cypher_debug(args),
        })
    }

    fn execute_request(&mut self, request: ThgRequest) -> ThgResponse {
        let command_name = request.command.clone();
        match ThgCommand::from_name(&request.command) {
            Ok(command) => self
                .execute(command, request.args)
                .unwrap_or_else(|error| ThgResponse::err(command_name, error, self.state_hash())),
            Err(error) => ThgResponse::err(command_name, error, self.state_hash()),
        }
    }

    fn state(&self) -> &ThgState {
        &self.state
    }
}

impl<S: ThgStore> ThgExecutor for StoreBackedThgExecutor<S> {
    fn execute(&mut self, command: ThgCommand, args: Value) -> ThgResult<ThgResponse> {
        let response = self.inner.execute(command, args)?;
        if response.ok {
            self.persist();
        }
        Ok(response)
    }

    fn execute_request(&mut self, request: ThgRequest) -> ThgResponse {
        let command_name = request.command.clone();
        match ThgCommand::from_name(&request.command) {
            Ok(command) => self
                .execute(command, request.args)
                .unwrap_or_else(|error| ThgResponse::err(command_name, error, self.state_hash())),
            Err(error) => ThgResponse::err(command_name, error, self.state_hash()),
        }
    }

    fn state(&self) -> &ThgState {
        self.inner.state()
    }
}

pub fn execute_request_json(executor: &mut InMemoryThgExecutor, request_json: &str) -> String {
    executor.execute_json(request_json)
}

fn generated_id(prefix: &str, seq: u64) -> String {
    format!("{prefix}:{seq:016x}")
}

fn string_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::to_string)
}

fn int_arg(args: &Value, key: &str) -> Option<i64> {
    args.get(key).and_then(Value::as_i64)
}

fn string_vec_arg(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn compile_toolkit(task_type: &str, required: &[String]) -> Vec<Value> {
    let catalog = vec![
        json!({
            "tool_id": "native_search",
            "name": "Native Search",
            "skills": ["local_webdoc_search", "redis_priors", "graph_candidates"],
            "inputs": ["query", "scope", "budget"],
            "outputs": ["ranked_results", "search_trace_id", "graph_candidates"],
            "cost": "low",
            "permissions": ["web_browse"],
        }),
        json!({
            "tool_id": "context_artifact_compile",
            "name": "Context Artifact Compile",
            "skills": ["capsule_packing", "token_ledger", "artifact_export"],
            "inputs": ["run_id", "task", "budget_tokens"],
            "outputs": ["context_artifact", "token_ledger", "provenance"],
            "cost": "medium",
            "permissions": ["write_context_artifact"],
        }),
        json!({
            "tool_id": "memory_patch_validation",
            "name": "Memory Patch Validation",
            "skills": ["proposal_review", "provenance_check"],
            "inputs": ["run_id", "patch"],
            "outputs": ["validation_result"],
            "cost": "low",
            "permissions": ["propose_memory_patch"],
        }),
    ];
    if required.is_empty() {
        if matches!(task_type, "search" | "research" | "plan" | "fix" | "review") {
            return catalog.into_iter().take(2).collect();
        }
        return catalog.into_iter().skip(1).take(1).collect();
    }
    let required_set: std::collections::BTreeSet<String> = required.iter().cloned().collect();
    let mut selected = Vec::new();
    for mut tool in catalog {
        let matched: Vec<String> = tool
            .get("skills")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|skill| required_set.contains(*skill))
            .map(str::to_string)
            .collect();
        if !matched.is_empty() {
            tool["matched_skills"] = json!(matched);
            selected.push(tool);
        }
    }
    selected
}

fn debug_cypher_rows(query: &str, graph: &Value) -> Value {
    let normalized = query.split_whitespace().collect::<Vec<_>>().join(" ");
    let nodes = graph
        .get("nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let edges = graph
        .get("edges")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    if normalized.starts_with("MATCH (n:") && normalized.ends_with("RETURN n") {
        let label = normalized
            .trim_start_matches("MATCH (n:")
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_end_matches(')')
            .to_string();
        let rows: Vec<Value> = nodes
            .into_iter()
            .filter(|node| {
                node.get("labels")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .any(|item| item.as_str() == Some(label.as_str()))
            })
            .map(|node| json!({ "n": node }))
            .collect();
        return json!(rows);
    }

    if normalized.starts_with("MATCH (a)-[e:") && normalized.contains("]->(b) RETURN a, e, b") {
        let edge_type = normalized
            .trim_start_matches("MATCH (a)-[e:")
            .split(']')
            .next()
            .unwrap_or("");
        let rows: Vec<Value> = edges
            .into_iter()
            .filter(|edge| edge.get("type").and_then(Value::as_str) == Some(edge_type))
            .map(|edge| json!({ "e": edge }))
            .collect();
        return json!(rows);
    }

    json!([])
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{InMemoryThgExecutor, ThgExecutor};
    use crate::commands::{ThgCommand, ThgRequest};

    #[test]
    fn command_sequence_updates_state_hash() {
        let mut executor = InMemoryThgExecutor::new();
        let first_hash = executor.state_hash();
        let begin = executor.execute_request(ThgRequest::new(
            ThgCommand::RunBegin.name(),
            json!({ "run_id": "run:1", "task": "ship THG" }),
        ));
        let step = executor.execute_request(ThgRequest::new(
            ThgCommand::RunStep.name(),
            json!({ "run_id": "run:1", "step_id": "step:1", "kind": "tool_call" }),
        ));

        assert!(begin.ok);
        assert!(step.ok);
        assert_ne!(first_hash, step.state_hash);
        assert_eq!(executor.state().runs["run:1"].steps.len(), 1);
    }

    #[test]
    fn json_executor_returns_compatible_response_shape() {
        let mut executor = InMemoryThgExecutor::new();
        let raw = executor.execute_json(
            r#"{"command":"THG.CONTEXT.PACK","args":{"artifact_id":"artifact:1","sections":[]}}"#,
        );
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();

        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["command"], "THG.CONTEXT.PACK");
        assert_eq!(parsed["payload"]["artifact_id"], "artifact:1");
        assert!(parsed["state_hash"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
    }

    #[test]
    fn store_backed_executor_persists_after_mutating_command() {
        use super::StoreBackedThgExecutor;
        use crate::store::{InMemoryThgStore, ThgStore};

        let store = InMemoryThgStore::new();
        let mut executor = StoreBackedThgExecutor::new(store);
        let response = executor.execute_request(ThgRequest::new(
            ThgCommand::RunBegin.name(),
            json!({ "run_id": "run:persisted", "task": "durable THG" }),
        ));

        assert!(response.ok);
        let saved = executor.store().load();
        assert_eq!(saved.runs["run:persisted"].task, "durable THG");
    }
}
