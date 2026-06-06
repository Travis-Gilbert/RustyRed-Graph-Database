# Benchmarks

One honest, reproducible benchmark. It measures the two numbers RustyRed leads
with: how fast you can get a graph in, and how fast a Personalized PageRank
query comes back over it. Everything here is **measured**, not projected — run
`scripts/bench/run.sh` against any instance and you get that machine's numbers.

This is deliberately a small, transparent harness rather than a benchmark-war
entry. The methodology is below so the numbers can be checked, reproduced, and
argued with.

## What it measures

| Metric | How |
|--------|-----|
| **Ingest rate** (nodes/sec, edges/sec) | One `POST` of `N` JSONL records to `/v1/tenants/{t}/graph/bulk/nodes` (then `2N` to `bulk/edges`), timed by curl's `%{time_total}`. Rate = records / wall-time. |
| **PPR latency** (p50 / p95, ms) | `PPR_TRIALS` calls to `/v1/tenants/{t}/graph/algorithms/ppr` from a single seed, one warm-up discarded, nearest-rank percentiles over the per-request `%{time_total}`. |

## Dataset

Deterministic and self-generating — no external fixtures:

- `N` nodes labelled `Bench` (default `N = 20000`).
- `2N` edges: a ring (`i → i+1`) plus one deterministic chord per node
  (`i → (7i + 3) mod N`), giving real branching for PPR to traverse.

## Methodology

- **Build profile:** the release binary (`cargo build --release`, which in this
  workspace is `lto = "fat"`, `codegen-units = 1`). This is what operators run.
- **Timing source:** `curl` `%{time_total}`. Portable across macOS (whose
  `date(1)` has no sub-second resolution) and Linux, and it brackets the real
  request/response round trip.
- **Warm-up:** one PPR call is issued and discarded before timing begins.
- **Percentiles:** nearest-rank over the sorted sample set.
- **Latency includes the full HTTP round trip** (loopback or network), not just
  engine time — so a remote instance's numbers fold in network latency by
  design. The named environment is reported in the header of every run.

## Reproduce

Start a server (auth off, ephemeral, for a clean measurement):

```bash
RUSTY_RED_REQUIRE_AUTH=false \
RUSTY_RED_REQUIRE_VOLUME=false \
RUSTY_RED_DURABILITY=none \
RUSTY_RED_DATA_DIR="$(mktemp -d)" \
cargo run --release -p rustyred-server
```

Then, in another shell:

```bash
BASE_URL=http://127.0.0.1:8380 NODES=20000 PPR_TRIALS=50 ./scripts/bench/run.sh
```

Against a deployed instance, point `BASE_URL` at it and pass a token with
`graph:read` + `graph:write`:

```bash
BASE_URL=https://<service>.up.railway.app TOKEN=<token> ./scripts/bench/run.sh
```

## Results

<!-- BENCH_RESULTS_START -->
| Environment | Ingest, nodes/sec | Ingest, edges/sec | PPR p50 | PPR p95 |
|-------------|-------------------|-------------------|---------|---------|
| Apple Silicon (Darwin arm64), release build, loopback HTTP | ~8,300 | ~4,800 | 27 ms | 42 ms |
| Railway (shared instance), client over public internet | ~3,700 | ~3,000 | 248 ms | 319 ms |

Run: `VERSION=0.9.1 NODES=20000 PPR_TRIALS=50 ./scripts/bench/run.sh` (20,000 nodes,
40,000 edges, 50 PPR trials, one warm-up discarded). Edge ingest is slower than
node ingest because each edge validates both endpoints and the bulk loader
commits in batches (default 500), refreshing the committed read view per batch.

Both rows are measured (2026-06-06). The Railway row is the **same harness run
from a developer laptop against the live demo over the public internet**, so its
PPR latency is dominated by client↔server round-trip time, not engine compute —
the loopback row isolates the engine. A client co-located with the instance sees
latency close to the loopback figure plus one network hop.
<!-- BENCH_RESULTS_END -->

Numbers are point-in-time and machine-specific. The header of each run records
the version, date, machine, and dataset size so any figure here can be traced
to the run that produced it.
