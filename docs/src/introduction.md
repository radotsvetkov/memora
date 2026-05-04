# Memora

**Verifiable cognitive memory for personal vaults. Cite-or-it-didn't-happen.**

Memora retrieves *claims*, not notes - atomic facts with source-span pointers,
validity windows, and privacy bands. Every LLM citation is architecturally
validated against your markdown. Hallucinations are caught structurally, not
by trust.

→ **[See the architecture in motion](https://radotsvetkov.github.io/memora)**

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

## What you get

| | |
|---|---|
| **Verified citations** | Every claim_id in an answer is re-validated against the source span before reaching you. Hallucinations stripped. |
| **Provenance + staleness** | Synthesis claims point to sources. Edit a note, downstream syntheses are auto-marked stale. |
| **Time-aware reasoning** | Claims have validity windows. "What was true in March" is queryable. Contradictions auto-supersede. |
| **Per-claim privacy** | Inline `<!--privacy:secret-->...<!--/privacy-->` markers. Secrets redacted at the wire boundary on cloud LLMs, type-system enforced. |
| **Active challenger** | Daily background worker surfaces stale claims, contradictions, cross-region patterns, and frontier gaps in your `world_map.md`. |
| **Hybrid retrieval** | BM25 + embedding + RRF fusion + Hebbian co-activation learning + spreading activation via wikilinks. |
| **Local-first** | Single Rust binary. SQLite + HNSW. Works fully offline with Ollama. |
| **Obsidian-native** | Plain markdown vault with frontmatter. Open and edit in Obsidian alongside Memora. |
| **MCP-native** | Drop into Claude Code, Cursor, or any MCP client over stdio. |

## Where to go next

- **[Quickstart](./quickstart.md)** - install and first verified citation in 10 minutes.
- **[Architecture](./architecture.md)** - claim graph, retrieval, validation pipeline.
- **[Obsidian guide](./obsidian-guide.md)** - daily-driver setup with Claude Code.
- **[Comparison](./comparison.md)** - vs RAG, LLM Wiki, and other systems.
- **[MCP tools](./mcp-tools.md)** - every tool, with examples.

## Status

v0.1.26. Indexes 100-note vaults in 5 to 10 minutes with Claude Haiku for about $0.30. Local Ollama is supported. Vault sizes up to a few thousand notes are the target. Larger scales are unmeasured. The active challenger surfaces decisions, contradictions, stale dependencies, and open questions in every atlas.

Issues, edge cases, and design discussions welcome at
[github.com/radotsvetkov/memora/issues](https://github.com/radotsvetkov/memora/issues).
