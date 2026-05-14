use axum::{extract::State, Json};
use serde_json::{json, Value};

use crate::state::AppState;

pub async fn openapi(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "openapi": "3.1.0",
        "info": {
            "title": state.config.api_title.as_str(),
            "version": "0.1.0"
        },
        "paths": {
            "/health": { "get": { "responses": { "200": { "description": "healthy" } } } },
            "/ready": { "get": { "responses": { "200": { "description": "ready" } } } },
            "/openapi.json": { "get": { "responses": { "200": { "description": "OpenAPI document" } } } },
            "/.well-known/mcp/thg.json": { "get": { "responses": { "200": { "description": "THG MCP discovery manifest" } } } },
            "/.well-known/agent.json": { "get": { "responses": { "200": { "description": "Agent discovery manifest" } } } },
            "/mcp": { "post": { "responses": { "200": { "description": "Streamable HTTP MCP JSON-RPC endpoint" } } } },
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
            "/v1/tenants/{tenant_id}/graph/nodes": {
                "post": { "responses": { "200": { "description": "Upsert graph node" } } }
            },
            "/v1/tenants/{tenant_id}/graph/nodes/{node_id}": {
                "get": { "responses": { "200": { "description": "Get graph node" } } }
            },
            "/v1/tenants/{tenant_id}/graph/nodes/query": {
                "post": { "responses": { "200": { "description": "Query graph nodes by label and exact scalar property indexes" } } }
            },
            "/v1/tenants/{tenant_id}/graph/edges": {
                "post": { "responses": { "200": { "description": "Upsert graph edge" } } }
            },
            "/v1/tenants/{tenant_id}/graph/edges/{edge_id}": {
                "get": { "responses": { "200": { "description": "Get graph edge" } } }
            },
            "/v1/tenants/{tenant_id}/graph/neighbors": {
                "post": { "responses": { "200": { "description": "Read graph neighbors from adjacency indexes" } } }
            },
            "/v1/tenants/{tenant_id}/graph/stats": {
                "get": { "responses": { "200": { "description": "Read graph stats" } } }
            },
            "/v1/tenants/{tenant_id}/graph/verify": {
                "get": { "responses": { "200": { "description": "Verify graph indexes" } } }
            },
            "/v1/tenants/{tenant_id}/context/pack": {
                "post": { "responses": { "200": { "description": "Context pack response" } } }
            },
            "/metrics": { "get": { "responses": { "200": { "description": "Admin metrics" } } } }
        }
    }))
}
