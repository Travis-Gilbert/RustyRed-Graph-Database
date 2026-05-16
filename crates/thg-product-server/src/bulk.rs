use serde_json::{json, Value};
use thg_core::{EdgeRecord, NodeRecord};

/// Buffers byte chunks across an HTTP streaming body and emits complete
/// newline-delimited lines as they become available.
#[derive(Default)]
pub struct LineSplitter {
    buffer: String,
}

impl LineSplitter {
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<String> {
        let text = match std::str::from_utf8(chunk) {
            Ok(text) => text,
            Err(_) => return Vec::new(),
        };
        self.buffer.push_str(text);
        let mut lines = Vec::new();
        while let Some(idx) = self.buffer.find('\n') {
            let line = self.buffer[..idx].trim_end_matches('\r').to_string();
            self.buffer.drain(..=idx);
            if !line.trim().is_empty() {
                lines.push(line);
            }
        }
        lines
    }

    pub fn flush(&mut self) -> Vec<String> {
        let trailing = self.buffer.trim().to_string();
        self.buffer.clear();
        if trailing.is_empty() {
            Vec::new()
        } else {
            vec![trailing]
        }
    }
}

pub fn jsonl_parse_node(line: &str) -> Result<NodeRecord, String> {
    let raw: Value = serde_json::from_str(line).map_err(|err| err.to_string())?;
    let id = raw
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| "node record missing string id".to_string())?
        .to_string();
    let labels = raw
        .get("labels")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let properties = raw
        .get("properties")
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    if !properties.is_object() {
        return Err("node properties must be a JSON object".to_string());
    }
    Ok(NodeRecord::new(id, labels, properties))
}

pub fn jsonl_parse_edge(line: &str) -> Result<EdgeRecord, String> {
    let raw: Value = serde_json::from_str(line).map_err(|err| err.to_string())?;
    let id = raw
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| "edge record missing string id".to_string())?
        .to_string();
    let from_id = raw
        .get("from_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "edge record missing string from_id".to_string())?
        .to_string();
    let to_id = raw
        .get("to_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "edge record missing string to_id".to_string())?
        .to_string();
    let edge_type = raw
        .get("type")
        .or_else(|| raw.get("edge_type"))
        .and_then(Value::as_str)
        .ok_or_else(|| "edge record missing string type".to_string())?
        .to_string();
    let properties = raw
        .get("properties")
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    if !properties.is_object() {
        return Err("edge properties must be a JSON object".to_string());
    }
    Ok(EdgeRecord::new(id, from_id, edge_type, to_id, properties))
}

/// Hand-rolled CSV parser for unquoted, comma-separated rows. The SPEC's
/// acceptance is 1000-row JSONL; CSV is a convenience for simple flat data.
/// If quoted fields are needed later, switch to the `csv` crate behind a
/// dependency cut.
pub struct CsvNodeParser {
    headers: Vec<String>,
    id_idx: usize,
    label_idx: Option<usize>,
}

impl CsvNodeParser {
    pub fn new(headers: Vec<String>) -> Self {
        let id_idx = headers
            .iter()
            .position(|h| h.eq_ignore_ascii_case("id"))
            .unwrap_or(0);
        let label_idx = headers.iter().position(|h| h.eq_ignore_ascii_case("label"));
        Self {
            headers,
            id_idx,
            label_idx,
        }
    }

    pub fn parse(&self, line: &str) -> Result<NodeRecord, String> {
        let fields: Vec<&str> = line.split(',').collect();
        if fields.len() != self.headers.len() {
            return Err(format!(
                "csv row width {} does not match header width {}",
                fields.len(),
                self.headers.len()
            ));
        }
        let id = fields[self.id_idx].trim().to_string();
        if id.is_empty() {
            return Err("csv row missing id".into());
        }
        let labels = match self.label_idx {
            Some(idx) if !fields[idx].trim().is_empty() => {
                vec![fields[idx].trim().to_string()]
            }
            _ => Vec::new(),
        };
        let mut props_obj = serde_json::Map::new();
        for (i, name) in self.headers.iter().enumerate() {
            if i == self.id_idx {
                continue;
            }
            if Some(i) == self.label_idx {
                continue;
            }
            let raw = fields[i].trim();
            props_obj.insert(name.clone(), json!(raw));
        }
        let mut node = NodeRecord::new(id, labels, Value::Object(props_obj));
        node.tombstone = false;
        Ok(node)
    }
}

pub struct CsvEdgeParser {
    headers: Vec<String>,
    id_idx: usize,
    from_idx: usize,
    to_idx: usize,
    type_idx: usize,
}

impl CsvEdgeParser {
    pub fn new(headers: Vec<String>, from_col: &str, to_col: &str) -> Result<Self, String> {
        let id_idx = headers
            .iter()
            .position(|h| h.eq_ignore_ascii_case("id"))
            .unwrap_or(0);
        let from_idx = headers
            .iter()
            .position(|h| h.eq_ignore_ascii_case(from_col))
            .ok_or_else(|| format!("missing source column: {from_col}"))?;
        let to_idx = headers
            .iter()
            .position(|h| h.eq_ignore_ascii_case(to_col))
            .ok_or_else(|| format!("missing target column: {to_col}"))?;
        let type_idx = headers
            .iter()
            .position(|h| {
                h.eq_ignore_ascii_case("edge_type")
                    || h.eq_ignore_ascii_case("type")
                    || h.eq_ignore_ascii_case("rel")
            })
            .ok_or_else(|| "missing edge_type column".to_string())?;
        Ok(Self {
            headers,
            id_idx,
            from_idx,
            to_idx,
            type_idx,
        })
    }

    pub fn parse(&self, line: &str) -> Result<EdgeRecord, String> {
        let fields: Vec<&str> = line.split(',').collect();
        if fields.len() != self.headers.len() {
            return Err(format!(
                "csv row width {} does not match header width {}",
                fields.len(),
                self.headers.len()
            ));
        }
        let id = fields[self.id_idx].trim().to_string();
        let from_id = fields[self.from_idx].trim().to_string();
        let to_id = fields[self.to_idx].trim().to_string();
        let edge_type = fields[self.type_idx].trim().to_string();
        if id.is_empty() || from_id.is_empty() || to_id.is_empty() || edge_type.is_empty() {
            return Err("csv edge row missing required columns".into());
        }
        let mut props_obj = serde_json::Map::new();
        for (i, name) in self.headers.iter().enumerate() {
            if i == self.id_idx || i == self.from_idx || i == self.to_idx || i == self.type_idx {
                continue;
            }
            props_obj.insert(name.clone(), json!(fields[i].trim()));
        }
        Ok(EdgeRecord::new(
            id,
            from_id,
            edge_type,
            to_id,
            Value::Object(props_obj),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_splitter_handles_partial_chunks() {
        let mut splitter = LineSplitter::default();
        let lines_one = splitter.feed(b"{\"id\":\"a\"}\n{\"id\":\"b");
        assert_eq!(lines_one, vec!["{\"id\":\"a\"}".to_string()]);
        let lines_two = splitter.feed(b"\"}\n{\"id\":\"c\"}\n");
        assert_eq!(
            lines_two,
            vec!["{\"id\":\"b\"}".to_string(), "{\"id\":\"c\"}".to_string()]
        );
        let trailing = splitter.flush();
        assert!(trailing.is_empty());
    }

    #[test]
    fn line_splitter_emits_trailing_when_no_newline() {
        let mut splitter = LineSplitter::default();
        splitter.feed(b"{\"id\":\"x\"}");
        let trailing = splitter.flush();
        assert_eq!(trailing, vec!["{\"id\":\"x\"}".to_string()]);
    }

    #[test]
    fn jsonl_parse_node_returns_record() {
        let record =
            jsonl_parse_node("{\"id\":\"n1\",\"labels\":[\"Doc\"],\"properties\":{}}").unwrap();
        assert_eq!(record.id, "n1");
        assert_eq!(record.labels, vec!["Doc".to_string()]);
    }

    #[test]
    fn csv_parse_node_uses_header_columns() {
        let parser = CsvNodeParser::new(vec!["id".into(), "label".into(), "path".into()]);
        let record = parser.parse("n1,Doc,src/lib.rs").unwrap();
        assert_eq!(record.id, "n1");
        assert_eq!(record.labels, vec!["Doc".to_string()]);
        assert_eq!(record.properties, serde_json::json!({"path": "src/lib.rs"}));
    }
}
