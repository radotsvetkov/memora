# Recommended Models

Memora makes two kinds of LLM calls. Extraction runs once per note and produces structured triples. Synthesis runs once per atlas and produces prose. Provider quality matters, especially for extraction.

## Anthropic Claude Haiku (recommended)

This is what Memora was tuned against. Best balance of cost and quality.

```toml
[llm]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
```

Cost: about $0.30 to index a 100-note vault.
Speed: 5 to 10 minutes with `parallelism = 8`.

Anthropic's free tier limits requests to 50 per minute. Add at least $5 of credit to reach Tier 1, or set `parallelism = 1` to stay under the free limit.

## OpenAI gpt-5-mini (alternative)

Comparable extraction quality at similar cost.

```toml
[llm]
provider = "openai"
model = "gpt-5-mini"
```

## Local (Ollama)

Use this when local-only is a hard requirement.

```toml
[llm]
provider = "ollama"
model = "qwen2.5:32b-instruct-q5_K_M"
```

Honest assessment:

- Qwen 14B is insufficient for production (hallucinates relationships, shallow triples).
- Qwen 32B is acceptable but misses cross-region patterns.
- Llama 70B can match Haiku quality with a large memory cost.
- Below 32B parameters, atlas synthesis quality drops noticeably.

Embeddings run locally regardless of chat provider:

```toml
[embed]
provider = "ollama"
model = "nomic-embed-text"
dim = 768
```
