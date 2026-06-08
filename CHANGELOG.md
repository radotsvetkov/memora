# Changelog

## [Unreleased]

### Added
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

### Fixed
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
