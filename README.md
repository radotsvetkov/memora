# Memora

**Catch your AI citing sources that don't say what it claims.** Memora re-reads the verbatim source span behind every citation and recomputes its blake3 hash — citations the source doesn't actually contain are stripped before the answer reaches you.

*Cite, or it didn't happen.*

→ **[See it in motion](https://radotsvetkov.github.io/memora)**

Memora is a local, single Rust binary (CLI + MCP server) that puts a verification layer between your markdown notes and an LLM. It extracts atomic **claims** with byte-level provenance, and when the model answers, it **rejects any citation it cannot re-prove against the source**. Works in Claude Desktop, Cursor, and any MCP client.

## The problem

Note-aware AI tools — RAG over Obsidian, second-brain wrappers, agent memory layers — retrieve text and then *trust the model to quote it faithfully*. When the model invents a meeting that never happened, puts words in someone's mouth, or cites a claim your notes don't contain, you have no structural defense. You either catch the hallucination yourself, or you ship it. For decisions, meeting notes, and anything an agent acts on, that is the wrong trust model.

## What Memora does differently

The atomic unit of memory is a **claim**, not a note. A claim is an extracted statement with:

- subject, predicate, object
- source note plus byte-range span
- **full-width blake3 (256-bit) fingerprint** of the source text
- valid_from and valid_until temporal window
- privacy band (public / private / secret)
- provenance edges to source claims when synthesized

When the LLM answers, it cites claim IDs. The validator re-reads the source span from your markdown, recomputes the fingerprint, and **rejects citations that do not match**. Unknown IDs are stripped; a superseded claim (expired `valid_until`) is flagged rather than asserted as current; and the model is retried with verified context only. **The citation contract is enforced by Rust types and span hashes, not by prompt compliance.**

## What it guarantees — and what it does not

Be precise about the promise, because precision is the point:

- **It guarantees provenance integrity.** The cited source span verbatim exists and is unmodified (hash-proven), and any quoted text is actually contained in that span. This is strictly stronger than model-asserted citation APIs, which return offsets the model claims without re-reading and re-hashing them.
- **It does not check entailment.** Memora verifies that the source *says* the quoted text — not that the quote *supports* the surrounding conclusion. A model can cite a real span and still draw an unsupported inference. Entailment scoring is on the roadmap; today, provenance is the contract.

## Reproducible proof

The differentiator is measurable, deterministically, with no API key:

```bash
make bench   # cargo run -p memora-bench --bin bench_citation_rejection
```

Over a labeled fixture covering every failure mode (hallucinated claim id, source edited after extraction → fingerprint mismatch, quote not present in the span), Memora **rejects 100% of fabricated citations and preserves 100% of valid ones**. A naive RAG / prompt-cite pipeline rejects **0%** by construction — it performs no post-generation verification. The harness exits non-zero on any regression, so it doubles as a CI gate for the core contract.

> Honesty note: this is the one metric Memora is built to win and the only quantitative claim in this README. Other quality numbers (retrieval accuracy, contradiction precision over a real vault) are **not yet measured** — the placeholder benchmark that once printed fabricated constants has been removed.

## What you get

| | |
|---|---|
| **Verified citations** | Every claim ID in an answer is re-validated against its source span before the answer is returned. Hallucinated and mismatched citations are stripped. |
| **Provenance + staleness** | Synthesis claims point to source claims. Edit a source note, dependent syntheses are marked stale. |
| **Time-aware reasoning** | Claims carry validity windows. Historical states stay queryable; superseded claims are flagged, not silently surfaced as current. |
| **Per-claim privacy** | Inline `<!--privacy:secret-->...<!--/privacy-->` markers apply sub-span privacy. Secret content is redacted at a single type-enforced wire boundary (`RedactedPayload`) before any cloud LLM or embedding call. |
| **Active challenger** | A daily challenger run surfaces decisions, contradictions, stale dependencies, and open questions in `world_map.md`. |
| **Hybrid retrieval** | BM25 plus embeddings plus reciprocal-rank fusion. |
| **Local-first** | Single Rust binary with SQLite and HNSW. Cloud LLM/embedding calls are off by default and gated behind an explicit flag; full local operation with Ollama. |
| **Obsidian-native** | Plain markdown vault with frontmatter. Keep editing in Obsidian. |
| **MCP-native** | Works with Claude Desktop, Cursor, and other MCP clients over stdio. |

## How Memora compares

Memora makes one claim no funded memory vendor makes: **post-generation, hash-reverified citation rejection.** It re-reads the verbatim span and recomputes the fingerprint; mismatches are stripped before the answer ships.

| | Memora | Mem0 / Zep / Letta / Cognee | Anthropic Citations API |
|---|---|---|---|
| Hash-reverified citation rejection | **Yes** | No (no re-read / rejection) | Model-asserted, not re-hashed |
| Entailment (does the quote *support* the claim) | No — provenance only, by design | Partial (LLM-judged) | No |
| Temporal validity / contradiction handling | Yes | Yes (Zep/Graphiti) | — |
| Scale, integrations, multimodal, hosted offering | Behind | Ahead | — |

Memora is behind on scale, integrations, multimodal ingestion, and ecosystem — and would rather you knew. It is not here to replace your memory store; it is the local check that rejects what your AI can't prove. See [docs/comparison.md](docs/src/comparison.md) for the full, honest breakdown.

## Recommended models

Memora makes two kinds of LLM calls. Extraction runs once per note and produces structured triples. Synthesis runs once per atlas and produces prose. Provider quality matters, especially for extraction.

### Anthropic Claude Haiku (recommended)

This is what Memora was tuned against. Best balance of cost and quality.

```toml
[llm]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
```

Cost: roughly $0.30 to index a 100-note vault. Speed: about 5 to 10 minutes with parallelism = 8 (estimates from development runs, not a published benchmark). Anthropic's free tier limits requests to 50 per minute. Add at least $5 of credit to reach Tier 1, or set parallelism = 1 to stay under the free limit.

### OpenAI gpt-5-mini (alternative)

Comparable extraction quality at similar cost.

```toml
[llm]
provider = "openai"
model = "gpt-5-mini"
```

### Local (Ollama)

Use this when local-only is a hard requirement.

```toml
[llm]
provider = "ollama"
model = "qwen2.5:32b-instruct-q5_K_M"
```

Honest assessment: Qwen 14B is insufficient for production (hallucinates relationships, produces shallow triples). Qwen 32B is acceptable but misses cross-region patterns. Llama 70B matches Haiku quality with significant memory cost. Below 32B parameters, atlas synthesis quality degrades noticeably.

Embeddings always run locally by default:

```toml
[embed]
provider = "ollama"
model = "nomic-embed-text"
dim = 768
```

## See it catch a hallucination in 10 seconds

No vault, no API key, no network — `memora demo` builds an ephemeral vault, feeds the
*real* validator an AI answer containing one of every kind of bad citation, and shows
the verdict:

```bash
memora demo          # terminal verdict
memora demo --open   # also opens an HTML "Proof Report"
```

You'll watch a verified citation pass (green), a hallucinated id, a misquote, and a
post-edit hash mismatch get **rejected** (red, struck through), and a retracted claim
get flagged **superseded** — then see the difference between what a naive tool would
have shown you as fact and what memora actually returns. It's the same check
`memora query` runs on every answer.

## Quickstart

Install (cargo):

```bash
cargo install --path crates/memora-cli
```

Or download a release binary from:
[github.com/radotsvetkov/memora/releases](https://github.com/radotsvetkov/memora/releases)

Configure `~/.config/memora/config.toml`:

```toml
[llm]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"

[embed]
provider = "ollama"
model = "nomic-embed-text"
dim = 768

[indexing]
parallelism = 8
```

Index your vault:

```bash
memora index --vault ~/your-vault
```

Ask:

```bash
memora query "What did we decide about drift's serialization format?" --vault ~/your-vault
```

Use with Claude Desktop (`claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "memora": {
      "command": "/absolute/path/to/memora-mcp",
      "env": {
        "MEMORA_VAULT": "/absolute/path/to/your-vault",
        "MEMORA_ENABLE_NETWORK_LLM": "1"
      }
    }
  }
}
```

By default, Memora never sends your notes to a cloud provider. Cloud LLM **and** cloud embedding calls are gated behind `MEMORA_ENABLE_NETWORK_LLM=1` — on the CLI and the MCP server alike — so a single config line cannot silently route content off-machine. With the flag unset, cited queries still work offline via the extractive verified fallback (`degraded: true`). MCP reads embedder and privacy settings from `{vault}/.memora/config.toml`.

## Status

v0.1.28. Indexes 100-note vaults in roughly 5 to 10 minutes with Claude Haiku for about $0.30. Local Ollama is supported. Vault sizes up to a few thousand notes are the target; larger scales are unmeasured. The active challenger surfaces decisions, contradictions, stale dependencies, and open questions. Privacy redaction runs through a single type-enforced wire boundary covering every cloud egress (LLM and embeddings); citation fingerprints are full-width 256-bit blake3 (legacy 64-bit fingerprints from older indexes still verify until you re-index).

Issues, edge cases, and design discussions welcome at [github.com/radotsvetkov/memora/issues](https://github.com/radotsvetkov/memora/issues).

## Not yet

- Mobile / non-Obsidian access
- Local LLM at production quality
- PDFs / web clippings / transcripts
- GUI for claim-graph and atlas review
- Entailment scoring (today: provenance integrity only)
- A stable embeddable SDK / library API (today: CLI + MCP)

## Docs, contributing, license

- Docs: [docs/src](docs/src/) and [project docs site](https://radotsvetkov.github.io/memora/docs/)
- Contributions and issues: [github.com/radotsvetkov/memora/issues](https://github.com/radotsvetkov/memora/issues)
- License: [Apache-2.0](LICENSE)
