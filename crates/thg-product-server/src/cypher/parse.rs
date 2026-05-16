use std::collections::BTreeMap;

use pest::iterators::Pair;
use pest::Parser;
use pest_derive::Parser;
use serde_json::Value;

use crate::cypher::ast::{
    CypherPattern, EdgeChain, EdgePattern, EdgeStep, EdgeVarLength, NodePattern, ParsedCypher,
    PropertyFilter, ReturnItem,
};
use crate::query_surface::QuerySurfaceError;

#[derive(Parser)]
#[grammar = "cypher/grammar.pest"]
pub struct CypherPestParser;

const DEFAULT_LIMIT: usize = 100;

pub fn parse_cypher_pest(
    query: &str,
    params: &BTreeMap<String, Value>,
) -> Result<ParsedCypher, QuerySurfaceError> {
    let normalized = normalize_query(query);
    if normalized.is_empty() {
        return Err(QuerySurfaceError::invalid(
            "empty_cypher_query",
            "query is required",
        ));
    }

    let mut pairs = CypherPestParser::parse(Rule::query, &normalized).map_err(|err| {
        QuerySurfaceError::invalid(
            "invalid_cypher_query",
            format!("pest parse error: {err}"),
        )
    })?;
    let query_pair = pairs
        .next()
        .ok_or_else(|| QuerySurfaceError::invalid("invalid_cypher_query", "empty pest output"))?;

    let mut pattern: Option<CypherPattern> = None;
    let mut where_filter: Option<PropertyFilter> = None;
    let mut returns: Vec<ReturnItem> = Vec::new();
    let mut limit: usize = DEFAULT_LIMIT;

    for inner in query_pair.into_inner() {
        match inner.as_rule() {
            Rule::match_clause => {
                pattern = Some(parse_match(inner, params)?);
            }
            Rule::where_clause => {
                where_filter = Some(parse_where(inner, params)?);
            }
            Rule::return_clause => {
                returns = parse_return_items(inner)?;
            }
            Rule::limit_clause => {
                limit = parse_limit_literal(inner)?;
            }
            Rule::EOI => {}
            other => {
                return Err(QuerySurfaceError::invalid(
                    "invalid_cypher_query",
                    format!("unexpected top-level rule: {other:?}"),
                ));
            }
        }
    }

    let pattern = pattern.ok_or_else(|| {
        QuerySurfaceError::invalid("invalid_cypher_query", "missing MATCH clause")
    })?;

    if returns.is_empty() {
        return Err(QuerySurfaceError::invalid(
            "empty_return_clause",
            "RETURN clause is required",
        ));
    }

    // If the MATCH bound a path alias (e.g. `MATCH p = ...`), any RETURN p
    // becomes a ReturnItem::Path rather than ReturnItem::Variable.
    let path_binding = match &pattern {
        CypherPattern::EdgeChain(c) => c.path_binding.clone(),
        CypherPattern::EdgeVarLength(v) => v.path_binding.clone(),
        _ => None,
    };
    if let Some(name) = &path_binding {
        for item in returns.iter_mut() {
            if let ReturnItem::Variable(binding) = item {
                if binding == name {
                    *item = ReturnItem::Path {
                        binding: binding.clone(),
                        expression: binding.clone(),
                    };
                }
            }
        }
    }

    Ok(ParsedCypher {
        normalized,
        pattern,
        where_filter,
        returns,
        limit,
        writes: Vec::new(),
    })
}

fn normalize_query(query: &str) -> String {
    query.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse_match(
    pair: Pair<Rule>,
    params: &BTreeMap<String, Value>,
) -> Result<CypherPattern, QuerySurfaceError> {
    let mut path_binding: Option<String> = None;
    let mut pattern_pair: Option<Pair<Rule>> = None;
    for child in pair.into_inner() {
        match child.as_rule() {
            Rule::path_binding => {
                if let Some(ident) = child.into_inner().next() {
                    path_binding = Some(ident.as_str().to_string());
                }
            }
            Rule::pattern => {
                pattern_pair = Some(child);
            }
            _ => {}
        }
    }
    let pattern_pair = pattern_pair.ok_or_else(|| {
        QuerySurfaceError::invalid("invalid_cypher_query", "MATCH pattern missing")
    })?;

    let inner_pair = pattern_pair.into_inner().next().ok_or_else(|| {
        QuerySurfaceError::invalid("invalid_cypher_query", "empty MATCH pattern")
    })?;
    match inner_pair.as_rule() {
        Rule::node_pattern => {
            let node = parse_node_pattern(inner_pair, params)?;
            Ok(CypherPattern::Node(node))
        }
        Rule::edge_chain_pattern => parse_edge_chain_pattern(inner_pair, params, path_binding),
        Rule::var_length_pattern => parse_var_length_pattern(inner_pair, params, path_binding),
        other => Err(QuerySurfaceError::invalid(
            "invalid_cypher_query",
            format!("unexpected pattern rule: {other:?}"),
        )),
    }
}

fn parse_edge_chain_pattern(
    pair: Pair<Rule>,
    params: &BTreeMap<String, Value>,
    path_binding: Option<String>,
) -> Result<CypherPattern, QuerySurfaceError> {
    let mut iter = pair.into_inner();
    let start_pair = iter.next().ok_or_else(|| {
        QuerySurfaceError::invalid("invalid_cypher_query", "missing chain start")
    })?;
    let start = parse_node_pattern(start_pair, params)?;
    let mut steps: Vec<EdgeStep> = Vec::new();
    for cont in iter {
        if !matches!(cont.as_rule(), Rule::edge_continuation) {
            continue;
        }
        let mut sub = cont.into_inner();
        let rel_pair = sub.next().ok_or_else(|| {
            QuerySurfaceError::invalid("invalid_cypher_query", "missing rel type in chain")
        })?;
        let edge_type = parse_rel_type(rel_pair)?;
        let target_pair = sub.next().ok_or_else(|| {
            QuerySurfaceError::invalid("invalid_cypher_query", "missing target in chain")
        })?;
        let target = parse_node_pattern(target_pair, params)?;
        steps.push(EdgeStep { edge_type, target });
    }
    if steps.len() < 2 {
        // Single step: fall back to the existing Edge variant for executor compat.
        let step = steps
            .into_iter()
            .next()
            .expect("step count >= 1 enforced by grammar");
        return Ok(CypherPattern::Edge(EdgePattern {
            left: start,
            edge_type: step.edge_type,
            right: step.target,
        }));
    }
    Ok(CypherPattern::EdgeChain(EdgeChain {
        start,
        steps,
        path_binding,
    }))
}

fn parse_var_length_pattern(
    pair: Pair<Rule>,
    params: &BTreeMap<String, Value>,
    path_binding: Option<String>,
) -> Result<CypherPattern, QuerySurfaceError> {
    let mut iter = pair.into_inner();
    let from_pair = iter.next().ok_or_else(|| {
        QuerySurfaceError::invalid("invalid_cypher_query", "missing var-length source")
    })?;
    let from = parse_node_pattern(from_pair, params)?;
    let edge_pair = iter.next().ok_or_else(|| {
        QuerySurfaceError::invalid("invalid_cypher_query", "missing var-length edge")
    })?;
    let mut edge_inner = edge_pair.into_inner();
    let rel_pair = edge_inner.next().ok_or_else(|| {
        QuerySurfaceError::invalid("invalid_cypher_query", "missing var-length rel type")
    })?;
    let edge_type = parse_rel_type(rel_pair)?;
    let (min, max) = if let Some(range_pair) = edge_inner.next() {
        if !matches!(range_pair.as_rule(), Rule::var_length_range) {
            return Err(QuerySurfaceError::invalid(
                "invalid_cypher_query",
                format!("unexpected var-length child: {:?}", range_pair.as_rule()),
            ));
        }
        let mut numbers = range_pair.into_inner();
        let first = numbers
            .next()
            .ok_or_else(|| {
                QuerySurfaceError::invalid(
                    "invalid_cypher_query",
                    "var-length range missing minimum",
                )
            })?
            .as_str()
            .parse::<usize>()
            .map_err(|err| {
                QuerySurfaceError::invalid(
                    "invalid_cypher_query",
                    format!("invalid var-length min: {err}"),
                )
            })?;
        let second = match numbers.next() {
            Some(p) => Some(p.as_str().parse::<usize>().map_err(|err| {
                QuerySurfaceError::invalid(
                    "invalid_cypher_query",
                    format!("invalid var-length max: {err}"),
                )
            })?),
            None => None,
        };
        match second {
            Some(max) => (first, Some(max)),
            None => (first, Some(first)),
        }
    } else {
        (1, None)
    };
    let to_pair = iter.next().ok_or_else(|| {
        QuerySurfaceError::invalid("invalid_cypher_query", "missing var-length target")
    })?;
    let to = parse_node_pattern(to_pair, params)?;
    Ok(CypherPattern::EdgeVarLength(EdgeVarLength {
        from,
        edge_type,
        min,
        max,
        to,
        path_binding,
    }))
}

fn parse_rel_type(pair: Pair<Rule>) -> Result<String, QuerySurfaceError> {
    for inner in pair.into_inner() {
        if matches!(inner.as_rule(), Rule::ident) {
            return Ok(inner.as_str().to_string());
        }
    }
    Err(QuerySurfaceError::invalid(
        "invalid_cypher_query",
        "relationship type missing identifier",
    ))
}

fn parse_node_pattern(
    pair: Pair<Rule>,
    params: &BTreeMap<String, Value>,
) -> Result<NodePattern, QuerySurfaceError> {
    let mut binding: Option<String> = None;
    let mut label: Option<String> = None;
    let mut properties: BTreeMap<String, Value> = BTreeMap::new();
    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::binding => {
                binding = Some(inner.as_str().to_string());
            }
            Rule::label => {
                for child in inner.into_inner() {
                    if matches!(child.as_rule(), Rule::ident) {
                        label = Some(child.as_str().to_string());
                    }
                }
            }
            Rule::property_block => {
                properties = parse_property_block(inner, params)?;
            }
            _ => {}
        }
    }
    let binding = binding.unwrap_or_else(|| "_anon".to_string());
    Ok(NodePattern {
        binding,
        label,
        properties,
    })
}

fn parse_property_block(
    pair: Pair<Rule>,
    params: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, QuerySurfaceError> {
    let mut out = BTreeMap::new();
    for entry in pair.into_inner() {
        if !matches!(entry.as_rule(), Rule::property_pair) {
            continue;
        }
        let mut name: Option<String> = None;
        let mut value: Option<Value> = None;
        for child in entry.into_inner() {
            match child.as_rule() {
                Rule::ident if name.is_none() => name = Some(child.as_str().to_string()),
                Rule::value => value = Some(parse_value(child, params)?),
                _ => {}
            }
        }
        if let (Some(name), Some(value)) = (name, value) {
            out.insert(name, value);
        }
    }
    Ok(out)
}

fn parse_value(
    pair: Pair<Rule>,
    params: &BTreeMap<String, Value>,
) -> Result<Value, QuerySurfaceError> {
    for inner in pair.into_inner() {
        return match inner.as_rule() {
            Rule::param => {
                let name = inner.as_str().trim_start_matches('$').to_string();
                params
                    .get(&name)
                    .cloned()
                    .ok_or_else(|| QuerySurfaceError::missing_param(&name))
            }
            Rule::string => {
                let raw = inner.as_str();
                let stripped = &raw[1..raw.len() - 1];
                Ok(Value::String(stripped.to_string()))
            }
            Rule::number => {
                let text = inner.as_str();
                if let Ok(int) = text.parse::<i64>() {
                    Ok(Value::Number(int.into()))
                } else if let Ok(float) = text.parse::<f64>() {
                    serde_json::Number::from_f64(float)
                        .map(Value::Number)
                        .ok_or_else(|| {
                            QuerySurfaceError::invalid(
                                "invalid_cypher_value",
                                format!("non-finite number literal: {text}"),
                            )
                        })
                } else {
                    Err(QuerySurfaceError::invalid(
                        "invalid_cypher_value",
                        format!("unparseable number: {text}"),
                    ))
                }
            }
            Rule::boolean => Ok(Value::Bool(inner.as_str().eq_ignore_ascii_case("true"))),
            Rule::null => Ok(Value::Null),
            other => Err(QuerySurfaceError::invalid(
                "invalid_cypher_value",
                format!("unsupported value rule: {other:?}"),
            )),
        };
    }
    Err(QuerySurfaceError::invalid(
        "invalid_cypher_value",
        "empty value",
    ))
}

fn parse_where(
    pair: Pair<Rule>,
    params: &BTreeMap<String, Value>,
) -> Result<PropertyFilter, QuerySurfaceError> {
    for inner in pair.into_inner() {
        if matches!(inner.as_rule(), Rule::where_expr) {
            let mut path: Option<(String, String)> = None;
            let mut value: Option<Value> = None;
            for child in inner.into_inner() {
                match child.as_rule() {
                    Rule::property_path => {
                        let mut idents = child.into_inner();
                        let binding = idents
                            .next()
                            .ok_or_else(|| {
                                QuerySurfaceError::invalid(
                                    "invalid_where_filter",
                                    "missing property binding",
                                )
                            })?
                            .as_str()
                            .to_string();
                        let key = idents
                            .next()
                            .ok_or_else(|| {
                                QuerySurfaceError::invalid(
                                    "invalid_where_filter",
                                    "missing property key",
                                )
                            })?
                            .as_str()
                            .to_string();
                        path = Some((binding, key));
                    }
                    Rule::value => {
                        value = Some(parse_value(child, params)?);
                    }
                    _ => {}
                }
            }
            let (binding, key) = path.ok_or_else(|| {
                QuerySurfaceError::invalid("invalid_where_filter", "WHERE path missing")
            })?;
            let value = value.ok_or_else(|| {
                QuerySurfaceError::invalid("invalid_where_filter", "WHERE value missing")
            })?;
            return Ok(PropertyFilter {
                binding,
                key,
                value,
            });
        }
    }
    Err(QuerySurfaceError::invalid(
        "invalid_where_filter",
        "WHERE expression missing",
    ))
}

fn parse_return_items(pair: Pair<Rule>) -> Result<Vec<ReturnItem>, QuerySurfaceError> {
    let mut items = Vec::new();
    for inner in pair.into_inner() {
        if !matches!(inner.as_rule(), Rule::return_items) {
            continue;
        }
        for item_pair in inner.into_inner() {
            if !matches!(item_pair.as_rule(), Rule::return_item) {
                continue;
            }
            let raw = item_pair.as_str().to_string();
            let mut sub_iter = item_pair.into_inner();
            let inner_pair = sub_iter.next().ok_or_else(|| {
                QuerySurfaceError::invalid(
                    "invalid_return_clause",
                    "empty return item",
                )
            })?;
            match inner_pair.as_rule() {
                Rule::count_call => {
                    let arg = inner_pair
                        .into_inner()
                        .next()
                        .ok_or_else(|| {
                            QuerySurfaceError::invalid(
                                "invalid_count",
                                "COUNT missing argument",
                            )
                        })?;
                    let text = arg.as_str().trim();
                    if text == "*" {
                        items.push(ReturnItem::Count {
                            binding: None,
                            expression: raw,
                        });
                    } else {
                        items.push(ReturnItem::Count {
                            binding: Some(text.to_string()),
                            expression: raw,
                        });
                    }
                }
                Rule::property_path => {
                    let mut idents = inner_pair.into_inner();
                    let binding = idents
                        .next()
                        .ok_or_else(|| {
                            QuerySurfaceError::invalid(
                                "invalid_return_clause",
                                "property path missing binding",
                            )
                        })?
                        .as_str()
                        .to_string();
                    let key = idents
                        .next()
                        .ok_or_else(|| {
                            QuerySurfaceError::invalid(
                                "invalid_return_clause",
                                "property path missing key",
                            )
                        })?
                        .as_str()
                        .to_string();
                    items.push(ReturnItem::Property {
                        binding,
                        key,
                        expression: raw,
                    });
                }
                Rule::ident => {
                    items.push(ReturnItem::Variable(inner_pair.as_str().to_string()));
                }
                other => {
                    return Err(QuerySurfaceError::invalid(
                        "invalid_return_clause",
                        format!("unsupported return item: {other:?}"),
                    ));
                }
            }
        }
    }
    Ok(items)
}

fn parse_limit_literal(pair: Pair<Rule>) -> Result<usize, QuerySurfaceError> {
    for inner in pair.into_inner() {
        if matches!(inner.as_rule(), Rule::integer) {
            return inner.as_str().parse::<usize>().map_err(|err| {
                QuerySurfaceError::invalid(
                    "invalid_limit_literal",
                    format!("limit must be a non-negative integer: {err}"),
                )
            });
        }
    }
    Err(QuerySurfaceError::invalid(
        "invalid_limit_literal",
        "LIMIT requires an integer literal",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grammar_parses_simple_node_match() {
        let pairs =
            CypherPestParser::parse(Rule::query, "MATCH (n:Doc) RETURN n LIMIT 10");
        assert!(pairs.is_ok(), "expected MATCH (n:Doc) RETURN n LIMIT 10 to parse: {:?}", pairs);
    }

    #[test]
    fn grammar_parses_where_filter() {
        let pairs = CypherPestParser::parse(
            Rule::query,
            "MATCH (n:Doc) WHERE n.path = $value RETURN n",
        );
        assert!(pairs.is_ok(), "expected WHERE filter to parse: {:?}", pairs);
    }

    #[test]
    fn grammar_parses_count_star() {
        let pairs =
            CypherPestParser::parse(Rule::query, "MATCH (n:Doc) RETURN count(n)");
        assert!(pairs.is_ok(), "expected COUNT to parse: {:?}", pairs);
    }

    #[test]
    fn grammar_parses_single_hop_edge() {
        let pairs = CypherPestParser::parse(
            Rule::query,
            "MATCH (a:Doc)-[:CITES]->(b:Doc) RETURN a, b LIMIT 5",
        );
        assert!(pairs.is_ok(), "expected single-hop edge to parse: {:?}", pairs);
    }
}

#[cfg(test)]
mod parse_to_ast_tests {
    use super::*;
    use crate::cypher::ast::{CypherPattern, ReturnItem};
    use std::collections::BTreeMap;

    #[test]
    fn parse_simple_node_match() {
        let parsed =
            parse_cypher_pest("MATCH (n:Doc) RETURN n LIMIT 10", &BTreeMap::new()).unwrap();
        assert_eq!(parsed.limit, 10);
        let CypherPattern::Node(node) = &parsed.pattern else {
            panic!("expected node pattern");
        };
        assert_eq!(node.binding, "n");
        assert_eq!(node.label.as_deref(), Some("Doc"));
        assert_eq!(parsed.returns.len(), 1);
        assert!(matches!(parsed.returns[0], ReturnItem::Variable(ref b) if b == "n"));
    }

    #[test]
    fn parse_default_limit_when_omitted() {
        let parsed =
            parse_cypher_pest("MATCH (n:Doc) RETURN n", &BTreeMap::new()).unwrap();
        assert_eq!(parsed.limit, 100);
    }

    #[test]
    fn parse_where_property_eq_param() {
        let mut params = BTreeMap::new();
        params.insert("value".to_string(), serde_json::json!("src/lib.rs"));
        let parsed = parse_cypher_pest(
            "MATCH (n:File) WHERE n.path = $value RETURN n",
            &params,
        )
        .unwrap();
        let filter = parsed.where_filter.expect("expected WHERE filter");
        assert_eq!(filter.binding, "n");
        assert_eq!(filter.key, "path");
        assert_eq!(filter.value, serde_json::json!("src/lib.rs"));
    }

    #[test]
    fn parse_count_star_into_count_item() {
        let parsed =
            parse_cypher_pest("MATCH (n:Doc) RETURN count(n)", &BTreeMap::new()).unwrap();
        assert_eq!(parsed.returns.len(), 1);
        let ReturnItem::Count { binding, expression } = &parsed.returns[0] else {
            panic!("expected count return item, got {:?}", parsed.returns[0]);
        };
        assert_eq!(binding.as_deref(), Some("n"));
        assert!(expression.contains("count"));
    }

    #[test]
    fn parse_single_hop_edge_pattern() {
        let parsed = parse_cypher_pest(
            "MATCH (a:Doc)-[:CITES]->(b:Doc) RETURN a, b LIMIT 5",
            &BTreeMap::new(),
        )
        .unwrap();
        let CypherPattern::Edge(edge) = &parsed.pattern else {
            panic!("expected edge pattern");
        };
        assert_eq!(edge.left.binding, "a");
        assert_eq!(edge.edge_type, "CITES");
        assert_eq!(edge.right.binding, "b");
    }

    #[test]
    fn parse_missing_param_errors() {
        let err = parse_cypher_pest(
            "MATCH (n:Doc) WHERE n.path = $value RETURN n",
            &BTreeMap::new(),
        )
        .unwrap_err();
        assert!(format!("{:?}", err).contains("missing_cypher_param"));
    }

    #[test]
    fn parse_empty_query_errors() {
        let err = parse_cypher_pest("   ", &BTreeMap::new()).unwrap_err();
        assert!(format!("{:?}", err).contains("empty_cypher_query"));
    }

    #[test]
    fn parse_multi_hop_into_edge_chain() {
        let parsed = parse_cypher_pest(
            "MATCH (a:Doc)-[:T1]->(b:Doc)-[:T2]->(c:Doc) RETURN c",
            &BTreeMap::new(),
        )
        .unwrap();
        let CypherPattern::EdgeChain(chain) = &parsed.pattern else {
            panic!("expected EdgeChain pattern, got {:?}", parsed.pattern);
        };
        assert_eq!(chain.start.binding, "a");
        assert_eq!(chain.steps.len(), 2);
        assert_eq!(chain.steps[0].edge_type, "T1");
        assert_eq!(chain.steps[1].target.binding, "c");
    }

    #[test]
    fn parse_bounded_var_length_into_edge_var_length() {
        let parsed = parse_cypher_pest(
            "MATCH (a:Doc)-[:T*1..3]->(b:Doc) RETURN b LIMIT 5",
            &BTreeMap::new(),
        )
        .unwrap();
        let CypherPattern::EdgeVarLength(var) = &parsed.pattern else {
            panic!("expected EdgeVarLength pattern");
        };
        assert_eq!(var.min, 1);
        assert_eq!(var.max, Some(3));
        assert_eq!(var.edge_type, "T");
        assert_eq!(parsed.limit, 5);
    }

    #[test]
    fn parse_unbounded_var_length_returns_max_none() {
        let parsed = parse_cypher_pest(
            "MATCH (a:Doc)-[:T*]->(b:Doc) RETURN b",
            &BTreeMap::new(),
        )
        .unwrap();
        let CypherPattern::EdgeVarLength(var) = &parsed.pattern else {
            panic!("expected EdgeVarLength pattern");
        };
        assert_eq!(var.min, 1);
        assert_eq!(var.max, None);
    }

    #[test]
    fn parse_path_binding_stores_alias() {
        let parsed = parse_cypher_pest(
            "MATCH p = (a:Doc)-[:T*]->(b:Doc) RETURN p",
            &BTreeMap::new(),
        )
        .unwrap();
        let CypherPattern::EdgeVarLength(var) = &parsed.pattern else {
            panic!("expected EdgeVarLength pattern");
        };
        assert_eq!(var.path_binding.as_deref(), Some("p"));
        assert!(matches!(
            parsed.returns[0],
            ReturnItem::Path { ref binding, .. } if binding == "p"
        ));
    }
}
