use serde_json::{json, Value};

use crate::commands::{RustyredCommand, RustyredRequest, RustyredResponse};
use crate::errors::{RustyredError, RustyredResult};
use crate::graph_store::{
    EdgeRecord, GraphStoreError, InMemoryGraphStore, NeighborQuery, NodeQuery, NodeRecord,
};
use crate::state::{
    stable_hash, ContextState, PatchState, RunState, RustyredEdge, RustyredNode, RustyredState,
    StepState,
};
use crate::store::RustyredStore;

pub trait RustyredExecutor {
    fn execute(
        &mut self,
        command: RustyredCommand,
        args: Value,
    ) -> RustyredResult<RustyredResponse>;
    fn execute_request(&mut self, request: RustyredRequest) -> RustyredResponse;
    fn state(&self) -> &RustyredState;
}

#[derive(Clone, Debug, Default)]
pub struct InMemoryRustyredExecutor {
    state: RustyredState,
    graph_store: InMemoryGraphStore,
}

impl InMemoryRustyredExecutor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_state(state: RustyredState) -> Self {
        Self {
            state,
            graph_store: InMemoryGraphStore::new(),
        }
    }

    pub fn state_hash(&self) -> String {
        self.state.hash()
    }

    pub fn execute_json(&mut self, request_json: &str) -> String {
        match serde_json::from_str::<RustyredRequest>(request_json) {
            Ok(request) => serde_json::to_string(&self.execute_request(request)).unwrap(),
            Err(exc) => {
                let response = RustyredResponse::err(
                    "RUSTYRED.UNKNOWN",
                    RustyredError::invalid_json(exc.to_string()),
                    self.state_hash(),
                );
                serde_json::to_string(&response).unwrap()
            }
        }
    }

    fn run_begin(&mut self, args: Value) -> RustyredResponse {
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
        let mut response = RustyredResponse::ok(
            RustyredCommand::RunBegin.name(),
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

    fn run_step(&mut self, args: Value) -> RustyredResponse {
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
        let edge = RustyredEdge {
            from_id: run_id.clone(),
            edge_type: "HAS_STEP".to_string(),
            to_id: step_id.clone(),
            properties: json!({ "index": index }),
        };
        let mut response = RustyredResponse::ok(
            RustyredCommand::RunStep.name(),
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

    fn run_get(&mut self, args: Value) -> RustyredResponse {
        let run_id = string_arg(&args, "run_id").unwrap_or_default();
        let run = self.state.runs.get(&run_id).cloned();
        RustyredResponse::ok(
            RustyredCommand::RunGet.name(),
            if run.is_some() { "ok" } else { "not_found" },
            json!({ "run": run }),
            self.state_hash(),
        )
    }

    fn tool_select(&mut self, args: Value) -> RustyredResponse {
        let task_type = string_arg(&args, "task_type").unwrap_or_else(|| "other".to_string());
        let required = string_vec_arg(&args, "required_skills");
        let toolkit = compile_toolkit(&task_type, &required);
        let mut response = RustyredResponse::ok(
            RustyredCommand::ToolSelect.name(),
            "ok",
            json!({ "task_type": task_type, "toolkit": toolkit }),
            self.state_hash(),
        );
        response.nodes.push(RustyredNode {
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
            response.nodes.push(RustyredNode {
                id: tool_id.clone(),
                labels: vec!["Tool".to_string()],
                properties: tool,
            });
            response.edges.push(RustyredEdge {
                from_id: format!("tasktype:{task_type}"),
                edge_type: "COMPILES_TOOL".to_string(),
                to_id: tool_id,
                properties: json!({}),
            });
        }
        response
    }

    fn context_pack(&mut self, args: Value) -> RustyredResponse {
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
        let mut response = RustyredResponse::ok(
            RustyredCommand::ContextPack.name(),
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

    fn context_get(&mut self, args: Value) -> RustyredResponse {
        let artifact_id = string_arg(&args, "artifact_id").unwrap_or_default();
        let context = self.state.contexts.get(&artifact_id).cloned();
        RustyredResponse::ok(
            RustyredCommand::ContextGet.name(),
            if context.is_some() { "ok" } else { "not_found" },
            json!({ "context": context }),
            self.state_hash(),
        )
    }

    fn patch_propose(&mut self, args: Value) -> RustyredResponse {
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
        let mut response = RustyredResponse::ok(
            RustyredCommand::PatchPropose.name(),
            "ok",
            json!({ "patch_id": patch_id, "status": "proposed" }),
            self.state_hash(),
        );
        response.nodes.push(node);
        if !run_id.is_empty() {
            response.edges.push(RustyredEdge {
                from_id: run_id,
                edge_type: "PROPOSED_PATCH".to_string(),
                to_id: patch_id,
                properties: json!({}),
            });
        }
        response
    }

    fn patch_validate(&mut self, args: Value) -> RustyredResponse {
        let patch_id = string_arg(&args, "patch_id").unwrap_or_default();
        let findings = args
            .get("findings")
            .cloned()
            .unwrap_or_else(|| json!([{ "code": "human_review_required" }]));
        if let Some(patch) = self.state.patches.get_mut(&patch_id) {
            patch.status = "needs_review".to_string();
            patch.findings = findings.clone();
        }
        RustyredResponse::ok(
            RustyredCommand::PatchValidate.name(),
            "ok",
            json!({ "patch_id": patch_id, "status": "needs_review", "findings": findings }),
            self.state_hash(),
        )
    }

    fn patch_commit(&mut self, args: Value) -> RustyredResponse {
        let patch_id = string_arg(&args, "patch_id").unwrap_or_default();
        if let Some(patch) = self.state.patches.get_mut(&patch_id) {
            patch.status = "committed".to_string();
        }
        RustyredResponse::ok(
            RustyredCommand::PatchCommit.name(),
            "ok",
            json!({ "patch_id": patch_id, "status": "committed" }),
            self.state_hash(),
        )
    }

    fn state_hash_command(&mut self, args: Value) -> RustyredResponse {
        let hash = match args.get("state") {
            Some(value) => stable_hash(value),
            None => self.state_hash(),
        };
        RustyredResponse::ok(
            RustyredCommand::StateHash.name(),
            "ok",
            json!({ "hash": hash }),
            self.state_hash(),
        )
    }

    fn cypher_debug(&mut self, args: Value) -> RustyredResponse {
        let graph = args.get("graph").cloned().unwrap_or_else(|| {
            let (nodes, edges) = self.state.graph();
            json!({ "nodes": nodes, "edges": edges })
        });
        let query = string_arg(&args, "query").unwrap_or_default();
        let rows = debug_cypher_rows(&query, &graph);
        RustyredResponse::ok(
            RustyredCommand::CypherDebug.name(),
            "ok",
            json!({ "rows": rows, "row_count": rows.as_array().map(Vec::len).unwrap_or(0) }),
            self.state_hash(),
        )
    }

    fn graph_node_upsert(&mut self, args: Value) -> RustyredResponse {
        let command = RustyredCommand::GraphNodeUpsert.name();
        self.state.next_seq();
        let node = match node_record_from_args(args) {
            Ok(node) => node,
            Err(error) => return RustyredResponse::err(command, error, self.state_hash()),
        };
        let response_node = rustyred_node_from_record(&node);
        match self.graph_store.upsert_node(node) {
            Ok(write) => {
                let mut response = RustyredResponse::ok(
                    command,
                    "ok",
                    json!({ "write": write, "node": response_node }),
                    self.state_hash(),
                );
                response.nodes.push(response_node);
                response
            }
            Err(error) => graph_store_response_error(command, error, self.state_hash()),
        }
    }

    fn graph_edge_upsert(&mut self, args: Value) -> RustyredResponse {
        let command = RustyredCommand::GraphEdgeUpsert.name();
        self.state.next_seq();
        let edge = match edge_record_from_args(args) {
            Ok(edge) => edge,
            Err(error) => return RustyredResponse::err(command, error, self.state_hash()),
        };
        let response_edge = rustyred_edge_from_record(&edge);
        match self.graph_store.upsert_edge(edge) {
            Ok(write) => {
                let mut response = RustyredResponse::ok(
                    command,
                    "ok",
                    json!({ "write": write, "edge": response_edge }),
                    self.state_hash(),
                );
                response.edges.push(response_edge);
                response
            }
            Err(error) => graph_store_response_error(command, error, self.state_hash()),
        }
    }

    fn graph_nodes_query(&mut self, args: Value) -> RustyredResponse {
        let command = RustyredCommand::GraphNodesQuery.name();
        let query = match serde_json::from_value::<NodeQuery>(args) {
            Ok(query) => query,
            Err(error) => {
                return RustyredResponse::err(
                    command,
                    RustyredError::new("invalid_graph_query", error.to_string()),
                    self.state_hash(),
                )
            }
        };
        let operation = if query.label.is_some() || !query.properties.is_empty() {
            "node_index_seek"
        } else {
            "node_scan"
        };
        let hits = self.graph_store.query_nodes(query);
        let nodes = hits
            .iter()
            .map(rustyred_node_from_record)
            .collect::<Vec<RustyredNode>>();
        let mut response = RustyredResponse::ok(
            command,
            "ok",
            json!({
                "nodes": hits,
                "plan": { "operation": operation },
                "stats": { "returned": nodes.len() },
            }),
            self.state_hash(),
        );
        response.nodes = nodes;
        response
    }

    fn graph_neighbors(&mut self, args: Value) -> RustyredResponse {
        let command = RustyredCommand::GraphNeighbors.name();
        let query = match serde_json::from_value::<NeighborQuery>(args) {
            Ok(query) => query,
            Err(error) => {
                return RustyredResponse::err(
                    command,
                    RustyredError::new("invalid_graph_query", error.to_string()),
                    self.state_hash(),
                )
            }
        };
        let hits = self.graph_store.neighbors(query);
        RustyredResponse::ok(
            command,
            "ok",
            json!({
                "neighbors": hits,
                "plan": { "operation": "adjacency_seek" },
                "stats": { "returned": hits.len() },
            }),
            self.state_hash(),
        )
    }

    fn graph_stats(&mut self) -> RustyredResponse {
        RustyredResponse::ok(
            RustyredCommand::GraphStats.name(),
            "ok",
            json!({ "stats": self.graph_store.stats() }),
            self.state_hash(),
        )
    }

    fn graph_verify(&mut self) -> RustyredResponse {
        let report = self.graph_store.verify();
        RustyredResponse::ok(
            RustyredCommand::GraphVerify.name(),
            if report.ok { "ok" } else { "drift_detected" },
            json!({ "report": report }),
            self.state_hash(),
        )
    }

    fn graph_rebuild_indexes(&mut self) -> RustyredResponse {
        match self.graph_store.rebuild_indexes() {
            Ok(report) => RustyredResponse::ok(
                RustyredCommand::GraphRebuildIndexes.name(),
                if report.after.ok {
                    "ok"
                } else {
                    "canonical_graph_problem"
                },
                json!({ "report": report }),
                self.state_hash(),
            ),
            Err(error) => RustyredResponse::err(
                RustyredCommand::GraphRebuildIndexes.name(),
                RustyredError::new(error.code, error.message),
                self.state_hash(),
            ),
        }
    }
}

#[derive(Clone, Debug)]
pub struct StoreBackedRustyredExecutor<S: RustyredStore> {
    store: S,
    inner: InMemoryRustyredExecutor,
}

impl<S: RustyredStore> StoreBackedRustyredExecutor<S> {
    pub fn new(store: S) -> Self {
        let state = store.load();
        Self {
            store,
            inner: InMemoryRustyredExecutor::from_state(state),
        }
    }

    pub fn store(&self) -> &S {
        &self.store
    }

    pub fn state_hash(&self) -> String {
        self.inner.state_hash()
    }

    pub fn execute_json(&mut self, request_json: &str) -> String {
        match serde_json::from_str::<RustyredRequest>(request_json) {
            Ok(request) => serde_json::to_string(&self.execute_request(request)).unwrap(),
            Err(exc) => {
                let response = RustyredResponse::err(
                    "RUSTYRED.UNKNOWN",
                    RustyredError::invalid_json(exc.to_string()),
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

impl RustyredExecutor for InMemoryRustyredExecutor {
    fn execute(
        &mut self,
        command: RustyredCommand,
        args: Value,
    ) -> RustyredResult<RustyredResponse> {
        Ok(match command {
            RustyredCommand::RunBegin => self.run_begin(args),
            RustyredCommand::RunStep => self.run_step(args),
            RustyredCommand::RunGet => self.run_get(args),
            RustyredCommand::ToolSelect => self.tool_select(args),
            RustyredCommand::ContextPack => self.context_pack(args),
            RustyredCommand::ContextGet => self.context_get(args),
            RustyredCommand::PatchPropose => self.patch_propose(args),
            RustyredCommand::PatchValidate => self.patch_validate(args),
            RustyredCommand::PatchCommit => self.patch_commit(args),
            RustyredCommand::StateHash => self.state_hash_command(args),
            RustyredCommand::CypherDebug => self.cypher_debug(args),
            RustyredCommand::GraphNodeUpsert => self.graph_node_upsert(args),
            RustyredCommand::GraphEdgeUpsert => self.graph_edge_upsert(args),
            RustyredCommand::GraphNodesQuery => self.graph_nodes_query(args),
            RustyredCommand::GraphNeighbors => self.graph_neighbors(args),
            RustyredCommand::GraphStats => self.graph_stats(),
            RustyredCommand::GraphVerify => self.graph_verify(),
            RustyredCommand::GraphRebuildIndexes => self.graph_rebuild_indexes(),
        })
    }

    fn execute_request(&mut self, request: RustyredRequest) -> RustyredResponse {
        let command_name = request.command.clone();
        if let Some(operation) = crate::plugin::builtin_plugin_registry().operation(&command_name) {
            let context = crate::plugin::PluginOperationContext {
                command: operation.command,
                state_hash: self.state_hash(),
            };
            return (operation.handler)(context, request.args);
        }
        match RustyredCommand::from_name(&request.command) {
            Ok(command) => self.execute(command, request.args).unwrap_or_else(|error| {
                RustyredResponse::err(command_name, error, self.state_hash())
            }),
            Err(error) => RustyredResponse::err(command_name, error, self.state_hash()),
        }
    }

    fn state(&self) -> &RustyredState {
        &self.state
    }
}

impl<S: RustyredStore> RustyredExecutor for StoreBackedRustyredExecutor<S> {
    fn execute(
        &mut self,
        command: RustyredCommand,
        args: Value,
    ) -> RustyredResult<RustyredResponse> {
        let response = self.inner.execute(command, args)?;
        if response.ok {
            self.persist();
        }
        Ok(response)
    }

    fn execute_request(&mut self, request: RustyredRequest) -> RustyredResponse {
        let command_name = request.command.clone();
        if let Some(operation) = crate::plugin::builtin_plugin_registry().operation(&command_name) {
            let context = crate::plugin::PluginOperationContext {
                command: operation.command,
                state_hash: self.state_hash(),
            };
            let response = (operation.handler)(context, request.args);
            if response.ok {
                self.persist();
            }
            return response;
        }
        match RustyredCommand::from_name(&request.command) {
            Ok(command) => self.execute(command, request.args).unwrap_or_else(|error| {
                RustyredResponse::err(command_name, error, self.state_hash())
            }),
            Err(error) => RustyredResponse::err(command_name, error, self.state_hash()),
        }
    }

    fn state(&self) -> &RustyredState {
        self.inner.state()
    }
}

pub fn execute_request_json(executor: &mut InMemoryRustyredExecutor, request_json: &str) -> String {
    executor.execute_json(request_json)
}

fn generated_id(prefix: &str, seq: u64) -> String {
    format!("{prefix}:{seq:016x}")
}

fn string_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::to_string)
}

fn bool_arg(args: &Value, key: &str) -> Option<bool> {
    args.get(key).and_then(Value::as_bool)
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

fn node_record_from_args(args: Value) -> Result<NodeRecord, RustyredError> {
    let id = string_arg(&args, "id")
        .or_else(|| string_arg(&args, "node_id"))
        .ok_or_else(|| RustyredError::new("empty_graph_field", "node.id is required"))?;
    let labels = string_vec_arg(&args, "labels");
    let properties = args.get("properties").cloned().unwrap_or_else(|| json!({}));
    let mut node = NodeRecord::new(id, labels, properties);
    node.tombstone = bool_arg(&args, "tombstone").unwrap_or(false);
    Ok(node)
}

fn edge_record_from_args(args: Value) -> Result<EdgeRecord, RustyredError> {
    let id = string_arg(&args, "id")
        .or_else(|| string_arg(&args, "edge_id"))
        .ok_or_else(|| RustyredError::new("empty_graph_field", "edge.id is required"))?;
    let from_id = string_arg(&args, "from_id")
        .ok_or_else(|| RustyredError::new("empty_graph_field", "edge.from_id is required"))?;
    let to_id = string_arg(&args, "to_id")
        .ok_or_else(|| RustyredError::new("empty_graph_field", "edge.to_id is required"))?;
    let edge_type = string_arg(&args, "type")
        .or_else(|| string_arg(&args, "edge_type"))
        .ok_or_else(|| RustyredError::new("empty_graph_field", "edge.type is required"))?;
    let properties = args.get("properties").cloned().unwrap_or_else(|| json!({}));
    let mut edge = EdgeRecord::new(id, from_id, edge_type, to_id, properties);
    edge.tombstone = bool_arg(&args, "tombstone").unwrap_or(false);
    Ok(edge)
}

fn rustyred_node_from_record(node: &NodeRecord) -> RustyredNode {
    RustyredNode {
        id: node.id.clone(),
        labels: node.labels.clone(),
        properties: node.properties.clone(),
    }
}

fn rustyred_edge_from_record(edge: &EdgeRecord) -> RustyredEdge {
    RustyredEdge {
        from_id: edge.from_id.clone(),
        edge_type: edge.edge_type.clone(),
        to_id: edge.to_id.clone(),
        properties: edge.properties.clone(),
    }
}

fn graph_store_response_error(
    command: impl Into<String>,
    error: GraphStoreError,
    state_hash: String,
) -> RustyredResponse {
    RustyredResponse::err(
        command,
        RustyredError::new(error.code, error.message),
        state_hash,
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{InMemoryRustyredExecutor, RustyredExecutor};
    use crate::commands::{RustyredCommand, RustyredRequest};

    #[test]
    fn command_sequence_updates_state_hash() {
        let mut executor = InMemoryRustyredExecutor::new();
        let first_hash = executor.state_hash();
        let begin = executor.execute_request(RustyredRequest::new(
            RustyredCommand::RunBegin.name(),
            json!({ "run_id": "run:1", "task": "ship RustyRed" }),
        ));
        let step = executor.execute_request(RustyredRequest::new(
            RustyredCommand::RunStep.name(),
            json!({ "run_id": "run:1", "step_id": "step:1", "kind": "tool_call" }),
        ));

        assert!(begin.ok);
        assert!(step.ok);
        assert_ne!(first_hash, step.state_hash);
        assert_eq!(executor.state().runs["run:1"].steps.len(), 1);
    }

    #[test]
    fn json_executor_returns_compatible_response_shape() {
        let mut executor = InMemoryRustyredExecutor::new();
        let raw = executor.execute_json(
            r#"{"command":"RUSTYRED.CONTEXT.PACK","args":{"artifact_id":"artifact:1","sections":[]}}"#,
        );
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();

        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["command"], "RUSTYRED.CONTEXT.PACK");
        assert_eq!(parsed["payload"]["artifact_id"], "artifact:1");
        assert!(parsed["state_hash"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
    }

    #[test]
    fn store_backed_executor_persists_after_mutating_command() {
        use super::StoreBackedRustyredExecutor;
        use crate::store::{InMemoryRustyredStore, RustyredStore};

        let store = InMemoryRustyredStore::new();
        let mut executor = StoreBackedRustyredExecutor::new(store);
        let response = executor.execute_request(RustyredRequest::new(
            RustyredCommand::RunBegin.name(),
            json!({ "run_id": "run:persisted", "task": "durable RustyRed" }),
        ));

        assert!(response.ok);
        let saved = executor.store().load();
        assert_eq!(saved.runs["run:persisted"].task, "durable RustyRed");
    }

    #[test]
    fn graph_commands_upsert_query_and_verify_graph_store() {
        let mut executor = InMemoryRustyredExecutor::new();
        let node_a = executor.execute_request(RustyredRequest::new(
            RustyredCommand::GraphNodeUpsert.name(),
            json!({
                "id": "node:a",
                "labels": ["File"],
                "properties": { "path": "src/lib.rs", "repo": "rusty-red" }
            }),
        ));
        let node_b = executor.execute_request(RustyredRequest::new(
            RustyredCommand::GraphNodeUpsert.name(),
            json!({
                "id": "node:b",
                "labels": ["File"],
                "properties": { "path": "src/main.rs", "repo": "rusty-red" }
            }),
        ));
        let edge = executor.execute_request(RustyredRequest::new(
            RustyredCommand::GraphEdgeUpsert.name(),
            json!({
                "id": "edge:ab",
                "from_id": "node:a",
                "type": "IMPORTS",
                "to_id": "node:b",
                "properties": { "weight": 1 }
            }),
        ));
        let query = executor.execute_request(RustyredRequest::new(
            RustyredCommand::GraphNodesQuery.name(),
            json!({
                "label": "File",
                "properties": { "path": "src/lib.rs" }
            }),
        ));
        let neighbors = executor.execute_request(RustyredRequest::new(
            RustyredCommand::GraphNeighbors.name(),
            json!({ "node_id": "node:a", "direction": "out" }),
        ));
        let verify = executor.execute_request(RustyredRequest::new(
            RustyredCommand::GraphVerify.name(),
            json!({}),
        ));
        let rebuild = executor.execute_request(RustyredRequest::new(
            RustyredCommand::GraphRebuildIndexes.name(),
            json!({}),
        ));

        assert!(node_a.ok);
        assert!(node_b.ok);
        assert!(edge.ok);
        assert_eq!(query.payload["plan"]["operation"], "node_index_seek");
        assert_eq!(query.payload["stats"]["returned"], 1);
        assert_eq!(query.nodes[0].id, "node:a");
        assert_eq!(neighbors.payload["plan"]["operation"], "adjacency_seek");
        assert_eq!(neighbors.payload["neighbors"][0]["node_id"], "node:b");
        assert_eq!(verify.payload["report"]["ok"], true);
        assert_eq!(rebuild.payload["report"]["after"]["ok"], true);
    }

    #[test]
    fn graph_edge_command_requires_live_endpoints() {
        let mut executor = InMemoryRustyredExecutor::new();
        executor.execute_request(RustyredRequest::new(
            RustyredCommand::GraphNodeUpsert.name(),
            json!({ "id": "node:a", "labels": ["File"] }),
        ));

        let response = executor.execute_request(RustyredRequest::new(
            RustyredCommand::GraphEdgeUpsert.name(),
            json!({
                "id": "edge:missing",
                "from_id": "node:a",
                "type": "IMPORTS",
                "to_id": "node:missing"
            }),
        ));

        assert!(!response.ok);
        assert_eq!(
            response.error.as_ref().map(|error| error.code.as_str()),
            Some("missing_graph_endpoint")
        );
    }

    #[test]
    fn plugin_operation_round_trips_through_json_executor() {
        let mut executor = InMemoryRustyredExecutor::new();
        let raw = executor.execute_json(
            r#"{"command":"RUSTYRED.PLUGIN.ECHO","args":{"message":"hello plugin"}}"#,
        );
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();

        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["command"], "RUSTYRED.PLUGIN.ECHO");
        assert_eq!(parsed["payload"]["args"]["message"], "hello plugin");
    }
}
