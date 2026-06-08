# How Memora compares

**Catch your AI citing sources that don't say what it claims.**

This page is deliberately honest about where Memora leads and where it is behind.
It is an *architectural* comparison, not a published head-to-head benchmark.

## The one claim no funded vendor makes

Memora performs **post-generation, hash-reverified citation rejection**: after the
model answers, Memora re-reads the verbatim source span behind each cited claim,
recomputes its blake3 fingerprint, and **strips any citation the source does not
actually contain** before the answer is returned. The contract is enforced by Rust
types and span hashes, not by prompting.

No funded agent-memory vendor does this. Mem0, Zep, Letta, and Cognee retrieve and
store memory well, but they do **not** re-read the source and reject citations.
Anthropic's Citations API returns offsets the model asserts — it does not
independently re-read and re-hash them.

| | Memora | Mem0 / Zep / Letta / Cognee | Anthropic Citations API |
|---|---|---|---|
| Hash-reverified citation rejection | **Yes** | No (no re-read / rejection) | Model-asserted, not re-hashed |
| Entailment — does the quote *support* the claim | No — provenance only, by design | Partial (LLM-judged) | No |
| Temporal validity windows | Yes | Yes (Zep/Graphiti) | — |
| Contradiction / supersession | Yes | Yes (Zep/Graphiti) | — |
| Per-claim privacy redaction at the wire | Yes | Varies | — |
| Local-first, single binary, no service | Yes | Mostly hosted/service | Cloud API |
| Scale, integrations breadth, multimodal | **Behind** | Ahead | — |
| Hosted offering, ecosystem, traction | **Behind** | Ahead | Ahead |

**Two of these rows matter more than the rest, and they are the honest ones:**
the "provenance only, by design" row (we tell you exactly where our guarantee ends)
and the "behind on scale/ecosystem" row (we are not pretending to out-scale a funded
team). Those two admissions are what make the top row believable.

## What Memora guarantees — and what it does not

- **Provenance integrity (guaranteed):** the cited source span verbatim exists, is
  unmodified (hash-proven), and contains any quoted text. This is strictly stronger
  than model-asserted citations.
- **Entailment (not guaranteed):** Memora does not judge whether the quote *supports*
  the surrounding conclusion. A model can cite a real span and still draw an
  unsupported inference. Entailment scoring is on the roadmap.

## The landscape, by category

For each system the question is narrow: *when an LLM answer must be grounded in your
content, how is the citation verified?*

### Agent-memory layers — Mem0, Zep / Graphiti, Letta, Cognee

The category leaders. Strong on storing, recalling, and (Zep/Graphiti) temporally
modeling memory across sessions, with broad framework integrations and hosted
offerings. Citations, where present, are **model- or prompt-level** — the system does
not re-read the original source span and reject a citation whose bytes don't match.
Memora is behind these on scale, integrations, and ecosystem, and overlaps them on
temporal validity and contradiction handling (so Memora does **not** market those as
unique). Memora's divergence is the hash-reverified rejection step and per-claim
privacy redaction at the wire boundary.

### Grounded-citation APIs — Anthropic Citations

Returns source locations the model asserts for a generated answer. Useful and
first-party, but the offsets are not independently re-read and re-hashed against the
source after generation. Memora's divergence: it treats the model's citation as a
*claim to be re-proven*, not as ground truth — and rejects it on a hash mismatch.

### PKM-AI over notes — Obsidian Copilot, Smart Connections, Khoj, Notion AI

Excellent zero-config retrieval and chat over personal notes, often free and local.
Citations are typically prompt-level: the model is asked to reference notes, with no
strict post-generation verification of each cited identifier against an immutable
span. Memora is slower and higher-friction here (it indexes claims and needs an LLM
for extraction), and diverges by enforcing the citation contract structurally rather
than trusting the model to quote faithfully.

### Traditional RAG over Obsidian / markdown

Index chunks, retrieve top-k, pass to the model with citation guidance in the prompt.
Temporal reasoning is usually recency-biased; privacy is folder/note-level;
synthesis staleness is rarely tracked. Memora diverges by retrieving **atomic claims
with byte-range provenance**, verifying every cited id against its source span,
modeling validity windows and supersession, and tracking stale derived claims over a
provenance DAG.

## Summary matrix (architectural)

| Dimension | Traditional RAG | PKM-AI (Copilot/Khoj/…) | Agent memory (Mem0/Zep/Letta) | Anthropic Citations | Memora |
|---|---|---|---|---|---|
| Atomic unit | chunks | notes/snippets | memories/graph nodes | model spans | atomic claims |
| Citation model | prompt-level | prompt-level | prompt/model-level | model-asserted offsets | **hash-reverified rejection** |
| Temporal model | recency | manual | recency / bi-temporal (Zep) | — | validity windows + supersession |
| Privacy model | folder/note | folder/note | varies | — | per-claim + typed wire redaction |
| Staleness tracking | usually none | usually none | varies | — | provenance DAG + stale flags |
| Deployment | library | app/plugin | mostly hosted | cloud API | local single binary |

## Final note on benchmarking

This is an **architectural** comparison. Memora has not been run in published
head-to-head retrieval benchmarks against these systems, and this page makes no such
numeric claims. The one number Memora does publish — its citation-rejection rate over
a labeled fixture — is reproducible locally with `make bench` and is described in the
[README](https://github.com/radotsvetkov/memora#reproducible-proof). Vendor
capabilities evolve; verify current feature sets against each vendor's own docs.
