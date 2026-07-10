# Memora

**Catch your AI citing sources that don't say what it claims. Cite, or it didn't happen.**

Memora retrieves *claims*, not notes - atomic facts with source-span pointers,
validity windows, and privacy bands. When the model answers, Memora re-reads the
verbatim source span behind every citation and recomputes its blake3 hash;
citations the source doesn't contain are rejected before the answer reaches you.
Hallucinated citations are caught structurally, not by trust.

→ **[See the architecture in motion](https://radotsvetkov.github.io/memora)**

Try it with no vault and no API key: `memora demo`. Once you have a vault,
`memora verify` runs the same check in CI or an eval suite and exits non-zero
on an unprovable citation — see the
[README](https://github.com/radotsvetkov/memora#verify-in-ci) for the GitHub
Action and Python wrapper.

---

## The problem

Personal AI memory tools, from RAG over Obsidian to second-brain wrappers, share one weakness: they retrieve notes and trust the
LLM to quote them faithfully. When the LLM fabricates a meeting that didn't
happen, puts words in someone's mouth, or cites a claim your notes don't
contain, you have no architectural defense. You either catch the
hallucination yourself, or you don't.

For a personal knowledge base with decisions and meeting notes, that is the wrong trust model.

## How Memora differs

The atomic unit of memory is a **claim**, not a note. A claim is an extracted
statement of fact with:

- subject, predicate, object
- source note + byte-range span
- blake3 fingerprint of the source text
- valid_from / valid_until temporal window
- privacy band (public / private / secret)
- provenance edges to source claims when synthesized

When the LLM answers, it cites claim ids. The validator re-reads the source
span from your markdown, recomputes the fingerprint, and rejects citations
that don't match. Hallucinated ids get stripped and the LLM is re-prompted
with verified-only context. **The citation contract is enforced by Rust types
and span hashes, not by prompt obedience.**

## What it guarantees - and what it does not

- **Provenance integrity (guaranteed):** the cited source span verbatim exists, is
  unmodified (hash-proven via full-width blake3), and contains any quoted text. This
  is strictly stronger than model-asserted citation APIs.
- **Entailment (optional, not guaranteed):** whether the source actually *supports*
  the conclusion (rather than just containing the quote) is a separate, harder
  question. `memora verify --entailment` runs an opt-in, LLM-judged check for it,
  kept clearly separate from the hash-proven provenance.

## What you get

| | |
|---|---|
| **Verified citations** | Every claim_id in an answer is re-validated against the source span before reaching you. Hallucinations stripped. |
| **Provenance + staleness** | Synthesis claims point to sources. Edit a note, downstream syntheses are auto-marked stale. |
| **Time-aware reasoning** | Claims have validity windows. "What was true in March" is queryable. Contradictions auto-supersede. |
| **Per-claim privacy** | Inline `<!--privacy:secret-->...<!--/privacy-->` markers. Secrets redacted at the wire boundary on cloud LLMs, type-system enforced. |
| **Active challenger** | Daily background worker surfaces stale claims, contradictions, cross-region patterns, and frontier gaps in your `world_map.md`. |
| **Document ingestion** | `memora ingest` pulls PDFs, web pages, text, and transcripts into the vault as verifiable notes (PDF/web behind opt-in features). |
| **Visual report** | `memora report` renders the vault — interactive claim graph, contradictions, stale dependencies, world map — as one offline HTML file. |
| **Optional entailment** | `memora verify --entailment` adds a best-effort, LLM-judged check that the source supports the claim, separate from the proof. |
| **Hybrid retrieval** | BM25 + embedding + reciprocal-rank fusion. |
| **Local-first** | Single Rust binary. SQLite + HNSW. Works fully offline with Ollama or a bundled on-device embedder. |
| **Obsidian-native** | Plain markdown vault with frontmatter. Open and edit in Obsidian alongside Memora. |
| **MCP-native** | Drop into Claude Code, Cursor, or any MCP client over stdio. |

## Where to go next

- **[Quickstart](./quickstart.md)** - install and first verified citation in 10 minutes.
- **[Ingesting documents](./ingesting.md)** - bring PDFs, web pages, and transcripts into the vault.
- **[Architecture](./architecture.md)** - claim graph, retrieval, validation pipeline.
- **[Obsidian guide](./obsidian-guide.md)** - daily-driver setup with Claude Code.
- **[Comparison](./comparison.md)** - honest breakdown vs Mem0, Zep, Letta, and Anthropic Citations.
- **[MCP tools](./mcp-tools.md)** - every tool, with examples.

## Status

v0.2.0. Indexes 100-note vaults in 5 to 10 minutes with Claude Haiku for about $0.30. Local Ollama is supported. Vault sizes up to a few thousand notes are the target. Larger scales are unmeasured. The active challenger surfaces decisions, contradictions, stale dependencies, and open questions in every atlas. MCP and watch now share the same embedder config, privacy redaction, and claim extraction pipeline as the CLI.

Issues, edge cases, and design discussions welcome at
[github.com/radotsvetkov/memora/issues](https://github.com/radotsvetkov/memora/issues).
