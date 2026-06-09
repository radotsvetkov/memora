# MCP Tools

All tools return JSON payloads.

Memora MCP reads configuration from `{vault}/.memora/config.toml` (embedder,
retrieval `top_k`, privacy, and LLM provider settings), matching the CLI.

## Environment variables

| Variable | Required | Purpose |
|----------|----------|---------|
| `MEMORA_VAULT` | yes | Path to your Obsidian-compatible vault |
| `MEMORA_INDEX_DB` | optional | Override index DB path (default: `{vault}/.memora/memora.db`) |
| `MEMORA_VECTOR_INDEX` | optional | Override vector index path (default: `{vault}/.memora/vectors`) |
| `MEMORA_ENABLE_NETWORK_LLM` | for LLM tools | Set to `1` to enable network LLM calls |

When `MEMORA_ENABLE_NETWORK_LLM` is unset, `memora_query_cited` still works using
**extractive verified fallback** (indexed claims only, `degraded: true`).
`memora_consolidate` and `memora_challenge` require network LLM and return an
error if it is disabled.

Example Claude Desktop config:

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

## `memora_query`

Input:

```json
{"query":"What did the team decide about drift's serialization format?","k":5}
```

Output:

```json
{"hits":[{"id":"note-1","summary":"drift moved to MessagePack in Q3","region":"projects/drift","score":0.12,"snippet":"..."}],"regions_used":["projects/drift"]}
```

Snippets redact secret note bodies and inline `<!--privacy:secret-->` regions.

## `memora_query_cited`

Input:

```json
{"query":"What did the team decide about drift's serialization format?","k":5}
```

Output:

```json
{
  "clean_text": "drift switched from JSON to MessagePack [claim:drf75a1c9e10b2aa]",
  "verified_count": 1,
  "degraded": false,
  "checks": [{"claim_id": "drf75a1c9e10b2aa", "status": "verified"}]
}
```

When network LLM is disabled, responses are built extractively from indexed
claims with `degraded: true`. Secret claims sent to cloud providers are redacted
per `[privacy].redact_secret_in_cloud` in vault config.

## `memora_get_note`

Input: `{"id":"note-1"}`

Output keys: `id`, `region`, `summary`, `privacy`, `body`, `body_redacted`,
`tags`, `refs`, `wikilinks`

Secret notes return `"body": "[redacted]"` and `"body_redacted": true`.

## `memora_get_atlas`

Input: `{"region":"projects/drift"}`

Output keys: `region`, `atlas_markdown`, `note_count`

## `memora_get_world_map`

Input: `{}`

Output: `{"markdown":"# World Map ..."}`

## `memora_capture`

Input:

```json
{"region":"inbox","summary":"Quick capture","body":"...", "tags":["inbox"], "privacy":"private"}
```

Output: `{"id":"note-...","path":"inbox/note-....md"}`

Region paths must stay inside the vault (`..` and absolute paths are rejected).
Captured notes are indexed with claim extraction when network LLM is enabled.

## `memora_consolidate`

Input: `{"scope":"all"}` or `{"scope":"region:work/projects"}`

Output keys: `regions_rebuilt`, `notes_moved`

Requires `MEMORA_ENABLE_NETWORK_LLM=1`.

## `memora_verify_claim`

Input: `{"claim_id":"abcd1234"}`

Output keys: `exists`, `span_intact`, `current_text`

## `memora_stale_claims`

Input: `{}`

Output: array of stale claim rows.

## `memora_contradictions`

Input: `{"subject":"drift-bench"}` (optional)

Output: array of contradiction rows.

## `memora_challenge`

Input: `{}`

Output: `ChallengerReport` JSON.

Requires `MEMORA_ENABLE_NETWORK_LLM=1`.

## `memora_decisions`

Input: `{}`

Output: `[{id,title,decided_on,status}]`
