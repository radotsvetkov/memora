# Memora

Verifiable cognitive memory for personal Obsidian-compatible vaults.
Rust + SQLite + MCP-native. Single binary. Local-first.

**Status: pre-v0.1, in development.**

## Pitch

Every existing personal-memory tool retrieves *notes* and trusts the LLM to
quote them faithfully. Memora retrieves *claims*: atomic factual statements
extracted from your notes, each with a source-span pointer, validity window,
and privacy band. Every LLM citation is architecturally validated against
the source span — hallucinations are caught structurally, not via prompting.

## What it gives you

- **Verified citations.** Hallucinated claim ids are stripped before the
  answer reaches you.
- **Provenance with staleness propagation.** When you edit a note,
  downstream synthesis is auto-marked rotten.
- **Time-aware reasoning.** Claims have validity windows; "what was true in
  March" actually works.
- **Per-claim privacy.** Inline `secret` markers redact at the wire boundary
  for cloud LLMs.
- **Active challenger.** A daily worker surfaces contradictions, stale
  dependencies, and frontier gaps in your world map.

## License

Apache-2.0
