# Memora

**Catch your AI citing sources that don't say what it claims.**

Memora is the independent verification layer for AI citations. It re-reads the exact source span behind every citation, recomputes its hash, and rejects anything the source does not actually contain before the answer ships to a user or an auditor. Deterministic and provider-agnostic: it verifies any model over your own sources, and never asks a second model to grade the first. Run it in CI, in your eval suite, in an MCP client, or as a Rust library.

*Your model shouldn't grade its own homework.*

[![License](https://img.shields.io/badge/license-Apache--2.0-3b82f6)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-f59e0b)](rust-toolchain.toml)
[![crates.io](https://img.shields.io/crates/v/memora-core)](https://crates.io/crates/memora-core)
[![Local-first](https://img.shields.io/badge/local--first-yes-10b981)](#)

→ **[See it in motion](https://radotsvetkov.github.io/memora)**

## See it in 10 seconds

No vault, no API key, no network:

```bash
memora demo          # terminal verdict
memora demo --open   # also opens an HTML Proof Report
```

`memora demo` builds a throwaway vault, feeds the real validator an AI answer that contains one of every kind of bad citation, and shows the verdict. A faithful citation passes (green). A hallucinated id, a misquote, and a citation whose source changed after extraction are rejected (red, struck through). A retracted claim is flagged superseded. It is the same check `memora query` runs on every answer.

## Verify in CI

Drop one check into your eval suite or pipeline and a hallucinated citation never ships. `memora verify` checks an answer's citations against your sources and exits non-zero if any cannot be proven.

```bash
memora verify --vault ./sources answer.txt          # human verdict, exit 1 on failure
memora verify --vault ./sources --json answer.txt | jq .problems
echo "drift uses MessagePack [claim:0123456789abcdef]." | memora verify --vault ./sources
```

Add `--entailment` for an optional, LLM-judged check that the source actually supports each verified citation (and `--fail-unsupported` to fail the build on a "no" verdict). It is best-effort, kept separate from the hash-proven provenance.

A reusable GitHub Action ships in this repo:

```yaml
- uses: radotsvetkov/memora/.github/actions/verify@main
  with:
    vault: ./sources
    file: ./agent-output.txt
```

Building in Python? `pip install memora-verify` and call it from your eval suite:

```python
from memora_verify import assert_cited

def test_assistant_is_grounded():
    assert_cited(my_rag_app(question), vault="./sources")   # raises on any unprovable citation
```

See [`bindings/python`](bindings/python) for the wrapper.

## The problem

Any AI that cites sources retrieves text and then trusts the model to quote it faithfully. RAG assistants, agent memory layers, and note-aware tools all share this weakness. When the model invents a source, misquotes a decision, or cites a passage that does not contain the claim, you have no structural defense. You either notice the hallucination yourself, or you ship it to a user, a customer, or an auditor. Prompt-level "please cite your sources" does not survive a confident model.

## How it works

The unit of memory is a claim, not a note. Memora extracts atomic subject-predicate-object claims from your markdown. Each claim records:

- the source note and a byte-range span
- a full-width blake3 (256-bit) fingerprint of that span
- a `valid_from` / `valid_until` window
- a privacy band (public, private, secret)
- provenance links to the claims it was derived from

When the model answers, it cites claims by id. The validator re-reads each cited span, recomputes the fingerprint, and rejects any citation that does not match. Unknown ids are stripped, superseded claims are flagged rather than presented as current, and the model is retried with verified context only. The contract is enforced by Rust types and span hashes, not by asking the model nicely.

## What it guarantees, and what it does not

Memora guarantees **provenance integrity**. The cited source verbatim contains the quoted text and has not been modified, proven by hash. That is stricter than model-asserted citation APIs, which never re-read the source.

Provenance is not the same as **entailment**. Hashing proves the source verbatim contains the quoted text, not that the source *supports* the conclusion built on it. A model can cite a real span and still draw an unsupported inference. Memora now offers an optional, LLM-judged entailment check (`memora verify --entailment`) that flags citations whose source does not support the assertion. It is best-effort, not a proof: provenance is the guarantee, entailment is the model's opinion, and Memora keeps the two clearly separate (and never sends `secret` content to a cloud model to do it).

## Reproducible proof

The one number Memora publishes is measurable on your machine, deterministically, with no API key:

```bash
make bench   # runs bench_citation_rejection
```

Across a labeled fixture covering every failure mode, Memora rejects 100% of fabricated citations and preserves 100% of valid ones. A naive prompt-cite pipeline rejects 0% by construction, because it never verifies anything after the model generates. The harness exits non-zero on any regression, so it doubles as a CI gate. Other quality numbers, such as retrieval accuracy and contradiction precision over a real vault, are not yet measured, and this README does not claim them.

## What you get

| | |
|---|---|
| **Verified citations** | Every claim id is re-validated against its source span before the answer returns. Hallucinated and mismatched citations are stripped. |
| **Provenance and staleness** | Synthesis claims point to their sources. Edit a source note, and dependent claims are marked stale. |
| **Time-aware reasoning** | Claims carry validity windows. Historical states stay queryable, and superseded claims are flagged, not surfaced as current. |
| **Per-claim privacy** | Inline `<!--privacy:secret-->...<!--/privacy-->` markers narrow privacy to a sub-span. Secret content is redacted at a single type-enforced wire boundary before any cloud call. |
| **Active challenger** | A daily run surfaces decisions, contradictions, stale dependencies, and open questions in `world_map.md`. |
| **Document ingestion** | `memora ingest` pulls PDFs, web pages, text, and transcripts into the vault as verifiable notes (PDF/web behind opt-in features). |
| **Visual report** | `memora report` renders the whole vault — interactive claim graph, contradictions, stale dependencies, world map — as one self-contained, offline HTML file. |
| **Optional entailment** | `memora verify --entailment` adds a best-effort, LLM-judged check that the source supports the claim, kept separate from the hash-proven provenance. |
| **Hybrid retrieval** | BM25, embeddings, and reciprocal-rank fusion over your claim graph. |
| **Local-first** | A single Rust binary with SQLite and HNSW. Cloud calls are off by default and gated behind one explicit flag. Fully offline with Ollama or a bundled on-device embedder. |
| **Obsidian-native** | A plain markdown vault with frontmatter. Keep editing in Obsidian. |
| **MCP-native** | Works with Claude Desktop, Cursor, and other MCP clients over stdio. |

## How Memora compares

Memora makes one claim no funded memory vendor makes: post-generation, hash-reverified citation rejection. It re-reads the verbatim span and recomputes the fingerprint, then strips mismatches before the answer ships.

| | Memora | Mem0 / Zep / Letta / Cognee | Anthropic Citations API |
|---|---|---|---|
| Hash-reverified citation rejection | **Yes** | No | Model-asserted, not re-hashed |
| Entailment (quote supports the claim) | Optional, LLM-judged (opt-in), kept separate from the proof | Partial (LLM-judged) | No |
| Temporal validity and contradiction | Yes | Yes (Zep/Graphiti) | n/a |
| Scale, integrations, hosted offering | Behind | Ahead | n/a |

Memora is behind on scale, integrations, multimodal ingestion, and ecosystem, and it would rather you knew. It is not here to replace your memory store. It is the local check that rejects what your AI cannot prove. See the full breakdown in [docs/comparison.md](docs/src/comparison.md).

## Use it as a library

Memora is embeddable. Add it with `cargo add memora-core`. The owned `Memora` facade opens a vault and verifies citations from your own Rust code, with no lifetimes to thread through:

```rust
use memora_core::Memora;

let memora = Memora::open("/path/to/vault")?;
let answer = memora.validate("drift uses MessagePack [claim:0123456789abcdef].").await?;
println!("{} verified, {} rejected", answer.verified_count, answer.unverified_count);
```

`validate` is pure verification (no network, no LLM). `query_verified` generates an answer with the configured LLM and re-validates every citation before returning it. `search` and `claim` are also available. The CLI's `query` command runs on this same facade.

## Recommended models

Memora makes two kinds of LLM calls. Extraction runs once per note and produces structured triples. Synthesis runs once per atlas and produces prose. Provider quality matters most for extraction.

**Anthropic Claude Haiku (recommended).** This is what Memora was tuned against, and it strikes the best balance of cost and quality.

```toml
[llm]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
```

Indexing a 100-note vault costs about $0.30 and takes roughly 5 to 10 minutes at `parallelism = 8` (estimates from development runs, not a published benchmark). Anthropic's free tier caps requests at 50 per minute, so add at least $5 of credit to reach Tier 1, or set `parallelism = 1` to stay under the free limit.

**OpenAI gpt-5-mini** is a comparable alternative at similar cost. **Ollama** (for example `qwen2.5:32b-instruct-q5_K_M`) covers the local-only case. Honestly, below 32B parameters local extraction degrades: it hallucinates relationships and produces shallow triples. Embeddings run locally by default with `nomic-embed-text` (via Ollama). For a fully
on-device embedder with no daemon, build with `--features local-embed` and set
`[embed] provider = "local"` (bundled `fastembed` BGE-small, 384-dim).

## Quickstart

Install with Homebrew, cargo, or a release binary:

```bash
brew install radotsvetkov/memora/memora   # macOS and Linux
cargo install memora-cli                   # any platform with Rust
```

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

Index a vault, then ask a question:

```bash
memora index --vault ~/your-vault
memora query "What did we decide about drift's serialization format?" --vault ~/your-vault
```

Bring in outside documents so they become verifiable too. `memora ingest` turns a
PDF, text file, or transcript into a vault note, then index picks it up:

```bash
memora ingest contract.pdf             --vault ~/your-vault --region legal    # PDF needs --features pdf
memora ingest standup.vtt              --vault ~/your-vault --region transcripts
memora ingest https://example.com/post --vault ~/your-vault --region web      # URL needs --features web
memora index --vault ~/your-vault
```

Generate a visual overview of the whole vault — an interactive claim graph, the contradictions and stale dependencies that need attention, and the world map — as one self-contained, offline HTML file:

```bash
memora report --vault ~/your-vault --open
```

Use it from Claude Desktop (`claude_desktop_config.json`):

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

By default Memora never sends your notes to a cloud provider. Cloud LLM and cloud embedding calls are gated behind `MEMORA_ENABLE_NETWORK_LLM=1` on both the CLI and the MCP server, so a single config line cannot route content off your machine by accident. With the flag unset, cited queries still work offline through an extractive verified fallback (`degraded: true`).

## Status

v0.1.29. Indexes a 100-note vault in about 5 to 10 minutes with Claude Haiku for roughly $0.30. Local Ollama is supported. Vault sizes up to a few thousand notes are the target, and larger scales are unmeasured. Privacy redaction runs through one type-enforced wire boundary that covers every cloud egress (LLM and embeddings). Citation fingerprints are full-width 256-bit blake3, and legacy 64-bit fingerprints from older indexes still verify until you re-index.

Issues, edge cases, and design discussions are welcome on the [issue tracker](https://github.com/radotsvetkov/memora/issues).

## Not yet

- Mobile and non-Obsidian access
- Local LLM at production quality
- A live, interactive GUI (today `memora report` generates a self-contained static HTML overview with an interactive claim graph; a running web app is future)
- Local NLI-model entailment (today entailment is optional and LLM-judged via `memora verify --entailment`, not a local model)

## Docs, contributing, license

- Documentation: [docs/src](docs/src/) and the [docs site](https://radotsvetkov.github.io/memora/docs/)
- Contributions and issues: [github.com/radotsvetkov/memora/issues](https://github.com/radotsvetkov/memora/issues)
- License: [Apache-2.0](LICENSE)
