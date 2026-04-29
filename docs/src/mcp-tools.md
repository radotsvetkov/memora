# MCP Tools

All tools return JSON payloads.

## `memora_query`

Input:

```json
{"query":"What changed in project X?","k":5}
```

Output:

```json
{"hits":[{"id":"note-1","summary":"...","region":"ops","score":0.12,"snippet":"..."}],"regions_used":["ops"]}
```

## `memora_query_cited`

Input:

```json
{"query":"Where does Rado work?","k":5}
```

Output:

```json
{"clean_text":"...","verified_count":2,"checks":[{"claim_id":"abcd","status":"verified"}]}
```

## `memora_get_note`

Input: `{"id":"note-1"}`

Output keys: `id`, `region`, `summary`, `body`, `tags`, `refs`, `wikilinks`, `hebbian_neighbors`

## `memora_get_atlas`

Input: `{"region":"work/projects"}`

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

Input: `{"subject":"INTERNORGA"}` (optional)

Output: array of contradiction rows.

## `memora_challenge`

Input: `{}`

Output: `ChallengerReport` JSON.

## `memora_decisions`

Input: `{}`

Output: `[{id,title,decided_on,status}]`
