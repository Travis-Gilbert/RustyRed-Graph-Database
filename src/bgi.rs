use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

fn parse_json(text: &str, surface: &str) -> PyResult<Value> {
    serde_json::from_str(text).map_err(|err| {
        PyValueError::new_err(format!("{surface} expected valid JSON: {err}"))
    })
}

fn canonical_json(value: &Value, surface: &str) -> PyResult<String> {
    serde_json::to_string(value).map_err(|err| {
        PyValueError::new_err(format!("{surface} could not serialize JSON: {err}"))
    })
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
pub fn bgi_compact_receipts_json(receipts_json: &str) -> PyResult<String> {
    let payload = parse_json(receipts_json, "bgi_compact_receipts_json")?;
    let receipts = payload.as_array().cloned().ok_or_else(|| {
        PyValueError::new_err("bgi_compact_receipts_json expected a JSON array")
    })?;
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

    let canonical_payload = canonical_json(
        &Value::Array(receipts),
        "bgi_compact_receipts_json payload",
    )?;
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
