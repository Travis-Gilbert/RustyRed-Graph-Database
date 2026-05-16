use std::collections::BTreeMap;

use serde_json::Value;

// Some AST fields are intentionally carried ahead of executor support so the
// parser can normalize future clauses without reparsing later.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct ParsedCypher {
    pub normalized: String,
    pub pattern: CypherPattern,
    pub where_filter: Option<PropertyFilter>,
    pub returns: Vec<ReturnItem>,
    pub limit: usize,
    /// Optional write clauses (§P3-A). Empty for read-only queries.
    pub writes: Vec<WriteClause>,
    /// Optional WITH pipeline clause (§P2-C).
    pub with_clause: Option<WithClause>,
    /// Optional ORDER BY clauses (§P2-C). Empty when not present.
    pub order_by: Vec<OrderBy>,
    /// Optional SKIP for paginated reads (§P2-C).
    pub skip: Option<usize>,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AggOp {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub enum WithItem {
    Field {
        binding: String,
        /// None means the whole node value (binding without `.property`).
        key: Option<String>,
        alias: String,
    },
    Aggregate {
        op: AggOp,
        binding: Option<String>,
        key: Option<String>,
        alias: String,
    },
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct WithClause {
    pub items: Vec<WithItem>,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct OrderBy {
    pub expression: String,
    pub descending: bool,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub enum SetExpr {
    Literal(Value),
    /// `n.prop + delta` style expression, kept symbolic so the executor can
    /// evaluate against the current node value.
    Increment {
        base_binding: String,
        base_key: String,
        delta: Value,
    },
}

#[derive(Clone, Debug, Default)]
pub struct MergeBranch {
    pub sets: Vec<(String, String, SetExpr)>,
}

#[derive(Clone, Debug)]
pub enum WriteClause {
    CreateNode {
        node: NodePattern,
    },
    CreateEdge {
        edge: EdgePattern,
    },
    Merge {
        node: NodePattern,
        on_create: Option<MergeBranch>,
        on_match: Option<MergeBranch>,
    },
    Set {
        binding: String,
        key: String,
        value: SetExpr,
    },
    Delete {
        binding: String,
        detach: bool,
    },
}

#[derive(Clone, Debug)]
pub enum CypherPattern {
    Node(NodePattern),
    Edge(EdgePattern),
    EdgeChain(EdgeChain),
    EdgeVarLength(EdgeVarLength),
}

#[derive(Clone, Debug)]
pub struct NodePattern {
    pub binding: String,
    pub label: Option<String>,
    pub properties: BTreeMap<String, Value>,
}

#[derive(Clone, Debug)]
pub struct EdgePattern {
    pub left: NodePattern,
    pub edge_type: String,
    pub right: NodePattern,
}

#[derive(Clone, Debug)]
pub struct EdgeStep {
    pub edge_type: String,
    pub target: NodePattern,
}

#[derive(Clone, Debug)]
pub struct EdgeChain {
    pub start: NodePattern,
    pub steps: Vec<EdgeStep>,
    pub path_binding: Option<String>,
}

#[derive(Clone, Debug)]
pub struct EdgeVarLength {
    pub from: NodePattern,
    pub edge_type: String,
    pub min: usize,
    pub max: Option<usize>,
    pub to: NodePattern,
    pub path_binding: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PropertyFilter {
    pub binding: String,
    pub key: String,
    pub value: Value,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub enum ReturnItem {
    Variable(String),
    Property {
        binding: String,
        key: String,
        expression: String,
    },
    Count {
        binding: Option<String>,
        expression: String,
    },
    Path {
        binding: String,
        expression: String,
    },
    Aggregate {
        op: AggOp,
        binding: Option<String>,
        key: Option<String>,
        expression: String,
    },
}

impl ReturnItem {
    pub fn key(&self) -> &str {
        match self {
            Self::Variable(binding) => binding.as_str(),
            Self::Property { expression, .. } => expression.as_str(),
            Self::Count { expression, .. } => expression.as_str(),
            Self::Path { expression, .. } => expression.as_str(),
            Self::Aggregate { expression, .. } => expression.as_str(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn ast_node_pattern_round_trips_label_and_props() {
        let mut props = BTreeMap::new();
        props.insert("path".to_string(), serde_json::json!("src/lib.rs"));
        let node = NodePattern {
            binding: "n".to_string(),
            label: Some("File".to_string()),
            properties: props,
        };
        assert_eq!(node.binding, "n");
        assert_eq!(node.label.as_deref(), Some("File"));
        assert_eq!(node.properties.len(), 1);
    }

    #[test]
    fn ast_parsed_cypher_holds_normalized_query() {
        let parsed = ParsedCypher {
            normalized: "MATCH (n:File) RETURN n LIMIT 10".to_string(),
            pattern: CypherPattern::Node(NodePattern {
                binding: "n".to_string(),
                label: Some("File".to_string()),
                properties: BTreeMap::new(),
            }),
            where_filter: None,
            returns: vec![ReturnItem::Variable("n".to_string())],
            limit: 10,
            writes: Vec::new(),
            with_clause: None,
            order_by: Vec::new(),
            skip: None,
        };
        assert_eq!(parsed.limit, 10);
    }

    #[test]
    fn ast_aggregate_return_item_round_trip() {
        let item = ReturnItem::Aggregate {
            op: AggOp::Sum,
            binding: Some("n".into()),
            key: Some("score".into()),
            expression: "sum(n.score)".into(),
        };
        assert_eq!(item.key(), "sum(n.score)");
    }

    #[test]
    fn write_clause_variants_construct() {
        use serde_json::json;
        let create = WriteClause::CreateNode {
            node: NodePattern {
                binding: "n".into(),
                label: Some("Doc".into()),
                properties: BTreeMap::new(),
            },
        };
        let set = WriteClause::Set {
            binding: "n".into(),
            key: "seen".into(),
            value: SetExpr::Literal(json!(5)),
        };
        let delete = WriteClause::Delete {
            binding: "n".into(),
            detach: true,
        };
        assert!(matches!(create, WriteClause::CreateNode { .. }));
        assert!(matches!(set, WriteClause::Set { .. }));
        let WriteClause::Delete { detach, .. } = delete else {
            panic!("expected delete")
        };
        assert!(detach);
    }

    #[test]
    fn ast_supports_edge_chain_pattern() {
        let chain = EdgeChain {
            start: NodePattern {
                binding: "a".into(),
                label: Some("Doc".into()),
                properties: BTreeMap::new(),
            },
            steps: vec![
                EdgeStep {
                    edge_type: "T1".into(),
                    target: NodePattern {
                        binding: "b".into(),
                        label: None,
                        properties: BTreeMap::new(),
                    },
                },
                EdgeStep {
                    edge_type: "T2".into(),
                    target: NodePattern {
                        binding: "c".into(),
                        label: None,
                        properties: BTreeMap::new(),
                    },
                },
            ],
            path_binding: None,
        };
        let pattern = CypherPattern::EdgeChain(chain);
        let CypherPattern::EdgeChain(c) = &pattern else {
            panic!("expected EdgeChain");
        };
        assert_eq!(c.steps.len(), 2);
        assert_eq!(c.steps[1].target.binding, "c");
    }

    #[test]
    fn ast_supports_edge_var_length_pattern() {
        let var = EdgeVarLength {
            from: NodePattern {
                binding: "a".into(),
                label: None,
                properties: BTreeMap::new(),
            },
            edge_type: "T".into(),
            min: 1,
            max: Some(3),
            to: NodePattern {
                binding: "b".into(),
                label: None,
                properties: BTreeMap::new(),
            },
            path_binding: None,
        };
        let pattern = CypherPattern::EdgeVarLength(var);
        let CypherPattern::EdgeVarLength(v) = &pattern else {
            panic!("expected EdgeVarLength");
        };
        assert_eq!(v.min, 1);
        assert_eq!(v.max, Some(3));
    }

    #[test]
    fn ast_return_item_path_variant() {
        let item = ReturnItem::Path {
            binding: "p".into(),
            expression: "p".into(),
        };
        assert_eq!(item.key(), "p");
    }
}
