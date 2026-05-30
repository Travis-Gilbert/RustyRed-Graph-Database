use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::graph_store::{unix_ms, EdgeRecord, GraphSnapshot, NodeRecord};
use crate::state::stable_hash;

pub const VERSIONED_GRAPH_PROTOCOL_VERSION: &str = "rustyred-versioned-graph-v1";
pub const GRAPH_PACK_COMPILER_VERSION: &str = "rustyred-graph-pack-compiler-v1";
pub const DEFAULT_GRAPH_BRANCH: &str = "main";

const PROLLY_MIN_ENTRIES: usize = 4;
const PROLLY_MAX_ENTRIES: usize = 16;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphObjectKind {
    Node,
    Edge,
}

impl GraphObjectKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Edge => "edge",
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GraphContentObject {
    pub key: String,
    pub kind: GraphObjectKind,
    pub hash: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parent_hashes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphTreeEntry {
    pub key: String,
    pub kind: GraphObjectKind,
    pub object_hash: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphTreeChild {
    pub hash: String,
    pub first_key: String,
    pub last_key: String,
    pub entries_total: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphTreeNode {
    pub hash: String,
    pub level: u32,
    pub first_key: String,
    pub last_key: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<GraphTreeEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<GraphTreeChild>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphProllyTree {
    pub root_hash: String,
    pub root_level: u32,
    pub entries_total: usize,
    pub nodes: Vec<GraphTreeNode>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphCommit {
    pub commit_hash: String,
    pub tree_hash: String,
    pub branch: String,
    pub parent_commits: Vec<String>,
    pub author: String,
    pub message: String,
    pub timestamp_unix_ms: u128,
    pub graph_version: u64,
    pub objects_total: usize,
    pub compiler_version: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GraphCompilerCapability {
    pub name: String,
    pub kind: String,
    pub mime_type: String,
    pub content_hash: String,
    pub body: Value,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GraphPackManifest {
    pub protocol_version: String,
    pub compiler_version: String,
    pub name: String,
    pub branch: String,
    pub commit_hash: String,
    pub tree_hash: String,
    pub graph_version: u64,
    pub nodes_total: usize,
    pub edges_total: usize,
    pub objects_total: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CompiledGraphPack {
    pub protocol_version: String,
    pub compiler_version: String,
    pub manifest: GraphPackManifest,
    pub commit: GraphCommit,
    pub tree: GraphProllyTree,
    pub capabilities: Vec<GraphCompilerCapability>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub objects: Vec<GraphContentObject>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GraphCompileOptions {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub parent_commits: Vec<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub timestamp_unix_ms: Option<u128>,
    #[serde(default = "default_include_payloads")]
    pub include_payloads: bool,
}

impl Default for GraphCompileOptions {
    fn default() -> Self {
        Self {
            name: None,
            branch: None,
            parent_commits: Vec::new(),
            author: None,
            message: None,
            timestamp_unix_ms: None,
            include_payloads: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphDiffEntry {
    pub key: String,
    pub kind: GraphObjectKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_hash: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphVersionDiff {
    pub protocol_version: String,
    pub base_tree_hash: String,
    pub target_tree_hash: String,
    pub added: Vec<GraphDiffEntry>,
    pub removed: Vec<GraphDiffEntry>,
    pub modified: Vec<GraphDiffEntry>,
    pub unchanged: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphVersionRef {
    pub name: String,
    pub commit_hash: String,
    pub tree_hash: String,
    pub graph_version: u64,
    pub updated_at_unix_ms: u128,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GraphVersionRepository {
    #[serde(default = "versioned_graph_protocol_string")]
    pub protocol_version: String,
    #[serde(default)]
    pub refs: Vec<GraphVersionRef>,
    #[serde(default)]
    pub packs: Vec<CompiledGraphPack>,
}

impl Default for GraphVersionRepository {
    fn default() -> Self {
        Self {
            protocol_version: VERSIONED_GRAPH_PROTOCOL_VERSION.to_string(),
            refs: Vec::new(),
            packs: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GraphRefUpdate {
    pub protocol_version: String,
    pub repository: GraphVersionRepository,
    pub reference: GraphVersionRef,
    pub pack: CompiledGraphPack,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GraphVersionLog {
    pub protocol_version: String,
    pub target: String,
    pub commits: Vec<GraphCommit>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GraphCheckoutResult {
    pub protocol_version: String,
    pub target: String,
    pub commit: GraphCommit,
    pub tree: GraphProllyTree,
    pub snapshot: GraphSnapshot,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphMergeStrategy {
    AutoConfidence,
    PreferOurs,
    PreferTheirs,
    Manual,
}

impl Default for GraphMergeStrategy {
    fn default() -> Self {
        Self::AutoConfidence
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GraphMergeOptions {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub parent_commits: Vec<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub timestamp_unix_ms: Option<u128>,
    #[serde(default = "default_include_payloads")]
    pub include_payloads: bool,
    #[serde(default)]
    pub strategy: GraphMergeStrategy,
    #[serde(default)]
    pub min_confidence_delta: f64,
}

impl Default for GraphMergeOptions {
    fn default() -> Self {
        Self {
            name: None,
            branch: None,
            parent_commits: Vec::new(),
            author: None,
            message: None,
            timestamp_unix_ms: None,
            include_payloads: true,
            strategy: GraphMergeStrategy::AutoConfidence,
            min_confidence_delta: 0.0,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphMergeSide {
    Ours,
    Theirs,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GraphMergeResolution {
    pub key: String,
    pub kind: GraphObjectKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<GraphMergeSide>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ours_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theirs_hash: Option<String>,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GraphMergeConflict {
    pub key: String,
    pub kind: GraphObjectKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ours_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theirs_hash: Option<String>,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GraphMergeResult {
    pub protocol_version: String,
    pub status: String,
    pub base_tree_hash: String,
    pub ours_tree_hash: String,
    pub theirs_tree_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_tree_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_snapshot: Option<GraphSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_pack: Option<CompiledGraphPack>,
    pub resolved: Vec<GraphMergeResolution>,
    pub conflicts: Vec<GraphMergeConflict>,
}

fn default_include_payloads() -> bool {
    true
}

fn versioned_graph_protocol_string() -> String {
    VERSIONED_GRAPH_PROTOCOL_VERSION.to_string()
}

pub fn compile_graph_pack(
    snapshot: &GraphSnapshot,
    options: GraphCompileOptions,
) -> CompiledGraphPack {
    let name = clean_or_default(options.name, "graph-pack");
    let branch = clean_or_default(options.branch, DEFAULT_GRAPH_BRANCH);
    let author = clean_or_default(options.author, "rustyred");
    let message = clean_or_default(options.message, "compile graph snapshot");
    let timestamp_unix_ms = options.timestamp_unix_ms.unwrap_or_else(unix_ms);
    let objects = snapshot_content_objects(snapshot, options.include_payloads);
    let tree = build_prolly_tree(&objects);
    let commit = build_commit(
        &tree,
        &branch,
        options.parent_commits,
        &author,
        &message,
        timestamp_unix_ms,
        snapshot.version,
    );
    let manifest = GraphPackManifest {
        protocol_version: VERSIONED_GRAPH_PROTOCOL_VERSION.to_string(),
        compiler_version: GRAPH_PACK_COMPILER_VERSION.to_string(),
        name: name.clone(),
        branch: branch.clone(),
        commit_hash: commit.commit_hash.clone(),
        tree_hash: tree.root_hash.clone(),
        graph_version: snapshot.version,
        nodes_total: snapshot.nodes.len(),
        edges_total: snapshot.edges.len(),
        objects_total: objects.len(),
    };
    let manifest_body = serde_json::to_value(&manifest).unwrap_or_else(|_| json!({}));
    let validator_body = json!({
        "validator": "rustyred.verify_tree_root",
        "tree_hash": tree.root_hash,
        "objects_total": objects.len(),
        "protocol_version": VERSIONED_GRAPH_PROTOCOL_VERSION,
    });
    let capabilities = vec![
        GraphCompilerCapability {
            name: format!("{name}.manifest"),
            kind: "graph_manifest".to_string(),
            mime_type: "application/json".to_string(),
            content_hash: stable_hash(&manifest_body),
            body: manifest_body,
        },
        GraphCompilerCapability {
            name: format!("{name}.verify_tree_root"),
            kind: "validator".to_string(),
            mime_type: "application/json".to_string(),
            content_hash: stable_hash(&validator_body),
            body: validator_body,
        },
    ];

    CompiledGraphPack {
        protocol_version: VERSIONED_GRAPH_PROTOCOL_VERSION.to_string(),
        compiler_version: GRAPH_PACK_COMPILER_VERSION.to_string(),
        manifest,
        commit,
        tree,
        capabilities,
        objects: if options.include_payloads {
            objects
        } else {
            Vec::new()
        },
    }
}

pub fn diff_graph_snapshots(base: &GraphSnapshot, target: &GraphSnapshot) -> GraphVersionDiff {
    let base_objects = snapshot_content_objects(base, false);
    let target_objects = snapshot_content_objects(target, false);
    let base_tree = build_prolly_tree(&base_objects);
    let target_tree = build_prolly_tree(&target_objects);
    let base_map = base_objects
        .into_iter()
        .map(|object| (object.key.clone(), object))
        .collect::<std::collections::BTreeMap<_, _>>();
    let target_map = target_objects
        .into_iter()
        .map(|object| (object.key.clone(), object))
        .collect::<std::collections::BTreeMap<_, _>>();

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut modified = Vec::new();
    let mut unchanged = 0usize;

    for (key, target_object) in &target_map {
        match base_map.get(key) {
            None => added.push(GraphDiffEntry {
                key: key.clone(),
                kind: target_object.kind.clone(),
                old_hash: None,
                new_hash: Some(target_object.hash.clone()),
            }),
            Some(base_object) if base_object.hash != target_object.hash => {
                modified.push(GraphDiffEntry {
                    key: key.clone(),
                    kind: target_object.kind.clone(),
                    old_hash: Some(base_object.hash.clone()),
                    new_hash: Some(target_object.hash.clone()),
                });
            }
            Some(_) => unchanged += 1,
        }
    }

    for (key, base_object) in &base_map {
        if !target_map.contains_key(key) {
            removed.push(GraphDiffEntry {
                key: key.clone(),
                kind: base_object.kind.clone(),
                old_hash: Some(base_object.hash.clone()),
                new_hash: None,
            });
        }
    }

    GraphVersionDiff {
        protocol_version: VERSIONED_GRAPH_PROTOCOL_VERSION.to_string(),
        base_tree_hash: base_tree.root_hash,
        target_tree_hash: target_tree.root_hash,
        added,
        removed,
        modified,
        unchanged,
    }
}

pub fn update_graph_ref(
    mut repository: GraphVersionRepository,
    pack: CompiledGraphPack,
    branch: Option<String>,
    updated_at_unix_ms: Option<u128>,
) -> GraphRefUpdate {
    repository.protocol_version = VERSIONED_GRAPH_PROTOCOL_VERSION.to_string();
    let ref_name = clean_or_default(
        branch.or_else(|| Some(pack.commit.branch.clone())),
        DEFAULT_GRAPH_BRANCH,
    );
    let updated_at_unix_ms = updated_at_unix_ms.unwrap_or_else(unix_ms);
    let reference = GraphVersionRef {
        name: ref_name.clone(),
        commit_hash: pack.commit.commit_hash.clone(),
        tree_hash: pack.tree.root_hash.clone(),
        graph_version: pack.commit.graph_version,
        updated_at_unix_ms,
    };

    repository
        .refs
        .retain(|reference| reference.name != ref_name);
    repository.refs.push(reference.clone());
    repository.refs.sort_by(|a, b| a.name.cmp(&b.name));

    repository
        .packs
        .retain(|existing| existing.commit.commit_hash != pack.commit.commit_hash);
    repository.packs.push(pack.clone());
    repository
        .packs
        .sort_by(|a, b| a.commit.commit_hash.cmp(&b.commit.commit_hash));

    GraphRefUpdate {
        protocol_version: VERSIONED_GRAPH_PROTOCOL_VERSION.to_string(),
        repository,
        reference,
        pack,
    }
}

pub fn graph_version_log(
    repository: &GraphVersionRepository,
    target: Option<&str>,
) -> GraphVersionLog {
    let target = target
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_GRAPH_BRANCH)
        .to_string();
    let commits_by_hash = repository
        .packs
        .iter()
        .map(|pack| (pack.commit.commit_hash.clone(), pack.commit.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut commits = Vec::new();
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::new();
    if let Some(head) = resolve_repository_target(repository, &target) {
        queue.push_back(head);
    }

    while let Some(commit_hash) = queue.pop_front() {
        if !seen.insert(commit_hash.clone()) {
            continue;
        }
        let Some(commit) = commits_by_hash.get(&commit_hash) else {
            continue;
        };
        commits.push(commit.clone());
        for parent in &commit.parent_commits {
            queue.push_back(parent.clone());
        }
    }

    GraphVersionLog {
        protocol_version: VERSIONED_GRAPH_PROTOCOL_VERSION.to_string(),
        target,
        commits,
    }
}

pub fn checkout_graph_version(
    repository: &GraphVersionRepository,
    target: &str,
) -> Option<GraphCheckoutResult> {
    let target = target.trim();
    if target.is_empty() {
        return None;
    }
    let commit_hash = resolve_repository_target(repository, target)?;
    let pack = repository
        .packs
        .iter()
        .find(|pack| pack.commit.commit_hash == commit_hash)?;
    let snapshot = graph_snapshot_from_pack(pack)?;
    Some(GraphCheckoutResult {
        protocol_version: VERSIONED_GRAPH_PROTOCOL_VERSION.to_string(),
        target: target.to_string(),
        commit: pack.commit.clone(),
        tree: pack.tree.clone(),
        snapshot,
    })
}

pub fn merge_graph_snapshots(
    base: &GraphSnapshot,
    ours: &GraphSnapshot,
    theirs: &GraphSnapshot,
    options: GraphMergeOptions,
) -> GraphMergeResult {
    let base_tree = build_prolly_tree(&snapshot_content_objects(base, false));
    let ours_tree = build_prolly_tree(&snapshot_content_objects(ours, false));
    let theirs_tree = build_prolly_tree(&snapshot_content_objects(theirs, false));
    let base_records = merge_records_by_key(base);
    let ours_records = merge_records_by_key(ours);
    let theirs_records = merge_records_by_key(theirs);
    let mut keys = BTreeSet::new();
    keys.extend(base_records.keys().cloned());
    keys.extend(ours_records.keys().cloned());
    keys.extend(theirs_records.keys().cloned());

    let mut merged_records = Vec::new();
    let mut resolved = Vec::new();
    let mut conflicts = Vec::new();

    for key in keys {
        let base_record = base_records.get(&key);
        let ours_record = ours_records.get(&key);
        let theirs_record = theirs_records.get(&key);
        let kind = merge_record_kind(base_record, ours_record, theirs_record);
        let base_hash = base_record.map(MergeRecord::hash);
        let ours_hash = ours_record.map(MergeRecord::hash);
        let theirs_hash = theirs_record.map(MergeRecord::hash);

        match resolve_merge_record(
            base_record,
            ours_record,
            theirs_record,
            &options.strategy,
            options.min_confidence_delta,
        ) {
            MergeDecision::Resolved {
                selected,
                record,
                reason,
            } => {
                if let Some(record) = record {
                    merged_records.push(record);
                }
                resolved.push(GraphMergeResolution {
                    key,
                    kind,
                    selected,
                    base_hash,
                    ours_hash,
                    theirs_hash,
                    reason,
                });
            }
            MergeDecision::Conflict { reason } => conflicts.push(GraphMergeConflict {
                key,
                kind,
                base_hash,
                ours_hash,
                theirs_hash,
                reason,
            }),
        }
    }

    let (merged_snapshot, merged_pack, merged_tree_hash) = if conflicts.is_empty() {
        let snapshot = snapshot_from_merge_records(
            base.version
                .max(ours.version)
                .max(theirs.version)
                .saturating_add(1),
            merged_records,
        );
        let pack = compile_graph_pack(
            &snapshot,
            GraphCompileOptions {
                name: options
                    .name
                    .or_else(|| Some("merged-graph-pack".to_string())),
                branch: options.branch,
                parent_commits: options.parent_commits,
                author: options.author,
                message: options
                    .message
                    .or_else(|| Some("merge graph snapshots".to_string())),
                timestamp_unix_ms: options.timestamp_unix_ms,
                include_payloads: options.include_payloads,
            },
        );
        (
            Some(snapshot),
            Some(pack.clone()),
            Some(pack.tree.root_hash.clone()),
        )
    } else {
        (None, None, None)
    };

    GraphMergeResult {
        protocol_version: VERSIONED_GRAPH_PROTOCOL_VERSION.to_string(),
        status: if conflicts.is_empty() {
            "clean".to_string()
        } else {
            "conflicted".to_string()
        },
        base_tree_hash: base_tree.root_hash,
        ours_tree_hash: ours_tree.root_hash,
        theirs_tree_hash: theirs_tree.root_hash,
        merged_tree_hash,
        merged_snapshot,
        merged_pack,
        resolved,
        conflicts,
    }
}

pub fn snapshot_content_objects(
    snapshot: &GraphSnapshot,
    include_payloads: bool,
) -> Vec<GraphContentObject> {
    let mut objects = Vec::with_capacity(snapshot.nodes.len() + snapshot.edges.len());
    for node in &snapshot.nodes {
        objects.push(node_content_object(node, include_payloads));
    }
    for edge in &snapshot.edges {
        objects.push(edge_content_object(edge, include_payloads));
    }
    objects.sort_by(|a, b| a.key.cmp(&b.key));
    objects
}

pub fn build_prolly_tree(objects: &[GraphContentObject]) -> GraphProllyTree {
    let entries = objects
        .iter()
        .map(|object| GraphTreeEntry {
            key: object.key.clone(),
            kind: object.kind.clone(),
            object_hash: object.hash.clone(),
        })
        .collect::<Vec<_>>();

    let mut all_nodes = Vec::new();
    let mut current_level = leaf_nodes(entries);
    all_nodes.extend(current_level.iter().cloned());

    let mut level = 1u32;
    while current_level.len() > 1 {
        let parents = parent_nodes(&current_level, level);
        all_nodes.extend(parents.iter().cloned());
        current_level = parents;
        level += 1;
    }

    let root = current_level
        .into_iter()
        .next()
        .unwrap_or_else(empty_leaf_node);
    GraphProllyTree {
        root_hash: root.hash.clone(),
        root_level: root.level,
        entries_total: objects.len(),
        nodes: all_nodes,
    }
}

fn node_content_object(node: &NodeRecord, include_payload: bool) -> GraphContentObject {
    GraphContentObject {
        key: format!("node/{}", node.id),
        kind: GraphObjectKind::Node,
        hash: node.content_hash.clone().unwrap_or_else(|| node.checksum()),
        parent_hashes: node.parent_hashes.clone(),
        payload: include_payload.then(|| serde_json::to_value(node).unwrap_or_else(|_| json!({}))),
    }
}

fn edge_content_object(edge: &EdgeRecord, include_payload: bool) -> GraphContentObject {
    GraphContentObject {
        key: format!("edge/{}", edge.id),
        kind: GraphObjectKind::Edge,
        hash: edge.content_hash.clone().unwrap_or_else(|| edge.checksum()),
        parent_hashes: edge.parent_hashes.clone(),
        payload: include_payload.then(|| serde_json::to_value(edge).unwrap_or_else(|_| json!({}))),
    }
}

fn leaf_nodes(entries: Vec<GraphTreeEntry>) -> Vec<GraphTreeNode> {
    if entries.is_empty() {
        return vec![empty_leaf_node()];
    }
    chunk_by_boundary(entries, |chunk| make_leaf_node(chunk.to_vec()))
}

fn parent_nodes(children: &[GraphTreeNode], level: u32) -> Vec<GraphTreeNode> {
    let descriptors = children
        .iter()
        .map(|child| GraphTreeChild {
            hash: child.hash.clone(),
            first_key: child.first_key.clone(),
            last_key: child.last_key.clone(),
            entries_total: child.entry_count(),
        })
        .collect::<Vec<_>>();
    chunk_by_boundary(descriptors, |chunk| make_parent_node(level, chunk.to_vec()))
}

fn chunk_by_boundary<T: Clone>(
    items: Vec<T>,
    make_node: impl Fn(&[T]) -> GraphTreeNode,
) -> Vec<GraphTreeNode>
where
    T: Serialize,
{
    let mut nodes = Vec::new();
    let mut chunk = Vec::new();
    for item in items {
        chunk.push(item);
        let item_hash = stable_hash(chunk.last().expect("chunk has item"));
        if should_split_chunk(chunk.len(), &item_hash) {
            nodes.push(make_node(&chunk));
            chunk.clear();
        }
    }
    if !chunk.is_empty() {
        nodes.push(make_node(&chunk));
    }
    nodes
}

fn make_leaf_node(entries: Vec<GraphTreeEntry>) -> GraphTreeNode {
    let first_key = entries
        .first()
        .map(|entry| entry.key.clone())
        .unwrap_or_default();
    let last_key = entries
        .last()
        .map(|entry| entry.key.clone())
        .unwrap_or_default();
    let hash = tree_node_hash(0, &first_key, &last_key, &entries, &[]);
    GraphTreeNode {
        hash,
        level: 0,
        first_key,
        last_key,
        entries,
        children: Vec::new(),
    }
}

fn make_parent_node(level: u32, children: Vec<GraphTreeChild>) -> GraphTreeNode {
    let first_key = children
        .first()
        .map(|child| child.first_key.clone())
        .unwrap_or_default();
    let last_key = children
        .last()
        .map(|child| child.last_key.clone())
        .unwrap_or_default();
    let hash = tree_node_hash(level, &first_key, &last_key, &[], &children);
    GraphTreeNode {
        hash,
        level,
        first_key,
        last_key,
        entries: Vec::new(),
        children,
    }
}

fn empty_leaf_node() -> GraphTreeNode {
    make_leaf_node(Vec::new())
}

fn tree_node_hash(
    level: u32,
    first_key: &str,
    last_key: &str,
    entries: &[GraphTreeEntry],
    children: &[GraphTreeChild],
) -> String {
    #[derive(Serialize)]
    struct TreeNodeHashInput<'a> {
        protocol_version: &'static str,
        level: u32,
        first_key: &'a str,
        last_key: &'a str,
        entries: &'a [GraphTreeEntry],
        children: &'a [GraphTreeChild],
    }

    stable_hash(TreeNodeHashInput {
        protocol_version: VERSIONED_GRAPH_PROTOCOL_VERSION,
        level,
        first_key,
        last_key,
        entries,
        children,
    })
}

fn build_commit(
    tree: &GraphProllyTree,
    branch: &str,
    parent_commits: Vec<String>,
    author: &str,
    message: &str,
    timestamp_unix_ms: u128,
    graph_version: u64,
) -> GraphCommit {
    #[derive(Serialize)]
    struct CommitHashInput<'a> {
        protocol_version: &'static str,
        tree_hash: &'a str,
        branch: &'a str,
        parent_commits: &'a [String],
        author: &'a str,
        message: &'a str,
        timestamp_unix_ms: u128,
        graph_version: u64,
        objects_total: usize,
        compiler_version: &'static str,
    }

    let commit_hash = stable_hash(CommitHashInput {
        protocol_version: VERSIONED_GRAPH_PROTOCOL_VERSION,
        tree_hash: &tree.root_hash,
        branch,
        parent_commits: &parent_commits,
        author,
        message,
        timestamp_unix_ms,
        graph_version,
        objects_total: tree.entries_total,
        compiler_version: GRAPH_PACK_COMPILER_VERSION,
    });

    GraphCommit {
        commit_hash,
        tree_hash: tree.root_hash.clone(),
        branch: branch.to_string(),
        parent_commits,
        author: author.to_string(),
        message: message.to_string(),
        timestamp_unix_ms,
        graph_version,
        objects_total: tree.entries_total,
        compiler_version: GRAPH_PACK_COMPILER_VERSION.to_string(),
    }
}

fn should_split_chunk(len: usize, hash: &str) -> bool {
    if len >= PROLLY_MAX_ENTRIES {
        return true;
    }
    len >= PROLLY_MIN_ENTRIES
        && hash
            .as_bytes()
            .last()
            .map(|byte| matches!(byte, b'0' | b'4' | b'8' | b'c'))
            .unwrap_or(false)
}

fn clean_or_default(value: Option<String>, default: &str) -> String {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_string())
}

impl GraphTreeNode {
    fn entry_count(&self) -> usize {
        if self.level == 0 {
            self.entries.len()
        } else {
            self.children.iter().map(|child| child.entries_total).sum()
        }
    }
}

fn resolve_repository_target(repository: &GraphVersionRepository, target: &str) -> Option<String> {
    repository
        .refs
        .iter()
        .find(|reference| reference.name == target)
        .map(|reference| reference.commit_hash.clone())
        .or_else(|| {
            repository
                .packs
                .iter()
                .find(|pack| pack.commit.commit_hash == target)
                .map(|pack| pack.commit.commit_hash.clone())
        })
}

fn graph_snapshot_from_pack(pack: &CompiledGraphPack) -> Option<GraphSnapshot> {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for object in &pack.objects {
        let payload = object.payload.clone()?;
        match object.kind {
            GraphObjectKind::Node => nodes.push(serde_json::from_value(payload).ok()?),
            GraphObjectKind::Edge => edges.push(serde_json::from_value(payload).ok()?),
        }
    }
    nodes.sort_by(|a: &NodeRecord, b| a.id.cmp(&b.id));
    edges.sort_by(|a: &EdgeRecord, b| a.id.cmp(&b.id));
    Some(GraphSnapshot {
        version: pack.commit.graph_version,
        nodes,
        edges,
    })
}

#[derive(Clone, Debug)]
enum MergeRecord {
    Node(NodeRecord),
    Edge(EdgeRecord),
}

impl MergeRecord {
    fn key(&self) -> String {
        match self {
            Self::Node(node) => format!("node/{}", node.id),
            Self::Edge(edge) => format!("edge/{}", edge.id),
        }
    }

    fn kind(&self) -> GraphObjectKind {
        match self {
            Self::Node(_) => GraphObjectKind::Node,
            Self::Edge(_) => GraphObjectKind::Edge,
        }
    }

    fn hash(&self) -> String {
        match self {
            Self::Node(node) => node.content_hash.clone().unwrap_or_else(|| node.checksum()),
            Self::Edge(edge) => edge.content_hash.clone().unwrap_or_else(|| edge.checksum()),
        }
    }
}

enum MergeDecision {
    Resolved {
        selected: Option<GraphMergeSide>,
        record: Option<MergeRecord>,
        reason: String,
    },
    Conflict {
        reason: String,
    },
}

fn merge_records_by_key(snapshot: &GraphSnapshot) -> BTreeMap<String, MergeRecord> {
    let mut records = BTreeMap::new();
    for node in &snapshot.nodes {
        let record = MergeRecord::Node(node.clone());
        records.insert(record.key(), record);
    }
    for edge in &snapshot.edges {
        let record = MergeRecord::Edge(edge.clone());
        records.insert(record.key(), record);
    }
    records
}

fn merge_record_kind(
    base: Option<&MergeRecord>,
    ours: Option<&MergeRecord>,
    theirs: Option<&MergeRecord>,
) -> GraphObjectKind {
    ours.or(theirs)
        .or(base)
        .map(MergeRecord::kind)
        .unwrap_or(GraphObjectKind::Node)
}

fn resolve_merge_record(
    base: Option<&MergeRecord>,
    ours: Option<&MergeRecord>,
    theirs: Option<&MergeRecord>,
    strategy: &GraphMergeStrategy,
    min_confidence_delta: f64,
) -> MergeDecision {
    let base_hash = base.map(MergeRecord::hash);
    let ours_hash = ours.map(MergeRecord::hash);
    let theirs_hash = theirs.map(MergeRecord::hash);

    if ours_hash == theirs_hash {
        return MergeDecision::Resolved {
            selected: ours.map(|_| GraphMergeSide::Ours),
            record: ours.cloned(),
            reason: "same_result".to_string(),
        };
    }
    if base_hash == ours_hash {
        return MergeDecision::Resolved {
            selected: theirs.map(|_| GraphMergeSide::Theirs),
            record: theirs.cloned(),
            reason: "theirs_only_change".to_string(),
        };
    }
    if base_hash == theirs_hash {
        return MergeDecision::Resolved {
            selected: ours.map(|_| GraphMergeSide::Ours),
            record: ours.cloned(),
            reason: "ours_only_change".to_string(),
        };
    }

    match strategy {
        GraphMergeStrategy::PreferOurs => MergeDecision::Resolved {
            selected: ours.map(|_| GraphMergeSide::Ours),
            record: ours.cloned(),
            reason: "strategy_prefer_ours".to_string(),
        },
        GraphMergeStrategy::PreferTheirs => MergeDecision::Resolved {
            selected: theirs.map(|_| GraphMergeSide::Theirs),
            record: theirs.cloned(),
            reason: "strategy_prefer_theirs".to_string(),
        },
        GraphMergeStrategy::AutoConfidence => {
            resolve_confidence_merge(ours, theirs, min_confidence_delta).unwrap_or_else(|| {
                MergeDecision::Conflict {
                    reason: "both_sides_changed".to_string(),
                }
            })
        }
        GraphMergeStrategy::Manual => MergeDecision::Conflict {
            reason: "manual_resolution_required".to_string(),
        },
    }
}

fn resolve_confidence_merge(
    ours: Option<&MergeRecord>,
    theirs: Option<&MergeRecord>,
    min_confidence_delta: f64,
) -> Option<MergeDecision> {
    let (Some(MergeRecord::Edge(ours)), Some(MergeRecord::Edge(theirs))) = (ours, theirs) else {
        return None;
    };
    let delta = (ours.effective_confidence() - theirs.effective_confidence()).abs();
    if delta <= min_confidence_delta.max(0.0) {
        return None;
    }
    if ours.effective_confidence() > theirs.effective_confidence() {
        Some(MergeDecision::Resolved {
            selected: Some(GraphMergeSide::Ours),
            record: Some(MergeRecord::Edge(ours.clone())),
            reason: "higher_edge_confidence".to_string(),
        })
    } else {
        Some(MergeDecision::Resolved {
            selected: Some(GraphMergeSide::Theirs),
            record: Some(MergeRecord::Edge(theirs.clone())),
            reason: "higher_edge_confidence".to_string(),
        })
    }
}

fn snapshot_from_merge_records(version: u64, records: Vec<MergeRecord>) -> GraphSnapshot {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for record in records {
        match record {
            MergeRecord::Node(node) => nodes.push(node),
            MergeRecord::Edge(edge) => edges.push(edge),
        }
    }
    nodes.sort_by(|a, b| a.id.cmp(&b.id));
    edges.sort_by(|a, b| a.id.cmp(&b.id));
    GraphSnapshot {
        version,
        nodes,
        edges,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::graph_store::{EdgeRecord, GraphSnapshot, NodeRecord};

    #[test]
    fn compiler_builds_stable_tree_independent_of_record_order() {
        let ada = NodeRecord::new("node:ada", ["Person"], json!({"name": "Ada"}));
        let bob = NodeRecord::new("node:bob", ["Person"], json!({"name": "Bob"}));
        let edge = EdgeRecord::new("edge:knows", "node:ada", "KNOWS", "node:bob", json!({}));
        let first = GraphSnapshot {
            version: 2,
            nodes: vec![ada.clone(), bob.clone()],
            edges: vec![edge.clone()],
        };
        let second = GraphSnapshot {
            version: 2,
            nodes: vec![bob, ada],
            edges: vec![edge],
        };
        let opts = GraphCompileOptions {
            timestamp_unix_ms: Some(1),
            ..GraphCompileOptions::default()
        };
        let first_pack = compile_graph_pack(&first, opts.clone());
        let second_pack = compile_graph_pack(&second, opts);

        assert_eq!(first_pack.tree.root_hash, second_pack.tree.root_hash);
        assert_eq!(
            first_pack.commit.commit_hash,
            second_pack.commit.commit_hash
        );
        assert_eq!(first_pack.manifest.objects_total, 3);
        assert_eq!(first_pack.capabilities.len(), 2);
    }

    #[test]
    fn diff_reports_added_removed_and_modified_records() {
        let base = GraphSnapshot {
            version: 1,
            nodes: vec![NodeRecord::new(
                "node:ada",
                ["Person"],
                json!({"name": "Ada"}),
            )],
            edges: Vec::new(),
        };
        let target = GraphSnapshot {
            version: 2,
            nodes: vec![
                NodeRecord::new("node:ada", ["Person"], json!({"name": "Ada Lovelace"})),
                NodeRecord::new("node:bob", ["Person"], json!({"name": "Bob"})),
            ],
            edges: Vec::new(),
        };
        let diff = diff_graph_snapshots(&base, &target);

        assert_eq!(diff.added.len(), 1);
        assert_eq!(diff.modified.len(), 1);
        assert_eq!(diff.removed.len(), 0);
        assert_eq!(diff.unchanged, 0);
    }

    #[test]
    fn refs_log_and_checkout_round_trip_snapshot_payloads() {
        let snapshot = GraphSnapshot {
            version: 7,
            nodes: vec![NodeRecord::new(
                "node:ada",
                ["Person"],
                json!({"name": "Ada"}),
            )],
            edges: Vec::new(),
        };
        let pack = compile_graph_pack(
            &snapshot,
            GraphCompileOptions {
                branch: Some("main".to_string()),
                timestamp_unix_ms: Some(1),
                ..GraphCompileOptions::default()
            },
        );
        let update = update_graph_ref(
            GraphVersionRepository::default(),
            pack.clone(),
            Some("main".to_string()),
            Some(2),
        );

        assert_eq!(update.reference.commit_hash, pack.commit.commit_hash);
        let log = graph_version_log(&update.repository, Some("main"));
        assert_eq!(log.commits.len(), 1);

        let checkout = checkout_graph_version(&update.repository, "main").unwrap();
        assert_eq!(checkout.commit.commit_hash, pack.commit.commit_hash);
        assert_eq!(checkout.snapshot.nodes[0].id, "node:ada");
    }

    #[test]
    fn three_way_merge_resolves_non_overlapping_changes() {
        let base = GraphSnapshot {
            version: 1,
            nodes: vec![NodeRecord::new(
                "node:ada",
                ["Person"],
                json!({"name": "Ada"}),
            )],
            edges: Vec::new(),
        };
        let ours = GraphSnapshot {
            version: 2,
            nodes: vec![
                NodeRecord::new("node:ada", ["Person"], json!({"name": "Ada Lovelace"})),
                NodeRecord::new("node:ours", ["Note"], json!({})),
            ],
            edges: Vec::new(),
        };
        let theirs = GraphSnapshot {
            version: 3,
            nodes: vec![
                NodeRecord::new("node:ada", ["Person"], json!({"name": "Ada"})),
                NodeRecord::new("node:theirs", ["Note"], json!({})),
            ],
            edges: Vec::new(),
        };

        let merged = merge_graph_snapshots(&base, &ours, &theirs, GraphMergeOptions::default());

        assert_eq!(merged.status, "clean");
        let snapshot = merged.merged_snapshot.unwrap();
        let ids = snapshot
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["node:ada", "node:ours", "node:theirs"]);
        assert!(merged.conflicts.is_empty());
    }

    #[test]
    fn three_way_merge_confidence_resolves_edge_conflicts() {
        let base_edge =
            EdgeRecord::new("edge:e", "a", "SUPPORTS", "b", json!({})).with_confidence(0.4);
        let ours_edge =
            EdgeRecord::new("edge:e", "a", "SUPPORTS", "b", json!({})).with_confidence(0.9);
        let theirs_edge =
            EdgeRecord::new("edge:e", "a", "SUPPORTS", "b", json!({})).with_confidence(0.6);
        let base = GraphSnapshot {
            version: 1,
            nodes: Vec::new(),
            edges: vec![base_edge],
        };
        let ours = GraphSnapshot {
            version: 2,
            nodes: Vec::new(),
            edges: vec![ours_edge],
        };
        let theirs = GraphSnapshot {
            version: 3,
            nodes: Vec::new(),
            edges: vec![theirs_edge],
        };

        let merged = merge_graph_snapshots(&base, &ours, &theirs, GraphMergeOptions::default());

        assert_eq!(merged.status, "clean");
        let edge = &merged.merged_snapshot.unwrap().edges[0];
        assert_eq!(edge.confidence, Some(0.9));
        assert_eq!(merged.resolved[0].reason, "higher_edge_confidence");
    }

    #[test]
    fn three_way_merge_reports_manual_node_conflict() {
        let base = GraphSnapshot {
            version: 1,
            nodes: vec![NodeRecord::new("node:a", ["Doc"], json!({"title": "base"}))],
            edges: Vec::new(),
        };
        let ours = GraphSnapshot {
            version: 2,
            nodes: vec![NodeRecord::new("node:a", ["Doc"], json!({"title": "ours"}))],
            edges: Vec::new(),
        };
        let theirs = GraphSnapshot {
            version: 3,
            nodes: vec![NodeRecord::new(
                "node:a",
                ["Doc"],
                json!({"title": "theirs"}),
            )],
            edges: Vec::new(),
        };

        let merged = merge_graph_snapshots(&base, &ours, &theirs, GraphMergeOptions::default());

        assert_eq!(merged.status, "conflicted");
        assert!(merged.merged_snapshot.is_none());
        assert_eq!(merged.conflicts.len(), 1);
    }
}
