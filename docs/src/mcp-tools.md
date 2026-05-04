# MCP Tools

All tools return JSON payloads.

## `memora_query`

Input:

```json
{"query":"What did the team decide about drift's serialization format?","k":5}
```

Output:

```json
{"hits":[{"id":"note-1","summary":"drift moved to MessagePack in Q3","region":"projects/drift","score":0.12,"snippet":"..."}],"regions_used":["projects/drift"]}
```

## `memora_query_cited`

Input:

```json
{"query":"What did the team decide about drift's serialization format?","k":5}
```

Output:

```json
{"clean_text":"drift switched from JSON to MessagePack [claim:drf75a1c9e10b2aa]","verified_count":1,"checks":[{"claim_id":"drf75a1c9e10b2aa","status":"verified"}]}
```

## `memora_get_note`

Input: `{"id":"note-1"}`

Output keys: `id`, `region`, `summary`, `body`, `tags`, `refs`, `wikilinks`, `hebbian_neighbors`

## `memora_get_atlas`

Input: `{"region":"projects/drift"}`

Output keys: `region`, `atlas_markdown`, `note_count`

## `memora_get_world_map`

Input: `{}`

Output: `{"markdown":"# World Map ..."}`

## `memora_neighbors`

Input: `{"id":"note-1","top_n":5}`

Output keys: `hebbian`, `wikilinks`

## `memora_record_useful`

Input: `{"query_id":"...","useful_ids":["note-a","note-b"]}`

Output: `{"ok":true}`

## `memora_capture`

Input:

```json
{"region":"inbox","summary":"Quick capture","body":"...", "tags":["inbox"], "privacy":"private"}
```

Output: `{"id":"note-...","path":"inbox/note-....md"}`

## `memora_consolidate`

Input: `{"scope":"all"}` or `{"scope":"region:work/projects"}`

Output keys: `regions_rebuilt`, `notes_moved`

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

## `memora_decisions`

Input: `{}`

Output: `[{id,title,decided_on,status}]`
