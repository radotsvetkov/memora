# memora-verify

Independent, deterministic verification of AI citations, for Python.

A thin wrapper around the [`memora`](https://github.com/radotsvetkov/memora) CLI.
Drop it into your AI eval suite or CI and fail the build when a model cites
something its source does not actually contain. The check re-reads the cited
source span and recomputes its hash — it does not ask a second model to grade the
first.

```bash
pip install memora-verify
```

> **Not yet on PyPI?** Publishing is wired up (see `RELEASING.md`) but may not
> have run yet. Until then, install straight from the repo:
> `pip install "git+https://github.com/radotsvetkov/memora.git#subdirectory=bindings/python"`

> **Prerequisite:** this wraps the `memora` binary. Install it once with
> `brew install radotsvetkov/memora/memora` or `cargo install memora-cli`, put it
> on `PATH`, or set `MEMORA_BIN` to its path. (Bundled-binary wheels are on the
> roadmap.)

## Use it

```python
from memora_verify import verify, assert_cited

answer = my_rag_app("What serialization format does drift use?")

# Inspect the result:
result = verify(answer, vault="./sources")
print(result.verified, "verified,", result.problems, "problems")
for check in result.checks:
    print(check.claim_id, check.status, check.reason)

# Or assert in one line (raises AssertionError if any citation is unprovable):
assert_cited(answer, vault="./sources")
```

## In your eval suite / pytest

```python
from memora_verify import assert_cited

def test_assistant_answers_are_grounded():
    answer = my_rag_app("What did we decide about the serialization format?")
    assert_cited(answer, vault="./sources")
```

That single assertion fails the test (and your CI) the moment the model cites a
source that doesn't say what it claims.

## Optional entailment

Provenance proves the source *contains* the quote. To also check, best-effort,
whether the source *supports* the claim (LLM-judged, kept separate from the
hash-proven part):

```python
result = verify(answer, vault="./sources", entailment=True)
# make "unsupported" fail too:
assert_cited(answer, vault="./sources", entailment=True, fail_unsupported=True)
```

## API

- `verify(answer, *, vault, binary=None, allow_superseded=False, entailment=False, fail_unsupported=False, timeout=120) -> VerifyResult`
- `assert_cited(answer, *, vault, **kwargs) -> VerifyResult` — verify and raise `AssertionError` on any unprovable citation.
- `VerifyResult` — `.ok`, `.problems`, `.verified`, `.unverified`, `.mismatch`, `.superseded`, `.unsupported`, `.entailment_checked`, `.clean_text`, `.checks` (list of `Check`), `.raw`, `.returncode`, and `.assert_ok()` / `.assert_verified()`.
- `Check` — `.claim_id`, `.status`, `.reason`, `.entailment`, `.verified`.
- `find_binary(binary=None) -> str` — locate the `memora` binary.
- Exceptions: `MemoraError`, `MemoraNotFound`.

## License

Apache-2.0. Part of the [memora](https://github.com/radotsvetkov/memora) project.
