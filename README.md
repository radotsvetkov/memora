# Memora

**Verifiable cognitive memory for personal vaults. Cite-or-it-didn't-happen.**

Memora retrieves *claims*, not notes — atomic facts with source-span
pointers, validity windows, and privacy bands. Every LLM citation is
architecturally validated against your markdown. Hallucinations are
caught structurally, not by trust.

→ **[See the architecture in motion](https://radotsvetkov.github.io/memora)**

---

## The problem

Personal AI memory tools — RAG over Obsidian, Karpathy's LLM Wiki
pattern, second-brain wrappers — share one weakness: they retrieve
notes and trust the LLM to quote them faithfully. When the LLM
fabricates a meeting that didn't happen, puts words in someone's
mouth, or cites a claim your notes don't contain, you have no
architectural defense. You either catch the hallucination yourself,
or you don't.

For a personal knowledge base — your decisions, your medical notes,
your meeting minutes — that's the wrong trust model.

## How Memora differs

The atomic unit of memory is a **claim**, not a note. A claim is an
extracted statement of fact with:

- subject, predicate, object
- source note + byte-range span
- blake3 fingerprint of the source text
- valid_from / valid_until temporal window
- privacy band (public / private / secret)
- provenance edges to source claims when synthesized

When the LLM answers, it cites claim ids. The validator re-reads
the source span from your markdown, recomputes the fingerprint, and
rejects citations that don't match. Hallucinated ids get stripped
and the LLM is re-prompted with verified-only context. The citation
contract is enforced by Rust types and span hashes — not by prompt
obedience.

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

## Quickstart

```bash
# install
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/radotsvetkov/memora/releases/latest/download/memora-cli-installer.sh | sh

# initialize a vault
memora init --vault ~/brain

# index it
memora index --vault ~/brain

# ask something
memora query "What did I decide about the Q1 roadmap?" --vault ~/brain
```

Full guide: [Quickstart](docs/src/quickstart.md) ·
[online docs](https://radotsvetkov.github.io/memora/docs/quickstart.html)

## Use it with Claude Code

Add this to your Claude Code MCP config:

```json
{
  "mcpServers": {
    "memora": {
      "command": "/usr/local/bin/memora-mcp",
      "env": {
        "MEMORA_VAULT": "/absolute/path/to/your/vault"
      }
    }
  }
}
```

Claude Code now has access to 14 Memora tools including `memora_query_cited`
(retrieval with verified citations), `memora_capture` (capture a session
note), `memora_stale_claims`, `memora_contradictions`, and the active
challenger.

Full integration guide: [Obsidian + Claude Code](docs/src/obsidian-guide.md)

## How it compares

Memora is not a faster RAG. It's a different architecture for what
"memory" means in personal AI tooling.

| | RAG over Obsidian | LLM Wiki (Karpathy pattern) | Memora |
|---|---|---|---|
| **Atomic unit** | note chunks | whole notes | atomic claims |
| **Citation enforcement** | prompt-level (trust the LLM) | prompt-level | architectural (span hash + retry) |
| **Temporal reasoning** | latest-edited-wins | none | per-claim validity windows |
| **Contradiction handling** | silent | manual | auto-supersession, surfaced |
| **Privacy** | folder/note-level (manual) | none | per-claim, type-enforced redaction |
| **Synthesis staleness** | not tracked | not tracked | provenance DAG, auto-flagged |

Detailed comparison: [Architectural comparison](docs/src/comparison.md)

## Architecture in one paragraph

A vault watcher parses markdown frontmatter and detects changes. Each
note flows through an LLM-driven extractor that produces atomic claims
with source-span byte ranges, blake3 fingerprints, and inherited
privacy bands. Claims are stored in SQLite with FTS5 for keyword
retrieval, embeddings in HNSW for semantic retrieval, and a Hebbian
edge graph for co-activation learning. A contradiction detector
auto-supersedes claims that conflict with newer ones. A daily
challenger surfaces stale dependencies, contradictions, and frontier
gaps. Queries flow through cascade retrieval: hybrid BM25+vector
fusion → spreading activation → per-claim privacy filter (typed at
compile time) → LLM answer formatting → citation validator → retry
with verified-only context if hallucinations are detected. Everything
exposed over MCP stdio.

Full architecture: [Architecture deep dive](docs/src/architecture.md) —
or [see it on the landing page](https://radotsvetkov.github.io/memora).

## Status

v0.1.4 — public release. The architecture is complete, the test suite
covers all five differentiators including end-to-end citation retry.
Comparative benchmarks against other systems are not yet published.
Real-world vault testing is in progress; vault sizes up to a few
thousand notes are the target. Larger scales are unmeasured.

Issues, edge cases, and design discussions welcome at
[github.com/radotsvetkov/memora/issues](https://github.com/radotsvetkov/memora/issues).

## Documentation

The full book is rendered at
[radotsvetkov.github.io/memora/docs/](https://radotsvetkov.github.io/memora/docs/)
and the markdown sources live in [`docs/src/`](docs/src/):

- [Quickstart](docs/src/quickstart.md) — install and first verified citation in 10 minutes
- [Architecture](docs/src/architecture.md) — claim graph, retrieval, validation
- [Citation protocol](docs/src/citation-protocol.md) — how validation works
- [Vault conventions](docs/src/vault-conventions.md) — frontmatter and folder layout
- [Obsidian + Claude Code guide](docs/src/obsidian-guide.md) — daily-driver setup
- [MCP tools reference](docs/src/mcp-tools.md) — every tool with examples
- [Comparison](docs/src/comparison.md) — vs RAG, vs LLM Wiki, vs other systems
- [Landing page](https://radotsvetkov.github.io/memora) — value prop, claim anatomy, live flow

## License

**Apache 2.0 only.** See [LICENSE](LICENSE).
