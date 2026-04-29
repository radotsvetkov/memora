# Vault Conventions

Memora reads markdown notes from your vault and writes derived artifacts under `.memora/`.

## Required frontmatter

```yaml
---
id: note-id
region: work/projects
source: personal
privacy: private
created: 2026-04-01T00:00:00Z
updated: 2026-04-01T00:00:00Z
summary: "Short summary"
tags: [tag1, tag2]
refs: []
---
```

## Inline privacy markers

Use markers to narrow privacy to a sub-span:

```md
This is public context.
<!--privacy:secret-->
Salary is 120k.
<!--/privacy-->
```

Claims extracted from secret spans are marked `secret` even when note privacy is broader.

## Derived files

- `world_map.md`
- `<region>/_atlas.md`
- `<region>/_index.md`
- `.memora/config.toml`
- `.memora/memora.db`
- `.memora/vectors/*`
- `.memora/last_challenger.json`

Derived markdown can be regenerated and should not be edited as source-of-truth notes.
