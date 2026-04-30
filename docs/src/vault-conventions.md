# Vault Conventions

Memora reads markdown notes from your vault and writes derived artifacts under `.memora/`.

## Frontmatter (recommended)

Memora works with plain Obsidian-created notes that have no frontmatter.
On first `memora index` or `memora watch`, it auto-fills any missing fields
and prepends a YAML block while preserving your original note body byte-for-byte.

You can still provide and manage frontmatter manually; Memora only fills missing
fields and keeps user-provided values as-is.

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

Example inferred frontmatter for an Obsidian note `work/Project Idea.md`:

```yaml
---
id: project-idea
region: work
source: personal
privacy: private
created: 2026-04-30T18:42:10Z
updated: 2026-04-30T18:42:10Z
summary: "First non-empty line from the note body"
tags: []
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
