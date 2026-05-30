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

fn default_include_payloads() -> bool {
    true
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
}
