use pest::Parser;
use pest_derive::Parser;

#[derive(Parser)]
#[grammar = "cypher/grammar.pest"]
pub struct CypherPestParser;

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
