use axum::{extract::State, Json};
use serde_json::{json, Value};

use crate::state::AppState;

pub async fn openapi(State(state): State<AppState>) -> Json<Value> {
    let tenant_parameter = json!({
        "name": "tenant_id",
        "in": "path",
        "required": true,
        "schema": { "type": "string" },
        "description": "Tenant namespace for graph and run state."
    });
    let node_id_parameter = json!({
        "name": "node_id",
        "in": "path",
        "required": true,
        "schema": { "type": "string" },
        "description": "Graph node identifier."
    });
    let edge_id_parameter = json!({
        "name": "edge_id",
        "in": "path",
        "required": true,
        "schema": { "type": "string" },
        "description": "Graph edge identifier."
    });
    let run_id_parameter = json!({
        "name": "run_id",
        "in": "path",
        "required": true,
        "schema": { "type": "string" },
        "description": "Agent run identifier."
    });

    Json(json!({
        "openapi": "3.1.0",
        "info": {
            "title": state.config.api_title.as_str(),
            "version": "0.1.0",
            "description": "Rusty Red Graph Database HTTP API. This document describes the graph/run/context HTTP surface and the MCP transport endpoint; it is not a RedisGraph, FalkorDB, or raw Redis protocol specification."
        },
        "tags": [
            { "name": "operations", "description": "Health, readiness, metrics, and discovery." },
            { "name": "mcp", "description": "Streamable HTTP MCP agent port over Rusty Red graph APIs." },
            { "name": "runs", "description": "THG-compatible run and batch command runtime." },
            { "name": "graph", "description": "First-class graph node, edge, adjacency, index, and verification routes." },
            { "name": "context", "description": "Context pack writes used by Context Theorem harness flows." }
        ],
        "security": [{ "bearerAuth": [] }],
        "paths": {
            "/health": {
                "get": {
                    "tags": ["operations"],
                    "summary": "Liveness probe",
                    "security": [],
                    "responses": {
                        "200": {
                            "description": "Service process is healthy.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/HealthResponse" }
                                }
                            }
                        }
                    }
                }
            },
            "/ready": {
                "get": {
                    "tags": ["operations"],
                    "summary": "Readiness probe",
                    "security": [],
                    "responses": {
                        "200": {
                            "description": "Redis-compatible backing store is reachable.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/ReadyResponse" }
                                }
                            }
                        },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/openapi.json": {
                "get": {
                    "tags": ["operations"],
                    "summary": "OpenAPI document",
                    "security": [],
                    "responses": { "200": { "description": "OpenAPI 3.1 document" } }
                }
            },
            "/.well-known/mcp/thg.json": {
                "get": {
                    "tags": ["mcp"],
                    "summary": "MCP discovery manifest",
                    "security": [],
                    "responses": {
                        "200": { "description": "MCP discovery manifest for the Rusty Red agent port." },
                        "404": { "description": "MCP endpoint is disabled." }
                    }
                }
            },
            "/.well-known/agent.json": {
                "get": {
                    "tags": ["mcp"],
                    "summary": "Agent discovery manifest",
                    "security": [],
                    "responses": {
                        "200": { "description": "Agent discovery manifest pointing to the MCP endpoint." },
                        "404": { "description": "MCP endpoint is disabled." }
                    }
                }
            },
            "/mcp": {
                "post": {
                    "tags": ["mcp"],
                    "summary": "Streamable HTTP MCP JSON-RPC endpoint",
                    "description": "Accepts MCP JSON-RPC requests. The tools and resources expose graph-native Rusty Red operations; raw Redis commands and keys are not part of this contract.",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/JsonRpcRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "MCP JSON-RPC response.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/JsonRpcResponse" }
                                }
                            }
                        },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "404": { "description": "MCP endpoint is disabled." }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/command": {
                "post": {
                    "tags": ["runs"],
                    "summary": "Execute a THG-compatible command",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/CommandRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/CommandResponse" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/batch": {
                "post": {
                    "tags": ["runs"],
                    "summary": "Execute multiple THG-compatible commands",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/BatchRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/CommandResponse" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/runs/{run_id}": {
                "get": {
                    "tags": ["runs"],
                    "summary": "Retrieve a run",
                    "parameters": [tenant_parameter.clone(), run_id_parameter],
                    "responses": {
                        "200": { "$ref": "#/components/responses/CommandResponse" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/query": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Run the current bounded graph query command",
                    "description": "Executes the THG.DEBUG.CYPHER compatibility command. This is intentionally a small debug/query bridge, not a full OpenCypher, GQL, RedisGraph, or FalkorDB compatibility layer.",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/GraphQueryRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/CommandResponse" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/nodes": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Upsert a graph node",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/NodeWriteRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/GraphWriteResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/nodes/{node_id}": {
                "get": {
                    "tags": ["graph"],
                    "summary": "Read a graph node",
                    "parameters": [tenant_parameter.clone(), node_id_parameter],
                    "responses": {
                        "200": {
                            "description": "Graph node response.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/NodeResponse" }
                                }
                            }
                        },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "404": { "description": "Node not found." },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/nodes/query": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Query graph nodes by label and exact scalar property indexes",
                    "description": "Returns non-tombstoned nodes matched by optional label and exact top-level scalar property values. Object and array property values are stored but not indexed by this route.",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/NodeQuery" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Node query result.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/NodeQueryResponse" }
                                }
                            }
                        },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/edges": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Upsert a graph edge",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/EdgeWriteRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/GraphWriteResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/edges/{edge_id}": {
                "get": {
                    "tags": ["graph"],
                    "summary": "Read a graph edge",
                    "parameters": [tenant_parameter.clone(), edge_id_parameter],
                    "responses": {
                        "200": {
                            "description": "Graph edge response.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/EdgeResponse" }
                                }
                            }
                        },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "404": { "description": "Edge not found." },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/neighbors": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Read graph neighbors from adjacency indexes",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/NeighborQuery" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Neighbor query result.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/NeighborResponse" }
                                }
                            }
                        },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/stats": {
                "get": {
                    "tags": ["graph"],
                    "summary": "Read graph stats",
                    "parameters": [tenant_parameter.clone()],
                    "responses": {
                        "200": {
                            "description": "Graph stats response.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/GraphStatsResponse" }
                                }
                            }
                        },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/verify": {
                "get": {
                    "tags": ["graph"],
                    "summary": "Verify graph indexes",
                    "description": "Checks stored graph records against adjacency, label, edge-type, and exact scalar property indexes. This route reports drift; rebuild/repair is planned as a separate admin slice.",
                    "parameters": [tenant_parameter.clone()],
                    "responses": {
                        "200": {
                            "description": "Graph verification report.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/VerifyResponse" }
                                }
                            }
                        },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/context/pack": {
                "post": {
                    "tags": ["context"],
                    "summary": "Write a context pack",
                    "parameters": [tenant_parameter],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "type": "object", "additionalProperties": true }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/CommandResponse" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/metrics": {
                "get": {
                    "tags": ["operations"],
                    "summary": "Read operational metrics",
                    "responses": {
                        "200": { "description": "Prometheus-style metrics or operational metric text." },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" }
                    }
                }
            }
        },
        "components": {
            "securitySchemes": {
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "description": "Optional for private-network deployments. Required when RUSTY_RED_REQUIRE_AUTH=true."
                }
            },
            "responses": {
                "CommandResponse": {
                    "description": "THG-compatible command response.",
                    "content": {
                        "application/json": {
                            "schema": { "type": "object", "additionalProperties": true }
                        }
                    }
                },
                "GraphWriteResponse": {
                    "description": "Graph write acknowledgement.",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "required": ["ok"],
                                "properties": {
                                    "ok": { "type": "boolean" },
                                    "node": { "$ref": "#/components/schemas/GraphWriteResult" },
                                    "edge": { "$ref": "#/components/schemas/GraphWriteResult" }
                                },
                                "additionalProperties": false
                            }
                        }
                    }
                },
                "GraphStoreError": {
                    "description": "Graph store validation or integrity error.",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/ErrorResponse" }
                        }
                    }
                },
                "Unauthorized": {
                    "description": "Missing or invalid bearer token when auth is required."
                },
                "Forbidden": {
                    "description": "Bearer token lacks the required scope or the request origin is not allowed."
                },
                "StoreUnavailable": {
                    "description": "Redis-compatible backing store is unavailable.",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/ErrorResponse" }
                        }
                    }
                }
            },
            "schemas": {
                "HealthResponse": {
                    "type": "object",
                    "required": ["status"],
                    "properties": { "status": { "const": "ok" } },
                    "additionalProperties": false
                },
                "ReadyResponse": {
                    "type": "object",
                    "required": ["status", "store"],
                    "properties": {
                        "status": { "const": "ready" },
                        "store": { "const": "ready" }
                    },
                    "additionalProperties": false
                },
                "ErrorResponse": {
                    "type": "object",
                    "required": ["error", "message"],
                    "properties": {
                        "error": { "type": "string" },
                        "message": { "type": "string" }
                    },
                    "additionalProperties": false
                },
                "JsonRpcRequest": {
                    "type": "object",
                    "required": ["jsonrpc", "method"],
                    "properties": {
                        "jsonrpc": { "const": "2.0" },
                        "id": {},
                        "method": { "type": "string" },
                        "params": { "type": "object", "additionalProperties": true }
                    },
                    "additionalProperties": true
                },
                "JsonRpcResponse": {
                    "type": "object",
                    "required": ["jsonrpc"],
                    "properties": {
                        "jsonrpc": { "const": "2.0" },
                        "id": {},
                        "result": {},
                        "error": {}
                    },
                    "additionalProperties": true
                },
                "CommandRequest": {
                    "type": "object",
                    "required": ["command"],
                    "properties": {
                        "command": {
                            "type": "string",
                            "examples": ["THG.RUN.BEGIN", "THG.RUN.GET", "THG.CONTEXT.PACK", "THG.DEBUG.CYPHER"]
                        },
                        "args": {
                            "type": "object",
                            "additionalProperties": true,
                            "default": {}
                        }
                    },
                    "additionalProperties": false
                },
                "BatchRequest": {
                    "type": "object",
                    "properties": {
                        "commands": {
                            "type": "array",
                            "items": { "$ref": "#/components/schemas/CommandRequest" },
                            "default": []
                        }
                    },
                    "additionalProperties": false
                },
                "GraphQueryRequest": {
                    "type": "object",
                    "required": ["query"],
                    "properties": {
                        "query": { "type": "string" },
                        "graph": { "type": "object", "additionalProperties": true, "default": {} },
                        "params": { "type": "object", "additionalProperties": true, "default": {} }
                    },
                    "additionalProperties": false
                },
                "NodeWriteRequest": {
                    "type": "object",
                    "required": ["id"],
                    "properties": {
                        "id": { "type": "string" },
                        "labels": {
                            "type": "array",
                            "items": { "type": "string" },
                            "default": []
                        },
                        "properties": {
                            "type": "object",
                            "additionalProperties": true,
                            "default": {}
                        },
                        "tombstone": { "type": "boolean", "default": false }
                    },
                    "additionalProperties": false
                },
                "EdgeWriteRequest": {
                    "type": "object",
                    "required": ["id", "from_id", "to_id", "type"],
                    "properties": {
                        "id": { "type": "string" },
                        "from_id": { "type": "string" },
                        "to_id": { "type": "string" },
                        "type": { "type": "string" },
                        "properties": {
                            "type": "object",
                            "additionalProperties": true,
                            "default": {}
                        },
                        "tombstone": { "type": "boolean", "default": false }
                    },
                    "additionalProperties": false
                },
                "ScalarPropertyValue": {
                    "oneOf": [
                        { "type": "string" },
                        { "type": "number" },
                        { "type": "integer" },
                        { "type": "boolean" },
                        { "type": "null" }
                    ]
                },
                "NodeQuery": {
                    "type": "object",
                    "properties": {
                        "label": { "type": "string" },
                        "properties": {
                            "type": "object",
                            "additionalProperties": { "$ref": "#/components/schemas/ScalarPropertyValue" },
                            "default": {}
                        },
                        "limit": {
                            "type": "integer",
                            "minimum": 1,
                            "default": 100
                        }
                    },
                    "additionalProperties": false
                },
                "NeighborQuery": {
                    "type": "object",
                    "required": ["node_id", "direction"],
                    "properties": {
                        "node_id": { "type": "string" },
                        "direction": { "type": "string", "enum": ["out", "in"] },
                        "edge_type": { "type": "string" }
                    },
                    "additionalProperties": false
                },
                "NodeRecord": {
                    "type": "object",
                    "required": ["id", "labels", "properties", "version", "tombstone"],
                    "properties": {
                        "id": { "type": "string" },
                        "labels": {
                            "type": "array",
                            "items": { "type": "string" }
                        },
                        "properties": {
                            "type": "object",
                            "additionalProperties": true
                        },
                        "version": { "type": "integer", "minimum": 0 },
                        "tombstone": { "type": "boolean" }
                    },
                    "additionalProperties": false
                },
                "EdgeRecord": {
                    "type": "object",
                    "required": [
                        "id",
                        "from_id",
                        "to_id",
                        "type",
                        "properties",
                        "version",
                        "tombstone"
                    ],
                    "properties": {
                        "id": { "type": "string" },
                        "from_id": { "type": "string" },
                        "to_id": { "type": "string" },
                        "type": { "type": "string" },
                        "properties": {
                            "type": "object",
                            "additionalProperties": true
                        },
                        "version": { "type": "integer", "minimum": 0 },
                        "tombstone": { "type": "boolean" }
                    },
                    "additionalProperties": false
                },
                "GraphWriteResult": {
                    "type": "object",
                    "required": ["id", "version", "checksum"],
                    "properties": {
                        "id": { "type": "string" },
                        "version": { "type": "integer", "minimum": 0 },
                        "checksum": { "type": "string" }
                    },
                    "additionalProperties": false
                },
                "NodeResponse": {
                    "type": "object",
                    "required": ["ok", "node"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "node": { "$ref": "#/components/schemas/NodeRecord" }
                    },
                    "additionalProperties": false
                },
                "NodeQueryResponse": {
                    "type": "object",
                    "required": ["ok", "nodes"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "nodes": {
                            "type": "array",
                            "items": { "$ref": "#/components/schemas/NodeRecord" }
                        }
                    },
                    "additionalProperties": false
                },
                "EdgeResponse": {
                    "type": "object",
                    "required": ["ok", "edge"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "edge": { "$ref": "#/components/schemas/EdgeRecord" }
                    },
                    "additionalProperties": false
                },
                "NeighborHit": {
                    "type": "object",
                    "required": ["edge_id", "node_id", "type"],
                    "properties": {
                        "edge_id": { "type": "string" },
                        "node_id": { "type": "string" },
                        "type": { "type": "string" }
                    },
                    "additionalProperties": false
                },
                "NeighborResponse": {
                    "type": "object",
                    "required": ["ok", "neighbors"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "neighbors": {
                            "type": "array",
                            "items": { "$ref": "#/components/schemas/NeighborHit" }
                        }
                    },
                    "additionalProperties": false
                },
                "GraphStats": {
                    "type": "object",
                    "required": [
                        "version",
                        "nodes_total",
                        "edges_total",
                        "labels_total",
                        "edge_types_total",
                        "property_keys_total",
                        "property_indexes_total"
                    ],
                    "properties": {
                        "version": { "type": "integer", "minimum": 0 },
                        "nodes_total": { "type": "integer", "minimum": 0 },
                        "edges_total": { "type": "integer", "minimum": 0 },
                        "labels_total": { "type": "integer", "minimum": 0 },
                        "edge_types_total": { "type": "integer", "minimum": 0 },
                        "property_keys_total": { "type": "integer", "minimum": 0 },
                        "property_indexes_total": { "type": "integer", "minimum": 0 }
                    },
                    "additionalProperties": false
                },
                "GraphStatsResponse": {
                    "type": "object",
                    "required": ["ok", "stats"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "stats": { "$ref": "#/components/schemas/GraphStats" }
                    },
                    "additionalProperties": false
                },
                "VerifyProblem": {
                    "type": "object",
                    "required": ["kind", "id", "detail"],
                    "properties": {
                        "kind": { "type": "string" },
                        "id": { "type": "string" },
                        "detail": { "type": "string" }
                    },
                    "additionalProperties": false
                },
                "VerifyReport": {
                    "type": "object",
                    "required": ["ok", "stats", "problems"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "stats": { "$ref": "#/components/schemas/GraphStats" },
                        "problems": {
                            "type": "array",
                            "items": { "$ref": "#/components/schemas/VerifyProblem" }
                        }
                    },
                    "additionalProperties": false
                },
                "VerifyResponse": {
                    "type": "object",
                    "required": ["ok", "verify"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "verify": { "$ref": "#/components/schemas/VerifyReport" }
                    },
                    "additionalProperties": false
                }
            }
        }
    }))
}
