use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

const TRACKING_PARAMS: [&str; 4] = ["fbclid", "gclid", "mc_cid", "mc_eid"];

#[pyfunction]
pub fn search_normalize_urls_batch(urls: Vec<String>) -> Vec<String> {
    urls.iter().map(|url| normalize_url(url)).collect()
}

#[pyfunction]
pub fn search_score_frontier_batch<'py>(
    py: Python<'py>,
    rows: &Bound<'py, PyAny>,
    query_tokens: Vec<String>,
) -> PyResult<Bound<'py, PyList>> {
    let out = PyList::empty_bound(py);
    for item in rows.iter()? {
        let item = item?;
        let dict = item.downcast::<PyDict>()?;
        let url = get_string(dict, "url")?
            .or_else(|| get_string(dict, "normalized_url").ok().flatten())
            .unwrap_or_default();
        let title = get_string(dict, "title")?.unwrap_or_default();
        let snippet = get_string(dict, "snippet")?.unwrap_or_default();
        let source_type = get_string(dict, "source_type")?.unwrap_or_default();
        let haystack = format!("{} {} {} {}", url, title, snippet, source_type).to_lowercase();
        let overlap = query_tokens
            .iter()
            .filter(|token| haystack.contains(&token.to_lowercase()))
            .count() as f64;
        let token_score = if query_tokens.is_empty() {
            0.0
        } else {
            overlap / query_tokens.len() as f64
        };
        let domain_health = get_f64(dict, "domain_health")?
            .or_else(|| get_f64(dict, "domain_health_score").ok().flatten())
            .unwrap_or(0.0);
        let freshness = get_f64(dict, "freshness")?
            .or_else(|| get_f64(dict, "freshness_hint").ok().flatten())
            .unwrap_or(0.0);
        let prior = get_f64(dict, "prior_successful_extraction")?.unwrap_or(0.0);
        let depth = get_i64(dict, "depth")?.unwrap_or(0).max(0) as f64;
        let components = PyDict::new_bound(py);
        components.set_item("token_overlap", token_score)?;
        components.set_item("domain_health", domain_health)?;
        components.set_item("freshness", freshness)?;
        components.set_item("prior_successful_extraction", prior)?;
        components.set_item("depth_penalty", -0.10 * depth)?;
        let score = token_score + domain_health + freshness + prior - (0.10 * depth);
        let row = PyDict::new_bound(py);
        for (key, value) in dict.iter() {
            row.set_item(key, value)?;
        }
        row.set_item("normalized_url", normalize_url(&url))?;
        row.set_item("score_components", components)?;
        row.set_item("score", score)?;
        out.append(row)?;
    }
    Ok(out)
}

#[pyfunction]
pub fn search_fuse_scores_batch<'py>(
    py: Python<'py>,
    rows: &Bound<'py, PyAny>,
    weights: &Bound<'py, PyDict>,
) -> PyResult<Bound<'py, PyList>> {
    let out = PyList::empty_bound(py);
    for item in rows.iter()? {
        let item = item?;
        let dict = item.downcast::<PyDict>()?;
        let row = PyDict::new_bound(py);
        for (key, value) in dict.iter() {
            row.set_item(key, value)?;
        }
        let mut fused = 0.0;
        if let Some(components_any) = dict.get_item("score_components")? {
            if let Ok(components) = components_any.downcast::<PyDict>() {
                for (key, value) in components.iter() {
                    let name = key.extract::<String>()?;
                    let score = value.extract::<f64>()?;
                    let weight = match weights.get_item(name.as_str())? {
                        Some(raw) => raw.extract::<f64>()?,
                        None => 1.0,
                    };
                    fused += score * weight;
                }
            }
        }
        row.set_item("fused_score", fused)?;
        out.append(row)?;
    }
    Ok(out)
}

#[pyfunction]
pub fn search_cosine_topk(
    query: Vec<f64>,
    vectors: Vec<(String, Vec<f64>)>,
    top_k: usize,
) -> Vec<(String, f64)> {
    let mut scored: Vec<(String, f64)> = vectors
        .into_iter()
        .map(|(id, vector)| (id, cosine(&query, &vector)))
        .collect();
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    scored.truncate(top_k);
    scored
}

fn normalize_url(url: &str) -> String {
    let trimmed = url.trim();
    let without_fragment = trimmed.split('#').next().unwrap_or("");
    let (scheme, rest) = match without_fragment.split_once("://") {
        Some((s, r)) if s.eq_ignore_ascii_case("http") || s.eq_ignore_ascii_case("https") => {
            (s.to_lowercase(), r)
        }
        _ => return String::new(),
    };
    let (host_and_path, query) = match rest.split_once('?') {
        Some((h, q)) => (h, q),
        None => (rest, ""),
    };
    let (host, path) = match host_and_path.split_once('/') {
        Some((h, p)) => (h.to_lowercase(), format!("/{}", p)),
        None => (host_and_path.to_lowercase(), "/".to_string()),
    };
    if host.is_empty() {
        return String::new();
    }
    let clean_query: Vec<&str> = query
        .split('&')
        .filter(|part| {
            let key = part.split('=').next().unwrap_or("").to_lowercase();
            !key.starts_with("utm_") && !TRACKING_PARAMS.contains(&key.as_str())
        })
        .filter(|part| !part.is_empty())
        .collect();
    if clean_query.is_empty() {
        format!("{}://{}{}", scheme, host, path)
    } else {
        format!("{}://{}{}?{}", scheme, host, path, clean_query.join("&"))
    }
}

fn cosine(query: &[f64], vector: &[f64]) -> f64 {
    if query.is_empty() || query.len() != vector.len() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut q_norm = 0.0;
    let mut v_norm = 0.0;
    for (a, b) in query.iter().zip(vector.iter()) {
        dot += a * b;
        q_norm += a * a;
        v_norm += b * b;
    }
    if q_norm == 0.0 || v_norm == 0.0 {
        return 0.0;
    }
    dot / (q_norm.sqrt() * v_norm.sqrt())
}

fn get_string(dict: &Bound<'_, PyDict>, key: &str) -> PyResult<Option<String>> {
    Ok(match dict.get_item(key)? {
        Some(value) => Some(value.extract::<String>()?),
        None => None,
    })
}

fn get_f64(dict: &Bound<'_, PyDict>, key: &str) -> PyResult<Option<f64>> {
    Ok(match dict.get_item(key)? {
        Some(value) => Some(value.extract::<f64>()?),
        None => None,
    })
}

fn get_i64(dict: &Bound<'_, PyDict>, key: &str) -> PyResult<Option<i64>> {
    Ok(match dict.get_item(key)? {
        Some(value) => Some(value.extract::<i64>()?),
        None => None,
    })
}
