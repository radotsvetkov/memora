# Memora

Verifiable cognitive memory for personal Obsidian-compatible vaults.

Memora stores and retrieves **claims**, not just notes. Every claim maps to an exact source span and fingerprint, so citations are validated against your markdown before answers are trusted.

## Why Memora

- Claim-graph memory with temporal validity windows.
- Citation verification against source spans (`span_fingerprint`).
- Privacy-aware extraction with inline secret markers.
- Challenger loop for stale claims, contradictions, and frontier gaps.
- MCP server and CLI in a single Rust-native stack.

## Quickstart

```bash
cargo build --release
./target/release/memora init --vault ./vault
./target/release/memora index --vault ./vault
./target/release/memora query "What changed?" --vault ./vault
```

## Install

### macOS (Apple Silicon and Intel) and Linux

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/radotsvetkov/memora/releases/latest/download/memora-installer.sh | sh
```

### Windows

Download the appropriate binary from the [latest release](https://github.com/radotsvetkov/memora/releases/latest):

- `memora-x86_64-pc-windows-msvc.zip`

Extract and place `memora.exe` and `memora-mcp.exe` somewhere on your `PATH`.

### From source (any platform with Rust 1.75+)

```bash
cargo install --git https://github.com/radotsvetkov/memora memora-cli memora-mcp
```

More: `docs/quickstart.md`.

## MCP integration (Claude Code)

Example MCP server config:

```json
{
  "mcpServers": {
    "memora": {
      "command": "/absolute/path/to/target/release/memora-mcp",
      "env": {
        "MEMORA_VAULT": "/absolute/path/to/vault",
        "MEMORA_INDEX_DB": "/absolute/path/to/vault/.memora/memora.db",
        "MEMORA_VECTOR_INDEX": "/absolute/path/to/vault/.memora/vectors"
      }
    }
  }
}
```

## Comparison highlight

Memora's wedge is structural citation verification:

- Most note-centric systems trust prompt obedience for citation correctness.
- Memora re-opens source notes and re-hashes source spans for every cited claim.

See full comparison: `docs/comparison.md`.

## Docs

- `docs/architecture.md`
- `docs/vault-conventions.md`
- `docs/mcp-tools.md`
- `docs/citation-protocol.md`
- `docs/comparison.md`
- `docs/quickstart.md`

## License

Apache-2.0
