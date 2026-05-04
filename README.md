A claim graph for your Obsidian vault. Verifiable citations, per-claim privacy, surfaces decisions and contradictions as your notes evolve.

# Memora

## Why

Notes accumulate faster than decisions get revisited. Three months later, teams cannot reliably answer what was decided, what changed, and what is now stale. Most AI note tools make this worse by retrieving chunks and trusting the model to cite correctly. Memora extracts atomic claims with span-level citations, tracks those claims as your vault evolves, and surfaces decisions, contradictions, stale dependencies, and open questions directly from the claim graph.

## What's Different

1. **Verifiable citations**  
   Every claim is fingerprinted to a source span. LLM outputs cite claim IDs, and citations are re-validated against markdown before delivery.

2. **Per-claim privacy bands**  
   Secret-marked spans never leave your machine unredacted. You can mark sensitive lines while still retrieving surrounding non-sensitive claims.

3. **Temporal validity**  
   Claims carry `valid_from` / `valid_until`. Superseded decisions remain queryable with context instead of disappearing.

4. **Claim-graph synthesis**  
   Atlases surface decisions, contradictions, stale dependencies, and open questions — not just retrieved chunks.

## Example Output

```markdown
## Stale dependencies
- **staleness-case-a-synthesis** depends on `retrieval-eval-notes` which is superseded by `retrieval-eval-notes-v2`
  (10 depends_on sources; 4 superseded_by sources)

## Open questions
- **memora: precision-vs-recall-tradeoff** - decision pending
  (9 supporting claims across [[ep-daily-2026-04-29]], [[ep-daily-2026-04-13]], and 7 more)
```

This is what an atlas looks like after Memora indexes a 100-note Obsidian vault. Each finding cites the exact notes that support it — click through to verify.

## Status

v0.1.26. Indexes 100-note vaults in 5-10 min with Anthropic Haiku (~$0.30 cost). Local Ollama is supported, but quality degrades meaningfully below 32B parameters. MCP-native — works with Claude Desktop and other MCP clients.

## Quick Start

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
model = "claude-3-5-haiku-latest"

[embed]
provider = "openai"
model = "text-embedding-3-small"
dim = 1536

[indexing]
parallelism = 8
```

Index your vault:

```bash
memora index --vault ~/your-vault
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

## Not Yet

- Mobile / non-Obsidian access
- Local LLM quality at production parity
- PDFs / web clippings / transcripts
- GUI for atlas review

## Docs, Contributing, License

- Docs: [docs/src](docs/src/) and [project docs site](https://radotsvetkov.github.io/memora/docs/)
- Contributions and issues: [github.com/radotsvetkov/memora/issues](https://github.com/radotsvetkov/memora/issues)
- License: [Apache-2.0](LICENSE)
