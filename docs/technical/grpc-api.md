# gRPC API

RustyRed serves gRPC on the **same port** as HTTP (default `8380`). `rustyred-server` merges the
tonic service onto the axum router and routes by content type: requests with
`Content-Type: application/grpc*` go to gRPC, everything else to the REST handlers. No separate port
or process is required.

- **Package:** `rustyred.v1`
- **Service:** `GraphDatabase`
- **Proto (canonical, vendored):** `vendor/proto/rustyred/v1/rustyred.proto`
- **Generated at build time** by `tonic-build` (tonic 0.12 / prost 0.13) via
  `crates/rustyred-server/build.rs`.

Generate a client from the vendored `.proto`. The service implements the same operations as the HTTP
surface, so the data model in [Data model](data-model.md) applies unchanged.

## RPC catalog

**Health**

| RPC | Request → Response |
|-----|--------------------|
| `Health` | `HealthRequest` → `HealthResponse` |
| `Ready` | `ReadyRequest` → `ReadyResponse` |

**Query**

| RPC | Request → Response |
|-----|--------------------|
| `Query` | `QueryRequest` → `QueryResponse` |
| `Cypher` | `CypherRequest` → `CypherResponse` |
| `CypherExplain` | `CypherRequest` → `CypherExplainResponse` |

**Transactions**

| RPC | Request → Response |
|-----|--------------------|
| `BeginTransaction` | `BeginTxnRequest` → `BeginTxnResponse` |
| `CommitTransaction` | `CommitTxnRequest` → `CommitTxnResponse` |
| `RollbackTransaction` | `RollbackTxnRequest` → `RollbackTxnResponse` |

**Node & edge CRUD**

| RPC | Request → Response |
|-----|--------------------|
| `UpsertNode` | `UpsertNodeRequest` → `Node` |
| `UpsertEdge` | `UpsertEdgeRequest` → `Edge` |
| `GetNode` | `GetNodeRequest` → `Node` |
| `GetEdge` | `GetEdgeRequest` → `Edge` |
| `QueryNodes` | `QueryNodesRequest` → `NodeList` |
| `Neighbors` | `NeighborsRequest` → `NeighborList` |

**Bulk ingest**

| RPC | Request → Response |
|-----|--------------------|
| `BulkInsertNodes` | `BulkNodesRequest` → `BulkInsertResponse` |
| `BulkInsertEdges` | `BulkEdgesRequest` → `BulkInsertResponse` |

**Vector**

| RPC | Request → Response |
|-----|--------------------|
| `VectorSearch` | `VectorSearchRequest` → `VectorSearchResponse` |
| `VectorHybridSearch` | `VectorHybridRequest` → `VectorSearchResponse` |
| `DesignateVectorProperty` | `DesignateVectorRequest` → `DesignateAck` |

**Full-text**

| RPC | Request → Response |
|-----|--------------------|
| `FulltextSearch` | `FulltextSearchRequest` → `FulltextSearchResponse` |
| `DesignateFulltextProperty` | `DesignateFulltextRequest` → `DesignateAck` |

**Spatial**

| RPC | Request → Response |
|-----|--------------------|
| `SpatialRadius` | `SpatialRadiusRequest` → `SpatialResponse` |
| `SpatialBoundingBox` | `SpatialBboxRequest` → `SpatialResponse` |
| `DesignateSpatialProperty` | `DesignateSpatialRequest` → `DesignateAck` |

**Epistemic & algorithms**

| RPC | Request → Response |
|-----|--------------------|
| `EpistemicNeighbors` | `EpistemicNeighborsRequest` → `EpistemicNeighborsResponse` |
| `PersonalizedPageRank` | `PPRRequest` → `PPRResponse` |
| `PageRank` | `PageRankRequest` → `PageRankResponse` |
| `ConnectedComponents` | `ComponentsRequest` → `ComponentsResponse` |
| `Communities` | `CommunitiesRequest` → `CommunitiesResponse` |

**Admin**

| RPC | Request → Response |
|-----|--------------------|
| `GraphStats` | `GraphStatsRequest` → `GraphStatsResponse` |
| `GraphVerify` | `GraphVerifyRequest` → `VerifyReport` |
| `RebuildIndexes` | `RebuildIndexesRequest` → `RebuildIndexesResponse` |

**Graph cache**

| RPC | Request → Response |
|-----|--------------------|
| `CachePut` | `CachePutRequest` → `CacheAck` |
| `CacheGet` | `CacheGetRequest` → `CacheGetResponse` |
| `CacheCheck` | `CacheCheckRequest` → `CacheCheckResponse` |
| `CacheExplain` | `CacheCheckRequest` → `CacheExplainResponse` |
| `CacheInvalidate` | `CacheInvalidateRequest` → `CacheAck` |
| `CacheStats` | `CacheStatsRequest` → `CacheStatsResponse` |

## Common messages

`Node` and `Edge` mirror the core records: `Node { id, labels[], properties }` and
`Edge { id, from_id, to_id, type, properties, confidence, epistemic_type, … }`, where `properties`
is a `PropertyMap` of typed `Property` values. `NeighborHit`/`NeighborList` carry the edge id, target
node, type, and optional confidence/epistemic type. See the `.proto` for exact field numbers.
