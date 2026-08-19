# ADR 0002 — Split document, filing, and argument across three models

Status: Proposed
Date: 2026-08-19
Decision drivers: Travis Gilbert

The decision is accepted as product direction. Status is Proposed
because it is not implemented. This ADR is not a claim that the
current public `0.9.1` snapshot already has document tables, collection
tables, or a published-ref API.

## Context

RustyRed `0.9.1` is graph-first. The shipped store is a directed
property graph with epistemic edges, Git-like version packs at
`/graph/version/*`, BM25 full-text, and HNSW vector search. Canonical
types live in `crates/rustyred-core/src/graph_store.rs`. There is no
document envelope, no collection membership table, and no publish
semantics for prose.

Product work still needs a place to put writing. The editor opens a
document. A public site lists essays. A field note becomes an essay.
Those are filing and argument problems, not graph-label problems.

The cheap encoding is to store Essay / FieldNote / Project as
first-class types or as node labels that replace Document. That freezes
genre into species. Changing a field note into an essay becomes a type
migration instead of a membership change. Folders become edge walks.
Presentation chrome (callout styling, hero colors, annotation offsets)
lands in the same row as the body.

The live editor already has a CRDT buffer
(`/v1/tenants/{t}/sync/yjs/:doc_id`) that persists as `YjsDoc` graph
nodes. Graph version packs already compile snapshots, move refs
(default branch `main`), and checkout commits. Search already indexes
designated node properties. markdown-theory already treats
article / note / log as templates, not types. None of that is a
document model. The missing piece is the split: what holds the
document, what holds filing, what holds argument.

The first consumer is the extracted thin public site
(`public-site/` on travisgilbert.me). It will keep markdown on disk
for now and later read published documents through this model.

## Decision

RustyRed is multi-model. Split responsibilities. Do not collapse
writing, filing, and argument into one graph label set.

### 1. Document model holds the document

A Document is the thing the editor opens. The envelope is:

- `id`
- `body` (markdown, v1)
- optional `title`
- a working ref and a published ref

Publish is a ref move, like updating `main` through
`/graph/version/ref`. It is not a boolean on the row.

This is also why the same model works as an IDE backend: the editor
buffer is the working tree; publish is the commit you ship.

Existing version packs (`/graph/version/compile`, `/diff`, `/ref`,
`/log`, `/checkout`, `/merge`) are the revision substrate. Do not
invent a second versioning system for documents.

The live Yjs room remains the collaborative working buffer. This ADR
does not replace that transport. It defines the durable envelope the
editor opens and the published ref a reader may serve.

Markdown body is v1. Do not wait on a block model.

### 2. Relational model holds filing

Collections, genres, membership (document in collection), and slugs
belong here. Slugs are unique per collection.

"Essay", "field note", and "project" are not types and not labels that
change the species of a document. They are metadata that connect
documents: collection membership, or a row that says this document is
filed as an essay. A field note can become an essay without changing
type.

The public site query is a join: published documents in the Essays
collection.

Folders are tables. Do not encode the folder tree as graph edges.

This is the product direction for filing. It is not a statement that
`0.9.1` already ships SQL tables.

### 3. Graph model holds argument

Related, cites, contradicts, and "this note became that essay" belong
on the graph. Use the existing epistemic types (`cites`, `contradicts`,
and the rest of `EpistemicType`) where they already fit. Add ordinary
relationship edges for related / became when those are not epistemic
claims.

Do not use the graph as a folder tree.

### Search and consumers

BM25 (`/graph/fulltext/*`) and vector search (`/graph/vector/*`)
already exist. Document body should be searchable through them once
the envelope is stored — designate the body property; do not stand up
a parallel index.

Stay aligned with markdown-theory: article / note / log are templates,
not storage types.

`public-site/` keeps markdown on disk until it can read the published
ref of documents in a collection.

### Out of the document envelope

Callout chrome, hero colors, and annotation offsets are views. They
do not live on the document row.

## Alternatives considered

### Option A — Split document / relational / graph (chosen)

- **Upside:** The editor, the public site, and argument traversal each
  hit the model that matches the query. Genre changes do not rewrite
  the document type. Publish reuses the version-pack ref machinery
  already shipped at `/graph/version/*`. Search reuses BM25 and HNSW.
  markdown-theory templates stay templates.
- **Risk:** Three models to keep coherent. A consumer that wants "all
  essays" must join published refs to collection membership instead of
  filtering a label. Mitigated by making that join the documented
  public-site query.
- **Validation:** A field note can be filed as an essay without a type
  change. Publish updates a ref, not a boolean. The public site lists
  published documents in a named collection. Graph queries for cites /
  contradicts / became do not double as folder walks.

### Option B — Essay / FieldNote / Project as first-class types or replacing node labels

- **Upside:** Fast to query "all essays" as `labels = ["Essay"]`.
  Matches a graph-only reading of the `0.9.1` snapshot.
- **Rejected because:** Filing becomes species. A field note that
  becomes an essay is a type migration, not a membership change.
  Conflicts with markdown-theory's template-not-type stance. The editor
  would open a subtype instead of a Document.

### Option C — Collections and folders as graph edges only

- **Upside:** No relational surface. Everything is already in
  `GraphStore`.
- **Rejected because:** Folder trees and unique-per-collection slugs
  are relational constraints. Encoding them as edges makes the public
  site query a traversal and makes uniqueness a convention. Folders
  are tables. The graph keeps argument.

### Option D — Store presentation chrome on the document envelope

- **Upside:** One row has everything a renderer needs: body, callout
  styling, hero colors, annotation offsets.
- **Rejected because:** Those are views. They change without changing
  the document. Putting them in the envelope couples publish to
  presentation and bloats the thing the editor opens.

### Option E — Wait for a block model

- **Upside:** Structured body from day one. Callouts and annotations
  could be first-class spans.
- **Rejected because:** v1 body is markdown. A block model can layer
  later without changing Document from "the thing the editor opens."
  Waiting blocks the public site and the editor for no filing benefit.

### Option F — `published` boolean on the document row

- **Upside:** Simple filter. No ref machinery.
- **Rejected because:** It throws away the version packs already
  shipped. Working vs published is the same shape as working tree vs
  `main`. A boolean cannot name which compiled pack is live, cannot
  roll back by moving a ref, and does not match the IDE-backend
  analogy this model is built on.

### Option G — A second document-specific versioning system

- **Upside:** Document history could look like a CMS (drafts table,
  revision rows) without using graph packs.
- **Rejected because:** `/graph/version/*` already compiles
  content-addressed packs, moves refs, logs, checkouts, and merges.
  A second history is drift. Documents use that substrate.

## Consequences

### Positive

- The editor has one species to open: Document.
- Filing can change without rewriting the document.
- Publish is a ref move over the existing version-pack substrate.
- Argument stays on the graph, next to `cites` / `contradicts`.
- The public site has a stable query shape: published documents in a
  collection.
- Search work is designation of body onto indexes that already exist.

### Negative

- `0.9.1` docs (`docs/technical/data-model.md` and the HTTP graph
  surface) remain the shipped snapshot. This ADR is ahead of the
  code. Readers must not treat Proposed as released.
- Until a relational surface exists, any implementation that stuffs
  collections into graph edges would contradict this decision even if
  it "works" on today's store.
- Yjs persistence as `YjsDoc` nodes is a CRDT implementation detail,
  not the document envelope. Bridging buffer → working ref →
  published ref is follow-up work, not specified here as an API.

### Operational

- No engine, proto, or HTTP change in the PR that records this ADR.
- `public-site/` continues to serve on-disk markdown until it can read
  published documents through this model.
- When implementation starts, document body is designated into the
  existing BM25 and vector indexes rather than growing a third search
  path.
- markdown-theory templates remain the presentation vocabulary for
  article / note / log. They do not become RustyRed storage types.
- Technical reference for the live graph stays in
  `docs/technical/data-model.md`. This ADR is the product split, not a
  replacement graph schema.

## Reversibility

Fully reversible until implementation lands. To revert the decision:

1. Mark this ADR Superseded and point at the replacement.
2. Keep storing collaborative buffers as `YjsDoc` graph nodes and
   filing as labels or edges, which is what `0.9.1` already allows.

After implementation, reversal is a migration: documents and
membership tables would fold back into graph records. The version-pack
refs can stay; they predate this ADR.

## Related

- `docs/technical/data-model.md` — shipped `0.9.1` graph snapshot
  (nodes, edges, epistemic types, content addressing). Not the
  document envelope.
- `docs/technical/http-api.md` — `/graph/version/*`,
  `/graph/fulltext/*`, `/graph/vector/*`.
- `crates/rustyred-core/src/versioned_graph.rs` —
  `DEFAULT_GRAPH_BRANCH` (`main`), compile / ref / checkout / merge.
- `crates/rustyred-core/src/graph_store.rs` — `NodeRecord`,
  `EdgeRecord`, `EpistemicType` (`cites`, `contradicts`, …).
- `crates/rustyred-core/src/fulltext.rs` — BM25 designation keyed by
  `(label, property)`.
- `crates/rustyred-server/src/yjs_sync.rs` — live CRDT buffer; persists
  as `YjsDoc` graph nodes. Transport, not the document model.
- `crates/rustyred-server/src/router.rs` — version, fulltext, vector,
  and Yjs routes as they exist today.
