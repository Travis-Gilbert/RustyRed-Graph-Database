#!/usr/bin/env bash
#
# RustyRed GraphDB benchmark harness.
#
# Measures two things against a running RustyRed instance:
#   1. Bulk ingest rate  (nodes/sec, edges/sec) via the JSONL bulk endpoints.
#   2. Personalized PageRank latency (p50 / p95, milliseconds) over the
#      ingested graph.
#
# Timing uses curl's own %{time_total} so it is portable across macOS (whose
# date(1) lacks sub-second resolution) and Linux. Results are MEASURED, not
# estimated. Re-run anywhere and you will get numbers for that machine.
#
# Usage:
#   BASE_URL=http://127.0.0.1:8380 TOKEN= ./scripts/bench/run.sh
#   BASE_URL=https://rustyred.example TOKEN=secret NODES=50000 ./scripts/bench/run.sh
#
# Env knobs (all optional):
#   BASE_URL     default http://127.0.0.1:8380
#   TOKEN        bearer token; omit/empty when the server runs REQUIRE_AUTH=false
#   TENANT       default "bench"
#   NODES        node count to ingest (default 20000); edges = 2 * NODES
#   PPR_TRIALS   PPR requests to time (default 50)
#   VERSION      version label for the report header (default 0.9.1)

set -euo pipefail

BASE_URL="${BASE_URL:-http://127.0.0.1:8380}"
TOKEN="${TOKEN:-}"
TENANT="${TENANT:-bench}"
NODES="${NODES:-20000}"
PPR_TRIALS="${PPR_TRIALS:-50}"
VERSION="${VERSION:-0.9.1}"

base="${BASE_URL%/}/v1/tenants/${TENANT}/graph"

auth_args=()
if [[ -n "${TOKEN}" ]]; then
  auth_args=(-H "Authorization: Bearer ${TOKEN}")
fi

workdir="$(mktemp -d)"
trap 'rm -rf "${workdir}"' EXIT
nodes_file="${workdir}/nodes.jsonl"
edges_file="${workdir}/edges.jsonl"

die() {
  echo "bench: $*" >&2
  exit 1
}

command -v curl >/dev/null || die "curl is required"
command -v awk >/dev/null || die "awk is required"

# --- generate the dataset -------------------------------------------------
# A ring plus a deterministic chord per node, so PPR has real fan-out.
awk -v n="${NODES}" 'BEGIN {
  for (i = 0; i < n; i++)
    printf "{\"id\":\"n:%d\",\"labels\":[\"Bench\"],\"properties\":{\"i\":%d}}\n", i, i
}' >"${nodes_file}"

awk -v n="${NODES}" 'BEGIN {
  for (i = 0; i < n; i++) {
    ring = (i + 1) % n
    chord = (i * 7 + 3) % n
    printf "{\"id\":\"e:r:%d\",\"from_id\":\"n:%d\",\"to_id\":\"n:%d\",\"type\":\"LINKS\"}\n", i, i, ring
    printf "{\"id\":\"e:c:%d\",\"from_id\":\"n:%d\",\"to_id\":\"n:%d\",\"type\":\"LINKS\"}\n", i, i, chord
  }
}' >"${edges_file}"

edge_count=$((NODES * 2))

# --- timed POST helper: prints %{time_total} seconds ----------------------
post_timed() {
  local url="$1" file="$2"
  curl -s -o /dev/null -w '%{time_total}' \
    ${auth_args[@]+"${auth_args[@]}"} \
    -H 'Content-Type: application/x-ndjson' \
    -X POST "${url}" --data-binary "@${file}"
}

ppr_body='{"seeds":{"n:0":1.0},"alpha":0.15,"epsilon":0.0001,"max_pushes":200000,"top_k":10}'

ppr_timed() {
  curl -s -o /dev/null -w '%{time_total}' \
    ${auth_args[@]+"${auth_args[@]}"} \
    -H 'Content-Type: application/json' \
    -X POST "${base}/algorithms/ppr" --data "${ppr_body}"
}

# --- environment header ---------------------------------------------------
echo "============================================================"
echo "RustyRed benchmark"
echo "  version    : ${VERSION}"
echo "  date (UTC) : $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
echo "  machine    : $(uname -msr)"
echo "  base url   : ${BASE_URL}"
echo "  tenant     : ${TENANT}"
echo "  nodes      : ${NODES}"
echo "  edges      : ${edge_count}"
echo "  ppr trials : ${PPR_TRIALS}"
echo "============================================================"

# --- reachability ---------------------------------------------------------
ready="$(curl -s -o /dev/null -w '%{http_code}' "${BASE_URL%/}/ready" || true)"
[[ "${ready}" == "200" ]] || die "server not ready at ${BASE_URL}/ready (got ${ready})"

# --- ingest ---------------------------------------------------------------
node_secs="$(post_timed "${base}/bulk/nodes" "${nodes_file}")"
edge_secs="$(post_timed "${base}/bulk/edges" "${edges_file}")"

node_rate="$(awk -v n="${NODES}" -v t="${node_secs}" 'BEGIN { printf (t > 0 ? "%.0f" : "n/a"), n / t }')"
edge_rate="$(awk -v n="${edge_count}" -v t="${edge_secs}" 'BEGIN { printf (t > 0 ? "%.0f" : "n/a"), n / t }')"

# --- PPR latency ----------------------------------------------------------
ppr_timed >/dev/null || true   # warmup, discarded
samples="${workdir}/ppr.txt"
: >"${samples}"
for ((i = 0; i < PPR_TRIALS; i++)); do
  ppr_timed >>"${samples}"
  echo >>"${samples}"
done

read -r ppr_min ppr_p50 ppr_p95 ppr_max < <(
  sort -n "${samples}" | awk '
    { a[NR] = $1 }
    END {
      n = NR
      if (n == 0) { print "0 0 0 0"; exit }
      # nearest-rank percentiles
      i50 = int(0.50 * n + 0.9999); if (i50 < 1) i50 = 1; if (i50 > n) i50 = n
      i95 = int(0.95 * n + 0.9999); if (i95 < 1) i95 = 1; if (i95 > n) i95 = n
      printf "%.2f %.2f %.2f %.2f\n", a[1]*1000, a[i50]*1000, a[i95]*1000, a[n]*1000
    }'
)

# --- report ---------------------------------------------------------------
echo
printf '%-22s %s\n' "metric" "result"
printf '%-22s %s\n' "----------------------" "------------------------"
printf '%-22s %s nodes/sec  (%s nodes in %ss)\n' "ingest: nodes" "${node_rate}" "${NODES}" "${node_secs}"
printf '%-22s %s edges/sec  (%s edges in %ss)\n' "ingest: edges" "${edge_rate}" "${edge_count}" "${edge_secs}"
printf '%-22s min %sms  p50 %sms  p95 %sms  max %sms\n' "ppr latency" "${ppr_min}" "${ppr_p50}" "${ppr_p95}" "${ppr_max}"
echo
