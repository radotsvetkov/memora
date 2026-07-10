# CLI reference

Every `memora` subcommand takes `--vault <path>` (default `vault`). Run
`memora <command> --help` for the authoritative, up-to-date flag list — this
page covers what each command is *for* and when to reach for it.

## Everyday commands

### `memora demo` / `memora demo --open`

Zero-config, offline, no API key. Builds a throwaway vault, feeds the real
validator an AI answer containing every kind of bad citation, and prints the
verdict. `--open` also renders an HTML Proof Report. Use this to see the
guarantee before touching your own vault, or to sanity-check a fresh install.

### `memora init --vault <path>`

Scaffolds a new vault: `.memora/config.toml`, an empty `world_map.md`, and a
sample note. Safe to re-run — it never overwrites files that already exist.

### `memora index --vault <path>`

Full pipeline: parse notes, extract claims, fingerprint spans, update the
BM25 and vector indexes, detect contradictions. Run this after adding notes
by hand (outside `memora watch`). `--auto-fix-frontmatter` prepends missing
YAML frontmatter instead of prompting; `--no-contradict` skips contradiction
detection (recommended for a large first import — see the in-command tip).

### `memora watch --vault <path>`

Keeps the index current as you edit: watches the filesystem and reindexes
changed notes incrementally. Also runs the **scheduler** in the background —
region atlases + the world map are rebuilt daily at 03:00 local time, and a
challenger pass (contradictions, stale claims, open questions) runs daily at
07:00. This is what makes `world_map.md` a living document; `memora
consolidate`/`memora challenge` (below) are the on-demand equivalents. Only
one `watch` can run per vault (it takes `.memora/watch.lock`).

### `memora query "<question>" --vault <path>`

Ask a question; get a verified, cited answer from the configured LLM (cloud
providers need `MEMORA_ENABLE_NETWORK_LLM=1`). `--raw` skips citation
formatting. Without network access, falls back to an offline extractive
answer (`degraded: true` in JSON output).

### `memora verify` / `memora ingest` / `memora report`

Covered in the [README](https://github.com/radotsvetkov/memora) and
[Ingesting documents](./ingesting.md) — the CI-verification wedge, document
ingestion, and the self-contained HTML vault overview.

## Diagnostics and maintenance

### `memora doctor --vault <path>`

Prints a health check: whether the vault, `.memora/config.toml`, the SQLite
index, and the vector index files exist, whether a `watch` lock is held, and
(if the index exists) row counts for notes, full-text search, and claims.
Reach for this first when something looks wrong — a missing config, a stale
watch lock, or an index that never got built are all visible at a glance.

```bash
memora doctor --vault ~/brain
```

### `memora privacy audit --vault <path>`

Scans every note for sensitive-looking keywords (salary, SSN, password,
medical terms, and similar) in notes that have **no explicit** `privacy:`
frontmatter, and lists them. It's a heuristic net for the vault owner, not a
security boundary — always set `privacy: private`/`secret` explicitly on
anything sensitive rather than relying on this catching it after the fact.

```bash
memora privacy audit --vault ~/brain
```

## Claim graph and consolidation, on demand

`memora watch` runs these automatically on a schedule (see above). Use these
directly when you want a result *now*, or outside a long-running `watch`
process (e.g. in a script or CI job over a vault you don't otherwise watch).

### `memora challenge --vault <path>`

Runs one challenger pass — contradictions, stale dependencies, cross-region
patterns, and open questions — over the current claim graph, and prints the
report as JSON. Writes the result to `world_map.md` and
`.memora/last_challenger.json` unless `--dry-run` is passed (which only
prints, without persisting).

### `memora consolidate --vault <path> [--region <name> | --all]`

Rebuilds region atlases (`_atlas.md`/`_index.md`) and the world map from the
current claim graph. With no flags, only rebuilds regions with changes since
the last run. `--region <name>` rebuilds one region on demand. `--all`
rebuilds every region regardless of whether it changed — use after a bulk
edit or a model/config change that should retroactively affect every
synthesis.

### `memora claims extract --note <id> --vault <path>`

Re-runs claim extraction for a single note by id and prints the resulting
claims as JSON, without touching the index — useful for inspecting or
debugging what the extractor produces for one note in isolation.

### `memora claims show <claim-id> --vault <path>`

Prints one claim (subject/predicate/object, source span, fingerprint,
validity window, privacy) and the exact source quote it resolves to. The
fastest way to answer "what does this citation actually point at?"

## MCP server

### `memora serve` / `memora-mcp`

Runs the MCP server over stdio — identical behavior whether invoked as
`memora serve` or the standalone `memora-mcp` binary (the latter is what
Claude Desktop/Cursor configs typically point `command` at, since it doesn't
need the rest of the CLI on `PATH`). See [MCP tools](./mcp-tools.md) for the
full tool list.
