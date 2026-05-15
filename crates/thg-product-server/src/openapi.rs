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
                            "description": "Configured graph store is ready. In embedded mode this proves the RedCore data directory is writable and journalable; in redis mode it proves Redis is reachable.",
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
            "/v1/command": {
                "post": {
                    "tags": ["runs"],
                    "summary": "Execute a THG-compatible command using explicit or default tenant policy",
                    "description": "Product-facing alias for command execution. When tenant_id is omitted, the configured default tenant is used.",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/PublicCommandRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/CommandResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/batch": {
                "post": {
                    "tags": ["runs"],
                    "summary": "Execute multiple THG-compatible commands using explicit or default tenant policy",
                    "description": "Product-facing alias for batch execution. When tenant_id is omitted, the configured default tenant is used.",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/PublicBatchRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/CommandResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/query": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Run the first native Rusty Red query subset",
                    "description": "Supports the product-facing native query subset for `node_match` and `neighbors`. When tenant_id is omitted, the configured default tenant is used.",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/PublicQueryRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Bounded native-query response.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/PublicQueryResponse" }
                                }
                            }
                        },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/cypher": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Run the first read-only OpenCypher-compatible subset",
                    "description": "Supports the first bounded `/v1/cypher` subset: MATCH, RETURN, simple equality WHERE, LIMIT, and single-hop outgoing relationship expansion. When tenant_id is omitted, the configured default tenant is used.",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/CypherRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Cypher subset query result.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/CypherQueryResponse" }
                                }
                            }
                        },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/cypher/explain": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Explain the first read-only OpenCypher-compatible subset",
                    "description": "Parses the same bounded `/v1/cypher` subset and returns plan plus compatibility-matrix details without executing writes or unsupported operators.",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/CypherRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Cypher subset explain result.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/CypherExplainResponse" }
                                }
                            }
                        },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/cache/put": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Store a graph-version-guarded cache entry",
                    "description": "Stores a first-pass GraphCache entry for query results, plans, bounded subgraphs, neighbor expansions, context packs, retrieval plans, semantic answer candidates, or modal parse results. When tenant_id is omitted, the configured default tenant is used.",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/GraphCachePutRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Graph cache entry stored.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/GraphCachePutResponse" }
                                }
                            }
                        },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/cache/get": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Read a graph-version-guarded cache entry",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/GraphCacheLookupRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Graph cache lookup result with cached value when accepted.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/GraphCacheLookupResponse" }
                                }
                            }
                        },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/cache/check": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Check whether a graph-version-guarded cache entry is usable",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/GraphCacheLookupRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Graph cache check result with hit, stale, and guard information.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/GraphCacheLookupResponse" }
                                }
                            }
                        },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/cache/explain": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Explain a graph-version-guarded cache decision",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/GraphCacheLookupRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Graph cache explain result with guard decisions.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/GraphCacheLookupResponse" }
                                }
                            }
                        },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/cache/invalidate": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Invalidate graph cache entries by scope or stale state",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/GraphCacheInvalidateRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Graph cache invalidation result.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/GraphCacheInvalidateResponse" }
                                }
                            }
                        },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/cache/stats": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Read GraphCache counters and stale-entry totals",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/GraphCacheStatsRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Graph cache stats.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/GraphCacheStatsResponse" }
                                }
                            }
                        },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
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
                    "summary": "Run the legacy debug query bridge",
                    "description": "Executes the older THG.DEBUG.CYPHER compatibility command. Prefer the product-facing `/v1/query`, `/v1/cypher`, and `/v1/cypher/explain` routes for the current public query surface.",
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
                    "description": "Checks stored graph records against adjacency, label, edge-type, and exact scalar property indexes. This route reports drift without mutating indexes.",
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
            "/v1/tenants/{tenant_id}/graph/rebuild-indexes": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Rebuild graph indexes",
                    "description": "Repairs derived adjacency, label, edge-type, and exact scalar property indexes from canonical graph records. It does not repair corrupted canonical nodes or edges.",
                    "parameters": [tenant_parameter.clone()],
                    "responses": {
                        "200": {
                            "description": "Graph rebuild report.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/RebuildIndexesResponse" }
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
                    "description": "Configured graph store is unavailable or not writable.",
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
                    "required": ["status", "store", "mode", "durability", "strict_acid", "require_volume"],
                    "properties": {
                        "status": { "const": "ready" },
                        "store": { "const": "ready" },
                        "mode": { "enum": ["embedded", "memory", "redis"] },
                        "durability": { "type": "string" },
                        "strict_acid": { "type": "boolean" },
                        "require_volume": { "type": "boolean" },
                        "data_dir": { "type": ["string", "null"] }
                    },
                    "additionalProperties": false
                },
                "ErrorResponse": {
                    "type": "object",
                    "required": ["error", "message"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "error": { "type": "string" },
                        "code": { "type": "string" },
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
                "PublicCommandRequest": {
                    "type": "object",
                    "required": ["command"],
                    "properties": {
                        "tenant_id": { "type": "string" },
                        "command": {
                            "type": "string",
                            "examples": ["THG.RUN.BEGIN", "THG.RUN.GET", "THG.CONTEXT.PACK", "THG.GRAPH.STATS"]
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
                "PublicBatchRequest": {
                    "type": "object",
                    "properties": {
                        "tenant_id": { "type": "string" },
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
                "PublicQueryRequest": {
                    "type": "object",
                    "properties": {
                        "tenant_id": { "type": "string" },
                        "operation": {
                            "type": "string",
                            "enum": ["node_match", "neighbors"]
                        },
                        "label": { "type": "string" },
                        "properties": {
                            "type": "object",
                            "additionalProperties": { "$ref": "#/components/schemas/ScalarPropertyValue" },
                            "default": {}
                        },
                        "node_id": { "type": "string" },
                        "direction": { "type": "string", "enum": ["out", "in"] },
                        "edge_type": { "type": "string" },
                        "limit": {
                            "type": "integer",
                            "minimum": 1,
                            "default": 100
                        }
                    },
                    "additionalProperties": false
                },
                "CypherRequest": {
                    "type": "object",
                    "required": ["query"],
                    "properties": {
                        "tenant_id": { "type": "string" },
                        "query": { "type": "string" },
                        "params": {
                            "type": "object",
                            "additionalProperties": { "$ref": "#/components/schemas/ScalarPropertyValue" },
                            "default": {}
                        }
                    },
                    "additionalProperties": false
                },
                "GraphCacheLookupRequest": {
                    "type": "object",
                    "required": ["kind", "key"],
                    "properties": {
                        "tenant_id": { "type": "string" },
                        "kind": {
                            "type": "string",
                            "enum": [
                                "query_result",
                                "query_plan",
                                "bounded_subgraph",
                                "neighbor_expansion",
                                "context_pack",
                                "retrieval_plan",
                                "semantic_answer_candidate",
                                "modal_parse_result"
                            ]
                        },
                        "key": {},
                        "index_manifest_hash": { "type": "string" },
                        "auth_scope_hash": { "type": "string" },
                        "retrieval_policy_hash": { "type": "string" },
                        "model_version": { "type": "string" },
                        "source_hashes": {
                            "type": "array",
                            "items": { "type": "string" },
                            "default": []
                        }
                    },
                    "additionalProperties": false
                },
                "GraphCachePutRequest": {
                    "type": "object",
                    "required": ["kind", "key", "value"],
                    "properties": {
                        "tenant_id": { "type": "string" },
                        "kind": {
                            "type": "string",
                            "enum": [
                                "query_result",
                                "query_plan",
                                "bounded_subgraph",
                                "neighbor_expansion",
                                "context_pack",
                                "retrieval_plan",
                                "semantic_answer_candidate",
                                "modal_parse_result"
                            ]
                        },
                        "key": {},
                        "value": {},
                        "metadata": {
                            "type": "object",
                            "additionalProperties": true,
                            "default": {}
                        },
                        "index_manifest_hash": { "type": "string" },
                        "auth_scope_hash": { "type": "string" },
                        "retrieval_policy_hash": { "type": "string" },
                        "model_version": { "type": "string" },
                        "source_hashes": {
                            "type": "array",
                            "items": { "type": "string" },
                            "default": []
                        }
                    },
                    "additionalProperties": false
                },
                "GraphCacheInvalidateRequest": {
                    "type": "object",
                    "properties": {
                        "tenant_id": { "type": "string" },
                        "all": { "type": "boolean", "default": false },
                        "stale_only": { "type": "boolean", "default": false },
                        "kind": { "type": "string" },
                        "key": {},
                        "index_manifest_hash": { "type": "string" },
                        "auth_scope_hash": { "type": "string" },
                        "retrieval_policy_hash": { "type": "string" },
                        "model_version": { "type": "string" },
                        "source_hashes": {
                            "type": "array",
                            "items": { "type": "string" },
                            "default": []
                        }
                    },
                    "additionalProperties": false
                },
                "GraphCacheStatsRequest": {
                    "type": "object",
                    "properties": {
                        "tenant_id": { "type": "string" }
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
                "CompatibilityMatrix": {
                    "type": "object",
                    "required": ["version", "supported", "rejected", "pending"],
                    "properties": {
                        "version": { "type": "string" },
                        "supported": {
                            "type": "array",
                            "items": { "type": "string" }
                        },
                        "rejected": {
                            "type": "array",
                            "items": { "type": "string" }
                        },
                        "pending": {
                            "type": "array",
                            "items": { "type": "string" }
                        }
                    },
                    "additionalProperties": false
                },
                "PublicQueryResponse": {
                    "type": "object",
                    "required": ["ok", "tenant", "operation", "stats", "explain"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "operation": { "type": "string" },
                        "nodes": {
                            "type": "array",
                            "items": { "$ref": "#/components/schemas/NodeRecord" }
                        },
                        "neighbors": {
                            "type": "array",
                            "items": { "$ref": "#/components/schemas/NeighborHit" }
                        },
                        "stats": { "type": "object", "additionalProperties": true },
                        "explain": { "type": "object", "additionalProperties": true }
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
                },
                "RebuildIndexesReport": {
                    "type": "object",
                    "required": ["repaired", "before", "after"],
                    "properties": {
                        "repaired": { "type": "boolean" },
                        "before": { "$ref": "#/components/schemas/VerifyReport" },
                        "after": { "$ref": "#/components/schemas/VerifyReport" }
                    },
                    "additionalProperties": false
                },
                "RebuildIndexesResponse": {
                    "type": "object",
                    "required": ["ok", "rebuild"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "rebuild": { "$ref": "#/components/schemas/RebuildIndexesReport" }
                    },
                    "additionalProperties": false
                },
                "GraphCachePutResult": {
                    "type": "object",
                    "required": ["stored", "kind", "fingerprint", "cache_key", "graph_version", "stored_at_ms"],
                    "properties": {
                        "stored": { "type": "boolean" },
                        "kind": { "type": "string" },
                        "fingerprint": { "type": "string" },
                        "cache_key": { "type": "string" },
                        "graph_version": { "type": "integer", "minimum": 0 },
                        "stored_at_ms": { "type": "integer", "minimum": 0 }
                    },
                    "additionalProperties": false
                },
                "GraphCacheLookupResult": {
                    "type": "object",
                    "required": ["hit", "accepted", "stale", "kind", "fingerprint", "cache_key", "reason", "graph_version", "guards"],
                    "properties": {
                        "hit": { "type": "boolean" },
                        "accepted": { "type": "boolean" },
                        "stale": { "type": "boolean" },
                        "kind": { "type": "string" },
                        "fingerprint": { "type": "string" },
                        "cache_key": { "type": "string" },
                        "reason": { "type": "string" },
                        "graph_version": { "type": "integer", "minimum": 0 },
                        "entry_graph_version": { "type": "integer", "minimum": 0 },
                        "entry_cache_key": { "type": "string" },
                        "stored_at_ms": { "type": "integer", "minimum": 0 },
                        "value": {},
                        "metadata": {},
                        "guards": {
                            "type": "object",
                            "additionalProperties": { "type": "string" }
                        }
                    },
                    "additionalProperties": false
                },
                "GraphCacheInvalidateResult": {
                    "type": "object",
                    "required": ["removed", "remaining", "stale_only"],
                    "properties": {
                        "removed": { "type": "integer", "minimum": 0 },
                        "remaining": { "type": "integer", "minimum": 0 },
                        "stale_only": { "type": "boolean" }
                    },
                    "additionalProperties": false
                },
                "GraphCacheStats": {
                    "type": "object",
                    "required": ["graph_version", "entries_total", "stale_entries", "puts", "hits", "misses", "stale_hits", "invalidations", "entries_by_kind"],
                    "properties": {
                        "graph_version": { "type": "integer", "minimum": 0 },
                        "entries_total": { "type": "integer", "minimum": 0 },
                        "stale_entries": { "type": "integer", "minimum": 0 },
                        "puts": { "type": "integer", "minimum": 0 },
                        "hits": { "type": "integer", "minimum": 0 },
                        "misses": { "type": "integer", "minimum": 0 },
                        "stale_hits": { "type": "integer", "minimum": 0 },
                        "invalidations": { "type": "integer", "minimum": 0 },
                        "entries_by_kind": {
                            "type": "object",
                            "additionalProperties": { "type": "integer", "minimum": 0 }
                        }
                    },
                    "additionalProperties": false
                },
                "GraphCachePutResponse": {
                    "type": "object",
                    "required": ["ok", "tenant", "cache"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "cache": { "$ref": "#/components/schemas/GraphCachePutResult" }
                    },
                    "additionalProperties": false
                },
                "GraphCacheLookupResponse": {
                    "type": "object",
                    "required": ["ok", "tenant", "cache"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "cache": { "$ref": "#/components/schemas/GraphCacheLookupResult" }
                    },
                    "additionalProperties": false
                },
                "GraphCacheInvalidateResponse": {
                    "type": "object",
                    "required": ["ok", "tenant", "cache"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "cache": { "$ref": "#/components/schemas/GraphCacheInvalidateResult" }
                    },
                    "additionalProperties": false
                },
                "GraphCacheStatsResponse": {
                    "type": "object",
                    "required": ["ok", "tenant", "cache"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "cache": { "$ref": "#/components/schemas/GraphCacheStats" }
                    },
                    "additionalProperties": false
                },
                "CypherQueryResponse": {
                    "type": "object",
                    "required": ["ok", "tenant", "query", "subset", "rows", "row_count", "stats", "explain"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "query": { "type": "string" },
                        "subset": { "type": "string" },
                        "rows": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "additionalProperties": true
                            }
                        },
                        "row_count": { "type": "integer", "minimum": 0 },
                        "stats": { "type": "object", "additionalProperties": true },
                        "explain": { "$ref": "#/components/schemas/CypherExplainResponse" }
                    },
                    "additionalProperties": false
                },
                "CypherExplainResponse": {
                    "type": "object",
                    "required": ["ok", "tenant", "query", "subset", "plan", "compatibility"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "query": { "type": "string" },
                        "subset": { "type": "string" },
                        "pattern": { "type": "string" },
                        "plan": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "additionalProperties": true
                            }
                        },
                        "compatibility": { "$ref": "#/components/schemas/CompatibilityMatrix" }
                    },
                    "additionalProperties": false
                }
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use axum::{extract::State, Json};
    use thg_core::RedCoreDurability;

    use super::openapi;
    use crate::{
        config::{Config, StorageMode},
        state::AppState,
    };

    #[tokio::test]
    async fn openapi_lists_rebuild_indexes_and_product_query_routes() {
        let state = AppState::new(Config {
            host: "127.0.0.1".to_string(),
            port: 8380,
            storage_mode: StorageMode::Memory,
            data_dir: "data/rusty-red".to_string(),
            require_volume: false,
            volume_available: false,
            durability: RedCoreDurability::None,
            snapshot_interval_writes: 0,
            strict_acid: false,
            concurrency: "single_writer".to_string(),
            txn_isolation: "snapshot".to_string(),
            redis_url: "not-a-redis-url".to_string(),
            redis_key_prefix: "rusty-red".to_string(),
            require_auth: false,
            allowed_origins: Vec::new(),
            api_tokens: Vec::new(),
            service_name: "rusty-red".to_string(),
            api_title: "Rusty Red".to_string(),
            public_url: None,
            mcp_enabled: true,
            mcp_read_only: true,
            mcp_allow_admin: false,
            mcp_default_tenant: "default".to_string(),
        });

        let Json(document) = openapi(State(state)).await;

        assert!(document
            .pointer("/paths/~1v1~1tenants~1{tenant_id}~1graph~1rebuild-indexes/post")
            .is_some());
        assert!(document.pointer("/paths/~1v1~1query/post").is_some());
        assert!(document.pointer("/paths/~1v1~1cypher/post").is_some());
        assert!(document
            .pointer("/paths/~1v1~1cypher~1explain/post")
            .is_some());
        assert!(document.pointer("/paths/~1v1~1cache~1check/post").is_some());
        assert!(document.pointer("/paths/~1v1~1cache~1put/post").is_some());
        assert_eq!(
            document.pointer("/components/schemas/RebuildIndexesResponse/properties/rebuild/$ref"),
            Some(&serde_json::Value::String(
                "#/components/schemas/RebuildIndexesReport".to_string()
            ))
        );
        assert_eq!(
            document
                .pointer("/components/schemas/CypherExplainResponse/properties/compatibility/$ref"),
            Some(&serde_json::Value::String(
                "#/components/schemas/CompatibilityMatrix".to_string()
            ))
        );
        assert_eq!(
            document.pointer("/components/schemas/GraphCacheLookupResponse/properties/cache/$ref"),
            Some(&serde_json::Value::String(
                "#/components/schemas/GraphCacheLookupResult".to_string()
            ))
        );
    }
}
