# Obsidian + Memora Daily Driver Guide

**Verifiable cognitive memory for personal vaults. Cite-or-it-didn't-happen.**

Memora retrieves claims, not notes - atomic facts with source-span pointers,
validity windows, and privacy bands. Every LLM citation is architecturally
validated against your markdown.

This guide is for Obsidian users who want Memora as a local-first memory backend
while using Claude Code as the conversational interface.

## What this guide assumes

- You already keep (or want to keep) a markdown vault.
- You want to preserve prose as prose, not force everything into rigid fields.
- You want model answers that are grounded in verifiable source spans.
- You want a practical workflow you can run daily with minimal friction.

## Recommended Vault Layout

Use a structure that keeps note intent clear and lets Memora infer context.

```text
~/brain/
  episodic/
    meeting-2025-q3-arch.md
    2025-10-design-review.md
  semantic/
    projects/
      drift/
        roadmap.md
        serialization-strategy-2025-q3.md
      drift-bench/
        benchmark-language-notes.md
    people/
      sarah.md
      james.md
      priya.md
  procedural/
    release-checklist.md
    incident-playbook.md
  world_map.md
  .memora/
    config.toml
    state.db
    logs/
```

### Why these folders

- **`episodic/`** for dated observations, meetings, and journals.
- **`semantic/<region>/`** for durable facts and reference knowledge by domain.
- **`procedural/`** for repeatable processes, checklists, runbooks.

This layout is not a hard requirement. It is a practical default that helps both
human navigation and memory extraction.

### `.memora/` sidecar and git

Keep `.memora/` out of your notes repository unless you explicitly want derived
state tracked. Typical `.gitignore` entry:

```gitignore
.memora/
```

Your markdown remains the durable, user-owned artifact; `.memora/` is generated
state for indexing, embeddings, and runtime metadata.

## Frontmatter Spec

Memora reads frontmatter for classification, temporal cues, and privacy defaults.

### Required fields (recommended baseline)

- `type`: `episodic` | `semantic` | `procedural`
- `date`: ISO date (`YYYY-MM-DD`) for primary event/authoring context
- `region`: short domain label (for semantic grouping)
- `privacy`: `public` | `private` | `secret`

### Optional fields

- `tags`: array of topical labels
- `status`: `draft` | `active` | `archived`
- `valid_from`, `valid_until`: explicit claim window hints for extraction
- `source`: external provenance hint (book, meeting, URL, etc.)
- `confidence`: self-assessed confidence marker for nuanced material

### Full frontmatter example

```yaml
---
title: drift Serialization Decision
type: semantic
date: 2025-09-12
region: projects/drift
privacy: private
tags: [drift, serialization, architecture]
status: active
valid_from: 2025-09-12
valid_until:
source: leadership-sync
confidence: medium
---
```

### Notes on strictness

Memora can still extract claims from imperfect notes. The frontmatter spec is a
quality lever, not a binary gate. Missing optional fields reduce structure but do
not block indexing.

## Inline Privacy Marker Syntax

Use inline markers when only part of a note is sensitive.

```markdown
This project update is broadly shareable.

<!--privacy:secret-->
The account number is 4451-XX and pending legal review details are in draft.
<!--/privacy-->

Action items remain public.
```

Supported marker levels:

- `public`
- `private`
- `secret`

### How inheritance works

Claim privacy is the strictest of:

1. note frontmatter privacy
2. inline marker privacy (if present)

So if note privacy is `private` and inline section is `secret`, extracted claims
from that section become `secret`.

## `world_map.md`: What It Is

`world_map.md` is a generated overview of your current memory topology and review
signals. It is not your canonical writing surface; it is a maintenance and
situation-awareness artifact.

Typical sections include:

- active regions and note density
- recently superseded claims
- stale derivative hotspots
- contradiction clusters
- frontier gaps (thinly connected domains)
- "Today's review" block from the challenger run

### When Memora rewrites it

Memora updates `world_map.md` during indexing/watch cycles when:

- enough new claims enter the graph
- contradiction/staleness landscape changes
- scheduled daily challenger run executes

You can edit this file manually, but treat generated sections as ephemeral.

### Reading "Today's review"

The challenger writes concise operational prompts, such as:

- "q4-rollout-plan depends on serialization-strategy-2024, now superseded."
- "drift-bench language disagreement: rust vs go."
- "drift backpressure strategy is still pending decision."

Read this section as a queue for targeted note maintenance, not as an alarm log.

## Daily Driver Workflow

This five-step loop is optimized for consistency, not perfection.

### 1) Capture in Obsidian (or via `memora_capture`)

Write naturally in Obsidian: meeting outcomes, decisions, reflections, plans.
If you are already in Claude Code and want quick capture, use `memora_capture`
to append a session note directly to the vault.

Guideline: prefer fast capture over over-structuring. Frontmatter and markers can
be refined afterward.

### 2) Run `memora watch` in a terminal tab

Keep one terminal running:

```bash
memora watch --vault ~/brain
```

The watch loop handles:

- file change detection
- indexing and extraction
- contradiction checks and supersession updates
- atlas/consolidation updates
- daily challenger pass

This is the background "memory maintenance engine." You keep writing; Memora keeps
the claim graph current.

### 3) Ask questions in Claude Code

Use Claude Code with Memora MCP configured. Ask normal questions:

- "What did the team decide about drift's serialization format?"
- "What changed in deployment strategy this month?"
- "Which assumptions in product strategy were superseded?"

Memora-backed responses include claim citations that are validated before display.

### 4) Reinforce useful retrievals with `memora_record_useful`

When a cited answer is useful, mark it:

- this feeds Q-value style reinforcement for retrieval policy
- high-value paths become easier to prioritize in future ranking

You are effectively training the memory prioritization loop with low-friction
feedback from real usage.

### 5) Review `world_map.md` regularly

Open the "Today's review" section every day or two. Act on one or two items:

- resolve contradictions where intent changed
- refresh stale syntheses after major note edits
- fill frontier gaps by capturing missing context

Small, regular interventions are better than monthly cleanup marathons.

## Claims vs Prose: What Goes Where

This distinction matters because Memora extracts claims from prose, but not every
sentence should be forced into atomic-fact style.

### Put in claims (naturally, through prose that contains facts)

- explicit decisions ("We chose X over Y.")
- commitments ("Deadline moved to May 10.")
- stable descriptors ("Service A depends on Service B.")
- measurable observations ("Error rate rose after deployment.")

### Leave as prose

- reflections and uncertainty exploration
- narrative context and emotional processing
- rough ideation where assertions are intentionally fluid
- personal meaning-making that is not a stable factual statement

Both are valuable. Reflection-heavy notes may yield fewer extracted claims, and
that is expected. The goal is not maximal claim density; the goal is faithful
memory with usable evidence.

### Practical writing pattern

A useful balance:

1. write free-form reflection first
2. include a short "Decisions / Facts" section when relevant
3. keep sensitive lines wrapped with inline markers when needed

This gives Memora clear extraction anchors while preserving narrative richness.

## Privacy Practice: Frontmatter vs Inline Markers

Use the privacy tools intentionally by granularity.

### Use frontmatter `privacy: secret` when

- the whole note is sensitive
- sharing any part externally is inappropriate
- the note is primarily credentials, legal details, or private health data

### Use inline markers when

- most of the note is shareable but a few passages are sensitive
- you want one note to hold both operational and confidential details
- you need surgical redaction without splitting files unnaturally

### Operational rule of thumb

- whole-note sensitivity -> frontmatter
- partial sensitivity -> inline markers

This keeps notes readable while preserving strict outbound redaction behavior.

## Querying Expectations in Practice

When you ask a question, expect three outcomes:

1. **Verified answer with citations** (normal path)
2. **Answer with fewer citations** because some were invalidated and stripped
3. **Retry-constrained answer** generated from verified-only context

If a claim citation cannot be validated against source span fingerprint, it does
not pass through as-is. This is central to Memora's trust model.

## Maintenance Habits That Keep Quality High

- Keep frontmatter consistent in newly created notes.
- Prefer one fact per sentence for high-signal decisions and commitments.
- Re-index after major refactors or bulk note moves.
- Resolve challenger-highlighted contradiction clusters early.
- Keep vault paths stable when possible; large path churn can increase stale
  maintenance overhead.

## Common Pitfalls

- **Over-optimizing templates too early**: start with a minimal frontmatter set.
- **Ignoring stale flags**: stale does not mean wrong, but it does mean review.
- **Using `secret` everywhere**: overuse reduces model utility; mark surgically.
- **Treating `world_map.md` as canonical prose**: it is generated maintenance
  output, not your long-form notebook.

## Minimal Setup Checklist

- [ ] Vault created and initialized with Memora
- [ ] `.memora/` gitignored
- [ ] Frontmatter defaults chosen
- [ ] `memora watch` running in one terminal tab
- [ ] MCP integration configured for Claude Code
- [ ] First verified cited query completed
- [ ] First `world_map.md` review done

## Closing

Obsidian remains your writing environment. Memora adds a verifiable memory layer
that tracks claim provenance, temporal validity, privacy boundaries, and citation
integrity. Used daily, the combination gives you both narrative freedom and
evidence-backed recall: write naturally, query confidently, and review memory
health through `world_map.md`.
