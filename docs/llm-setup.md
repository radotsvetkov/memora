# LLM setup (Ollama)

Memora’s Ollama client sends two knobs on every `/api/chat` request that affect throughput on local GPUs:

## `keep_alive` (default `24h`)

Chat completions include `keep_alive: "24h"` so Ollama keeps the loaded model resident between indexing batches. Without this, cold reload between notes dominates wall time.

## Dedicated embedding model

Configure embeddings separately from the chat model so bulk indexing does not run retrieval vectors through the same Llama weights:

```toml
[llm]
provider = "ollama"
model = "llama3.1:8b"
embedding_model = "nomic-embed-text"
endpoint = "http://localhost:11434"

[embed]
provider = "ollama"
dim = 768
```

Use `ollama pull nomic-embed-text` (or your chosen embedding tag) before indexing. `[embed].dim` must match the model output width (768 for `nomic-embed-text`).
