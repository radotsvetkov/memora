# Memora Architecture

Memora is a local-first memory engine where the atomic unit is a **claim**, not a note.

## Claim graph

Each claim stores:

- `subject`, `predicate`, `object`
- `note_id`, `span_start`, `span_end`, `span_fingerprint`
- temporal range (`valid_from`, `valid_until`)
- privacy band (`public`, `private`, `secret`)
- relation edges (`entails`, `contradicts`, `supersedes`, `derives`, `co_occurs`)

The graph is persisted in SQLite and linked back to markdown source spans in the vault.

## Citation pipeline

1. Retriever picks candidate notes (hybrid BM25 + vectors + spread).
2. Candidate claims are gathered from those notes.
3. LLM answers with `[claim:...]` markers.
4. Validator re-opens source notes, re-reads the claim span, recomputes `span_fingerprint`, and verifies quote overlap.
5. Unverified markers are stripped from `clean_text`.

This guarantees the answer only keeps claims that are structurally verifiable.

## Provenance and staleness

When a source claim changes or is superseded, provenance edges mark downstream derived claims stale.
Stale claims are collected in `stale_claims` and shown by challenger/consolidation outputs.

## Privacy

- Notes default to `private`.
- Inline markers `<!--privacy:secret--> ... <!--/privacy-->` override local span privacy.
- `secret` claims are redacted before cloud-bound prompts (`subject` kept, predicate/object redacted).

## Challenger loop

Challenger runs daily and emits:

- stale claim update/archive proposals
- contradiction alerts
- cross-region "home note" suggestions
- frontier gaps (low-confidence or sparse predicate evidence)

The report is written to `.memora/last_challenger.json` and merged into `world_map.md`.
