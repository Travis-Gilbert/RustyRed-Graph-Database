use axum::Json;
use serde_json::{json, Value};

pub async fn openapi() -> Json<Value> {
    Json(json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Theorem Context THG API",
            "version": "0.1.0"
        },
        "paths": {
            "/health": { "get": { "responses": { "200": { "description": "healthy" } } } },
            "/ready": { "get": { "responses": { "200": { "description": "ready" } } } },
            "/openapi.json": { "get": { "responses": { "200": { "description": "OpenAPI document" } } } },
            "/v1/tenants/{tenant_id}/command": {
                "post": { "responses": { "200": { "description": "THG command response" } } }
            },
            "/v1/tenants/{tenant_id}/batch": {
                "post": { "responses": { "200": { "description": "Batch THG command response" } } }
            },
            "/v1/tenants/{tenant_id}/runs/{run_id}": {
                "get": { "responses": { "200": { "description": "THG run response" } } }
            },
            "/v1/tenants/{tenant_id}/graph/query": {
                "post": { "responses": { "200": { "description": "Graph query response" } } }
            },
            "/v1/tenants/{tenant_id}/context/pack": {
                "post": { "responses": { "200": { "description": "Context pack response" } } }
            },
            "/metrics": { "get": { "responses": { "200": { "description": "Admin metrics" } } } }
        }
    }))
}
