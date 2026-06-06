#!/usr/bin/env bash
#
# Seed the RustyRed public demo with a small epistemic knowledge graph about
# RustyRed itself: Claim nodes carrying 8-dim embeddings, a Source, and
# confidence-weighted epistemic edges (Supports / Cites). Designates the
# embedding property for HNSW vector search and the text property for BM25
# full-text, so the read-only demo token can exercise vector search, full-text
# search, epistemic traversal, and the graph algorithms.
#
# Usage:
#   BASE_URL=https://<service>.up.railway.app TOKEN=<graph:write token> ./scripts/demo/seed.sh
#
# Env: BASE_URL (required), TOKEN (required, needs graph:write), TENANT (default "demo").

set -euo pipefail

BASE="${BASE_URL:?set BASE_URL}"
TOKEN="${TOKEN:?set TOKEN (needs graph:write)}"
TENANT="${TENANT:-demo}"
base="${BASE%/}/v1/tenants/${TENANT}/graph"

jpost() {
  curl -fsS -H "Authorization: Bearer ${TOKEN}" -H "Content-Type: application/json" \
    -X POST "${base}/$1" -d "$2" >/dev/null
}

echo "designating vector + full-text properties..."
jpost vector/designate '{"label":"Claim","property":"embedding","dimension":8}'
jpost fulltext/designate '{"label":"Claim","property":"text"}'

echo "loading nodes..."
curl -fsS -H "Authorization: Bearer ${TOKEN}" -H "Content-Type: application/x-ndjson" \
  -X POST "${base}/bulk/nodes" --data-binary @- >/dev/null <<'JSONL'
{"id":"claim:graph","labels":["Claim"],"properties":{"text":"RustyRed is a graph and vector database in one engine","embedding":[0.90,0.10,0.20,0.00,0.10,0.00,0.00,0.00]}}
{"id":"claim:vector","labels":["Claim"],"properties":{"text":"RustyRed runs HNSW vector search over node properties","embedding":[0.80,0.20,0.30,0.10,0.00,0.00,0.00,0.00]}}
{"id":"claim:epistemic","labels":["Claim"],"properties":{"text":"RustyRed models confidence-weighted epistemic edges","embedding":[0.85,0.15,0.25,0.05,0.05,0.00,0.00,0.00]}}
{"id":"claim:mcp","labels":["Claim"],"properties":{"text":"RustyRed exposes a first-class MCP agent port","embedding":[0.10,0.00,0.00,0.90,0.20,0.10,0.00,0.00]}}
{"id":"claim:ram","labels":["Claim"],"properties":{"text":"RustyRed is RAM-first with append-only-file durability","embedding":[0.00,0.10,0.10,0.80,0.30,0.00,0.00,0.00]}}
{"id":"claim:rust","labels":["Claim"],"properties":{"text":"RustyRed is written in Rust","embedding":[0.05,0.00,0.00,0.85,0.25,0.10,0.00,0.00]}}
{"id":"source:readme","labels":["Source"],"properties":{"text":"RustyRed GraphDB README","embedding":[0.40,0.10,0.10,0.40,0.10,0.00,0.00,0.00]}}
JSONL

echo "linking epistemic edges..."
edges=(
  'e:cite:graph|source:readme|claim:graph|CITES|cites|1.0'
  'e:cite:vector|source:readme|claim:vector|CITES|cites|1.0'
  'e:cite:mcp|source:readme|claim:mcp|CITES|cites|1.0'
  'e:sup:graph_vector|claim:graph|claim:vector|SUPPORTS|supports|0.95'
  'e:sup:graph_epi|claim:graph|claim:epistemic|SUPPORTS|supports|0.9'
  'e:sup:rust_ram|claim:rust|claim:ram|SUPPORTS|supports|0.9'
  'e:sup:mcp_graph|claim:mcp|claim:graph|SUPPORTS|supports|0.85'
)
for spec in "${edges[@]}"; do
  IFS='|' read -r id from to type epi conf <<<"${spec}"
  jpost edges "{\"id\":\"${id}\",\"from_id\":\"${from}\",\"to_id\":\"${to}\",\"type\":\"${type}\",\"epistemic_type\":\"${epi}\",\"confidence\":${conf}}"
done

echo "demo seeded into tenant '${TENANT}'."
