# License

**Apache 2.0 only.** See [LICENSE](https://github.com/radotsvetkov/memora/blob/main/LICENSE).

## Why Apache 2.0

Personal memory infrastructure should be portable, forkable, and embeddable
without surprise. Apache 2.0 was chosen specifically because:

- **It's permissive without being a free-for-all.** Companies and individuals
  can build on Memora - wrappers, internal forks, MCP integrations,
  distribution bundles - without infecting their own license. That matters
  for adoption inside organizations where copyleft would be a non-starter.

- **It includes an explicit patent grant.** Contributors who hold patents
  cannot retroactively assert them against downstream users of the code they
  contributed to. For a tool whose differentiators (claim graph, span
  fingerprint, citation validator) sit close to active patent thickets in
  the LLM tooling space, the explicit grant is non-negotiable.

- **It survives M&A.** A future Memora acquirer cannot revoke the license
  on existing releases. Once Apache 2.0 is published, it stays Apache 2.0
  for that version, forever.

- **It's compatible with the rest of the Rust ecosystem.** Most Cargo crates
  are MIT or Apache 2.0 dual-licensed; choosing Apache 2.0 lets us depend on
  them without friction and lets others depend on us symmetrically.

## What Apache 2.0 means in practice

You can:

- Use Memora commercially.
- Modify Memora and distribute the modifications (under Apache 2.0).
- Patent your own improvements while keeping Memora's grant intact.
- Bundle Memora into a closed-source product, as long as you preserve the
  notice file and don't claim Memora endorsement.
- Run Memora on private vaults, internal infrastructure, or air-gapped
  systems without contacting anyone.

You must:

- Include a copy of the Apache 2.0 license with any redistribution.
- Preserve copyright, patent, trademark, and attribution notices.
- State significant changes if you redistribute modified versions.
- Not use the **Memora** name or logo to imply endorsement of a fork without
  permission.

## SPDX identifier

```
SPDX-License-Identifier: Apache-2.0
```

## Full text

See [LICENSE](https://github.com/radotsvetkov/memora/blob/main/LICENSE) in
the repository root for the canonical Apache 2.0 text.
