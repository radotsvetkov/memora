# Memora Architecture Deep Dive

**Verifiable cognitive memory for personal vaults. Cite-or-it-didn't-happen.**

Memora retrieves claims, not notes - atomic facts with source-span pointers,
validity windows, and privacy bands. Every LLM citation is architecturally
validated against your markdown.

This document describes the concrete system design in four layers:

1. Note Graph (vault and markdown substrate)
2. Claim Graph (typed factual representation)
3. Retrieval and Validation (query-time execution)
4. Agent Interfaces (MCP tools and client integration)

The goal is not to maximize retrieval throughput at all costs. The goal is to
make unsupported claims difficult to surface and easy to detect.

## Layered System Diagram

```text
+--------------------------------------------------------------------------+
| Layer 4: Agent Interfaces (MCP over stdio)                               |
|  memora_query_cited | memora_capture | memora_stale_claims | challenger  |
+--------------------------------------------------------------------------+
                                  |
                                  v
+--------------------------------------------------------------------------+
| Layer 3: Retrieval + Validation                                          |
|  Hybrid retrieval (BM25 + vector + RRF)                                  |
|  Spreading activation over links/edges                                   |
|  Privacy filter + typed prompt builder                                   |
|  Citation validator (span re-read + fingerprint recompute + retry)       |
+--------------------------------------------------------------------------+
                                  |
                                  v
+--------------------------------------------------------------------------+
| Layer 2: Claim Graph (SQLite + edge model)                               |
|  claim(subject,predicate,object,span,fingerprint,time,privacy,status)    |
|  edges: entails | contradicts | supersedes | derives | co_occurs         |
|  provenance DAG + stale propagation                                      |
+--------------------------------------------------------------------------+
                                  |
                                  v
+--------------------------------------------------------------------------+
| Layer 1: Note Graph (Obsidian vault + watcher)                           |
|  markdown notes + frontmatter + inline privacy markers                   |
|  file events -> parse -> extraction jobs                                 |
+--------------------------------------------------------------------------+
```

## Design Principles

Memora uses a narrow set of invariants:

- **Claims are addressable units**. All downstream reasoning references claim
  ids, not free text snippets.
- **Source spans are mandatory**. Every extracted claim stores a byte range into
  a specific markdown file.
- **Fingerprints are mandatory**. A blake3 hash over the exact source span is
  captured at extraction and checked at validation time.
- **Validity is explicit**. Claims can become historical (`valid_until`) without
  disappearing from history.
- **Privacy is monotonic**. A claim can only move toward stricter visibility
  when inherited or overridden.

These rules are enforced in the data model and request pipeline, not only by
prompts or post-hoc heuristics.

## Layer 1: Note Graph

The Note Graph is the user-owned vault. Memora does not replace it and does not
rewrite user prose as a primary persistence strategy. Notes remain markdown
files editable in Obsidian or any text editor.

### 1.1 Vault as source of truth

Memora treats the vault as canonical for natural language content. Derived state
is stored in `.memora/` and the local database; the authoritative prose remains
in markdown.

Input channels into this layer:

- manual note edits in Obsidian
- captured notes from MCP tools (for example `memora_capture`)
- file-level moves, renames, and deletions

### 1.2 Structural parsing

Each note is parsed for:

- frontmatter fields (type, date, region, privacy, tags, status, etc.)
- body sections
- inline privacy markers:
  `<!--privacy:secret--> ... <!--/privacy-->`

The parser outputs typed segments with byte offsets. These byte offsets are the
backbone for source-span references in claims.

### 1.3 Incremental change detection

A watcher detects modified files and schedules extraction jobs. The queue keeps
work bounded and avoids full re-indexing for every keystroke. Hash-based change
detection ensures unchanged notes are skipped.

### 1.4 Why this layer matters

Most memory systems jump directly from note chunks to retrieval. Memora adds a
formal extraction boundary so claims can carry strict provenance. This boundary
is what makes downstream citation checks deterministic.

## Layer 2: Claim Graph

The Claim Graph is the core memory model. A claim has typed fields and graph
relations to other claims.

### 2.1 Claim schema

Core fields:

- `subject`, `predicate`, `object`
- `source_note`
- `source_span_start`, `source_span_end` (byte offsets)
- `span_fingerprint` (blake3)
- `valid_from`, `valid_until`
- `privacy_band` (`public`, `private`, `secret`)
- lifecycle flags (`current`, `superseded`, `stale`, etc.)

This schema separates statement identity from rendering text. The same concept
can be tracked across time with explicit supersession edges rather than silent
overwrite.

### 2.2 Edge model

Memora stores directional, typed edges:

- `entails`
- `contradicts`
- `supersedes`
- `derives`
- `co_occurs`

Edges support both retrieval-time context expansion and maintenance-time staleness
propagation.

### 2.3 Storage strategy

Primary persistence is SQLite:

- relational tables for claims and edges
- FTS5 index for lexical retrieval
- JSON1-compatible metadata fields for structured payloads

Embeddings are stored for semantic retrieval and indexed with HNSW. This allows
hybrid retrieval while keeping the memory model local-first.

### 2.4 Provenance and stale propagation

Synthesis claims store provenance links to source claims (`derives`). If any
source becomes invalid or superseded, dependent syntheses are marked stale.

This stale bit is not an error state. It is a review signal: the synthesis might
still be useful, but it should be revalidated against current inputs.

## Layer 3: Retrieval and Validation

This layer is where query-time guarantees are enforced.

### 3.1 Retrieval cascade

Retrieval is not a single query operation. It is a staged cascade:

1. lexical retrieval from FTS5 (BM25-style scoring)
2. vector retrieval from HNSW embeddings
3. reciprocal-rank fusion (RRF)
4. graph expansion via co-activation and spreading activation
5. privacy filtering
6. prompt context assembly

The output is a ranked claim set, not note paragraphs.

### 3.2 Citation validator

The validator checks every claim id cited by the model:

1. fetch claim row by id
2. open source markdown note
3. read exact byte span
4. recompute blake3 fingerprint
5. compare to stored fingerprint

If any citation fails (missing id, span mismatch, fingerprint mismatch), that
citation is rejected. Depending on tool mode, rejected citations are stripped and
the model is retried using verified-only context.

### 3.3 Retry semantics

Retry is scoped and deterministic:

- first pass: full retrieved context
- validation pass: verify cited ids and spans
- fallback pass: re-prompt with only validated claim context

This avoids silently passing unverifiable citations through to the user.

### 3.4 Temporal and contradiction awareness

Retrieval defaults to currently valid claims. Historical queries can include
time filters to reconstruct prior states without flattening history.

Contradictions are represented explicitly and alter validity windows rather than
deleting data. That keeps historical trails auditable.

## Layer 4: Agent Interfaces

Memora exposes capabilities through MCP over stdio. Clients query tools instead
of linking to private internals.

### 4.1 Tool surface

Representative interfaces include:

- cited query tool for validated responses
- capture tools for episodic logging
- contradiction and stale-claim inspection tools
- challenger outputs for periodic review

Tool responses are shaped around claim ids and provenance so clients can show
evidence, not just summaries.

### 4.2 Why MCP boundary matters

The interface layer keeps guarantees portable across clients. A user can switch
between environments without changing citation behavior because validation lives
inside Memora, not in client-specific prompt templates.

### 4.3 Failure mode handling

If external model output includes invalid citations, Memora handles that inside
the pipeline. The client receives either verified results or explicit failure
signals, not a hidden downgrade.

## Claim Lifecycle Walk-through

This is a concrete path for one statement from note to cited answer.

1. **User writes text** in `semantic/projects/drift/roadmap.md`:
   "drift switched serialization from JSON to MessagePack after throughput benchmarks."
2. **Watcher detects file change** and schedules extraction.
3. **Extractor emits claim candidate**:
   `drift | uses_serialization | messagepack`.
4. **Source span recorded** as byte offsets into the markdown file.
5. **Fingerprint computed** over that exact span and stored in claim row.
6. **Claim indexed** in FTS and embedding index; edges updated as needed.
7. **User asks query** through an MCP client.
8. **Retrieval returns ranked claim set** including this claim id.
9. **Model drafts answer** and cites `[claim:abc123]`.
10. **Validator re-opens source note**, slices stored byte range, recomputes
    blake3, and confirms it matches stored fingerprint.
11. **Citation marked verified** and answer is returned.
12. **User sees output** with verified citations that map back to source text.

If step 10 fails, citation is removed and the retry path runs on verified-only
claims.

## Contradiction Handling Walk-through

Contradictions are a first-class maintenance event.

1. A new claim arrives: `drift-bench | uses_language | go`.
2. System fetches candidate claims with matching subject/predicate and current
   validity windows.
3. Contradiction judge evaluates pairs (`new`, `candidate`) for conflict, such as `go` vs `rust`.
4. If conflict is confirmed:
   - older claim receives `valid_until = new.valid_from` (or event timestamp)
   - `supersedes` edge is written (`new -> old`)
   - `contradicts` edge is written for explainability
5. Derivative claims linked to the older claim are marked stale.
6. Challenger later surfaces stale derivatives for review/regeneration.

Two properties are intentional:

- old data is retained for historical reasoning
- current reasoning can exclude superseded claims by default

## Privacy Redaction Walk-through

Privacy is applied during extraction and enforced before prompt construction.

1. Parser reads note-level frontmatter privacy.
2. Parser reads inline privacy markers in body spans.
3. Each extracted claim receives `privacy = max(note_level, inline_level)`.
4. Query pipeline applies privacy filter before prompt assembly.
5. Prompt builder accepts typed claim wrappers that distinguish:
   - local-safe claims
   - redacted claims for cloud transport
6. For cloud model calls, secret claims are transformed:
   - `subject` preserved
   - `predicate`, `object` replaced with `[redacted]`
7. Non-redacted secret claims are rejected by type constraints and cannot enter
   outbound model payload construction.

This architecture turns privacy from a policy suggestion into a compile-time and
runtime boundary.

## Five Differentiators

### 1) Architectural citation enforcement

Most systems ask the model to cite correctly and then trust it. Memora stores
source spans and fingerprints at extraction time, then verifies cited ids by
re-reading markdown at response time. This allows deterministic rejection of
invalid citations and a retry path constrained to verified context.

### 2) Claim-first memory model

The memory primitive is an atomic claim, not a note chunk. Claims carry temporal
state, privacy, provenance, and edges. That enables operations like supersession,
stale propagation, and relation-aware retrieval that are difficult to represent
when the only unit is free text.

### 3) Time-aware contradiction handling

Contradictions are modeled as graph relations with validity window updates. New
claims can supersede old ones without deleting history. Queries can target either
the current world state or historical states, and stale derivatives can be
identified explicitly.

### 4) Per-claim privacy with typed redaction

Privacy is resolved at claim level using note metadata plus inline marker
overrides. Secret claims can still contribute structurally while restricting
outbound content. Cloud-bound payloads enforce redaction before model transport,
which is stronger than relying on operators to remember manual filtering.

### 5) Active challenger loop

Memora runs a periodic challenger that inspects stale dependencies,
contradictions, and sparsely connected frontier areas. This is not just retrieval:
it is maintenance of memory integrity over time. The result is a continuously
reviewable world map instead of a static index.

## What We Do Not Do

- **Not a faster RAG benchmark story**: Memora's primary claim is architectural
  verifiability. Benchmark speed/latency comparisons against other systems are
  not published here.
- **Not a multimodal memory store yet**: current substrate is markdown-first with
  structured metadata; rich multimodal ingestion is future work.
- **Not federated multi-vault reasoning yet**: primary target is one personal
  vault and its sidecar state.
- **Not a research-agent framework**: Memora is a memory engine and interface
  layer, not an autonomous planner for open-ended web research.

## Operational Notes

- Local-first operation works offline with local model backends.
- Derived state is reproducible from vault content plus config.
- Citation verification depends on byte-accurate spans; manual file rewrites are
  expected and handled through re-indexing.
- Claims may be skipped when extraction confidence or structure is insufficient;
  absence of a claim does not imply absence of prose.

## Closing

Memora is engineered around a strict contract: if an answer cites your notes,
the citation must survive structural checks against the source text. The system
is intentionally conservative where trust boundaries are involved, and that
conservatism is what enables personal-memory workflows where incorrect citations
are treated as system errors, not normal behavior.
