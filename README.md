Verifiable cognitive memory for personal vaults. Cite, or it didn't happen.

# Memora

→ **[See the architecture in motion](https://radotsvetkov.github.io/memora)**

## The problem

Teams write decisions in Obsidian, then lose track of what was decided, what changed, and what is stale. Most note-aware AI tools retrieve chunks and trust the model to cite correctly. That trust breaks under pressure. Memora extracts atomic claims with span-level provenance, then validates citations against source spans before an answer is returned.

## How Memora differs

The atomic unit of memory is a **claim**, not a note. A claim is an extracted statement with:

- subject, predicate, object
- source note plus byte-range span
- blake3 fingerprint of the source text
- valid_from and valid_until temporal window
- privacy band (public / private / secret)
- provenance edges to source claims when synthesized

When the LLM answers, it cites claim IDs. The validator re-reads the source span from markdown, recomputes the fingerprint, and rejects citations that do not match. Unknown IDs are stripped and the model is retried with verified context only. The citation contract is enforced by Rust types and span hashes, not prompt compliance.

## What you get

| | |
|---|---|
| **Verified citations** | Every claim ID in an answer is re-validated against source spans before the answer is returned. |
| **Provenance + staleness** | Synthesis claims point to source claims. Edit a source note, dependent syntheses are marked stale. |
| **Time-aware reasoning** | Claims carry validity windows. Historical states remain queryable while current state stays clear. |
| **Per-claim privacy** | Inline `<!--privacy:secret-->...<!--/privacy-->` markers apply sub-span privacy and cloud calls redact secret claim bodies. |
| **Active challenger** | A daily challenger run surfaces decisions, contradictions, stale dependencies, and open questions in `world_map.md`. |
| **Hybrid retrieval** | BM25 plus embeddings plus rank fusion, then graph-aware expansion over claim links and wikilinks. |
| **Local-first** | Single Rust binary with SQLite and HNSW. Full local operation is available with Ollama. |
| **Obsidian-native** | Plain markdown vault with frontmatter. Keep editing in Obsidian. |
| **MCP-native** | Works with Claude Desktop, Cursor, and other MCP clients over stdio. |

## Recommended models

Memora makes two kinds of LLM calls. Extraction runs once per note and produces structured triples. Synthesis runs once per atlas and produces prose. Provider quality matters, especially for extraction.

### Anthropic Claude Haiku (recommended)

This is what Memora was tuned against. Best balance of cost and quality.

```toml
[llm]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
```

Cost: about $0.30 to index a 100-note vault. Speed: 5 to 10 minutes with parallelism = 8. Anthropic's free tier limits requests to 50 per minute. Add at least $5 of credit to reach Tier 1, or set parallelism = 1 to stay under the free limit.

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

Embeddings always run locally regardless of chat provider:

```toml
[embed]
provider = "ollama"
model = "nomic-embed-text"
dim = 768
```

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
        "MEMORA_VAULT": "/absolute/path/to/your-vault"
      }
    }
  }
}
```

## Status

v0.1.26. Indexes 100-note vaults in 5 to 10 minutes with Claude Haiku for about $0.30. Local Ollama is supported. Vault sizes up to a few thousand notes are the target. Larger scales are unmeasured. The active challenger now surfaces decisions, contradictions, stale dependencies, and open questions in every atlas.

Issues, edge cases, and design discussions welcome at [github.com/radotsvetkov/memora/issues](https://github.com/radotsvetkov/memora/issues).

## Not yet

- Mobile / non-Obsidian access
- Local LLM at production quality
- PDFs / web clippings / transcripts
- GUI for atlas review

## Docs, contributing, license

- Docs: [docs/src](docs/src/) and [project docs site](https://radotsvetkov.github.io/memora/docs/)
- Contributions and issues: [github.com/radotsvetkov/memora/issues](https://github.com/radotsvetkov/memora/issues)
- License: [Apache-2.0](LICENSE)
