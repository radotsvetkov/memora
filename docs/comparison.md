# Comparison

| System | Unit of memory | Verifiable citations | Temporal validity | Privacy redaction at claim-level | MCP-native |
|---|---|---|---|---|---|
| **Memora** | Claim graph | Yes (span fingerprint + quote checks) | Yes (`valid_from` / `valid_until`) | Yes (`secret` span-aware redaction) | Yes |
| Ori-Mnemos | Note/chunk-centric | Partial / prompt-level | Limited | Limited | Varies |
| MDEMG | Graph memory abstractions | Usually indirect | Varies | Varies | Varies |
| claude-obsidian tooling | Note retrieval | Prompt-dependent | Limited | Vault-level only | Via wrappers |

Memora's core distinction is structural claim verification against source spans, not retrieval quality alone.
