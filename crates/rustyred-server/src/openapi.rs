use axum::{extract::State, Json};
use serde_json::{json, Value};

use crate::state::AppState;

const API_VERSION: &str = env!("CARGO_PKG_VERSION");

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
            "version": API_VERSION,
            "description": "Rusty Red Graph Database HTTP API. This document describes the graph/run/context HTTP surface and the MCP transport endpoint; it is not a RedisGraph, FalkorDB, or raw Redis protocol specification."
        },
        "tags": [
            { "name": "operations", "description": "Health, readiness, metrics, and discovery." },
            { "name": "mcp", "description": "Streamable HTTP MCP agent port over Rusty Red graph APIs." },
            { "name": "runs", "description": "Rusty Red compatibility command runtime." },
            { "name": "graph", "description": "First-class graph node, edge, adjacency, index, and verification routes." },
            { "name": "search", "description": "Standalone RustyWeb crawl and graph-native search routes." },
            { "name": "federation", "description": "Signed RustyWeb Web Commons fragment verification, trust gating, and merge receipts." },
            { "name": "instant-kg", "description": "Harness Instant KG merged base+session-delta code graph queries." },
            { "name": "transactions", "description": "Open and commit staged transaction workflows for Cypher writes." },
            { "name": "context", "description": "Context pack writes for graph-backed agent workflows." }
        ],
        "security": [{ "bearerAuth": [] }],
        "paths": {
            "/": {
                "get": {
                    "tags": ["search"],
                    "summary": "Render the graph-native search home page",
                    "responses": {
                        "200": {
                            "description": "Self-contained RustyWeb SERP HTML for the default tenant.",
                            "content": { "text/html": { "schema": { "type": "string" } } }
                        },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" }
                    }
                }
            },
            "/search": {
                "get": {
                    "tags": ["search"],
                    "summary": "Render graph-native search results",
                    "parameters": [
                        { "name": "q", "in": "query", "schema": { "type": "string" } },
                        { "name": "tenant", "in": "query", "schema": { "type": "string" } }
                    ],
                    "responses": {
                        "200": {
                            "description": "Self-contained RustyWeb SERP HTML.",
                            "content": { "text/html": { "schema": { "type": "string" } } }
                        },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" }
                    }
                }
            },
            "/search.json": {
                "get": {
                    "tags": ["search"],
                    "summary": "Return graph-native search results as JSON",
                    "parameters": [
                        { "name": "q", "in": "query", "schema": { "type": "string" } },
                        { "name": "tenant", "in": "query", "schema": { "type": "string" } }
                    ],
                    "responses": {
                        "200": {
                            "description": "Substrate search payload with hits and LINKS_TO edges.",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "ok": { "type": "boolean" },
                                            "tenant": { "type": "string" },
                                            "search": { "type": "object", "additionalProperties": true }
                                        },
                                        "required": ["ok", "tenant", "search"]
                                    }
                                }
                            }
                        },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" }
                    }
                }
            },
            "/crawl": {
                "post": {
                    "tags": ["search"],
                    "summary": "Run a bounded RustyWeb frontier crawl into the tenant graph",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": {
                                    "type": "object",
                                    "properties": {
                                        "tenant": { "type": "string" },
                                        "run_id": { "type": "string" },
                                        "seeds": { "type": "array", "items": { "type": "string", "format": "uri" } },
                                        "budget": { "type": "object", "additionalProperties": true },
                                        "scope": { "type": "object", "additionalProperties": true }
                                    },
                                    "required": ["seeds"]
                                }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Crawl receipt and graph transaction.",
                            "content": { "application/json": { "schema": { "type": "object", "additionalProperties": true } } }
                        },
                        "400": { "$ref": "#/components/responses/BadRequest" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" }
                    }
                }
            },
            "/federate/submit": {
                "post": {
                    "tags": ["federation"],
                    "summary": "Submit a signed RustyWeb Web Commons fragment",
                    "description": "Verifies Ed25519 signatures, applies Web Commons trust gates, merges accepted pages and links into the tenant graph, and returns a per-page receipt. Receipt/hash-only payloads are compatibility no-ops.",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": {
                                    "type": "object",
                                    "properties": {
                                        "tenant": { "type": "string" },
                                        "federable": { "type": "boolean" },
                                        "graph_delta_hash": { "type": "string" },
                                        "receipt": { "type": "object", "additionalProperties": true },
                                        "fragment": { "type": "object", "additionalProperties": true }
                                    }
                                }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Federation fragment verified and merged, or compatibility receipt validated as a no-op.",
                            "content": { "application/json": { "schema": { "type": "object", "additionalProperties": true } } }
                        },
                        "400": { "$ref": "#/components/responses/BadRequest" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" }
                    }
                }
            },
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
            "/.well-known/mcp/rustyred.json": {
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
            "/metrics": {
                "get": {
                    "tags": ["operations"],
                    "summary": "Prometheus metrics",
                    "description": "Admin-scoped Prometheus 0.0.4 text exposition for request counters, query latency percentiles, and graph/runtime activity.",
                    "responses": {
                        "200": {
                            "description": "Prometheus text exposition.",
                            "content": {
                                "text/plain": {
                                    "schema": { "type": "string" }
                                }
                            }
                        },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" }
                    }
                }
            },
            "/v1/diagnostics/slow_queries": {
                "get": {
                    "tags": ["operations"],
                    "summary": "Slow-query diagnostics",
                    "description": "Admin-scoped ring buffer of slow queries plus execution detail such as query kind, elapsed nanoseconds, and touched graph counts.",
                    "responses": {
                        "200": {
                            "description": "Slow-query snapshot.",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "entries": {
                                                "type": "array",
                                                "items": {
                                                    "type": "object",
                                                    "properties": {
                                                        "recorded_at_unix_ms": { "type": "string" },
                                                        "nanos": { "type": "integer" },
                                                        "kind": { "type": "string" },
                                                        "detail": { "type": "string" },
                                                        "nodes_visited": { "type": "integer" },
                                                        "edges_touched": { "type": "integer" }
                                                    }
                                                }
                                            },
                                            "count": { "type": "integer" }
                                        }
                                    }
                                }
                            }
                        },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" }
                    }
                }
            },
            "/v1/diagnostics/config": {
                "get": {
                    "tags": ["operations"],
                    "summary": "Static runtime configuration snapshot",
                    "description": "Admin-scoped runtime config, including slow-query settings and startup-only tenant override details. Runtime mutation of tenant config is not supported in this slice.",
                    "responses": {
                        "200": {
                            "description": "Static config snapshot.",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "service": { "type": "string" },
                                            "status": { "type": "string" },
                                            "auth_required": { "type": "boolean" },
                                            "configured_origins": { "type": "integer" },
                                            "storage_mode": { "type": "string" },
                                            "durability": { "type": "string" },
                                            "strict_acid": { "type": "boolean" },
                                            "tenant_memory_quota_bytes": { "type": "integer" },
                                            "tenant_memory_quota_supported": { "type": "boolean" },
                                            "tenant_memory_quota_enforced": { "type": "boolean" },
                                            "slow_query_threshold_nanos": { "type": "integer" },
                                            "slow_query_capacity": { "type": "integer" },
                                            "slow_query_log_enabled": { "type": "boolean" },
                                            "tenant_config_overrides": { "type": "integer" },
                                            "tenant_config_runtime_mutation_supported": { "type": "boolean" },
                                            "tenant_config_tenants": {
                                                "type": "array",
                                                "items": { "type": "string" }
                                            },
                                            "tenant_config_overrides_detail": {
                                                "type": "object",
                                                "additionalProperties": { "type": "object" }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" }
                    }
                }
            },
            "/v1/command": {
                "post": {
                    "tags": ["runs"],
                    "summary": "Execute a compatibility command using explicit or default tenant policy",
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
                    "summary": "Execute multiple compatibility commands using explicit or default tenant policy",
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
                    "summary": "Run the first OpenCypher-compatible subset",
                    "description": "Supports the bounded `/v1/cypher` subset: MATCH, RETURN, simple equality WHERE, LIMIT, outgoing multi-hop chains, bounded variable-length expansion, path aliases, and CREATE/MERGE/SET/DELETE write clauses. When tenant_id is omitted, the configured default tenant is used. If tx_id is provided, matching write statements are staged into that open transaction.",
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
                                    "schema": {
                                        "oneOf": [
                                            { "$ref": "#/components/schemas/CypherQueryResponse" },
                                            { "$ref": "#/components/schemas/CypherWriteStagingResponse" }
                                        ]
                                    }
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
            "/v1/transactions/begin": {
                "post": {
                    "tags": ["transactions"],
                    "summary": "Begin a transaction for staging Cypher writes",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/TransactionBeginRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Transaction started and assigned a tx_id.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/TransactionBeginResponse" }
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
            "/v1/transactions/commit": {
                "post": {
                    "tags": ["transactions"],
                    "summary": "Commit a staged Cypher transaction",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/TransactionMutationRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Transaction committed and mutations applied.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/TransactionCommitResponse" }
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
            "/v1/transactions/rollback": {
                "post": {
                    "tags": ["transactions"],
                    "summary": "Rollback a staged transaction",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/TransactionMutationRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Transaction rolled back and discarded.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/TransactionRollbackResponse" }
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
                    "summary": "Explain the bounded OpenCypher-compatible subset",
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
                    "summary": "Execute a compatibility command",
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
                    "summary": "Execute multiple compatibility commands",
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
                    "description": "Executes the older RUSTYRED.DEBUG.CYPHER compatibility command. Prefer the product-facing `/v1/query`, `/v1/cypher`, and `/v1/cypher/explain` routes for the current public query surface.",
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
            "/v1/tenants/{tenant_id}/graph/version/compile": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Compile a versioned graph pack",
                    "description": "Builds a public RustyRed content-addressed graph pack from the current tenant graph. The pack contains Git-like commit metadata, a Prolly-style tree, declarative compiler capabilities, and optionally the graph record payloads.",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/GraphVersionCompileRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Compiled graph pack.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/GraphVersionCompileResponse" }
                                }
                            }
                        },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/version/diff": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Diff graph snapshots by content hash",
                    "description": "Compares a base graph snapshot with the current tenant graph, or with an explicit target snapshot, using RustyRed content hashes and Prolly tree roots.",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/GraphVersionDiffRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Graph version diff.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/GraphVersionDiffResponse" }
                                }
                            }
                        },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/version/ref": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Update a graph version branch ref",
                    "description": "Compiles the current tenant graph and updates a branch ref in a caller-supplied graph version repository value. The repository is returned for caller-side persistence.",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/GraphVersionRefRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Updated graph version repository and ref.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/GraphVersionRefResponse" }
                                }
                            }
                        },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/version/log": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Read graph version commit log",
                    "description": "Walks commit history from a branch name or commit hash in a caller-supplied graph version repository value.",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/GraphVersionLogRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Graph version log.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/GraphVersionLogResponse" }
                                }
                            }
                        },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/version/checkout": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Checkout a graph version snapshot",
                    "description": "Reconstructs a graph snapshot from a branch name or commit hash in a caller-supplied graph version repository value. This route is read-only and does not mutate the tenant graph.",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/GraphVersionCheckoutRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Checked-out graph snapshot.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/GraphVersionCheckoutResponse" }
                                }
                            }
                        },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/version/merge": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Three-way merge graph snapshots",
                    "description": "Merges base/ours/theirs graph snapshots by content hash. Non-overlapping changes resolve automatically; edge conflicts can resolve by confidence; unresolved conflicts are returned explicitly without mutating the tenant graph.",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/GraphVersionMergeRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Graph version merge result.",
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/GraphVersionMergeResponse" }
                                }
                            }
                        },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/vector/designate": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Designate a vector property",
                    "description": "Registers a node label/property pair as a fixed-dimension HNSW vector field. Existing matching nodes are indexed during designation; later node upserts refresh the index.",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/VectorDesignateRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/DesignationResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/vector/search": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Search vector indexes",
                    "description": "Runs an HNSW nearest-neighbor search over the designated vector property. Results include distance and the current node record when available.",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/VectorSearchRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/SearchResultsResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/vector/hybrid": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Search vector indexes with graph proximity scoring",
                    "description": "Blends vector similarity with graph-distance scores from explicit graph seeds. Per-request scoring overrides can adjust alpha, confidence weighting, and edge-type weights.",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/HybridSearchRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/HybridResultsResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/epistemic-neighbors": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Traverse confidence-weighted epistemic edges",
                    "description": "Returns neighboring graph nodes reachable through optional epistemic edge-type and confidence filters.",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/EpistemicNeighborsRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/EpistemicNeighborsResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/algorithms/ppr": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Run Personalized PageRank",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/AlgorithmPprRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/AlgorithmScoresResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/algorithms/components": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Compute connected components",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/AlgorithmComponentsRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/ComponentsResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/algorithms/pagerank": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Run global PageRank",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/AlgorithmPageRankRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/AlgorithmScoresResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/algorithms/communities": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Detect label-propagation communities",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/AlgorithmCommunitiesRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/CommunitiesResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/spatial/designate": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Designate a spatial index",
                    "description": "Registers latitude/longitude node properties for the configured spatial backend. Upserts of matching nodes refresh the index.",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/SpatialDesignateRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/SpatialDesignationResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/spatial/radius": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Run a spatial radius query",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/SpatialRadiusRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/SpatialIdsResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/spatial/bbox": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Run a spatial bounding-box query",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/SpatialBboxRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/SpatialIdsResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/spatial/designate_geometry": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Designate a geometry index",
                    "description": "Registers a node label and property holding geometry (point, WKB, or WKT) for the S2-cover geometry index. Served by the spatial plugin-operation route. Upserts of matching nodes refresh the cover.",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/SpatialGeometryDesignateRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/GeometryDesignationResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/spatial/within": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Run a geometry within query",
                    "description": "Returns node ids whose designated geometry lies within the query geometry. Two-phase: S2 cover broad phase, then an exact geo predicate narrow phase.",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/SpatialGeometryQueryRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/GeometryIdsResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/spatial/intersects": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Run a geometry intersects query",
                    "description": "Returns node ids whose designated geometry intersects the query geometry. Two-phase: S2 cover broad phase, then an exact geo predicate narrow phase.",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/SpatialGeometryQueryRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/GeometryIdsResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/spatial/contains": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Run a geometry contains-point query",
                    "description": "Returns node ids whose designated polygon or multipolygon geometry contains the given lat/lon point. Two-phase: S2 cell lookup broad phase, then an exact geo contains narrow phase.",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/SpatialContainsRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/GeometryIdsResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/fulltext/designate": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Designate a full-text index",
                    "description": "Registers a node label/property pair for the configured full-text backend and indexes existing matching nodes.",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/FullTextDesignateRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/DesignationResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/fulltext/search": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Search a full-text index",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/FullTextSearchRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/SearchResultsResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/bulk/nodes": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Bulk load graph nodes",
                    "description": "Accepts newline-delimited JSON node records or CSV. CSV uses the first row as headers unless the `headers` query parameter is provided. Mutations flush in batches and return line-level errors.",
                    "parameters": [
                        tenant_parameter.clone(),
                        { "name": "batch_size", "in": "query", "schema": { "type": "integer", "minimum": 1, "default": 500 } },
                        { "name": "headers", "in": "query", "schema": { "type": "string" }, "description": "Optional comma-separated CSV headers." }
                    ],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/jsonl": { "schema": { "type": "string" } },
                            "text/csv": { "schema": { "type": "string" } }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/BulkIngestResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/graph/bulk/edges": {
                "post": {
                    "tags": ["graph"],
                    "summary": "Bulk load graph edges",
                    "description": "Accepts newline-delimited JSON edge records or CSV. CSV uses `from_id` and `to_id` columns by default; override those names with `from_col` and `to_col`.",
                    "parameters": [
                        tenant_parameter.clone(),
                        { "name": "batch_size", "in": "query", "schema": { "type": "integer", "minimum": 1, "default": 500 } },
                        { "name": "headers", "in": "query", "schema": { "type": "string" }, "description": "Optional comma-separated CSV headers." },
                        { "name": "from_col", "in": "query", "schema": { "type": "string", "default": "from_id" } },
                        { "name": "to_col", "in": "query", "schema": { "type": "string", "default": "to_id" } }
                    ],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/jsonl": { "schema": { "type": "string" } },
                            "text/csv": { "schema": { "type": "string" } }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/BulkIngestResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/instant-kg/status": {
                "post": {
                    "tags": ["instant-kg"],
                    "summary": "Inspect Harness Instant KG merged-view status",
                    "description": "Uses the tenant's committed graph as the immutable base artifact and overlays an optional session delta without mutating stored graph state.",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": false,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/InstantKgViewRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/InstantKgResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/instant-kg/ppr": {
                "post": {
                    "tags": ["instant-kg"],
                    "summary": "Run PPR over a Harness Instant KG merged view",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/InstantKgPprRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/InstantKgResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/instant-kg/impact": {
                "post": {
                    "tags": ["instant-kg"],
                    "summary": "Compute code-object impact over a merged Instant KG view",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/InstantKgImpactRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/InstantKgResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/instant-kg/related-objects": {
                "post": {
                    "tags": ["instant-kg"],
                    "summary": "Find related code objects over a merged Instant KG view",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/InstantKgRelatedRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/InstantKgResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/instant-kg/search": {
                "post": {
                    "tags": ["instant-kg"],
                    "summary": "Search code objects over a merged Instant KG view",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/InstantKgSearchRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/InstantKgResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
                        "401": { "$ref": "#/components/responses/Unauthorized" },
                        "403": { "$ref": "#/components/responses/Forbidden" },
                        "503": { "$ref": "#/components/responses/StoreUnavailable" }
                    }
                }
            },
            "/v1/tenants/{tenant_id}/instant-kg/explain-edge": {
                "post": {
                    "tags": ["instant-kg"],
                    "summary": "Explain edge evidence over a merged Instant KG view",
                    "parameters": [tenant_parameter.clone()],
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/InstantKgExplainEdgeRequest" }
                            }
                        }
                    },
                    "responses": {
                        "200": { "$ref": "#/components/responses/InstantKgResponse" },
                        "400": { "$ref": "#/components/responses/GraphStoreError" },
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
                    "description": "Rusty Red compatibility command response.",
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
                "DesignationResponse": {
                    "description": "Index designation acknowledgement.",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/DesignationResponseBody" }
                        }
                    }
                },
                "SpatialDesignationResponse": {
                    "description": "Spatial index designation acknowledgement.",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/SpatialDesignationResponseBody" }
                        }
                    }
                },
                "SearchResultsResponse": {
                    "description": "Search result list.",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/SearchResultsResponseBody" }
                        }
                    }
                },
                "HybridResultsResponse": {
                    "description": "Hybrid vector/graph search result list.",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/HybridResultsResponseBody" }
                        }
                    }
                },
                "EpistemicNeighborsResponse": {
                    "description": "Epistemic neighbor traversal result.",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/EpistemicNeighborsResponseBody" }
                        }
                    }
                },
                "AlgorithmScoresResponse": {
                    "description": "Node-score algorithm result.",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/AlgorithmScoresResponseBody" }
                        }
                    }
                },
                "ComponentsResponse": {
                    "description": "Connected-components result.",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/ComponentsResponseBody" }
                        }
                    }
                },
                "CommunitiesResponse": {
                    "description": "Community detection result.",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/CommunitiesResponseBody" }
                        }
                    }
                },
                "SpatialIdsResponse": {
                    "description": "Spatial query result.",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/SpatialIdsResponseBody" }
                        }
                    }
                },
                "GeometryIdsResponse": {
                    "description": "Geometry topology query result (within, intersects, contains).",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/GeometryIdsResponseBody" }
                        }
                    }
                },
                "GeometryDesignationResponse": {
                    "description": "Geometry index designation acknowledgement.",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/GeometryDesignationResponseBody" }
                        }
                    }
                },
                "BulkIngestResponse": {
                    "description": "Bulk ingest report.",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/BulkIngestResponseBody" }
                        }
                    }
                },
                "InstantKgResponse": {
                    "description": "Harness Instant KG merged-view response.",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/InstantKgResponseBody" }
                        }
                    }
                },
                "Unauthorized": {
                    "description": "Missing or invalid bearer token when auth is required."
                },
                "Forbidden": {
                    "description": "Bearer token lacks the required scope or the request origin is not allowed."
                },
                "BadRequest": {
                    "description": "The request is malformed or rejected by route-specific validation.",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/ErrorResponse" }
                        }
                    }
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
                            "examples": ["RUSTYRED.RUN.BEGIN", "RUSTYRED.RUN.GET", "RUSTYRED.CONTEXT.PACK", "RUSTYRED.DEBUG.CYPHER"]
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
                            "examples": ["RUSTYRED.RUN.BEGIN", "RUSTYRED.RUN.GET", "RUSTYRED.CONTEXT.PACK", "RUSTYRED.GRAPH.STATS"]
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
                        "tx_id": { "type": "string" },
                        "params": {
                            "type": "object",
                            "additionalProperties": { "$ref": "#/components/schemas/ScalarPropertyValue" },
                            "default": {}
                        }
                    },
                    "additionalProperties": false
                },
                "TransactionBeginRequest": {
                    "type": "object",
                    "properties": {
                        "tenant_id": { "type": "string" }
                    },
                    "additionalProperties": false
                },
                "TransactionMutationRequest": {
                    "type": "object",
                    "required": ["tx_id"],
                    "properties": {
                        "tx_id": { "type": "string" },
                        "tenant_id": { "type": "string" }
                    },
                    "additionalProperties": false
                },
                "TransactionBeginResponse": {
                    "type": "object",
                    "required": ["ok", "tenant", "tx_id"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "tx_id": { "type": "string" }
                    },
                    "additionalProperties": false
                },
                "TransactionCommitResponse": {
                    "type": "object",
                    "required": ["ok", "tenant", "transaction"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "transaction": {
                            "type": "object",
                            "additionalProperties": true
                        }
                    },
                    "additionalProperties": false
                },
                "TransactionRollbackResponse": {
                    "type": "object",
                    "required": ["ok", "tenant", "tx_id", "status"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "tx_id": { "type": "string" },
                        "status": { "type": "string" }
                    },
                    "additionalProperties": false
                },
                "CypherWriteStagingResponse": {
                    "type": "object",
                    "required": ["ok", "tenant", "query", "tx_id", "subset", "staged_mutations"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "query": { "type": "string" },
                        "tx_id": { "type": "string" },
                        "subset": { "type": "string" },
                        "staged_mutations": { "type": "integer", "minimum": 0 }
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
                                "modal_parse_result",
                                "vector_search_result",
                                "epistemic_traversal"
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
                                "modal_parse_result",
                                "vector_search_result",
                                "epistemic_traversal"
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
                        "tombstone": { "type": "boolean" },
                        "content_hash": { "type": "string" },
                        "parent_hashes": {
                            "type": "array",
                            "items": { "type": "string" }
                        }
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
                        "tombstone": { "type": "boolean" },
                        "confidence": { "type": "number" },
                        "epistemic_type": { "type": "string" },
                        "provenance": {
                            "type": "object",
                            "additionalProperties": true
                        },
                        "content_hash": { "type": "string" },
                        "parent_hashes": {
                            "type": "array",
                            "items": { "type": "string" }
                        }
                    },
                    "additionalProperties": false
                },
                "GraphSnapshot": {
                    "type": "object",
                    "required": ["version", "nodes", "edges"],
                    "properties": {
                        "version": { "type": "integer", "minimum": 0 },
                        "nodes": {
                            "type": "array",
                            "items": { "$ref": "#/components/schemas/NodeRecord" }
                        },
                        "edges": {
                            "type": "array",
                            "items": { "$ref": "#/components/schemas/EdgeRecord" }
                        }
                    },
                    "additionalProperties": false
                },
                "GraphVersionCompileRequest": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "branch": { "type": "string", "default": "main" },
                        "parent_commits": {
                            "type": "array",
                            "items": { "type": "string" }
                        },
                        "author": { "type": "string" },
                        "message": { "type": "string" },
                        "timestamp_unix_ms": { "type": "integer" },
                        "include_payloads": { "type": "boolean", "default": true }
                    },
                    "additionalProperties": false
                },
                "GraphVersionCompileResponse": {
                    "type": "object",
                    "required": ["ok", "tenant", "pack"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "pack": {
                            "type": "object",
                            "additionalProperties": true
                        }
                    },
                    "additionalProperties": false
                },
                "GraphVersionDiffRequest": {
                    "type": "object",
                    "required": ["base"],
                    "properties": {
                        "base": { "$ref": "#/components/schemas/GraphSnapshot" },
                        "target": { "$ref": "#/components/schemas/GraphSnapshot" }
                    },
                    "additionalProperties": false
                },
                "GraphVersionDiffResponse": {
                    "type": "object",
                    "required": ["ok", "tenant", "diff"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "diff": {
                            "type": "object",
                            "additionalProperties": true
                        }
                    },
                    "additionalProperties": false
                },
                "GraphVersionRepository": {
                    "type": "object",
                    "properties": {
                        "protocol_version": { "type": "string" },
                        "refs": { "type": "array", "items": { "type": "object" } },
                        "packs": { "type": "array", "items": { "type": "object" } }
                    },
                    "additionalProperties": true
                },
                "GraphVersionRefRequest": {
                    "type": "object",
                    "properties": {
                        "repository": { "$ref": "#/components/schemas/GraphVersionRepository" },
                        "name": { "type": "string" },
                        "branch": { "type": "string", "default": "main" },
                        "parent_commits": {
                            "type": "array",
                            "items": { "type": "string" }
                        },
                        "author": { "type": "string" },
                        "message": { "type": "string" },
                        "timestamp_unix_ms": { "type": "integer" },
                        "updated_at_unix_ms": { "type": "integer" },
                        "include_payloads": { "type": "boolean", "default": true }
                    },
                    "additionalProperties": false
                },
                "GraphVersionRefResponse": {
                    "type": "object",
                    "required": ["ok", "tenant", "ref_update"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "ref_update": {
                            "type": "object",
                            "additionalProperties": true
                        }
                    },
                    "additionalProperties": false
                },
                "GraphVersionLogRequest": {
                    "type": "object",
                    "required": ["repository"],
                    "properties": {
                        "repository": { "$ref": "#/components/schemas/GraphVersionRepository" },
                        "target": { "type": "string", "default": "main" }
                    },
                    "additionalProperties": false
                },
                "GraphVersionLogResponse": {
                    "type": "object",
                    "required": ["ok", "tenant", "log"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "log": {
                            "type": "object",
                            "additionalProperties": true
                        }
                    },
                    "additionalProperties": false
                },
                "GraphVersionCheckoutRequest": {
                    "type": "object",
                    "required": ["repository", "target"],
                    "properties": {
                        "repository": { "$ref": "#/components/schemas/GraphVersionRepository" },
                        "target": { "type": "string" }
                    },
                    "additionalProperties": false
                },
                "GraphVersionCheckoutResponse": {
                    "type": "object",
                    "required": ["ok", "tenant", "checkout"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "checkout": {
                            "type": "object",
                            "additionalProperties": true
                        }
                    },
                    "additionalProperties": false
                },
                "GraphVersionMergeRequest": {
                    "type": "object",
                    "required": ["base", "theirs"],
                    "properties": {
                        "base": { "$ref": "#/components/schemas/GraphSnapshot" },
                        "ours": { "$ref": "#/components/schemas/GraphSnapshot" },
                        "theirs": { "$ref": "#/components/schemas/GraphSnapshot" },
                        "name": { "type": "string" },
                        "branch": { "type": "string", "default": "main" },
                        "parent_commits": {
                            "type": "array",
                            "items": { "type": "string" }
                        },
                        "author": { "type": "string" },
                        "message": { "type": "string" },
                        "timestamp_unix_ms": { "type": "integer" },
                        "include_payloads": { "type": "boolean", "default": true },
                        "strategy": {
                            "type": "string",
                            "enum": ["auto_confidence", "prefer_ours", "prefer_theirs", "manual"],
                            "default": "auto_confidence"
                        },
                        "min_confidence_delta": { "type": "number", "default": 0.0 }
                    },
                    "additionalProperties": false
                },
                "GraphVersionMergeResponse": {
                    "type": "object",
                    "required": ["ok", "tenant", "merge"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "merge": {
                            "type": "object",
                            "additionalProperties": true
                        }
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
                "VectorDesignateRequest": {
                    "type": "object",
                    "required": ["label", "property", "dimension"],
                    "properties": {
                        "label": { "type": "string" },
                        "property": { "type": "string" },
                        "dimension": { "type": "integer", "minimum": 1 }
                    },
                    "additionalProperties": false
                },
                "VectorSearchRequest": {
                    "type": "object",
                    "required": ["query", "property"],
                    "properties": {
                        "query": {
                            "type": "array",
                            "items": { "type": "number" },
                            "minItems": 1
                        },
                        "k": { "type": "integer", "minimum": 1, "default": 10 },
                        "label": { "type": "string" },
                        "property": { "type": "string" }
                    },
                    "additionalProperties": false
                },
                "HybridSearchRequest": {
                    "type": "object",
                    "required": ["query", "property", "graph_seeds"],
                    "properties": {
                        "query": {
                            "type": "array",
                            "items": { "type": "number" },
                            "minItems": 1
                        },
                        "k": { "type": "integer", "minimum": 1, "default": 10 },
                        "label": { "type": "string" },
                        "property": { "type": "string" },
                        "graph_seeds": {
                            "type": "array",
                            "items": { "type": "string" },
                            "default": []
                        },
                        "max_hops": { "type": "integer", "minimum": 0, "default": 3 },
                        "alpha": { "type": "number" },
                        "confidence_weighted_graph_distance": { "type": "boolean" },
                        "edge_type_weights": {
                            "type": "object",
                            "additionalProperties": { "type": "number" }
                        }
                    },
                    "additionalProperties": false
                },
                "EpistemicNeighborsRequest": {
                    "type": "object",
                    "required": ["node_id"],
                    "properties": {
                        "node_id": { "type": "string" },
                        "epistemic_types": {
                            "type": "array",
                            "items": {
                                "type": "string",
                                "enum": ["Supports", "Contradicts", "Tension", "Derives", "Cites"]
                            }
                        },
                        "min_confidence": { "type": "number" },
                        "max_depth": { "type": "integer", "minimum": 1 }
                    },
                    "additionalProperties": false
                },
                "AlgorithmPprRequest": {
                    "type": "object",
                    "required": ["seeds"],
                    "properties": {
                        "seeds": {
                            "type": "object",
                            "additionalProperties": { "type": "number" }
                        },
                        "alpha": { "type": "number", "default": 0.15 },
                        "epsilon": { "type": "number", "default": 0.0001 },
                        "max_pushes": { "type": "integer", "minimum": 1, "default": 200000 },
                        "top_k": { "type": "integer", "minimum": 1 }
                    },
                    "additionalProperties": false
                },
                "AlgorithmComponentsRequest": {
                    "type": "object",
                    "properties": {
                        "directed": { "type": "boolean", "default": false }
                    },
                    "additionalProperties": false
                },
                "AlgorithmPageRankRequest": {
                    "type": "object",
                    "properties": {
                        "damping": { "type": "number", "default": 0.85 },
                        "max_iter": { "type": "integer", "minimum": 1, "default": 100 },
                        "tolerance": { "type": "number", "default": 0.000001 },
                        "top_k": { "type": "integer", "minimum": 1 }
                    },
                    "additionalProperties": false
                },
                "AlgorithmCommunitiesRequest": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                },
                "SpatialDesignateRequest": {
                    "type": "object",
                    "required": ["label", "lat_property", "lon_property"],
                    "properties": {
                        "label": { "type": "string" },
                        "lat_property": { "type": "string" },
                        "lon_property": { "type": "string" },
                        "resolution": { "type": "integer", "minimum": 0, "default": 8 }
                    },
                    "additionalProperties": false
                },
                "SpatialRadiusRequest": {
                    "type": "object",
                    "required": ["label", "lat_property", "lon_property", "lat", "lon", "radius_km"],
                    "properties": {
                        "label": { "type": "string" },
                        "lat_property": { "type": "string" },
                        "lon_property": { "type": "string" },
                        "lat": { "type": "number" },
                        "lon": { "type": "number" },
                        "radius_km": { "type": "number", "minimum": 0 }
                    },
                    "additionalProperties": false
                },
                "SpatialBboxRequest": {
                    "type": "object",
                    "required": ["label", "lat_property", "lon_property", "min_lat", "min_lon", "max_lat", "max_lon"],
                    "properties": {
                        "label": { "type": "string" },
                        "lat_property": { "type": "string" },
                        "lon_property": { "type": "string" },
                        "min_lat": { "type": "number" },
                        "min_lon": { "type": "number" },
                        "max_lat": { "type": "number" },
                        "max_lon": { "type": "number" }
                    },
                    "additionalProperties": false
                },
                "SpatialGeometryDesignateRequest": {
                    "type": "object",
                    "required": ["label", "property"],
                    "properties": {
                        "label": { "type": "string" },
                        "property": { "type": "string" },
                        "encoding": { "type": "string", "enum": ["point", "wkb", "wkt", "subgraph"], "default": "wkb" },
                        "resolution": { "type": "integer", "minimum": 0, "maximum": 15, "default": 8 }
                    },
                    "additionalProperties": false
                },
                "SpatialContainsRequest": {
                    "type": "object",
                    "required": ["label", "property", "lat", "lon"],
                    "properties": {
                        "label": { "type": "string" },
                        "property": { "type": "string" },
                        "lat": { "type": "number" },
                        "lon": { "type": "number" }
                    },
                    "additionalProperties": false
                },
                "SpatialGeometryQueryRequest": {
                    "type": "object",
                    "required": ["label", "property", "geometry"],
                    "properties": {
                        "label": { "type": "string" },
                        "property": { "type": "string" },
                        "geometry": { "description": "Query geometry whose concrete shape depends on encoding: a WKT string, a WKB byte array, or a point value." },
                        "encoding": { "type": "string", "enum": ["point", "wkb", "wkt", "subgraph"], "default": "wkt" }
                    },
                    "additionalProperties": false
                },
                "GeometryIdsResponseBody": {
                    "type": "object",
                    "required": ["ok", "tenant", "operation", "count", "node_ids"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "operation": { "type": "string" },
                        "count": { "type": "integer", "minimum": 0 },
                        "node_ids": { "type": "array", "items": { "type": "string" } }
                    },
                    "additionalProperties": false
                },
                "GeometryDesignationResponseBody": {
                    "type": "object",
                    "required": ["ok", "tenant", "operation", "designated"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "operation": { "type": "string" },
                        "designated": {
                            "type": "object",
                            "required": ["label", "property", "encoding", "resolution"],
                            "properties": {
                                "label": { "type": "string" },
                                "property": { "type": "string" },
                                "encoding": { "type": "string" },
                                "resolution": { "type": "integer" }
                            },
                            "additionalProperties": false
                        }
                    },
                    "additionalProperties": false
                },
                "FullTextDesignateRequest": {
                    "type": "object",
                    "required": ["label", "property"],
                    "properties": {
                        "label": { "type": "string" },
                        "property": { "type": "string" }
                    },
                    "additionalProperties": false
                },
                "FullTextSearchRequest": {
                    "type": "object",
                    "required": ["property", "query"],
                    "properties": {
                        "label": { "type": "string" },
                        "property": { "type": "string" },
                        "query": { "type": "string" },
                        "k": { "type": "integer", "minimum": 1, "default": 10 }
                    },
                    "additionalProperties": false
                },
                "DesignationResponseBody": {
                    "type": "object",
                    "required": ["ok", "label", "property"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "label": { "type": "string" },
                        "property": { "type": "string" },
                        "dimension": { "type": "integer", "minimum": 1 }
                    },
                    "additionalProperties": false
                },
                "SpatialDesignationResponseBody": {
                    "type": "object",
                    "required": ["ok", "tenant", "label", "lat_property", "lon_property", "resolution"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "label": { "type": "string" },
                        "lat_property": { "type": "string" },
                        "lon_property": { "type": "string" },
                        "resolution": { "type": "integer", "minimum": 0 }
                    },
                    "additionalProperties": false
                },
                "SearchResultHit": {
                    "type": "object",
                    "required": ["node_id"],
                    "properties": {
                        "node_id": { "type": "string" },
                        "distance": { "type": "number" },
                        "score": { "type": "number" },
                        "node": {
                            "oneOf": [
                                { "$ref": "#/components/schemas/NodeRecord" },
                                { "type": "null" }
                            ]
                        }
                    },
                    "additionalProperties": false
                },
                "SearchResultsResponseBody": {
                    "type": "object",
                    "required": ["ok", "results"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "results": {
                            "type": "array",
                            "items": { "$ref": "#/components/schemas/SearchResultHit" }
                        }
                    },
                    "additionalProperties": false
                },
                "HybridResultsResponseBody": {
                    "type": "object",
                    "required": ["ok", "results", "scoring"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "results": {
                            "type": "array",
                            "items": { "$ref": "#/components/schemas/SearchResultHit" }
                        },
                        "scoring": {
                            "type": "object",
                            "properties": {
                                "alpha": { "type": "number" },
                                "confidence_weighted_graph_distance": { "type": "boolean" },
                                "edge_type_weights": {
                                    "type": "object",
                                    "additionalProperties": { "type": "number" }
                                }
                            },
                            "additionalProperties": false
                        }
                    },
                    "additionalProperties": false
                },
                "EpistemicNeighborsResponseBody": {
                    "type": "object",
                    "required": ["ok", "results"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "results": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "required": ["edge", "node"],
                                "properties": {
                                    "edge": { "$ref": "#/components/schemas/EdgeRecord" },
                                    "node": { "$ref": "#/components/schemas/NodeRecord" }
                                },
                                "additionalProperties": false
                            }
                        }
                    },
                    "additionalProperties": false
                },
                "NodeScore": {
                    "type": "object",
                    "required": ["node_id", "score"],
                    "properties": {
                        "node_id": { "type": "string" },
                        "score": { "type": "number" }
                    },
                    "additionalProperties": false
                },
                "AlgorithmScoresResponseBody": {
                    "type": "object",
                    "required": ["ok", "tenant", "scores"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "alpha": { "type": "number" },
                        "epsilon": { "type": "number" },
                        "damping": { "type": "number" },
                        "scores": {
                            "type": "array",
                            "items": { "$ref": "#/components/schemas/NodeScore" }
                        }
                    },
                    "additionalProperties": false
                },
                "ComponentsResponseBody": {
                    "type": "object",
                    "required": ["ok", "tenant", "directed", "components", "count"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "directed": { "type": "boolean" },
                        "components": {
                            "type": "array",
                            "items": {
                                "type": "array",
                                "items": { "type": "string" }
                            }
                        },
                        "count": { "type": "integer", "minimum": 0 }
                    },
                    "additionalProperties": false
                },
                "CommunityHit": {
                    "type": "object",
                    "required": ["node_id", "community_id"],
                    "properties": {
                        "node_id": { "type": "string" },
                        "community_id": { "type": "string" }
                    },
                    "additionalProperties": false
                },
                "CommunitiesResponseBody": {
                    "type": "object",
                    "required": ["ok", "tenant", "algorithm", "communities", "modularity"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "algorithm": { "const": "label_propagation" },
                        "communities": {
                            "type": "array",
                            "items": { "$ref": "#/components/schemas/CommunityHit" }
                        },
                        "modularity": { "type": "number" }
                    },
                    "additionalProperties": false
                },
                "SpatialIdsResponseBody": {
                    "type": "object",
                    "required": ["ok", "tenant", "count", "node_ids"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "count": { "type": "integer", "minimum": 0 },
                        "node_ids": {
                            "type": "array",
                            "items": { "type": "string" }
                        }
                    },
                    "additionalProperties": false
                },
                "BulkError": {
                    "type": "object",
                    "required": ["line", "code", "message"],
                    "properties": {
                        "line": { "type": "integer", "minimum": 1 },
                        "code": { "type": "string" },
                        "message": { "type": "string" },
                        "record_id": { "type": "string" }
                    },
                    "additionalProperties": false
                },
                "BulkIngestResponseBody": {
                    "type": "object",
                    "required": ["ok", "tenant", "inserted", "failed", "errors", "batches"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "inserted": { "type": "integer", "minimum": 0 },
                        "failed": { "type": "integer", "minimum": 0 },
                        "errors": {
                            "type": "array",
                            "items": { "$ref": "#/components/schemas/BulkError" }
                        },
                        "batches": { "type": "integer", "minimum": 0 }
                    },
                    "additionalProperties": false
                },
                "InstantKgDelta": {
                    "type": "object",
                    "properties": {
                        "commit_sha": { "type": ["string", "null"] },
                        "changed_files": { "type": "array", "items": { "type": "string" } },
                        "objects": { "type": "array", "items": { "$ref": "#/components/schemas/NodeRecord" } },
                        "edges": { "type": "array", "items": { "$ref": "#/components/schemas/EdgeRecord" } },
                        "tombstoned_object_ids": { "type": "array", "items": { "type": "string" } },
                        "removed_edge_ids": { "type": "array", "items": { "type": "string" } }
                    },
                    "additionalProperties": false
                },
                "InstantKgManifest": {
                    "type": "object",
                    "properties": {
                        "repo_id": { "type": "string" },
                        "repo_hash": { "type": "string" },
                        "commit_sha": { "type": "string" },
                        "encoder_version": { "type": "string" },
                        "ingest_version": { "type": "string" },
                        "base_graph_hash": { "type": "string" },
                        "encoded_files": { "type": "array", "items": { "type": "object", "additionalProperties": true } },
                        "objects_total": { "type": "integer", "minimum": 0 },
                        "edges_total": { "type": "integer", "minimum": 0 }
                    },
                    "additionalProperties": false
                },
                "InstantKgViewRequest": {
                    "type": "object",
                    "properties": {
                        "manifest": { "$ref": "#/components/schemas/InstantKgManifest" },
                        "delta": { "$ref": "#/components/schemas/InstantKgDelta" }
                    },
                    "additionalProperties": false
                },
                "InstantKgPprRequest": {
                    "allOf": [
                        { "$ref": "#/components/schemas/InstantKgViewRequest" },
                        {
                            "type": "object",
                            "required": ["seeds"],
                            "properties": {
                                "seeds": { "type": "object", "additionalProperties": { "type": "number" } },
                                "alpha": { "type": "number", "default": 0.15 },
                                "epsilon": { "type": "number", "default": 0.0001 },
                                "max_pushes": { "type": "integer", "minimum": 1, "default": 200000 },
                                "top_k": { "type": "integer", "minimum": 1, "default": 10 }
                            }
                        }
                    ]
                },
                "InstantKgImpactRequest": {
                    "allOf": [
                        { "$ref": "#/components/schemas/InstantKgViewRequest" },
                        {
                            "type": "object",
                            "properties": {
                                "seed": { "type": "string" },
                                "symbol_name": { "type": "string" },
                                "direction": { "type": "string", "enum": ["out", "in"], "default": "out" },
                                "max_depth": { "type": "integer", "minimum": 1, "default": 2 }
                            }
                        }
                    ]
                },
                "InstantKgRelatedRequest": {
                    "allOf": [
                        { "$ref": "#/components/schemas/InstantKgViewRequest" },
                        {
                            "type": "object",
                            "required": ["seed"],
                            "properties": {
                                "seed": { "type": "string" },
                                "kinds": { "type": "array", "items": { "type": "string" } },
                                "top_k": { "type": "integer", "minimum": 1, "default": 10 }
                            }
                        }
                    ]
                },
                "InstantKgSearchRequest": {
                    "allOf": [
                        { "$ref": "#/components/schemas/InstantKgViewRequest" },
                        {
                            "type": "object",
                            "required": ["query"],
                            "properties": {
                                "query": { "type": "string" },
                                "kinds": { "type": "array", "items": { "type": "string" } },
                                "top_k": { "type": "integer", "minimum": 1, "default": 10 }
                            }
                        }
                    ]
                },
                "InstantKgExplainEdgeRequest": {
                    "allOf": [
                        { "$ref": "#/components/schemas/InstantKgViewRequest" },
                        {
                            "type": "object",
                            "required": ["src", "dst"],
                            "properties": {
                                "src": { "type": "string" },
                                "dst": { "type": "string" }
                            }
                        }
                    ]
                },
                "InstantKgResponseBody": {
                    "type": "object",
                    "required": ["ok", "tenant", "status"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tenant": { "type": "string" },
                        "status": { "type": "object", "additionalProperties": true },
                        "stats": { "$ref": "#/components/schemas/GraphStats" },
                        "seed": { "type": "string" },
                        "query": { "type": "string" },
                        "src": { "type": "string" },
                        "dst": { "type": "string" },
                        "results": { "type": "array", "items": { "type": "object", "additionalProperties": true } },
                        "explanations": { "type": "array", "items": { "type": "object", "additionalProperties": true } }
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
                        "property_indexes_total",
                        "memory_bytes",
                        "memory_quota_bytes"
                    ],
                    "properties": {
                        "version": { "type": "integer", "minimum": 0 },
                        "nodes_total": { "type": "integer", "minimum": 0 },
                        "edges_total": { "type": "integer", "minimum": 0 },
                        "labels_total": { "type": "integer", "minimum": 0 },
                        "edge_types_total": { "type": "integer", "minimum": 0 },
                        "property_keys_total": { "type": "integer", "minimum": 0 },
                        "property_indexes_total": { "type": "integer", "minimum": 0 },
                        "memory_bytes": { "type": "integer", "minimum": 0 },
                        "memory_quota_bytes": { "type": "integer", "minimum": 0 }
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
    use rustyred_core::RedCoreDurability;

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
            tenant_memory_quota_bytes: 0,
            tenant_memory_quota_config_error: None,
            tenant_config_overrides: Default::default(),
            tenant_config_error: None,
            slow_query_threshold_nanos: 100_000_000,
            slow_query_capacity: 128,
            slow_query_log: None,
            hybrid_scoring: rustyred_core::HybridScoringConfig::default(),
            redis_url: "not-a-redis-url".to_string(),
            redis_key_prefix: "rusty-red".to_string(),
            require_auth: false,
            allowed_origins: Vec::new(),
            api_tokens: Vec::new(),
            service_name: "rusty-red".to_string(),
            api_title: "Rusty Red".to_string(),
            public_url: None,
            federate: true,
            federate_hub_url: None,
            federate_token: None,
            federate_peer_id: None,
            federate_private_key: None,
            federate_provenance: false,
            federate_snapshot_text_bytes: rustyred_search::DEFAULT_WEB_COMMONS_SNAPSHOT_TEXT_BYTES,
            mcp_enabled: true,
            mcp_read_only: true,
            mcp_allow_admin: false,
            mcp_default_tenant: "default".to_string(),
        });

        let Json(document) = openapi(State(state)).await;

        assert_eq!(
            document.pointer("/info/version"),
            Some(&serde_json::Value::String(
                env!("CARGO_PKG_VERSION").to_string()
            ))
        );

        for (path, method) in [
            ("/metrics", "get"),
            ("/", "get"),
            ("/search", "get"),
            ("/search.json", "get"),
            ("/crawl", "post"),
            ("/federate/submit", "post"),
            ("/v1/diagnostics/slow_queries", "get"),
            ("/v1/diagnostics/config", "get"),
            ("/v1/query", "post"),
            ("/v1/cypher", "post"),
            ("/v1/cypher/explain", "post"),
            ("/v1/transactions/begin", "post"),
            ("/v1/transactions/commit", "post"),
            ("/v1/transactions/rollback", "post"),
            ("/v1/cache/check", "post"),
            ("/v1/cache/put", "post"),
            ("/v1/tenants/{tenant_id}/graph/rebuild-indexes", "post"),
            ("/v1/tenants/{tenant_id}/graph/version/compile", "post"),
            ("/v1/tenants/{tenant_id}/graph/version/diff", "post"),
            ("/v1/tenants/{tenant_id}/graph/version/ref", "post"),
            ("/v1/tenants/{tenant_id}/graph/version/log", "post"),
            ("/v1/tenants/{tenant_id}/graph/version/checkout", "post"),
            ("/v1/tenants/{tenant_id}/graph/version/merge", "post"),
            ("/v1/tenants/{tenant_id}/graph/vector/designate", "post"),
            ("/v1/tenants/{tenant_id}/graph/vector/search", "post"),
            ("/v1/tenants/{tenant_id}/graph/vector/hybrid", "post"),
            ("/v1/tenants/{tenant_id}/graph/epistemic-neighbors", "post"),
            ("/v1/tenants/{tenant_id}/graph/algorithms/ppr", "post"),
            (
                "/v1/tenants/{tenant_id}/graph/algorithms/components",
                "post",
            ),
            ("/v1/tenants/{tenant_id}/graph/algorithms/pagerank", "post"),
            (
                "/v1/tenants/{tenant_id}/graph/algorithms/communities",
                "post",
            ),
            ("/v1/tenants/{tenant_id}/graph/spatial/designate", "post"),
            ("/v1/tenants/{tenant_id}/graph/spatial/radius", "post"),
            ("/v1/tenants/{tenant_id}/graph/spatial/bbox", "post"),
            (
                "/v1/tenants/{tenant_id}/graph/spatial/designate_geometry",
                "post",
            ),
            ("/v1/tenants/{tenant_id}/graph/spatial/within", "post"),
            ("/v1/tenants/{tenant_id}/graph/spatial/intersects", "post"),
            ("/v1/tenants/{tenant_id}/graph/spatial/contains", "post"),
            ("/v1/tenants/{tenant_id}/graph/fulltext/designate", "post"),
            ("/v1/tenants/{tenant_id}/graph/fulltext/search", "post"),
            ("/v1/tenants/{tenant_id}/graph/bulk/nodes", "post"),
            ("/v1/tenants/{tenant_id}/graph/bulk/edges", "post"),
            ("/v1/tenants/{tenant_id}/instant-kg/status", "post"),
            ("/v1/tenants/{tenant_id}/instant-kg/ppr", "post"),
            ("/v1/tenants/{tenant_id}/instant-kg/impact", "post"),
            ("/v1/tenants/{tenant_id}/instant-kg/related-objects", "post"),
            ("/v1/tenants/{tenant_id}/instant-kg/search", "post"),
            ("/v1/tenants/{tenant_id}/instant-kg/explain-edge", "post"),
        ] {
            let encoded_path = path.replace('/', "~1");
            let pointer = format!("/paths/{encoded_path}/{method}");
            assert!(
                document.pointer(&pointer).is_some(),
                "missing OpenAPI path {path} {method}"
            );
        }

        assert_eq!(
            document.pointer("/components/schemas/RebuildIndexesResponse/properties/rebuild/$ref"),
            Some(&serde_json::Value::String(
                "#/components/schemas/RebuildIndexesReport".to_string()
            ))
        );
        assert_eq!(
            document.pointer("/components/schemas/CypherRequest/properties/tx_id/type"),
            Some(&serde_json::Value::String("string".to_string()))
        );
        assert_eq!(
            document.pointer(
                "/paths/~1v1~1diagnostics~1config/get/responses/200/content/application~1json/schema/properties/tenant_config_runtime_mutation_supported/type"
            ),
            Some(&serde_json::Value::String("boolean".to_string()))
        );
        assert_eq!(
            document.pointer(
                "/components/schemas/TransactionCommitResponse/properties/transaction/type"
            ),
            Some(&serde_json::Value::String("object".to_string()))
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
        assert_eq!(
            document.pointer("/components/schemas/GraphVersionDiffRequest/properties/base/$ref"),
            Some(&serde_json::Value::String(
                "#/components/schemas/GraphSnapshot".to_string()
            ))
        );

        let graph_stats_required = document
            .pointer("/components/schemas/GraphStats/required")
            .and_then(|value| value.as_array())
            .expect("GraphStats.required");
        let graph_stats_required = graph_stats_required
            .iter()
            .filter_map(|value| value.as_str())
            .collect::<Vec<_>>();
        assert!(graph_stats_required.contains(&"memory_bytes"));
        assert!(graph_stats_required.contains(&"memory_quota_bytes"));
    }
}
