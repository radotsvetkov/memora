# Vault Conventions

Memora reads markdown notes from your vault and writes derived artifacts under `.memora/`.

## Frontmatter (recommended)

Memora works with plain Obsidian-created notes that have no frontmatter.
On first `memora index` or `memora watch`, it auto-fills any missing fields
and prepends a YAML block while preserving your original note body byte-for-byte.

You can still provide and manage frontmatter manually. Memora applies a few
quality-of-life normalizations while indexing:

- `region` follows the note's folder path relative to the vault root.
- `updated` follows the file mtime (second precision).
- `refs` can optionally mirror detected wikilinks (configurable).
- Invalid `source` / `privacy` enum values are normalized to defaults.

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
- `.memora/watch.lock` (present while `memora watch` is running)

Derived markdown can be regenerated and should not be edited as source-of-truth notes.

## Frontmatter config knobs

In `.memora/config.toml`:

```toml
[frontmatter]
refs_mode = "sync_from_wikilinks" # or "manual"
```

- `sync_from_wikilinks`: keep `refs` aligned to `[[wikilinks]]` in body.
- `manual`: never auto-rewrite `refs`.
