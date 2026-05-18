# Security Policy

Rusty Red is an in-memory graph + vector database that runs as a
single web service. This document describes the threat model, the
default security posture, what an operator is responsible for, and
how to report a vulnerability.

## Default posture

The shipped Dockerfile defaults to **authentication required**:

```
RUSTY_RED_REQUIRE_AUTH=true
RUSTY_RED_MCP_READ_ONLY=true
RUSTY_RED_MCP_ALLOW_ADMIN=false
RUSTY_RED_REQUIRE_VOLUME=true
```

In this posture:

- `/v1/*` and `/mcp` reject requests that do not present a valid
  bearer token from `RUSTY_RED_API_TOKENS`.
- `/health`, `/ready`, `/openapi.json`, `/.well-known/agent.json`,
  `/.well-known/mcp/rustyred.json`, and `/metrics` remain unauthenticated;
  they expose no tenant data or mutable surface.
- MCP starts in read-only mode. Write tools are unreachable until the
  operator explicitly enables them.
- The service refuses to start without a persistent volume mounted
  at `RUSTY_RED_DATA_DIR`, so a misconfigured deploy fails loudly
  rather than running on ephemeral storage and silently losing data.

Operators who change any of these defaults are responsible for the
resulting risk.

## Authentication model

Authentication is bearer-token. Tokens are configured via the
`RUSTY_RED_API_TOKENS` environment variable as a comma-separated
list, each entry of the form `<token>:<scope>[,<scope>...]` where
the supported scopes are:

| Scope | Grants |
|---|---|
| `read` | All `GET` routes and read-only `POST` queries (`/v1/query`, `/v1/cypher` with non-mutating clauses, `/v1/cache/get`, etc.). |
| `write` | All `read` plus mutating routes (`/v1/cypher` with `CREATE`/`MERGE`/`SET`/`DELETE`, `/v1/tenants/{id}/graph/nodes`, bulk ingest, etc.). |
| `admin` | All `write` plus `/v1/tenants/{id}/graph/rebuild-indexes`, `/v1/tenants/{id}/graph/verify`, and the MCP admin tool surface (only when `RUSTY_RED_MCP_ALLOW_ADMIN=true`). |

Tokens are matched against the `Authorization: Bearer <token>` header
in HTTP requests and against the MCP `auth` parameter for the `/mcp`
endpoint. Token strings should be ≥ 32 bytes of cryptographically
random data; we recommend generating them with
`openssl rand -hex 32`. Rotate tokens by editing the env and
restarting the service — there is no in-band token rotation API.

Tokens **do not** carry per-tenant scope. A token with `write` can
write to any tenant whose data lives in this Rusty Red instance.
Multi-tenant deployments that need per-tenant scoping should run
one Rusty Red instance per tenant or front the service with an
external auth layer that issues per-tenant signed requests.

## Tenancy isolation

Tenant isolation is enforced at the keyspace layer: all per-tenant
state is stored under keys prefixed with
`{RUSTY_RED_KEY_PREFIX}:{tenant_id}:...`. Routes under
`/v1/tenants/{tenant_id}/...` operate on that tenant's keyspace and
only that keyspace. There is no cross-tenant query API.

The tenant_id is operator-supplied; the service does not validate
ownership. The auth layer described above gives the operator total
access — tenant separation in Rusty Red is a data-organization
boundary, not a trust boundary against an authenticated caller.

## What is in scope

We treat the following as security issues and will respond to
reports about them:

- Authentication bypass on `/v1/*` or `/mcp` when
  `RUSTY_RED_REQUIRE_AUTH=true`.
- Cross-tenant data leakage via `/v1/tenants/{tenant_id}/...`
  routes — a request scoped to one `tenant_id` returning data
  belonging to a different `tenant_id`.
- Privilege escalation across scopes — a `read` token executing
  a write or admin operation, a `write` token executing an admin
  operation.
- MCP read-only bypass — a write tool reachable when
  `RUSTY_RED_MCP_READ_ONLY=true`.
- Memory-safety bugs in the Rust crates that lead to crashes,
  uninitialized reads, or out-of-bounds writes triggered by
  attacker-controlled input.
- Persistence-layer corruption triggered by valid input — i.e., an
  AOF or snapshot write path that produces a state the service
  cannot reload.
- Information disclosure via unauthenticated routes that goes
  beyond intentional surface (`/health`, `/ready`, `/openapi.json`,
  `/metrics`, `/.well-known/*` are intentionally open and not
  considered leakage).

## What is out of scope (for now)

We do not currently treat the following as security issues; pull
requests improving them are welcome but not under embargo:

- Denial of service via expensive Cypher queries, large bulk
  ingests, or unbounded HNSW searches. Operators are expected to
  rate-limit or quota at the ingress layer.
- Side-channel timing attacks against token comparison or
  HNSW search.
- Algorithmic complexity attacks against graph algorithms (PPR,
  PageRank, community detection) on adversarial inputs.
- Supply-chain attacks on transitive crate dependencies.
- The legacy `RUSTY_RED_MODE=redis` compatibility path. This mode
  exists only for migrating off older deployments and is not part
  of the recommended production posture.
- The `/v1/tenants/{tenant_id}/graph/query` debug bridge. Use
  `/v1/query`, `/v1/cypher`, and `/v1/cypher/explain` for the
  product surface.

## Operator responsibilities

These are not security issues if you ignore them; they are how
you remain secure:

1. **Run with `RUSTY_RED_REQUIRE_AUTH=true` on any reachable
   endpoint.** The Dockerfile default. Do not flip it back to
   `false` unless the service is on a private network with
   another trust layer in front.
2. **Generate tokens cryptographically and rotate them.**
   `openssl rand -hex 32` per token. Rotate on operator changes
   and on suspected compromise.
3. **Never bake tokens into a client or commit them to git.**
   Inject via Railway environment variables or equivalent.
4. **Keep the volume backed up.** The persistence layer is
   AOF + snapshot. Snapshot is taken every
   `RUSTY_RED_SNAPSHOT_INTERVAL_WRITES`; AOF replays the gap.
   Volume loss = data loss.
5. **Pin a tagged release rather than `main`.** `main` may carry
   unreleased changes. Tagged releases are what receive security
   patches.
6. **Watch `/metrics`.** Sudden auth-rejection spikes or
   unexpected write-rate growth are the first signal of an
   attempted compromise.

## Reporting a vulnerability

Email **security@\<your-domain\>** with:

- A description of the issue.
- Steps to reproduce, including the route, payload, and
  environment configuration.
- The Rusty Red version (`git rev-parse HEAD` or tag).
- Any logs or output that demonstrates the problem.

If you prefer, file a private GitHub Security Advisory on the
repository instead of email.

We will acknowledge receipt and provide a response with a
disclosure timeline. We do not currently run a bug-bounty program;
we will credit reporters in release notes unless asked to be
anonymous.

## Supported versions

| Version | Supported |
|---|---|
| `main` | Latest commit — receives all fixes. Not recommended for production. |
| Latest tagged release | Receives security fixes. |
| Older tagged releases | Best-effort; no guarantee. |

There is no LTS branch at this time. Operators should plan to
track tagged releases.
