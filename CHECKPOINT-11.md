# CHECKPOINT 11 — Challenger

## Validation

- `cargo fmt` ✅
- `cargo clippy --all-targets -- -D warnings` ✅
- `cargo test --all` ✅
- New integration tests:
  - `stale_claim_emits_update_proposal`
  - `contradiction_pair_emits_contradiction_alert`
  - `cross_region_subject_without_home_note_emits_alert`
  - `low_confidence_claim_emits_frontier_alert`
  - `rerun_is_idempotent_except_for_timestamps`

## Diff (high level)

- Added challenger module:
  - `crates/memora-core/src/challenger/mod.rs`
  - `crates/memora-core/src/challenger/report.rs`
  - `crates/memora-core/src/challenger/scan.rs`
- Wired challenger into scheduler:
  - `crates/memora-core/src/scheduler.rs`
  - `crates/memora-core/src/lib.rs`
- Added CLI command:
  - `crates/memora-cli/src/commands/challenge.rs`
  - `crates/memora-cli/src/commands/mod.rs`
  - `crates/memora-cli/src/main.rs`
  - `crates/memora-cli/Cargo.toml`
  - `Cargo.lock`
- Added integration tests:
  - `crates/memora-core/tests/it_challenger.rs`

## Full `scan.rs`

Full source is in:

- `crates/memora-core/src/challenger/scan.rs`

The file defines:

- `Challenger` and `ChallengerConfig`
- `run_once()` with:
  - stale review with LLM JSON proposals (`update` / `archive`)
  - contradiction summarization for unacknowledged contradiction pairs
  - cross-region subject home-note detection
  - frontier gap identification (low-confidence + rare predicates)
- `persist_report()` to:
  - replace/add `## Today's review (auto-YYYY-MM-DD)` in `world_map.md`
  - save `.memora/last_challenger.json`
  - save timestamped `.memora/challenger-YYYYMMDDTHHMMSSZ.json`

## Sample generated `world_map.md` section

```md
## Today's review (auto-2026-04-29)
_generated at 2026-04-29T00:00:00Z_

### Stale claims
- [f8ab31f1a4d92c10] Stale claim needs update to 'Rado works_at Memora Labs'. (note-stale)

### Contradictions
- [2f83e6152d2cb551 vs 8c8a017c912d5aa4] These claims conflict about the same subject.

### Cross-region patterns
- [INTERNORGA] Subject 'INTERNORGA' spans 4 regions without a home note; consider creating 'internorga'. (regions: apac/events, eu/events, mena/events, us/events)

### Frontier gaps
- [9c0665adf3c9db8b] Low-confidence claim needs clarification. Question: What primary source confirms this statement?
```

## Notes

- Scheduler now runs two daily loops:
  - consolidation at `config.consolidation.daily_at` (existing)
  - challenger at `config.challenger.daily_at` (default `07:00`)
- New CLI command:
  - `memora challenge [--dry-run]`
  - always computes and prints report JSON
  - persists report to `world_map.md` and `.memora/last_challenger.json` unless `--dry-run`
- Idempotency behavior:
  - repeated runs produce same alert content
  - timestamps move forward on each run
