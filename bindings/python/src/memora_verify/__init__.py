"""memora-verify — independent, deterministic verification of AI citations.

A thin Python wrapper around the ``memora`` CLI. Use it in your AI eval suite or
CI to fail the build when a model cites something its source does not actually
contain.

    from memora_verify import verify, assert_cited

    result = verify(answer_text, vault="./sources")
    print(result.verified, "verified,", result.problems, "problems")

    # in a test:
    assert_cited(answer_text, vault="./sources")   # raises if any citation is unprovable

This shells out to the ``memora`` binary (install it with
``brew install radotsvetkov/memora/memora`` or ``cargo install memora-cli``, put
it on PATH, or point ``MEMORA_BIN`` at it). Bundled-binary wheels are on the
roadmap; today the binary is a prerequisite.
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional, Union

__all__ = [
    "verify",
    "assert_cited",
    "VerifyResult",
    "Check",
    "MemoraError",
    "MemoraNotFound",
    "find_binary",
    "__version__",
]

__version__ = "0.1.0"

PathLike = Union[str, "os.PathLike[str]"]


class MemoraError(RuntimeError):
    """The ``memora`` binary failed or produced output we could not parse."""


class MemoraNotFound(MemoraError):
    """The ``memora`` binary could not be located."""


def find_binary(binary: Optional[PathLike] = None) -> str:
    """Locate the ``memora`` binary.

    Resolution order: the ``binary`` argument, then ``$MEMORA_BIN``, then
    ``memora`` on ``PATH``. Raises :class:`MemoraNotFound` if none is found.
    """
    candidate = binary or os.environ.get("MEMORA_BIN") or shutil.which("memora")
    if not candidate:
        raise MemoraNotFound(
            "could not find the `memora` binary. Install it "
            "(`brew install radotsvetkov/memora/memora` or `cargo install memora-cli`), "
            "put it on PATH, or set the MEMORA_BIN environment variable to its path."
        )
    return str(candidate)


@dataclass(frozen=True)
class Check:
    """The verdict for a single ``[claim:ID]`` citation in an answer."""

    claim_id: str
    status: str  # verified | unverified | fingerprint_mismatch | quote_mismatch | superseded
    reason: str
    entailment: Optional[str] = None  # entailed | unsupported | unchecked (if --entailment)

    @property
    def verified(self) -> bool:
        return self.status == "verified"


@dataclass(frozen=True)
class VerifyResult:
    """The parsed result of ``memora verify --json``.

    ``ok`` mirrors the process exit code: ``True`` means every citation was
    provable (and, with ``fail_unsupported``, entailed).
    """

    ok: bool
    problems: int
    verified: int
    unverified: int
    mismatch: int
    superseded: int
    unsupported: Optional[int]
    entailment_checked: bool
    clean_text: str
    checks: List[Check]
    raw: Dict[str, Any]
    returncode: int

    def assert_ok(self) -> "VerifyResult":
        """Raise :class:`AssertionError` if any citation could not be proven."""
        if not self.ok:
            raise AssertionError(self._failure_message())
        return self

    # Alias that reads naturally in tests.
    assert_verified = assert_ok

    def _failure_message(self) -> str:
        bad = [c for c in self.checks if c.status != "verified"]
        lines = [f"  - [claim:{c.claim_id}] {c.status}: {c.reason}" for c in bad]
        if self.entailment_checked:
            unsupported = [
                f"  - [claim:{c.claim_id}] entailment: unsupported"
                for c in self.checks
                if c.entailment == "unsupported"
            ]
            lines += unsupported
        detail = "\n".join(lines) if lines else "  (see result.raw for details)"
        return f"memora found {self.problems} unprovable citation(s):\n{detail}"


def verify(
    answer: str,
    *,
    vault: PathLike,
    binary: Optional[PathLike] = None,
    allow_superseded: bool = False,
    entailment: bool = False,
    fail_unsupported: bool = False,
    timeout: Optional[float] = 120,
) -> VerifyResult:
    """Verify the citations in ``answer`` against the sources in ``vault``.

    Args:
        answer: The AI answer text, containing ``[claim:ID]`` markers.
        vault: Path to the indexed memora vault / source corpus.
        binary: Explicit path to the ``memora`` binary (else auto-discovered).
        allow_superseded: Do not count superseded citations as problems.
        entailment: Also run the optional, LLM-judged entailment check.
        fail_unsupported: With ``entailment``, treat "unsupported" as a failure.
        timeout: Seconds to wait for the binary (``None`` to wait forever).

    Returns:
        A :class:`VerifyResult`. Call :meth:`VerifyResult.assert_ok` in tests.
    """
    bin_path = find_binary(binary)
    cmd = [bin_path, "verify", "--json", "--vault", str(vault)]
    if allow_superseded:
        cmd.append("--allow-superseded")
    if entailment:
        cmd.append("--entailment")
    if fail_unsupported:
        cmd.append("--fail-unsupported")

    try:
        proc = subprocess.run(
            cmd,
            input=str(answer),
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    except FileNotFoundError as exc:
        raise MemoraNotFound(f"failed to run {bin_path!r}: {exc}") from exc
    except OSError as exc:
        raise MemoraError(f"failed to run {bin_path!r}: {exc}") from exc
    except subprocess.SubprocessError as exc:
        raise MemoraError(f"`memora verify` did not complete: {exc}") from exc

    out = proc.stdout.strip()
    if not out:
        raise MemoraError(
            f"`memora verify` produced no output (exit {proc.returncode}).\n"
            f"stderr:\n{proc.stderr.strip()}"
        )
    try:
        data = json.loads(out)
    except json.JSONDecodeError as exc:
        raise MemoraError(
            f"could not parse `memora verify` output as JSON (exit {proc.returncode}): {exc}\n"
            f"stdout:\n{out}\nstderr:\n{proc.stderr.strip()}"
        ) from exc

    if "checks" not in data or "problems" not in data:
        raise MemoraError(
            "`memora verify --json` output is missing expected fields "
            "('checks'/'problems'). This usually means the installed `memora` "
            f"CLI is a version memora-verify {__version__} does not understand.\n"
            f"stdout:\n{out}"
        )

    checks = [
        Check(
            claim_id=str(c.get("claim_id", "")),
            status=str(c.get("status", "")),
            reason=str(c.get("reason", "")),
            entailment=c.get("entailment"),
        )
        for c in data.get("checks", [])
    ]
    return VerifyResult(
        ok=(proc.returncode == 0),
        problems=int(data.get("problems", 0)),
        verified=int(data.get("verified", 0)),
        unverified=int(data.get("unverified", 0)),
        mismatch=int(data.get("mismatch", 0)),
        superseded=int(data.get("superseded", 0)),
        unsupported=data.get("unsupported"),
        entailment_checked=bool(data.get("entailment_checked", False)),
        clean_text=str(data.get("clean_text", "")),
        checks=checks,
        raw=data,
        returncode=proc.returncode,
    )


def assert_cited(answer: str, *, vault: PathLike, **kwargs: Any) -> VerifyResult:
    """Verify and raise :class:`AssertionError` if any citation is unprovable.

    A one-liner for AI eval suites and pytest:

        def test_answer_is_grounded():
            assert_cited(my_rag_app(question), vault="./sources")
    """
    return verify(answer, vault=vault, **kwargs).assert_ok()
