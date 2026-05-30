# Orchestrate Plan: Railway Template Publication

Status: planned
Owner: Travis Gilbert (executes), Claude (Claude-executable items marked)
Origin: `/orchestrate mode=plan` 2026-05-22

## Executive Summary

- **Goal:** Publish RustyRed-Graph-Database as a one-click Railway template with a README that pairs the existing human-centered voice with operator-grade technical content.
- **Intent:** A working deploy button, a hermetic Docker build, a README an operator can read top-to-bottom and deploy from in confidence.
- **Summary of work:** Five workstreams. W1 vendors `rustyred.proto` so the Docker build no longer depends on `git submodule update --init`. W2 rewrites the README into three audience layers preserving voice. W3 adds `.env.example` and `docs/railway-template.md`. W4 writes the ADR. W5 is the Railway dashboard runbook the user executes plus the post-publication badge swap.

## Current Condition

- `Cargo.toml:11-19` — Workspace contains `rustyred-server` (the Railway target binary) plus four sibling crates.
- `Dockerfile:1-40` — Multi-stage Rust → debian:bookworm-slim. Security-by-default env vars present; does **not** COPY `proto/`.
- `railway.toml:1-8` — Dockerfile builder, `/ready` healthcheck, 300s timeout, 3 retries. Correct shape; no changes needed.
- `crates/rustyred-server/build.rs:10-23` — Requires `proto/rustyred/v1/rustyred.proto` at compile time. Currently satisfied only by submodule init.
- `crates/rustyred-server/src/config.rs:92` — Server reads Railway's `PORT` env var natively.
- `crates/rustyred-server/src/config.rs:108,269` — Server recognizes `RAILWAY_VOLUME_MOUNT_PATH` and refuses to start without it when `REQUIRE_VOLUME=true`.
- `.gitmodules` — `proto/` tracks `https://github.com/Travis-Gilbert/theorem-protos.git`, branch `main`. Current HEAD `b64a414`. No tags upstream.
- `README.md` — 306 lines. Lines 1-9 carry the project voice. Line 13 contains a process-leak note to the author. Railway section (lines 200-214) is thin. Audience-interleaved (build steps before deploy story).
- `SECURITY.md` — Production-grade, matches Dockerfile defaults. Authoritative auth doc.
- `scripts/sync-downstream-subtree.sh:118-123` — Uses `git subtree pull --squash`. Carries vendored proto path automatically; no change required.
- `.railwayignore` — Excludes target, env, node_modules, dist, pyc. Clean.

## Goal

- **User-visible outcome:** A working "Deploy on Railway" button in the README. A reader can deploy in under five clicks and receive a service with auth enabled, a persistent volume, and a fresh API token they can copy.
- **System behavior:** Docker build succeeds on Railway from a fresh clone without submodule init. `/health` and `/ready` both return 200 once the volume is mounted. `/v1/*` is 401 without a bearer token; 200 with the configured token. MCP read-only until operator opts in.
- **Data/model changes:** None. No schema changes, no AOF/snapshot format changes.
- **Operational impact:** New operator-facing artifacts (`.env.example`, `docs/railway-template.md`, ADR). README restructured. Build context now includes `vendor/proto/`. CI gains a vendored-proto-up-to-date check.
- **What must not regress:**
  - Local dev workflow (`cargo check --workspace`, `cargo run -p rustyred-server`) still works for a developer who has run `git submodule update --init`.
  - The downstream sync workflow continues to pick up the upstream tree correctly.
  - SECURITY.md remains authoritative for the auth model; README's auth summary links to it, doesn't reimplement it.
  - The voice in `README.md:1-9`, the "What you can't do yet" section, the benchmark numbers, and the algorithm citation survive verbatim.

## Context Stack

| Context | Source | Trust | Why it matters |
|---|---|---|---|
| Server env-var contract | `crates/rustyred-server/src/config.rs:92-212` | code | Confirms Railway-native `PORT` + `RAILWAY_VOLUME_MOUNT_PATH` handling. Drives the env-var reference table in W2/W3. |
| Build dependency on proto | `crates/rustyred-server/build.rs:10-23` | code | Defines what W1 must satisfy. |
| Submodule pin state | `git -C proto rev-parse HEAD` = `b64a414`, branch `main`, no tags | git | Vendoring snapshot anchor. |
| Auth model | `SECURITY.md:38-86` | doc | Source of truth for token format and scope strings. README must link to this, not duplicate it. |
| Railway template variable functions | `https://docs.railway.com/templates/create#template-variable-functions` | external | Resolves the auto-secret-generation question. |
| Railway env var inventory | `https://docs.railway.com/variables/reference` | external | Confirms `PORT` and `RAILWAY_VOLUME_MOUNT_PATH` are injected by Railway. |
| Railway template deploy flow | `https://docs.railway.com/templates/deploy` | external | Confirms eject behavior, updatable templates, badge link target. |
| Downstream sync semantics | `scripts/sync-downstream-subtree.sh:118-123` | code | Confirms `vendor/proto/` flows downstream automatically. |

## Delegation Map

| Work type | Route to | Why |
|---|---|---|
| Docker build hermeticity | execute mode (Claude) | Code/file edits, no specialist needed. |
| README rewrite | execute mode (Claude) | Doc edit, voice constraint is explicit. |
| ADR authoring | execute mode (Claude) | Doc edit. |
| Operator surface docs | execute mode (Claude) | Doc edit. |
| Railway dashboard work | user | Only the user can authenticate to Railway and click through publication. |
| Smoke deploy after publication | user + validator-reporter (Claude review) | User clicks the button; Claude reviews the resulting deploy logs and `/ready` response. |
| Federation/learning candidates | federation-learning-recorder (post-publication) | Capture the vendored-proto pattern as a routing lesson. |

No SDK/database harness work in this plan. No Redis harness work. No GraphRAG context required. No UI visual work (README is text, not pixels).

## Action Rail

Immediate next actions, ordered:

| Action | Risk | Validator | Approval | Route |
|---|---|---|---|---|
| Begin W1 with `RT-101` (create vendored proto dir + commit snapshot) | low | `diff -q vendor/proto/rustyred/v1/rustyred.proto proto/rustyred/v1/rustyred.proto` returns empty | proceed | Claude execute |
| Continue W1 through `RT-106` (build hermeticity proven via Docker build from fresh clone) | low | `docker build -t rustyred:test .` succeeds in a sibling clone with `proto/` deleted | proceed | Claude execute |
| W2 README rewrite | low (reversible) | Diff review with user | review before commit | Claude execute, user reviews |
| W3 + W4 (.env.example, railway-template.md, ADR) | low | Cross-link review | proceed | Claude execute |
| W5 dashboard work | medium (live publish) | Smoke deploy via own button | user-only | User |
| Badge URL swap | low | README renders correctly on github.com | proceed | Claude execute, post-publish |

## Checklist

Stable IDs `RT-1XX` (W1: build hermeticity), `RT-2XX` (W2: README), `RT-3XX` (W3: operator surface), `RT-4XX` (W4: ADR), `RT-5XX` (W5: publication).

### W1 — Build hermeticity (vendor the proto)

| ID | Task | Codebase grounding | Route | Acceptance criteria | Validation | Risk | Status |
|---|---|---|---|---|---|---|---|
| RT-101 | Create `vendor/proto/rustyred/v1/rustyred.proto` as a byte-exact copy of `proto/rustyred/v1/rustyred.proto` at submodule commit `b64a414`. | `proto/rustyred/v1/`, `git -C proto rev-parse HEAD` | Claude | File exists, byte-identical to submodule source. | `diff -q vendor/proto/rustyred/v1/rustyred.proto proto/rustyred/v1/rustyred.proto` empty exit. | low | planned |
| RT-102 | Update `crates/rustyred-server/build.rs` to look up the proto in this order: (1) `<workspace>/vendor/proto/rustyred/v1/rustyred.proto`, (2) fallback `<workspace>/proto/rustyred/v1/rustyred.proto` for backward-compat with dev workflows that already have the submodule populated. | `crates/rustyred-server/build.rs:10-23` | Claude | `build.rs` prefers vendored; falls back; emits a clear error if neither exists. `cargo:rerun-if-changed` is emitted for both paths. | `cargo build -p rustyred-server` succeeds from both states: with submodule initialized AND with `rm -rf proto && git rm --cached proto`. | low | planned |
| RT-103 | Update `Dockerfile` to `COPY vendor ./vendor` alongside the existing `COPY crates ./crates` and `COPY src ./src` lines. | `Dockerfile:5-7` | Claude | New COPY line is present, ordered for layer caching (after `Cargo.toml`/`Cargo.lock`, before `crates`/`src` to avoid invalidating Rust cache on vendor edits). | `docker build -t rustyred:test .` in a clone with `proto/` deleted succeeds. | low | planned |
| RT-104 | Add `scripts/sync-vendored-proto.sh` that copies `proto/rustyred/v1/rustyred.proto` → `vendor/proto/rustyred/v1/rustyred.proto` and emits the source commit to a `vendor/proto/SOURCE_COMMIT` file. | `scripts/sync-downstream-subtree.sh` (style reference) | Claude | Script is executable, has `set -euo pipefail`, prints what it did, is idempotent. | Run it; diff is empty after; `vendor/proto/SOURCE_COMMIT` contains current `proto/` HEAD. | low | planned |
| RT-105 | Add `.github/workflows/vendored-proto-up-to-date.yml` that runs on PR: checks out with `--recurse-submodules`, runs the sync script in dry-run mode, fails if a diff is produced. | `.github/workflows/sync-downstream.yml` (style reference) | Claude | Workflow runs on `pull_request`, uses `actions/checkout@v4` with `submodules: recursive`, runs `scripts/sync-vendored-proto.sh --check`, exits non-zero on diff. | Open a PR with intentional vendor drift, observe red CI; remove drift, observe green. | low | planned |
| RT-106 | Verify Docker build is hermetic from a fresh state. | All W1 outputs | Claude | `docker build -t rustyred:test .` succeeds in a clone with `proto/` absent and no submodule init step. | Live `docker build` in a sibling worktree-free clone (`git clone --depth 1` into `/tmp`, `rm -rf proto/`, build). | low — fast feedback | planned |

### W2 — README rewrite (preserve voice, restructure for audience)

Voice-preservation rule: lines 1-9 of current `README.md` reproduce verbatim. The "What you can't do yet" section, the benchmark table with its commentary ("acceptance gate: must be >= 20x"), and the algorithm citation reproduce verbatim. The voice editorializing ("Written in Rust, the best way to write a database. In my humble opinion.") stays.

New section order: (1) Hero + voice, (2) Deploy & operate, (3) Develop & extend, (4) Reference. Concrete plan:

| ID | Task | Source | Route | Acceptance criteria | Validation | Risk | Status |
|---|---|---|---|---|---|---|---|
| RT-201 | Hero block — keep `README.md:1-9` verbatim. Insert Railway deploy badge **without** the process-leak note from line 13. Add a 2-sentence "what happens when you click this" caption directly below the badge. | `README.md:1-13` | Claude | Lines 1-9 byte-identical to current. Line 13's "Note put template ID here…" is deleted. Badge URL still contains the placeholder until W5 resolves it. | Diff review. | low | planned |
| RT-202 | New section: **Deploy on Railway (quickstart)**. Three sub-bullets: (a) click the badge, (b) confirm the auto-generated `RUSTY_RED_API_TOKENS` value Railway shows (call out that it's a fresh hex token), (c) wait for `/ready` to flip green. Link to `docs/railway-template.md` for the full operator guide. | new content | Claude | Section is < 30 lines, scannable, names the env vars an operator must look at. | Diff review. | low | planned |
| RT-203 | New section: **Auth model in one screen**. Recap the bearer-token + scopes model from `SECURITY.md` in ≤15 lines. End with: "Full threat model and scope reference: see [SECURITY.md](SECURITY.md)." | `SECURITY.md:38-86` | Claude | No content duplicated from SECURITY.md verbatim; this is a summary that points there. | Diff review against SECURITY.md to confirm no copy-paste drift risk. | low | planned |
| RT-204 | New section: **Environment variable reference**. Single table with columns: Variable, Default, Required-if, Notes. Cover: `RUSTY_RED_HOST`, `PORT`/`RUSTY_RED_PORT`, `RUSTY_RED_MODE`, `RUSTY_RED_DATA_DIR`, `RAILWAY_VOLUME_MOUNT_PATH`, `RUSTY_RED_REQUIRE_VOLUME`, `RUSTY_RED_DURABILITY`, `RUSTY_RED_SNAPSHOT_INTERVAL_WRITES`, `RUSTY_RED_REQUIRE_AUTH`, `RUSTY_RED_API_TOKENS`, `RUSTY_RED_KEY_PREFIX`, `RUSTY_RED_SERVICE_NAME`, `RUSTY_RED_API_TITLE`, `RUSTY_RED_PUBLIC_URL`, `RUSTY_RED_MCP_ENABLED`, `RUSTY_RED_MCP_READ_ONLY`, `RUSTY_RED_MCP_ALLOW_ADMIN`, `RUSTY_RED_MCP_DEFAULT_TENANT`, `RUSTY_RED_ALLOWED_ORIGINS`, `RUSTY_RED_STRICT_ACID` + dependents, `RUSTY_RED_TENANT_MEMORY_QUOTA_BYTES`, `RUSTY_RED_SLOW_QUERY_NANOS`, `RUSTY_RED_SLOW_QUERY_CAPACITY`, `RUSTY_RED_SLOW_QUERY_LOG`, `RUSTY_RED_FULLTEXT_BACKEND`, `RUSTY_RED_SPATIAL_BACKEND`, `RUSTY_RED_TENANT_CONFIG_PATH`, `RUSTY_RED_TENANT_CONFIG_JSON`. | `crates/rustyred-server/src/config.rs:88-220` | Claude | Every env var present in `config.rs` either appears in the table or is explicitly in the "advanced/internal" footnote. Defaults match `Dockerfile:24-36` and `config.rs`. | Cross-grep: every `env_first(&["RUSTY_RED_*"...]` in `config.rs` is accounted for in the table. | medium — long table, drift risk | planned |
| RT-205 | New section: **Persistence and the volume**. One paragraph: AOF + snapshot, the volume requirement, why `REQUIRE_VOLUME=true` fails the start when missing, what happens on Railway redeploy (volume persists), how to back up (snapshot file copy). | `crates/rustyred-server/src/config.rs:108-130,265-275`, `Dockerfile:24-36` | Claude | ≤12 lines. Calls out the loud-fail behavior as a feature, not a bug. | Diff review. | low | planned |
| RT-206 | New section: **Observability**. One paragraph each on `/metrics` (17 Prometheus counters), the slow-query ring buffer, the diagnostics endpoints. State what an operator should alarm on (auth-rejection spikes, unexpected write-rate growth). | `README.md:28`, `SECURITY.md:168-170` | Claude | ≤20 lines. Specific. No "consider observability" filler. | Diff review. | low | planned |
| RT-207 | New section: **Upgrade and version pinning**. One paragraph: track tagged releases, the `rustyred-upgrade-format` migration path (existing on-disk-format-stable claim from current README line 18), don't pin to `main` in production. | `README.md:18`, `SECURITY.md:152-167` | Claude | ≤10 lines. Echoes SECURITY.md operator responsibility #5 without duplicating it. | Diff review. | low | planned |
| RT-208 | Restructure existing technical content (Build, Product server, Routes, MCP tools, Compatibility command server, Rust-native helper crate, Algorithm reference, License) into a **Develop & extend** section that comes *after* the operator content. Preserve voice. Keep "What you can't do yet" early in this section as a roadmap signal. | `README.md:15-306` | Claude | All current technical content present, no duplication, ordering flows: roadmap → architecture → build → product server → routes → MCP → compat-server → Rust helper crate → algorithm → license. | Diff review: byte-count of preserved sections matches. | medium — large reflow, easy to drop a sentence | planned |
| RT-209 | Remove the placeholder note at `README.md:13`. | `README.md:13` | Claude | String "Note put template ID here before making public" no longer exists in README. | `grep -n "put template ID here" README.md` returns empty. | low | planned |

### W3 — Operator surface (`.env.example` + `docs/railway-template.md`)

| ID | Task | Codebase grounding | Route | Acceptance criteria | Validation | Risk | Status |
|---|---|---|---|---|---|---|---|
| RT-301 | Create `.env.example` listing every `RUSTY_RED_*` env var from the W2 reference table, with default value (or empty for required-with-no-default), and an inline comment marking required-for-prod. Group: required, security, durability, MCP, observability, advanced. | `crates/rustyred-server/src/config.rs:88-220`, `Dockerfile:24-36` | Claude | File parses as a valid dotenv (no quoting bugs). Required vars labeled. Comments are sparse and useful. | `set -a; source .env.example; set +a; env | grep RUSTY_RED` shows expected names. | low | planned |
| RT-302 | Create `docs/railway-template.md`. Outline: (1) what this template is, (2) one-click flow, (3) variables Railway will prompt for, (4) volume layout, (5) what auth posture you get out-of-the-box, (6) how to scale (volume size, replica count caveats), (7) backup/restore, (8) upgrade path, (9) when to eject from the upstream template repo, (10) troubleshooting. | new content | Claude | ≤ 250 lines. Each section has a concrete action or check. Cross-links to README and SECURITY.md instead of duplicating. | Diff review. | low | planned |
| RT-303 | Cross-link `.env.example` and `docs/railway-template.md` from the README's "Deploy & operate" section. | RT-202, RT-301, RT-302 | Claude | Both links present and resolve via relative path on github.com. | Manual link check on rendered README. | low | planned |
| RT-304 | Update `.gitignore` if necessary to ensure `.env` is excluded but `.env.example` is tracked. | `.gitignore` current content | Claude | `.env` ignored, `.env.example` not ignored. | `git check-ignore .env && git check-ignore -v .env.example` (second should not match). | low | planned |

### W4 — ADR

| ID | Task | Codebase grounding | Route | Acceptance criteria | Validation | Risk | Status |
|---|---|---|---|---|---|---|---|
| RT-401 | Create `docs/adr/0001-vendored-proto-for-railway-build.md`. Format: Context, Decision, Alternatives Considered (A vendored, B in-Docker submodule init, C committed generated bindings, D Railway-side submodule flag — with the same upside/risk/validation matrix from the Theorem Brief), Consequences, Reversibility, Sync Discipline (how `scripts/sync-vendored-proto.sh` and the CI check enforce no-drift). | This plan + the Theorem Brief options table | Claude | ADR is self-contained — a reader who has not seen the brief can understand the decision and the rejected alternatives. | Diff review. | low | planned |
| RT-402 | Establish `docs/adr/README.md` if it doesn't exist — single line: "Architecture Decision Records, numbered. See ADR 0001 onward." | new | Claude | File exists; future ADRs follow the same numbering. | `ls docs/adr/` shows both files. | low | planned |

### W5 — Railway publication (user-executes-in-dashboard items)

| ID | Task | Grounding | Route | Acceptance criteria | Validation | Risk | Status |
|---|---|---|---|---|---|---|---|
| RT-501 | **User:** Open `https://railway.com/workspace/templates` → `New Template`. Add one service, source = GitHub repo `Travis-Gilbert/RustyRed-Graph-Database`, branch `main`. | Railway docs `templates/create` | User | Service appears in the template composer. | UI screenshot. | low | planned |
| RT-502 | **User:** In the service Settings tab: set Healthcheck Path = `/ready`. No custom start command (Dockerfile `CMD` already sets it). Enable public networking with HTTP. | `railway.toml:6`, Railway docs `templates/create` | User | Settings persist. | UI. | low | planned |
| RT-503 | **User:** In the service Variables tab, define these template-level variables. **Critical:** `RUSTY_RED_API_TOKENS` uses the Railway secret function so each deploy gets a fresh token. Format below the table. | Railway docs `templates/create#template-variable-functions` | User | Variables saved on the template. Deployers see prompts for non-secret-derived values; secret-derived value is generated server-side. | Test-deploy from own template; verify the resulting env shows a 64-char hex token in the value. | medium — secret-function syntax has to be exact | planned |
| RT-504 | **User:** Right-click the service → Attach Volume → mount path `/app/data/rusty-red`, default size 1 GiB. | `Dockerfile:27`, `crates/rustyred-server/src/config.rs:108,269` | User | Volume declared on the template. | UI. | low | planned |
| RT-505 | **User:** `Create Template`. Capture the resulting template URL/short-code. | Railway docs `templates/create` | User | Template URL captured (looks like `https://railway.com/new/template/<code>`). | URL pasted into a temporary note for RT-507. | low | planned |
| RT-506 | **User:** Smoke-deploy from own template into a throwaway Railway project. Confirm: (a) build succeeds, (b) `/health` 200, (c) `/ready` 200 after volume mounts, (d) `/v1/health-check-style-route` 401 without token, (e) `/v1/...` 200 with the auto-generated token from the env. | Railway docs `templates/deploy` | User | All five checks pass. Logs show no submodule errors. | Manual via the Railway dashboard + curl from a terminal. | medium — first-time deploy may surface unknowns | planned |
| RT-507 | **Claude:** Replace the placeholder `RUSTY_RED_GRAPH_DATABASE_TEMPLATE_ID` in the README badge URL with the real template short-code from RT-505. Commit on its own. | `README.md:11` | Claude | Badge URL renders the deploy button and links to the live template. | Click the rendered badge on github.com from a private window; confirm it lands on the Railway template page. | low | planned |
| RT-508 | **User:** Publish the template via `Publish` on the workspace templates page so it appears in the marketplace. (Optional now, required for marketplace visibility and kickback eligibility.) | Railway docs `templates/create` (Managing your templates) | User | Template moves from Personal to Published. | UI. | low | planned |

**RT-503 variable definitions (template composer Variables tab):**

```text
# Required — security
RUSTY_RED_REQUIRE_AUTH=true
RUSTY_RED_API_TOKENS=${{secret(64, "abcdef0123456789")}}=graph:read|graph:write|context:read|admin:read

# Branding / namespacing
RUSTY_RED_KEY_PREFIX=rusty-red:tenant
RUSTY_RED_SERVICE_NAME=rusty-red-graph-database
RUSTY_RED_API_TITLE=Rusty Red Graph Database API

# MCP posture
RUSTY_RED_MCP_ENABLED=true
RUSTY_RED_MCP_READ_ONLY=true
RUSTY_RED_MCP_ALLOW_ADMIN=false

# Durability — defaults from Dockerfile; expose for power users
RUSTY_RED_DURABILITY=aof_everysec
RUSTY_RED_SNAPSHOT_INTERVAL_WRITES=1000

# Data location — must match the attached volume mount path
RUSTY_RED_DATA_DIR=/app/data/rusty-red
RUSTY_RED_REQUIRE_VOLUME=true
RUSTY_RED_MODE=embedded
```

The `${{secret(64, "abcdef0123456789")}}` function yields a 64-character hex string per deploy — exact equivalent of `openssl rand -hex 32`. Confirmed by Railway docs `templates/create#template-variable-functions`. At deploy time, `RUSTY_RED_API_TOKENS` will look like `<64-hex-chars>=graph:read|graph:write|context:read|admin:read`, matching the `<secret>=<scope>|<scope>|...` format documented in `SECURITY.md:38-54`. (An earlier draft of this plan prefixed the value with `admin=` — that would have been malformed; the format has no name prefix.)

## Test Strategy

- **Preflight checks:** `cargo check --workspace`, `cargo build -p rustyred-server`, `cargo test -p rustyred-server` — all must pass after RT-102.
- **Focused tests:** Run any existing Rust tests that exercise the server. No new tests required by this plan; the build itself is the test for hermeticity.
- **Integration tests:** Local Docker build twice — once with submodule populated, once with `proto/` deleted. Both must succeed. (RT-106 acceptance.)
- **Regression tests:** Local dev workflow probe — `git submodule update --init && cargo run -p rustyred-server` must still boot.
- **Type/lint/static checks:** `cargo clippy --workspace -- -D warnings` if currently green; do not introduce new warnings.
- **Manual smoke checks:** RT-506 — five-point check on the smoke deploy.
- **Performance/security checks:** None new; use Rust benchmark or smoke harnesses when this plan is refreshed.

## Production Gates

- [ ] Tests pass or failures are explained.
- [ ] No unchecked migration or data risk. (No schema changes in this plan.)
- [ ] No secrets or destructive commands introduced. (`RUSTY_RED_API_TOKENS` only appears as the `${{secret(...)}}` template recipe; no real tokens committed.)
- [ ] Error paths considered. (`build.rs` fallback path; vendored-proto-mismatch CI failure path; volume-missing start refusal already exists.)
- [ ] Observability/logging considered. (New README section RT-206 documents what to alarm on; no new logging surfaces required.)
- [ ] Rollback/revert path exists. (Each commit is independently revertable. Vendored proto can be deleted to fall back to submodule-only at any time. Railway template can be unpublished.)
- [ ] Docs/ADR updated. (W2, W3, W4 produce these.)
- [ ] Redis/harness writeback is N/A — this plan does not touch Theseus/Theorem state.
- [ ] Final report can reconcile every checklist item. (Reconciliation table will mirror this checklist's IDs.)

## Epistemic Ledger

| Primitive | Entry | Evidence | Confidence | Action |
|---|---|---|---|---|
| Claim | Server already reads Railway's `PORT` and `RAILWAY_VOLUME_MOUNT_PATH` natively | `crates/rustyred-server/src/config.rs:92,108,269` | high | use as-is |
| Claim | Submodule `theorem-protos` tracks `main`, current HEAD `b64a414`, no upstream tags | `git submodule status`, `git -C proto tag` | high | vendor this commit as the pin |
| Claim | Railway supports server-side secret generation via `${{secret(length, alphabet)}}` template function with explicit `openssl rand -hex 32` recipe | Railway docs `templates/create#template-variable-functions` | high | use for `RUSTY_RED_API_TOKENS` |
| Claim | `git subtree pull --squash` carries `vendor/proto/` to downstream consumers without script change | `scripts/sync-downstream-subtree.sh:118-123` | high | no W1/W5 changes needed for downstream |
| Assumption | Railway badge link format remains `https://railway.com/new/template/<code>?utm_…` | Current `README.md:11` placeholder + Railway docs `templates/deploy` | medium | verify at RT-505 |
| Assumption | 1 GiB volume default is enough for meaningful template exploration and within free/Hobby plan reach | Plan input from user | medium | template publisher can adjust default before RT-505 |
| Gap | Whether Railway template can declare default-on/default-off toggle for `RUSTY_RED_REQUIRE_AUTH` distinct from the value the user can edit | Railway docs read in this pass | low | accept default-true and let operators edit before deploy |
| Tension | "Auth required by default" vs "one-click deploy" | Theorem Brief | resolved | resolved by `${{secret(...)}}` — see RT-503 |
| Tension | Voice preservation vs operator-grade restructure | Theorem Brief | resolved | resolved by RT-201/RT-208 strategy: voice sections reproduce verbatim; operator content is net-new and lives in a new section block |
| Decision | Vendor proto over submodule-init-in-Docker over committed-generated-bindings | Theorem Brief options table; user concession on this turn | high | W1 implements |
| Decision | Volume default = 1 GiB | User confirmation on this turn | high | W5 implements |
| Method | `scripts/sync-vendored-proto.sh` with `--check` mode + CI workflow ensures no drift between vendored and submodule proto | RT-104, RT-105 | high | net-new in this plan |
| Outcome | (filled at execution / `/orchestrate mode=execute`) | n/a | n/a | n/a |

## Explicit Non-Goals and Deferrals

| Item | Why deferred | Risk of deferral | Follow-up |
|---|---|---|---|
| Pinning `theorem-protos` to a versioned tag upstream | The upstream repo currently has no tags. Tagging is an upstream concern, not this plan's. | Vendoring already locks the snapshot at commit `b64a414`; CI guard catches drift. | When upstream cuts a `v0.1.0` tag, update `.gitmodules` branch field and re-sync. |
| Adding a `theorem-protos`-version field to `/v1/diagnostics/config` | Out of scope for Railway templating. | Operators can read `vendor/proto/SOURCE_COMMIT` from the image if needed. | Future PR, not blocked by this plan. |
| Multi-service Railway template (e.g., adding a Postgres companion) | User intent is single-service RAM-first DB. | None — explicit design. | If later product needs persistence beyond AOF/snapshot. |
| Per-tenant scoped tokens | `SECURITY.md:81-86` already declares this out of scope. | Documented; operators front the service with an external auth layer if they need it. | Future architecture decision. |
| Removing the `proto/` submodule entirely | Submodule remains the developer-facing source of truth for protos; vendored copy is the build-context artifact. | None — both can coexist with the CI guard. | If submodule maintenance becomes a burden, revisit Option C (commit generated bindings). |
| Replacing the deploy badge with a Railway-marketplace-discoverable URL after publication | Will happen in RT-507 once the template short-code is known. | Plan covers this explicitly. | RT-507. |
| Hardening `.railwayignore` further | Current contents are correct for the Docker builder; Railway only uses this for Nixpacks/Buildpack builds anyway. | Low. | None. |

## Execution Instructions

- **Start with:** `RT-101` (create the vendored proto file). Bottom-up: build hermeticity first, README rewrite second, operator docs third, ADR fourth, Railway dashboard last.
- **Preserve:**
  - `README.md:1-9` verbatim.
  - "What you can't do yet" section verbatim, just relocated.
  - Benchmark table verbatim including the M1 Max framing and the 20x floor commentary.
  - Algorithm citation verbatim.
  - SECURITY.md is untouched by this plan.
  - `railway.toml` is untouched.
  - Existing `.gitmodules` is untouched (the submodule stays).
- **Run between RT items:**
  - After RT-101/RT-102/RT-103: `cargo build -p rustyred-server` (must succeed).
  - After RT-106: `docker build .` in a clone with `proto/` deleted (must succeed).
  - After W2 commits: render the README on github.com and read top-to-bottom.
  - After RT-505: smoke deploy.
- **Commits (conventional, no emojis, scope required):**
  - `chore(proto): vendor rustyred.proto for hermetic Docker builds` (W1 RT-101…RT-104)
  - `ci(proto): add vendored-proto-up-to-date workflow` (RT-105)
  - `chore(docker): copy vendored proto into build context` (RT-103 if split out)
  - `docs(readme): rewrite for Railway template operators, preserve voice` (W2)
  - `docs(ops): add .env.example and Railway template guide` (W3)
  - `docs(adr): record vendored-proto decision for Railway build` (W4)
  - `docs(readme): swap placeholder template ID for live Railway template short-code` (RT-507)
- **Report using Orchestrate Report format** after execution. Reconcile every `RT-1XX`…`RT-5XX` ID; mark blocked items with evidence; do not silently drop.

## Open Questions Resolved In This Pass

1. **Q1 (Railway secret generation):** Resolved. `${{secret(64, "abcdef0123456789")}}` is the exact `openssl rand -hex 32` equivalent. Source: `https://docs.railway.com/templates/create#template-variable-functions`.
2. **Q2 (Submodule pinning):** Resolved. Submodule is on `theorem-protos@main`, HEAD `b64a414`, no upstream tags. Vendoring acts as the pin.
3. **Q3 (Badge URL format):** Confirmed shape; exact short-code captured at RT-505.
4. **Q4 (Downstream sync):** Resolved. `git subtree pull --squash` carries the vendored path automatically. No script change required.

## Claude-executable vs User-executes Split

| Workstream | Claude-executable items | User-executes items |
|---|---|---|
| W1 build hermeticity | RT-101 through RT-106 | none |
| W2 README rewrite | RT-201 through RT-209 | review the diff |
| W3 operator surface | RT-301 through RT-304 | review the diff |
| W4 ADR | RT-401, RT-402 | review the diff |
| W5 Railway publication | RT-507 (badge swap, after RT-505) | RT-501 through RT-506, RT-508 |

---

**Next step:** `/orchestrate mode=execute` against this plan, starting at `RT-101`. The execute pass should produce an `Orchestrate Report` that reconciles every `RT-` ID and surfaces any tension that emerged during implementation.
