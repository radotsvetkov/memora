# Architectural Comparison

**Verifiable cognitive memory for personal vaults. Cite-or-it-didn't-happen.**

Memora retrieves claims, not notes — atomic facts with source-span pointers,
validity windows, and privacy bands. Every LLM citation is architecturally
validated against your markdown.

This document compares memory architectures, not benchmark performance.

## Scope and framing

The systems below are useful in different contexts, and several have influenced
how practitioners think about personal knowledge tooling. The comparison here
focuses on one narrow question:

**How does each system model memory when an LLM answer needs to be grounded in a
user's personal notes?**

For each system, we look at:

- primary unit of retrieval
- citation mechanism
- temporal state handling
- privacy boundaries
- synthesis staleness handling
- where Memora diverges

## 1) Traditional RAG over Obsidian

**One-sentence description:** Index markdown notes into chunks, retrieve top-k
chunks with keyword/vector search, and pass those chunks to the model as context.

### Architectural primitive

The core primitive is a **chunk of note text** (often overlapping windows).
Metadata may include file path, heading, tags, or timestamps, but chunk text is
still the object supplied directly to the model.

### Citations

Usually **prompt-level citation guidance**. The prompt asks the model to cite
notes or quote excerpts, but the pipeline often does not perform a strict
post-generation verification of every cited identifier against immutable spans.

### Temporal reasoning

Typically weak. Some setups use file modified time as a recency signal, which is
helpful for ranking but not equivalent to claim-level validity windows. Temporal
queries often degrade into "newer chunks rank higher."

### Privacy handling

Commonly note-level or folder-level segregation (for example separate sensitive
folders excluded from indexing). Fine-grained within-note redaction is possible,
but usually depends on conventions, not typed enforcement.

### Synthesis staleness

Rarely modeled explicitly. If a synthesis answer from last week depends on notes
that changed today, the system does not track that dependency graph by default.

### Where Memora diverges

Memora extracts **atomic claims** with source byte spans and fingerprints. The
model cites claim ids; citations are verified against markdown spans before output
is accepted. Contradictions and supersession update claim validity windows, and
derived claims can be marked stale when their dependencies change.

## 2) LLM Wiki (Karpathy pattern)

**One-sentence description:** Curated wiki-style notes are maintained and queried
through model prompts, with emphasis on human-maintained pages and summaries.

### Architectural primitive

The primitive is generally the **wiki page / full note** rather than atomic
factual units. Retrieval can be simple grep-like search, embedding search, or
manual page curation.

### Citations

Again mostly **prompt-level trust**. The model is asked to reference sections or
pages, but strict claim-id-to-span validation is typically outside scope.

### Temporal reasoning

Usually manual. Edits update the page narrative, but there is no built-in
claim-level time window model unless custom logic is added.

### Privacy handling

Mostly repository-level or file-level policies. Fine-grained secret redaction can
be done procedurally, but not usually via a typed claim transport boundary.

### Synthesis staleness

Not tracked as a first-class graph concern in the base pattern. If summaries
depend on old sections, freshness is largely a human maintenance task.

### Where Memora diverges

Memora keeps note prose but converts factual content into explicit claim records
with provenance edges. Staleness becomes computable (`derives` edges + stale
marks) and contradiction handling updates validity windows instead of relying on
manual page rewrites.

## 3) Ori-Mnemos

**One-sentence description:** A memory-oriented framework focused on organizing
and recalling information from accumulated user interactions.

### Architectural primitive

Typically event/memory entries or chunk-like artifacts depending on deployment.
The unit is still generally larger and less strictly typed than subject-predicate-
object claims with immutable source spans.

### Citations

In most practical setups, citations are either absent or generated via prompt
conventions. There is limited evidence of mandatory span-hash validation against
source markdown at answer time.

### Temporal reasoning

Temporal metadata may exist at entry level, but claim-level supersession and
contradiction windows are not generally the default mechanism.

### Privacy handling

Commonly policy/config driven at source or collection level. Per-claim typed
redaction at model boundary is not the typical baseline.

### Synthesis staleness

Dependency-tracked stale propagation is not usually central. Refresh behavior is
often periodic re-summarization rather than DAG-aware invalidation.

### Where Memora diverges

Memora's core divergence is strict verifiability: every claim maps to a source
span with fingerprint, and cited ids are validated structurally. Temporal changes
are represented as supersession edges and validity updates, not only recency.

## 4) MDEMG

**One-sentence description:** A memory graph style approach that structures
knowledge for retrieval and agent use across sessions.

### Architectural primitive

Graph nodes often represent memories/documents/entities, with varying granularity
and schema. The primitive can be richer than flat chunks, but is not necessarily
anchored to byte-accurate spans in user markdown.

### Citations

Often graph-aware retrieval with model-mediated explanation. Citation guarantees
are typically **semantic/prompt level**, not strict span-hash contracts.

### Temporal reasoning

Some graph memory systems include timestamps or recency weighting, but explicit
`valid_from`/`valid_until` per atomic claim with contradiction-based supersession
is uncommon as a default.

### Privacy handling

Depends on deployment policy and node metadata. Fine-grained transport redaction
for secret fragments is usually custom rather than enforced by a dedicated typed
pipeline.

### Synthesis staleness

Graph updates may occur, but "this derived claim is stale because one ancestor
was superseded" is not universally enforced without additional logic.

### Where Memora diverges

Memora combines graph edges with strict source anchoring and citation validation.
It treats provenance and stale propagation as standard maintenance behavior, not
optional add-ons.

## 5) Vanilla Obsidian + Claude Code (no memory layer)

**One-sentence description:** Use Obsidian as note storage and ask questions in
Claude Code directly, without a dedicated extraction/claim memory engine.

### Architectural primitive

The primitive is whatever context is manually selected or searched at runtime:
files, snippets, or ad hoc passages. There is no persistent claim graph.

### Citations

Citations depend on what the model is prompted to provide and what context was
manually supplied. Verification is user-driven rather than architecturally
enforced.

### Temporal reasoning

Handled manually by inspecting timestamps, file history, or narrative context.
No built-in claim-level temporal windows or supersession semantics.

### Privacy handling

Relies on user judgment and workspace boundaries. There is no dedicated claim
privacy lattice with mandatory redaction at model transport boundaries.

### Synthesis staleness

Not tracked by default. If prior synthesis depended on now-edited notes, there
is no automatic stale marker.

### Where Memora diverges

Memora adds a dedicated memory backend between vault and model:
claim extraction, provenance, contradiction handling, stale propagation, and
citation verification. The user still owns markdown, but verification does not
depend on manual checking for every answer.

## Summary Matrix

| Dimension | RAG over Obsidian | LLM Wiki | Ori-Mnemos | MDEMG | Obsidian + Claude Code | Memora |
|---|---|---|---|---|---|---|
| Atomic unit | chunks | pages/notes | memory entries | graph nodes/memories | ad hoc snippets | atomic claims |
| Citation model | prompt-level | prompt-level | prompt-level/varies | semantic/prompt-level | manual/prompt-level | architectural validation |
| Temporal model | recency-biased | manual edits | entry timestamps | recency/metadata | manual | validity windows + supersession |
| Privacy model | folder/note policy | file policy | policy/config | metadata/policy | manual | per-claim + typed redaction |
| Staleness tracking | usually none | usually none | limited | limited/custom | none | provenance DAG + stale flags |

## Final note on benchmarking

This is an **architectural** comparison, not a performance comparison. Memora has
not been run in published head-to-head benchmarks against these systems. The
distinctions here are about memory modeling, citation guarantees, temporal
representation, and privacy boundaries — not claims about retrieval speed or
quality rankings.
