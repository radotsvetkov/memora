# LLM setup

Canonical model recommendations now live in `docs/src/models.md`.
This page keeps Ollama-specific setup notes that are useful for local runs.

## Ollama runtime behavior

Memora sends `keep_alive: "24h"` on `/api/chat` calls so Ollama keeps the loaded model in memory across indexing batches. This reduces repeated cold starts during long index runs.

## Recommended local config

Use the same model values documented in `docs/src/models.md`:

## MCP network LLM

Set `MEMORA_ENABLE_NETWORK_LLM=1` in the MCP server environment for tools that
call a network LLM (`memora_consolidate`, `memora_challenge`, and LLM-backed
`memora_query_cited`). Without it, cited queries use extractive verified fallback.

MCP loads LLM and embed settings from `{vault}/.memora/config.toml`.

## Example vault config

```toml
[llm]
provider = "ollama"
model = "qwen2.5:32b-instruct-q5_K_M"
endpoint = "http://localhost:11434"

[embed]
provider = "ollama"
model = "nomic-embed-text"
dim = 768
```

## Setup command

```bash
ollama pull qwen2.5:32b-instruct-q5_K_M
ollama pull nomic-embed-text
```

`[embed].dim` must match the embedding model output width (768 for `nomic-embed-text`).
