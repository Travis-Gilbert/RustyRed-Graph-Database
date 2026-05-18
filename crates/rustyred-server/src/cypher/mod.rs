//! Pest-based Cypher parser. Replaces the hand-rolled parser in `query_surface.rs`.
//!
//! Entry point: `parse_cypher_pest`. Returns the same `ParsedCypher` the
//! executor consumed before the migration.

pub mod ast;
pub mod compile;
pub mod parse;
pub mod planner;

#[cfg(test)]
mod deps_smoke {
    #[test]
    fn pest_crate_is_linkable() {
        // Compile-time: if pest is missing from Cargo.toml this fails to build.
        let span = pest::Span::new("MATCH", 0, 5).unwrap();
        assert_eq!(span.as_str(), "MATCH");
    }
}
