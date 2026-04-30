# Obsidian Properties Template

Use this template in Obsidian so `source` and `privacy` are always visible and quick to edit.

## Suggested property options

- `source`: `personal`, `reference`, `derived`
- `privacy`: `public`, `private`, `secret`

Recommended defaults:

- `source: personal`
- `privacy: private`

## Example note template

```yaml
---
id: "{{title}}"
region: default
source: personal
privacy: private
created: 2026-01-01T00:00:00Z
updated: 2026-01-01T00:00:00Z
summary: ""
tags: []
refs: []
---
```

## Notes

- Memora keeps timestamps in second precision (`...T12:34:56Z`).
- If `frontmatter.refs_mode = "sync_from_wikilinks"`, `refs` is rewritten from body wikilinks.
- Invalid `source` / `privacy` values are normalized to `personal` / `private`.
