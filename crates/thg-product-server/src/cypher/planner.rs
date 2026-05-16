//! Row-shape operators for the WITH/ORDER BY/SKIP pipeline.
//!
//! `aggregate` groups rows by N keys and finalizes SUM/AVG/MIN/MAX/COUNT;
//! `sort_rows` orders rows by one or more `OrderBy` clauses (numeric and
//! string-aware); `skip_rows` drops a leading prefix. The executor in
//! `query_surface` pipes its materialized rows through these three operators
//! after the MATCH phase produced them.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::cypher::ast::{AggOp, OrderBy};

#[derive(Clone, Debug)]
pub struct AggregateOutput {
    pub alias: String,
    pub op: AggOp,
    /// Source column key inside each input row. `None` is the COUNT(*) case:
    /// every non-null row contributes one to the count and no value is read.
    pub source_key: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AggregateSpec {
    pub group_keys: Vec<String>,
    pub aggs: Vec<AggregateOutput>,
}

pub fn aggregate(rows: &[Map<String, Value>], spec: &AggregateSpec) -> Vec<Map<String, Value>> {
    // Two parallel BTreeMaps keyed by the same group-string so the deterministic
    // emit-order matches what tests assert against.
    let mut groups: BTreeMap<String, (Map<String, Value>, BTreeMap<String, AggAccumulator>)> =
        BTreeMap::new();
    for row in rows {
        let key = make_group_key(&spec.group_keys, row);
        let entry = groups.entry(key).or_insert_with(|| {
            let mut group_row = Map::new();
            for key_name in &spec.group_keys {
                if let Some(v) = row.get(key_name) {
                    group_row.insert(key_name.clone(), v.clone());
                }
            }
            let accs: BTreeMap<String, AggAccumulator> = spec
                .aggs
                .iter()
                .map(|out| (out.alias.clone(), AggAccumulator::default()))
                .collect();
            (group_row, accs)
        });
        for out in &spec.aggs {
            let source = match &out.source_key {
                Some(k) => row.get(k).cloned().unwrap_or(Value::Null),
                None => Value::Null,
            };
            let acc = entry
                .1
                .get_mut(&out.alias)
                .expect("accumulator initialised in or_insert_with");
            acc.observe(out.op, &source);
        }
    }
    let mut out_rows: Vec<Map<String, Value>> = Vec::with_capacity(groups.len());
    for (_, (group_row, accs)) in groups {
        let mut row = group_row;
        for out in &spec.aggs {
            let acc = accs
                .get(&out.alias)
                .expect("accumulator present at finalize");
            row.insert(out.alias.clone(), acc.finalize(out.op));
        }
        out_rows.push(row);
    }
    out_rows
}

pub fn sort_rows(rows: &mut [Map<String, Value>], order: &[OrderBy]) {
    if order.is_empty() {
        return;
    }
    rows.sort_by(|a, b| {
        for clause in order {
            let av = a.get(&clause.expression).cloned().unwrap_or(Value::Null);
            let bv = b.get(&clause.expression).cloned().unwrap_or(Value::Null);
            let ord = value_cmp(&av, &bv);
            if ord != std::cmp::Ordering::Equal {
                return if clause.descending {
                    ord.reverse()
                } else {
                    ord
                };
            }
        }
        std::cmp::Ordering::Equal
    });
}

pub fn skip_rows(rows: Vec<Map<String, Value>>, skip: usize) -> Vec<Map<String, Value>> {
    if skip == 0 {
        return rows;
    }
    rows.into_iter().skip(skip).collect()
}

fn value_cmp(a: &Value, b: &Value) -> std::cmp::Ordering {
    match (a, b) {
        (Value::Number(an), Value::Number(bn)) => an
            .as_f64()
            .unwrap_or(f64::NAN)
            .partial_cmp(&bn.as_f64().unwrap_or(f64::NAN))
            .unwrap_or(std::cmp::Ordering::Equal),
        (Value::String(av), Value::String(bv)) => av.cmp(bv),
        (Value::Bool(av), Value::Bool(bv)) => av.cmp(bv),
        (Value::Null, Value::Null) => std::cmp::Ordering::Equal,
        (Value::Null, _) => std::cmp::Ordering::Less,
        (_, Value::Null) => std::cmp::Ordering::Greater,
        _ => format!("{a}").cmp(&format!("{b}")),
    }
}

fn make_group_key(keys: &[String], row: &Map<String, Value>) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(keys.len());
    for k in keys {
        parts.push(format!(
            "{}={}",
            k,
            row.get(k).cloned().unwrap_or(Value::Null)
        ));
    }
    parts.join("|")
}

#[derive(Debug, Clone, Default)]
struct AggAccumulator {
    /// Total rows observed; this is what COUNT emits.
    row_count: u64,
    /// Count of rows whose source value parsed as a finite f64; AVG divides by this.
    numeric_count: u64,
    sum: f64,
    min: Option<f64>,
    max: Option<f64>,
}

impl AggAccumulator {
    fn observe(&mut self, _op: AggOp, v: &Value) {
        // Every observed row contributes to the row count. COUNT(*) and
        // count(n) both reach this path with v = Null because no source column
        // is attached, but the matched row itself is what's being counted.
        self.row_count += 1;
        if let Some(num) = v.as_f64() {
            self.sum += num;
            self.numeric_count += 1;
            self.min = Some(self.min.map_or(num, |m| m.min(num)));
            self.max = Some(self.max.map_or(num, |m| m.max(num)));
        }
    }

    fn finalize(&self, op: AggOp) -> Value {
        match op {
            AggOp::Count => json!(self.row_count),
            AggOp::Sum => {
                // Emit integer JSON when the running sum is a whole number; this
                // keeps `sum(integer_column)` round-tripping cleanly in JSON.
                if self.sum.fract() == 0.0 && self.sum.abs() < (i64::MAX as f64) {
                    json!(self.sum as i64)
                } else {
                    json!(self.sum)
                }
            }
            AggOp::Avg => {
                if self.numeric_count == 0 {
                    Value::Null
                } else {
                    json!(self.sum / self.numeric_count as f64)
                }
            }
            AggOp::Min => self.min.map(|v| json!(v)).unwrap_or(Value::Null),
            AggOp::Max => self.max.map(|v| json!(v)).unwrap_or(Value::Null),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cypher::ast::{AggOp, OrderBy};
    use serde_json::json;

    fn rows_sample() -> Vec<serde_json::Map<String, serde_json::Value>> {
        vec![
            {
                let mut m = serde_json::Map::new();
                m.insert("cat".into(), json!("blue"));
                m.insert("n.score".into(), json!(5));
                m
            },
            {
                let mut m = serde_json::Map::new();
                m.insert("cat".into(), json!("blue"));
                m.insert("n.score".into(), json!(7));
                m
            },
            {
                let mut m = serde_json::Map::new();
                m.insert("cat".into(), json!("red"));
                m.insert("n.score".into(), json!(3));
                m
            },
        ]
    }

    #[test]
    fn aggregate_sum_groups_rows_by_key() {
        let rows = rows_sample();
        let spec = AggregateSpec {
            group_keys: vec!["cat".into()],
            aggs: vec![AggregateOutput {
                alias: "total".into(),
                op: AggOp::Sum,
                source_key: Some("n.score".into()),
            }],
        };
        let out = aggregate(&rows, &spec);
        assert_eq!(out.len(), 2);
        let blue = out.iter().find(|r| r["cat"] == "blue").unwrap();
        assert_eq!(blue["total"], json!(12));
        let red = out.iter().find(|r| r["cat"] == "red").unwrap();
        assert_eq!(red["total"], json!(3));
    }

    #[test]
    fn sort_orders_rows_by_key_desc() {
        let mut rows = vec![
            {
                let mut m = serde_json::Map::new();
                m.insert("c".into(), json!(1));
                m
            },
            {
                let mut m = serde_json::Map::new();
                m.insert("c".into(), json!(3));
                m
            },
            {
                let mut m = serde_json::Map::new();
                m.insert("c".into(), json!(2));
                m
            },
        ];
        sort_rows(
            &mut rows,
            &[OrderBy {
                expression: "c".into(),
                descending: true,
            }],
        );
        assert_eq!(rows[0]["c"], json!(3));
        assert_eq!(rows[2]["c"], json!(1));
    }

    #[test]
    fn skip_drops_leading_rows() {
        let rows = vec![
            {
                let mut m = serde_json::Map::new();
                m.insert("c".into(), json!(1));
                m
            },
            {
                let mut m = serde_json::Map::new();
                m.insert("c".into(), json!(2));
                m
            },
            {
                let mut m = serde_json::Map::new();
                m.insert("c".into(), json!(3));
                m
            },
        ];
        let trimmed = skip_rows(rows, 2);
        assert_eq!(trimmed.len(), 1);
        assert_eq!(trimmed[0]["c"], json!(3));
    }

    #[test]
    fn aggregate_avg_returns_mean() {
        let rows = rows_sample();
        let spec = AggregateSpec {
            group_keys: vec!["cat".into()],
            aggs: vec![AggregateOutput {
                alias: "mean".into(),
                op: AggOp::Avg,
                source_key: Some("n.score".into()),
            }],
        };
        let out = aggregate(&rows, &spec);
        let blue = out.iter().find(|r| r["cat"] == "blue").unwrap();
        assert_eq!(blue["mean"], json!(6.0));
        let red = out.iter().find(|r| r["cat"] == "red").unwrap();
        assert_eq!(red["mean"], json!(3.0));
    }

    #[test]
    fn aggregate_min_max_track_extrema() {
        let rows = rows_sample();
        let spec = AggregateSpec {
            group_keys: vec!["cat".into()],
            aggs: vec![
                AggregateOutput {
                    alias: "lo".into(),
                    op: AggOp::Min,
                    source_key: Some("n.score".into()),
                },
                AggregateOutput {
                    alias: "hi".into(),
                    op: AggOp::Max,
                    source_key: Some("n.score".into()),
                },
            ],
        };
        let out = aggregate(&rows, &spec);
        let blue = out.iter().find(|r| r["cat"] == "blue").unwrap();
        assert_eq!(blue["lo"], json!(5.0));
        assert_eq!(blue["hi"], json!(7.0));
    }
}
