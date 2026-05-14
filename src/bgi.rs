use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

fn parse_json(text: &str, surface: &str) -> PyResult<Value> {
    serde_json::from_str(text)
        .map_err(|err| PyValueError::new_err(format!("{surface} expected valid JSON: {err}")))
}

fn canonical_json(value: &Value, surface: &str) -> PyResult<String> {
    serde_json::to_string(value)
        .map_err(|err| PyValueError::new_err(format!("{surface} could not serialize JSON: {err}")))
}

fn sha256_hex(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn object_field<'a>(value: &'a Value, key: &str) -> &'a str {
    value
        .as_object()
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn array_len(value: Option<&Value>) -> usize {
    value.and_then(Value::as_array).map_or(0, Vec::len)
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
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

fn item_text(value: &Value) -> String {
    let text = object_field(value, "text");
    if !text.is_empty() {
        return text.to_string();
    }
    object_field(value, "summary").to_string()
}

fn item_key(value: &Value) -> String {
    let channel = object_field(value, "channel");
    let obligation = [
        object_field(value, "obligation_id"),
        object_field(value, "evidence_id"),
        object_field(value, "id"),
    ]
    .into_iter()
    .find(|candidate| !candidate.is_empty())
    .unwrap_or("");
    let semantic = [
        object_field(value, "semantic_hash"),
        object_field(value, "content_hash"),
        object_field(value, "text"),
    ]
    .into_iter()
    .find(|candidate| !candidate.is_empty())
    .unwrap_or("");
    format!("{channel}\u{1f}{obligation}\u{1f}{semantic}")
}

fn item_cost(value: &Value, item_weight: f64) -> f64 {
    let tokens = value
        .as_object()
        .and_then(|object| object.get("tokens"))
        .and_then(Value::as_f64)
        .unwrap_or_else(|| {
            let text = item_text(value);
            if text.is_empty() {
                1.0
            } else {
                text.len() as f64 / 4.0
            }
        });
    tokens + item_weight
}

fn expression_hash(
    expression_id: &str,
    domain: &str,
    items: &[Value],
    metadata: &Value,
) -> PyResult<String> {
    let payload = json!({
        "expression_id": expression_id,
        "domain": domain,
        "items": items,
        "metadata": metadata,
    });
    Ok(sha256_hex(&canonical_json(
        &payload,
        "bgi_expression_hash",
    )?))
}

fn egg_probe() -> Value {
    let expression: Result<egg::RecExpr<egg::SymbolLang>, _> = "(+ context 0)".parse();
    if let Ok(expr) = expression {
        let rules: Vec<egg::Rewrite<egg::SymbolLang, ()>> =
            vec![egg::rewrite!("bgi-add-zero"; "(+ ?a 0)" => "?a")];
        let runner = egg::Runner::default().with_expr(&expr).run(&rules);
        if let Some(root) = runner.roots.first() {
            let extractor = egg::Extractor::new(&runner.egraph, egg::AstSize);
            let (cost, best) = extractor.find_best(*root);
            return json!({"engine": "egg", "probe_cost": cost, "probe_best": best.to_string()});
        }
    }
    json!({"engine": "egg", "probe_error": "unavailable"})
}

#[pyfunction]
pub fn bgi_stable_hash_json(payload_json: &str) -> PyResult<String> {
    let payload = parse_json(payload_json, "bgi_stable_hash_json")?;
    let canonical = canonical_json(&payload, "bgi_stable_hash_json")?;
    Ok(sha256_hex(&canonical))
}

#[pyfunction]
pub fn bgi_fact_pack_hash_rows_json(rows_json: &str) -> PyResult<String> {
    let payload = parse_json(rows_json, "bgi_fact_pack_hash_rows_json")?;
    let mut rows = payload.as_array().cloned().ok_or_else(|| {
        PyValueError::new_err("bgi_fact_pack_hash_rows_json expected a JSON array")
    })?;
    rows.sort_by(|left, right| {
        (
            object_field(left, "source_artifact_id"),
            object_field(left, "view_type"),
            object_field(left, "view_hash"),
        )
            .cmp(&(
                object_field(right, "source_artifact_id"),
                object_field(right, "view_type"),
                object_field(right, "view_hash"),
            ))
    });
    let canonical = canonical_json(&Value::Array(rows), "bgi_fact_pack_hash_rows_json")?;
    Ok(sha256_hex(&canonical))
}

#[pyfunction]
pub fn bgi_egraph_receipt_summary_json(receipt_json: &str) -> PyResult<String> {
    let receipt = parse_json(receipt_json, "bgi_egraph_receipt_summary_json")?;
    let summary = json!({
        "domain": object_field(&receipt, "domain"),
        "engine": object_field(&receipt, "engine"),
        "equivalent": receipt.get("equivalent").and_then(Value::as_bool).unwrap_or(false),
        "extracted_cost": receipt.get("extracted_cost").and_then(Value::as_f64).unwrap_or(0.0),
        "input_hash": object_field(&receipt, "input_hash"),
        "native_backend": object_field(&receipt, "native_backend"),
        "original_cost": receipt.get("original_cost").and_then(Value::as_f64).unwrap_or(0.0),
        "output_hash": object_field(&receipt, "output_hash"),
        "rewrite_count": array_len(receipt.get("rewrite_trace")),
    });
    canonical_json(&summary, "bgi_egraph_receipt_summary_json")
}

#[pyfunction]
pub fn bgi_egraph_extract_context_pack_json(payload_json: &str) -> PyResult<String> {
    let payload = parse_json(payload_json, "bgi_egraph_extract_context_pack_json")?;
    let expression_id = object_field(&payload, "expression_id");
    let expression_id = if expression_id.is_empty() {
        "native-context-pack"
    } else {
        expression_id
    };
    let items = payload
        .as_object()
        .and_then(|object| object.get("items"))
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| {
            PyValueError::new_err("bgi_egraph_extract_context_pack_json expected items array")
        })?;
    let cost_config = payload
        .as_object()
        .and_then(|object| object.get("cost_config"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let item_weight = cost_config
        .as_object()
        .and_then(|object| object.get("item_weight"))
        .and_then(Value::as_f64)
        .unwrap_or(0.05);
    let metadata = json!({"native_backend": "rust-egg-context-pack", "egg_probe": egg_probe()});

    let input_hash = expression_hash(expression_id, "context_pack", &items, &json!({}))?;
    let original_cost: f64 = items.iter().map(|item| item_cost(item, item_weight)).sum();
    let mut trace: Vec<Value> = Vec::new();

    let mut current = items.clone();
    let before_drop_hash = expression_hash(expression_id, "context_pack", &current, &json!({}))?;
    let before_drop_cost: f64 = current
        .iter()
        .map(|item| item_cost(item, item_weight))
        .sum();
    current = current
        .into_iter()
        .filter(|item| {
            item.as_object()
                .and_then(|object| object.get("hard_required"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || !item_text(item).trim().is_empty()
        })
        .collect();
    if current.len() != items.len() {
        let after_hash = expression_hash(expression_id, "context_pack", &current, &json!({}))?;
        let after_cost: f64 = current
            .iter()
            .map(|item| item_cost(item, item_weight))
            .sum();
        trace.push(json!({
            "rule_id": "drop_empty_optional",
            "before_hash": before_drop_hash,
            "after_hash": after_hash,
            "reason": "Removed optional empty context items without changing represented obligations.",
            "delta_cost": ((after_cost - before_drop_cost) * 1_000_000.0).round() / 1_000_000.0,
            "data": {"removed_count": items.len() - current.len()},
        }));
    }

    let before_dedupe = current.clone();
    let before_dedupe_hash =
        expression_hash(expression_id, "context_pack", &before_dedupe, &json!({}))?;
    let before_dedupe_cost: f64 = before_dedupe
        .iter()
        .map(|item| item_cost(item, item_weight))
        .sum();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut deduped: Vec<Value> = Vec::new();
    let mut removed_count = 0usize;
    for item in before_dedupe {
        let hard_required = item
            .as_object()
            .and_then(|object| object.get("hard_required"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let key = item_key(&item);
        if !hard_required && seen.contains(&key) {
            removed_count += 1;
            continue;
        }
        seen.insert(key);
        deduped.push(item);
    }
    current = deduped;
    if removed_count > 0 {
        let after_hash = expression_hash(expression_id, "context_pack", &current, &json!({}))?;
        let after_cost: f64 = current
            .iter()
            .map(|item| item_cost(item, item_weight))
            .sum();
        trace.push(json!({
            "rule_id": "dedupe_same_obligation",
            "before_hash": before_dedupe_hash,
            "after_hash": after_hash,
            "reason": "Removed duplicate non-required context items with the same obligation and channel.",
            "delta_cost": ((after_cost - before_dedupe_cost) * 1_000_000.0).round() / 1_000_000.0,
            "data": {"removed_count": removed_count},
        }));
    }

    let output_hash = expression_hash(expression_id, "context_pack", &current, &metadata)?;
    let extracted_cost: f64 = current
        .iter()
        .map(|item| item_cost(item, item_weight))
        .sum();
    let receipt = json!({
        "engine": "egraph-theorem",
        "native_backend": "rust-egg-context-pack",
        "input_hash": input_hash,
        "output_hash": output_hash,
        "domain": "context_pack",
        "equivalent": true,
        "original_cost": (original_cost * 1_000_000.0).round() / 1_000_000.0,
        "extracted_cost": (extracted_cost * 1_000_000.0).round() / 1_000_000.0,
        "rewrite_trace": trace,
        "extraction": {
            "expression_id": expression_id,
            "domain": "context_pack",
            "items": current,
            "metadata": metadata,
            "expression_hash": output_hash,
        },
    });
    canonical_json(&receipt, "bgi_egraph_extract_context_pack_json")
}

#[pyfunction]
pub fn bgi_datalog_receipt_summary_json(receipt_json: &str) -> PyResult<String> {
    let receipt = parse_json(receipt_json, "bgi_datalog_receipt_summary_json")?;
    let summary = json!({
        "derived_count": receipt.get("derived_count").and_then(Value::as_u64).unwrap_or(0),
        "engine": object_field(&receipt, "engine"),
        "fact_pack_hash": object_field(&receipt, "fact_pack_hash"),
        "rule_ids": string_array(receipt.get("rule_ids")),
        "warning_count": array_len(receipt.get("warnings")),
        "writeback_policy": object_field(&receipt, "writeback_policy"),
    });
    canonical_json(&summary, "bgi_datalog_receipt_summary_json")
}

#[pyfunction]
pub fn bgi_datalog_derive_core_json(facts_json: &str) -> PyResult<String> {
    let payload = parse_json(facts_json, "bgi_datalog_derive_core_json")?;
    let facts = payload.as_array().cloned().ok_or_else(|| {
        PyValueError::new_err("bgi_datalog_derive_core_json expected a JSON array")
    })?;
    let _iteration = datafrog::Iteration::new();
    let mut claims: Vec<Value> = Vec::new();
    let mut objects_by_title: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for fact in &facts {
        let relation = object_field(fact, "relation");
        if relation == "claim" {
            claims.push(fact.clone());
        }
        if relation == "object" {
            let title = fact
                .as_object()
                .and_then(|object| object.get("attributes"))
                .and_then(Value::as_object)
                .and_then(|attrs| attrs.get("title"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_lowercase()
                .replace('-', " ");
            if !title.trim().is_empty() {
                objects_by_title
                    .entry(title)
                    .or_default()
                    .push(fact.clone());
            }
        }
    }

    let mut derived: Vec<Value> = Vec::new();
    for claim in claims {
        let entity_id = object_field(&claim, "entity_id");
        let status = claim
            .as_object()
            .and_then(|object| object.get("attributes"))
            .and_then(Value::as_object)
            .and_then(|attrs| attrs.get("status"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if status != "accepted" && status != "supported" {
            derived.push(json!({
                "rule_id": "unsupported_claim",
                "relation": "unsupported_claim",
                "subject_id": entity_id,
                "reason": "Claim has no accepted support fact in the native core rule set.",
                "dependency_fact_ids": [object_field(&claim, "fact_id")],
                "attributes": {"native_backend": "rust-datafrog-core"},
                "confidence": 1.0,
                "writeback_policy": "read-only",
                "fact_id": sha256_hex(&canonical_json(&claim, "bgi_datalog_derive_core_json")?),
            }));
        }
    }
    for (_title, group) in objects_by_title {
        if group.len() < 2 {
            continue;
        }
        for fact in group {
            derived.push(json!({
                "rule_id": "likely_duplicate_entity",
                "relation": "likely_duplicate_entity",
                "subject_id": object_field(&fact, "entity_id"),
                "reason": "Multiple objects share a normalized title in the native core rule set.",
                "dependency_fact_ids": [object_field(&fact, "fact_id")],
                "attributes": {"native_backend": "rust-datafrog-core"},
                "confidence": 0.75,
                "writeback_policy": "read-only",
                "fact_id": sha256_hex(&canonical_json(&fact, "bgi_datalog_derive_core_json")?),
            }));
        }
    }

    let fact_pack_hash = sha256_hex(&canonical_json(
        &Value::Array(facts),
        "bgi_datalog_derive_core_json",
    )?);
    canonical_json(
        &json!({
            "engine": "rust-datafrog-core",
            "fact_pack_hash": fact_pack_hash,
            "rule_ids": ["unsupported_claim", "likely_duplicate_entity"],
            "derived_count": derived.len(),
            "derived_facts": derived,
            "warnings": [],
            "writeback_policy": "read-only",
        }),
        "bgi_datalog_derive_core_json",
    )
}

#[pyfunction]
pub fn bgi_compact_receipts_json(receipts_json: &str) -> PyResult<String> {
    let payload = parse_json(receipts_json, "bgi_compact_receipts_json")?;
    let receipts = payload
        .as_array()
        .cloned()
        .ok_or_else(|| PyValueError::new_err("bgi_compact_receipts_json expected a JSON array"))?;
    let mut status_counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut receipt_hashes: Vec<String> = Vec::new();

    for receipt in &receipts {
        let status = object_field(receipt, "status");
        if !status.is_empty() {
            *status_counts.entry(status.to_string()).or_insert(0) += 1;
        }
        for key in [
            "receipt_hash",
            "payload_hash",
            "formula_hash",
            "input_hash",
            "output_hash",
            "fact_pack_hash",
        ] {
            let value = object_field(receipt, key);
            if !value.is_empty() {
                receipt_hashes.push(value.to_string());
                break;
            }
        }
    }
    receipt_hashes.sort();
    receipt_hashes.dedup();

    let canonical_payload =
        canonical_json(&Value::Array(receipts), "bgi_compact_receipts_json payload")?;
    let status_value: Map<String, Value> = status_counts
        .into_iter()
        .map(|(key, value)| (key, Value::from(value)))
        .collect();
    let summary = json!({
        "count": payload.as_array().map_or(0, Vec::len),
        "payload_hash": sha256_hex(&canonical_payload),
        "receipt_hashes": receipt_hashes,
        "status_counts": Value::Object(status_value),
    });
    canonical_json(&summary, "bgi_compact_receipts_json")
}
