# Public Projection

Status: Proposed
Date: 2026-08-19
Decision drivers: Travis Gilbert
Implementation target: extracted RustyRed / Theorem line, not this
public `0.9.1` graph-first snapshot

Companion plan to [ADR 0002](../adr/0002-document-model.md). 0002 is
the model split (document / relational filing / graph argument). This
file is the compile story for travisgilbert.me. It is not a second
ADR and it does not change the split.

Do not implement this against `0.9.1` in this repo. `public-site/`
(travisgilbert.me thin extract) stays on-disk markdown until the
extracted backend exists.

## The thing

travisgilbert.me is a **compile target of Theorem**, not a CMS and not
a giant backend application.

Protect the simplicity of `public-site/`: static, reads simple Markdown
collections, strips CommonPlace / Studio / graph explorer / Monaco /
etc.

The relationship is:

```
work → Theorem/RustyRed → public projection → travisgilbert.me
```

Not:

```
travisgilbert.me → giant backend application
```

Theorem can know everything. The site can know almost nothing.

When you publish, a Theorem capability builds a versioned
`public-site-manifest` of only approved objects and relations. That
materializes into the public-site build as JSON/Markdown. Next.js
statically renders it the way it currently renders `content/`. For
anything interactive later (Ask My Work, public MCP) there is one
narrow public read-only Theorem service. Everything else works if
Theorem is down. The job portfolio must never be unavailable because
the backend is having opinions.

## Stay aligned with ADR 0002

- One Document: `id`, `body` (markdown v1), optional `title`, working
  and published refs. Publish is a ref move, not a boolean.
- Essay / field note / project / writing are not storage types. Filing
  is relational: collections, membership, slugs unique per collection.
- Graph holds argument: related, cites, contradicts, became.
- Callout chrome, hero colors, and annotation offsets stay out of the
  envelope. Those are views.

## Facets and non-document objects

**publishable** is a facet, not a type: slug, visibility, summary,
published date, featured. Combined with the published ref.

Project, Writing, Artifact, Skill, Experience, Demo, ResumeClaim,
Source, and Publication are not first-class storage types that replace
Document.

Some of those are documents filed a certain way. Some are relational
or graph objects that cite documents (ResumeClaim, Skill, Experience,
Artifact, Source).

Related work on the site: three useful connections with a reason
("shared question", causal, etc.). Do not bring back the force-directed
graph UI.

## Do not migrate markdown now

Keep the new Markdown site as canonical initially. Ingest those
documents into Theorem. Build projection features around them. Reverse
the flow (Theorem materializes Markdown/static projection) only once
authoring in Theorem is actually preferred.

The job hunt is active. Do not let "improve personal site" become
"finish Theorem's document platform."

## Ship ladder

Restrained. Each rung assumes the previous is already serving the
portfolio.

1. **First ship:** thin static site + Theorem public graph + Related
   Work + structured project/artifact evidence.
2. **Then** job-specific portfolio views (for example
   `/for/product-systems`, or a private application link). No
   compatibility-percentage meter.
3. **Then** Ask My Work: search over the public projection only,
   answers with real projects / essays / artifacts — not a homepage
   chatbot.
4. **Then** a tiny read-only public MCP/API (`search_work`,
   `get_project`, `get_writing`, `get_resume_fact`, `get_artifact`).
   Footer can say the portfolio is machine-readable.

Standing Brief / derived `/now` is later: draft from activity, human
approves before publish.

## Alternatives considered

### Option A — Compile target (chosen)

- **Upside:** `public-site/` stays a static Markdown/JSON build. The
  portfolio survives Theorem being down. Publish is still ADR 0002's
  ref move; the manifest is a projection of approved published refs
  and relations, not a second CMS.
- **Rejected the inverse because:** a giant backend behind
  travisgilbert.me makes the job site depend on Theorem's opinions at
  request time. That is the failure mode this plan exists to prevent.

### Option B — CMS in the site

- **Upside:** Edit essays in Next.js. No extract required to ship
  copy.
- **Rejected because:** the site becomes the system of record. Filing,
  argument, and publish refs leak into `public-site/`. Theorem then
  has to scrape its own compile target. Conflicts with "the site can
  know almost nothing."

### Option C — Project / Writing as storage types that replace Document

- **Upside:** The site query looks like `type = Project`.
- **Rejected because:** ADR 0002. Those names are filing or citing
  objects, not species. A publishable facet plus collection membership
  plus a published ref is enough for the manifest. Typed
  Project/Writing storage would freeze genre into the envelope.

## Consequences

### Positive

- `public-site/` can stay thin: Markdown collections in, static pages
  out.
- Publish remains a ref move on a Document. The manifest is a
  downstream compile of approved published objects and relations.
- Interactive features (Ask My Work, public MCP) have one narrow
  read-only service. The rest of the site does not wait on it.
- The job hunt is not gated on finishing Theorem's document platform.

### Negative

- Until the extract exists, the site cannot actually compile from
  Theorem. On-disk markdown stays canonical; ingest is one-way.
- A versioned manifest is another artifact to keep coherent with
  published refs. It is a projection, not a second versioning system
  (ADR 0002: reuse `/graph/version/*` on the extract).
- Related Work is three reasoned links, not a graph explorer. Anyone
  expecting the old force-directed UI will not get it.

### Operational

- Do not implement document tables, collection membership, a
  published-ref API, or this projection against `0.9.1` in this repo.
- Do not migrate markdown off disk until authoring in Theorem is
  preferred.
- No "Powered by Theorem" chatbot on the homepage.
- No compatibility-percentage meter on job-specific views.
- Standing Brief / `/now` waits; a human approves before publish.

## Out of scope

- Engine, proto, or HTTP work in this public `0.9.1` snapshot.
- Turning `public-site/` into CommonPlace, Studio, a graph explorer,
  or a Monaco editor.
- Force-directed graph UI.
- Homepage chatbot.
- Reversing Markdown canonicalization before Theorem authoring is
  actually preferred.
- Inventing first-class storage types for Project / Writing /
  Artifact / Skill / Experience / Demo / ResumeClaim / Source /
  Publication.

## Related

- [ADR 0002](../adr/0002-document-model.md) — document / filing /
  argument split. This plan consumes that split; it does not replace
  it.
- `public-site/` on travisgilbert.me — thin static extract. Canonical
  markdown until the extracted backend exists.
