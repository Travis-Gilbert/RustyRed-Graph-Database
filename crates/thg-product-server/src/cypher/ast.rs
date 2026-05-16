use std::collections::BTreeMap;

use serde_json::Value;

#[derive(Clone, Debug)]
pub struct ParsedCypher {
    pub normalized: String,
    pub pattern: CypherPattern,
    pub where_filter: Option<PropertyFilter>,
    pub returns: Vec<ReturnItem>,
    pub limit: usize,
}

#[derive(Clone, Debug)]
pub enum CypherPattern {
    Node(NodePattern),
    Edge(EdgePattern),
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
pub struct PropertyFilter {
    pub binding: String,
    pub key: String,
    pub value: Value,
}

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
}

impl ReturnItem {
    pub fn key(&self) -> &str {
        match self {
            Self::Variable(binding) => binding.as_str(),
            Self::Property { expression, .. } => expression.as_str(),
            Self::Count { expression, .. } => expression.as_str(),
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
        };
        assert_eq!(parsed.limit, 10);
    }
}
