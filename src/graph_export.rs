use numpy::{IntoPyArray, PyReadonlyArray1};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::collections::HashMap;

#[pyfunction]
pub fn graph_remap_ids_batch<'py>(
    py: Python<'py>,
    object_ids: PyReadonlyArray1<'py, i64>,
) -> PyResult<Bound<'py, PyDict>> {
    let ids = object_ids.as_slice()?.to_vec();
    let (dense_ids, unique_ids) = py.allow_threads(move || {
        let mut map: HashMap<i64, i64> = HashMap::with_capacity(ids.len());
        let mut unique: Vec<i64> = Vec::new();
        let mut dense: Vec<i64> = Vec::with_capacity(ids.len());
        for id in ids {
            let next = map.len() as i64;
            let entry = map.entry(id).or_insert_with(|| {
                unique.push(id);
                next
            });
            dense.push(*entry);
        }
        (dense, unique)
    });
    let out = PyDict::new_bound(py);
    out.set_item("dense_ids", dense_ids.into_pyarray_bound(py))?;
    out.set_item("unique_ids", unique_ids.into_pyarray_bound(py))?;
    Ok(out)
}

#[pyfunction]
pub fn graph_pack_edges_batch<'py>(
    py: Python<'py>,
    source_ids: PyReadonlyArray1<'py, i64>,
    target_ids: PyReadonlyArray1<'py, i64>,
    relation_ids: PyReadonlyArray1<'py, i32>,
    dense_id_by_object_id: HashMap<i64, i64>,
) -> PyResult<Bound<'py, PyDict>> {
    let src_values = source_ids.as_slice()?.to_vec();
    let dst_values = target_ids.as_slice()?.to_vec();
    let rel_values = relation_ids.as_slice()?.to_vec();
    let (src, dst, rel) = py.allow_threads(move || {
        let mut src_out: Vec<i64> = Vec::with_capacity(src_values.len());
        let mut dst_out: Vec<i64> = Vec::with_capacity(dst_values.len());
        let mut rel_out: Vec<i32> = Vec::with_capacity(rel_values.len());
        for idx in 0..src_values.len().min(dst_values.len()).min(rel_values.len()) {
            let Some(src_dense) = dense_id_by_object_id.get(&src_values[idx]) else {
                continue;
            };
            let Some(dst_dense) = dense_id_by_object_id.get(&dst_values[idx]) else {
                continue;
            };
            src_out.push(*src_dense);
            dst_out.push(*dst_dense);
            rel_out.push(rel_values[idx]);
        }
        (src_out, dst_out, rel_out)
    });
    let out = PyDict::new_bound(py);
    out.set_item("src", src.into_pyarray_bound(py))?;
    out.set_item("dst", dst.into_pyarray_bound(py))?;
    out.set_item("rel", rel.into_pyarray_bound(py))?;
    Ok(out)
}
