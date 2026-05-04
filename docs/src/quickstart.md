# Quickstart (10 Minutes)

**Verifiable cognitive memory for personal vaults. Cite-or-it-didn't-happen.**

Memora retrieves claims, not notes - atomic facts with source-span pointers,
validity windows, and privacy bands. Every LLM citation is architecturally
validated against your markdown.

This walkthrough takes a determined user from install to first verified cited
answer in about 10 minutes.

## 1) Install Memora

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/radotsvetkov/memora/releases/latest/download/memora-cli-installer.sh | sh
```

<details>
<summary>What just happened</summary>

The installer places the Memora binaries on your system path (typically including
`memora` and `memora-mcp`), so you can run CLI commands and expose MCP tools to
clients without manual build steps.

</details>

## 2) Initialize a vault

```bash
memora init --vault ~/brain
```

<details>
<summary>What just happened</summary>

Memora created a vault scaffold with:

- `world_map.md` (generated memory overview surface)
- a sample semantic region folder
- `.memora/config.toml` for local configuration and sidecar state

Your markdown vault stays human-readable and Obsidian-compatible from the start.

</details>

## 3) Add your first note manually

Create `~/brain/semantic/projects/drift/roadmap.md`:

```markdown
---
title: drift Serialization Decision
type: semantic
date: 2025-09-12
region: projects/drift
privacy: private
tags: [drift, serialization, architecture]
---

drift switched its serialization format from JSON to MessagePack in Q3.
Benchmarking showed a 3x throughput improvement.
Decision recorded in roadmap, retro, and review notes.
```

<details>
<summary>What just happened</summary>

You wrote plain markdown with frontmatter. No proprietary format is required.
Memora will use this as extraction input and anchor future citations to precise
source spans inside this file.

</details>

## 4) Index the vault

```bash
memora index --vault ~/brain
```

<details>
<summary>What just happened</summary>

Indexing runs a full pipeline:

1. note metadata parsed and recorded
2. claim extraction generates atomic claim candidates
3. each stored claim is linked to source byte range and fingerprinted
4. lexical and vector indexes are updated
5. contradiction checks compare new/current claims and supersede older conflicts

The result is a queryable claim graph rather than a plain chunk index.

</details>

## 5) Ask your first query

```bash
memora query "What did we decide about drift's serialization format?" --vault ~/brain
```

<details>
<summary>What just happened</summary>

Memora retrieved claims, assembled model context, generated an answer, and
validated cited claim ids against source spans before returning output. In the
answer footer, citation entries correspond to claim ids that survived validation.

</details>

## 6) Add Claude Code MCP integration

Add one server entry to your MCP config:

```json
{"mcpServers":{"memora":{"command":"/usr/local/bin/memora-mcp","env":{"MEMORA_VAULT":"/absolute/path/to/brain"}}}}
```

<details>
<summary>What just happened</summary>

Claude Code can now call Memora tools over stdio MCP. Instead of passing raw note
chunks manually, you get memory-tool access with claim-aware retrieval and
built-in citation validation behavior.

</details>

## 7) Ask the same question in Claude Code

Prompt:

```text
What did we decide about drift's serialization format?
```

<details>
<summary>What just happened</summary>

Claude Code routed the request through Memora MCP tools. The response surfaced
with verified citations tied to your markdown-backed claims. If a citation fails
validation, Memora strips/retries with verified-only context before returning.

</details>

## Next 5 minutes (recommended)

- Run `memora watch --vault ~/brain` in a dedicated terminal tab.
- Add inline secret markers in mixed-sensitivity notes.
- Check `world_map.md` after your next capture session.
- Use `memora_record_useful` when a cited answer was especially helpful.

## Troubleshooting basics

- `command not found: memora` -> restart shell or verify installer path export.
- No claims extracted -> confirm frontmatter + note content + successful indexing.
- MCP tool unavailable -> verify `memora-mcp` path and `MEMORA_VAULT` value.
- Sparse answers -> add more factual statements; reflection-only notes yield fewer
  claims by design.
