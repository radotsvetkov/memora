# Changelog

## [Unreleased]

### Added
- `memora report` — a self-contained, offline HTML overview of a vault: summary stats, an interactive force-directed claim graph (provenance, contradiction, and supersession edges), the contradictions/supersessions and stale dependencies that need attention, and the world map. One file, no server, no network, no CDN (system fonts only). All vault content is HTML-escaped and the embedded graph data is `\u`-escaped, so note content cannot inject markup. `--open` opens it in the browser.
- Optional entailment check: `memora verify --entailment` asks an LLM whether the cited source actually *supports* each verified citation (not just contains the quote), and `--fail-unsupported` makes a "no" verdict fail the build. It is best-effort and kept clearly separate from the hash-proven provenance layer; `secret` content is never sent to a cloud model. New `EntailmentChecker` / `Entailment` in `memora-core` and `Memora::entailment_checker()`. This closes the one capability the README previously disclaimed.
- `memora ingest <file_or_url>` — bring external documents into the vault as notes so they become verifiable through the normal index → extract → verify pipeline. Supports plain text, markdown, VTT/SRT transcripts, PDF (behind the `pdf` feature), and web pages — a URL or `.html` file (behind the `web` feature; readable-text extraction via `scraper`, title becomes the summary, scripts/styles dropped). Optional features keep the default binary lean: `cargo install memora-cli --features "pdf web"`. Re-ingesting the same source updates the same note rather than duplicating it. See `docs/src/ingesting.md`.
- `[embed] provider = "local"` is now wired to the on-device `fastembed` BGE-small embedder (build with `--features local-embed`). Previously this provider silently fell back to deterministic vectors; without the feature it now fails with clear guidance instead.

### Fixed
- Vector index compaction. `hnsw_rs` has no delete, so every re-index and deletion left the old vector in the graph forever — unbounded growth, and (worse) accumulated tombstones could crowd out live results in search, which only over-fetches. The index now keeps its live vectors and compacts (rebuilds from them, dropping tombstones) at the end of every `full_rebuild`. Old on-disk indexes load intact via an explicit legacy decoder (a `serde(default)` would not have worked: bincode is positional and cannot default a missing field), so the upgrade needs no forced re-embed.
- Contradiction detection during `full_rebuild` is now deterministic. It previously ran inline during the parallel per-note phase, doing a non-transactional read-then-write that raced across notes (so a cross-note contradiction could be detected or missed depending on commit timing). It now runs once, after every note's claims are committed, in a single ordered pass. `claims_contradict` verdicts are cached by claim-pair tuple, so each pair is checked once.

### Changed
- Release workflow no longer attempts to auto-publish the Homebrew formula (it required a cross-repo token kept out of CI), so release runs stay green. The tap (`radotsvetkov/homebrew-memora`) is updated manually per release; see RELEASING.md.

### Removed
- The unused "cognitive" retrieval layer: Hebbian co-activation, spreading activation, and Q-value reinforcement. None of it ran in production (only via a code path with no callers), so it was dead weight that over-stated what retrieval does. Removed `QValueLearner`, `HebbianLearner`, `spread`, the dead `search_with_spread_and_record` path, and the MCP tools that surfaced it (`memora_neighbors`, `memora_record_useful` — they returned empty/erroring results). Production retrieval is BM25 + embeddings + reciprocal-rank fusion, as documented. The `notes.qvalue` / `hebbian_edges` / `retrievals` tables are retained (harmless) to avoid a schema migration.

## [0.1.29] - 2026-06-09

### Added
- Distribution: Homebrew formula publishing via cargo-dist (tap `radotsvetkov/homebrew-memora`) for the CLI, alongside the existing shell installer and GitHub release binaries. crates.io readiness for the libraries (internal deps centralized in `[workspace.dependencies]` with versions; `memora-llm` and `memora-core` package cleanly). See `RELEASING.md` for both channels.
- `memora verify`: verify an AI answer's citations against a vault and exit non-zero if any cannot be proven (reads a file or stdin, `--json` for machine output, `--allow-superseded`). Built on the `Memora` facade. Plus a reusable GitHub Action (`.github/actions/verify`) so a pipeline fails the build on an unprovable citation ("CI for hallucinations"). Verdict rendering is shared with `memora demo` via a single module.
- `Memora::query_verified`: the LLM-backed cited-answer path on the facade (cloud providers gated behind `MEMORA_ENABLE_NETWORK_LLM`). The CLI `query` command is now a thin wrapper over the facade, removing duplicated wiring; the network gate is centralized in `memora_core::vault_config::network_llm_enabled`.
- Owned `Memora` library facade (`Memora::open`, `validate`, `search`, `claim`) so the engine is embeddable from other Rust code without touching the lifetime-borrowed internals. `memora-core` gained crates.io metadata (description, keywords, categories).
- Supply-chain and contract gates in CI: `cargo-deny` (advisories, licenses, bans, sources) via `deny.toml`, plus the deterministic citation-rejection benchmark now runs in CI so a regression in the core guarantee fails the build.
- `memora demo`: a zero-config, no-API-key, offline command that builds an ephemeral vault and runs the real validator over an AI answer containing every failure mode (verified, hallucinated id, misquote, post-edit hash mismatch, superseded), rendering a terminal verdict and an optional HTML "Proof Report" (`--open`).
- Type-enforced redaction choke-point (`RedactedPayload`) at the LLM wire boundary: secret claim content cannot reach a cloud provider without passing through redaction, enforced across the challenger, answer, consolidate, contradiction, and extraction paths (forgetting to redact a new egress site is now a compile error).
- `Superseded` citation status: a cited claim whose `valid_until` has expired is surfaced as superseded rather than asserted as current. Exposed via the validator, `CitedAnswer.superseded_count`, and MCP `memora_verify_claim` (`superseded` + `valid_until`).
- Deterministic, no-API-key citation-rejection benchmark (`make bench` → `bench_citation_rejection`): measures fabricated-citation rejection rate and valid-citation preservation rate over a labeled fixture, exits non-zero on regression (CI gate for the core contract).

### Changed
- Citation fingerprints are now full-width blake3 (256-bit) instead of 64-bit truncated. Legacy 64-bit fingerprints from older indexes still verify until the vault is re-indexed.
- Cloud embedding providers (`[embed] provider = "openai"`) are gated behind `MEMORA_ENABLE_NETWORK_LLM=1` in `memora-core` (covering both CLI and MCP), and the real `OpenAiEmbedder` is now wired (it previously fell through to deterministic local vectors).
- CLI cloud LLM and embedding calls are gated behind `MEMORA_ENABLE_NETWORK_LLM=1` (parity with MCP); a config line can no longer silently route content off-machine.
- Secret-claim subjects are redacted (not only predicate/object) before cloud calls.
- Repositioned README, docs, and landing page around verifiable citation rejection; dropped the "cognitive memory" framing; added an explicit "provenance integrity, not entailment" boundary; rewrote the comparison to confront Mem0/Zep/Letta/Cognee and the Anthropic Citations API honestly.
- Rebuilt the landing page with a cleaner, professional design (sans body type, restrained palette, accurate copy, an honest static render of `memora demo`) and polished the README to feature the demo and read more naturally.

### Fixed
- Staleness propagation is now transitive: editing a source claim marks its derivatives and their derivatives in turn (A → B → C marks both B and C), with cycle protection. Previously only direct (single-hop) derivatives were marked.
- First-run `database is locked` noise: establish WAL mode on a single connection before the pool opens connections concurrently, so they don't race the journal-mode switch on a fresh db.
- The challenger now routes all prompts through the privacy filter (it previously embedded raw secret claims and note spans into cloud prompts).
- Removed fabricated placeholder benchmark numbers: `bench_personal_vault` printed hardcoded metrics (0.94/0.88/0.00) and `bench_locomo` returned `retrieval@k = 1.0` for any non-empty fixture; both are now honest.

## [0.1.28] - 2026-05-28

### Fixed
- Redact secret inline spans before cloud LLM claim extraction; skip extraction for wholly secret notes on cloud destinations.
- MCP `memora_get_note` redacts secret note bodies and sets `body_redacted`; query snippets respect note privacy.
- Reject vault path traversal in `memora_capture` and constrain indexed note reads to the vault root.
- Preserve existing claims when claim extraction fails transiently instead of deleting them.
- Wire claim extraction into `memora watch` so the claim graph stays current during file watching.
- MCP `memora_record_useful` returns an error when `query_id` is unknown.

### Changed
- MCP loads embedder and retrieval settings from `.memora/config.toml` (parity with CLI).
- MCP cited queries use extractive verified fallback when network LLM is disabled (`degraded: true`).
- MCP consolidate/challenge require `MEMORA_ENABLE_NETWORK_LLM=1` and a configured provider.
- Privacy settings from `[privacy]` in config are applied to the query pipeline (`redact_secret_in_cloud`, `warn_on_secret_query`).
- Shared `DeterministicEmbedder`, `build_embedder`, and `VaultConfig` moved into `memora-core`.

## [0.1.27] - 2026-05-04

### Fixed
- Include post-tag consolidation and clippy fixes in a released build.

### Changed
- Move README product slogan under the `Memora` title.

## [0.1.26] - 2026-05-04 (Launch readiness)

### Added
- Active challenger surfaces decisions, contradictions, stale dependencies, and open questions in every atlas.
- Cross-region detection for contradictions and open questions.
- Predicate exclusivity gating to prevent false-positive contradictions.
- Object normalization for decision detection (for example, "stainless" and "stainless-templates" treated as one decision).
- Strong-predicate filter for recent decisions (filters single-claim noise).
- Verbatim claim deduplication at consolidation render time with stable claim ID selection and source list truncation at 12 entries.
- Recommended models documentation.
- Updated landing page demonstrating challenger output.

### Changed
- Atlas synthesis now omits decided pairs from "Open questions" sections to prevent duplicate surfacing.
- CLI summary now reports separate counts for empty extractions, rate-limited failures, parse failures, and invalid claims.
- Indexer exits non-zero when rate-limited count > 0 to surface partial-success runs to wrapper scripts.
- All documentation examples updated to a consistent fictional domain.

### Fixed
- Indexer no longer indexes generated `_atlas.md` and `_index.md` files as content notes.
- Watcher no longer triggers reindex when consolidate writes atlas files.
- Rate-limit failures now properly counted as errors instead of silent warnings.
- Repeated verbatim claims no longer pad atlas displays.

## [0.1.21] - 2026-05-02

### Changed
- Faster first-time indexing with local LLMs: bounded parallel note processing (`[indexing] parallelism`), `--no-contradict` on `memora index`, dedicated Ollama embedding model via `/api/embeddings`, `keep_alive` on chat completions, and structured JSON extraction paths.

### Fixed
- SQLite `PRAGMA busy_timeout=60000` for parallel rebuild writers.
- Remove redundant `.into_iter()` in the parallel indexer stream (Rust 1.95 `clippy::useless_conversion`).

## [0.1.4] - 2026-04-29

### Fixed
- Embed SQLite migration SQL files directly into release binaries so `memora index` and `memora query` no longer fail on installed builds with missing CI-only migration paths.
- Format embedded migration constant in `sqlite.rs` to satisfy `cargo fmt --check` in release CI.

## [0.1.5] - 2026-04-29

### Fixed
- Re-release the migration hotfix with rustfmt-clean source so the tag-triggered Release workflow passes end-to-end.

## [0.1.6] - 2026-04-29

### Fixed
- Normalize free-form natural-language queries before SQLite FTS5 `MATCH` so prompts like `What did I decide about the Q1 roadmap?` do not fail with a syntax error.

## [0.1.7] - 2026-04-29

### Fixed
- Wire `memora index` to run claim extraction and persist claims during full rebuild so `memora query` can return citation-grounded answers from indexed notes.

## [0.1.8] - 2026-04-29

### Fixed
- Add heuristic claim-extraction fallback when local models return malformed JSON or extraction calls fail, so indexing still produces claims.
- Add extractive citation-backed answer fallback when the model returns uncited generic chat output despite available claims.

## [0.1.9] - 2026-04-30

### Fixed
- Fix `memora watch` runtime panic by removing nested Tokio `block_on` usage and awaiting vault events directly inside the async command loop.

## [0.1.10] - 2026-04-30

### Fixed
- Keep `memora watch` running when a single file event fails parsing (for example, a note missing YAML frontmatter) by logging and continuing instead of exiting.
