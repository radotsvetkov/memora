# Citation Protocol

Memora treats citations as verifiable references to claim IDs.

## Claim marker format

LLM responses use `[claim:<id>]`.

Example:

```text
drift switched to MessagePack for serialization [claim:drf75a1c9e10b2aa]
```

## Verification steps

1. Parse claim markers from LLM output.
2. Resolve each claim ID from SQLite.
3. Resolve note path and byte span.
4. Re-read source span from markdown body.
5. Recompute BLAKE3 fingerprint.
6. Optionally verify quote overlap.

## Status values

- `verified`
- `unverified` (claim missing)
- `fingerprint_mismatch` (source changed)
- `quote_mismatch` (marker quote unsupported)

## Verified answer semantics

`clean_text` is rewritten to keep only statements supported by verified claim markers.

This means `"verified"` in Memora is an architectural property (data + hash + span), not a prompt instruction.

For example, a verified claim can point to `semantic/projects/drift/roadmap.md` with a span that captures the MessagePack decision text.
