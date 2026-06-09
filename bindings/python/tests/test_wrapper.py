import json
import subprocess

import pytest

import memora_verify as mv


def _completed(stdout, returncode=0, stderr=""):
    def _run(cmd, input=None, capture_output=True, text=True, timeout=None):
        return subprocess.CompletedProcess(cmd, returncode, stdout, stderr)

    return _run


def test_verify_parses_a_clean_answer(monkeypatch):
    payload = {
        "verified": 1,
        "unverified": 0,
        "mismatch": 0,
        "superseded": 0,
        "problems": 0,
        "clean_text": "ok",
        "checks": [{"claim_id": "0123abcd", "status": "verified", "reason": "hash matches"}],
    }
    monkeypatch.setenv("MEMORA_BIN", "/bin/echo")
    monkeypatch.setattr(mv.subprocess, "run", _completed(json.dumps(payload), 0))
    r = mv.verify("drift uses MessagePack [claim:0123abcd].", vault="./sources")
    assert r.ok
    assert r.verified == 1 and r.problems == 0
    assert r.checks[0].verified
    r.assert_ok()  # must not raise


def test_verify_flags_unprovable_citation(monkeypatch):
    payload = {
        "verified": 0,
        "unverified": 1,
        "mismatch": 0,
        "superseded": 0,
        "problems": 1,
        "clean_text": "",
        "checks": [{"claim_id": "deadbeef", "status": "unverified", "reason": "hallucinated id"}],
    }
    monkeypatch.setenv("MEMORA_BIN", "/bin/echo")
    monkeypatch.setattr(mv.subprocess, "run", _completed(json.dumps(payload), 1))
    r = mv.verify("nope [claim:deadbeef].", vault="./sources")
    assert not r.ok and r.problems == 1
    with pytest.raises(AssertionError) as excinfo:
        r.assert_ok()
    assert "deadbeef" in str(excinfo.value)


def test_assert_cited_raises_on_problem(monkeypatch):
    payload = {
        "verified": 0,
        "unverified": 1,
        "problems": 1,
        "checks": [{"claim_id": "x", "status": "unverified", "reason": "r"}],
    }
    monkeypatch.setenv("MEMORA_BIN", "/bin/echo")
    monkeypatch.setattr(mv.subprocess, "run", _completed(json.dumps(payload), 1))
    with pytest.raises(AssertionError):
        mv.assert_cited("x [claim:x].", vault="./v")


def test_entailment_fields_parsed(monkeypatch):
    payload = {
        "verified": 1,
        "unverified": 0,
        "mismatch": 0,
        "superseded": 0,
        "problems": 0,
        "clean_text": "ok",
        "entailment_checked": True,
        "unsupported": 1,
        "checks": [
            {"claim_id": "a", "status": "verified", "reason": "ok", "entailment": "unsupported"}
        ],
    }
    monkeypatch.setenv("MEMORA_BIN", "/bin/echo")
    monkeypatch.setattr(mv.subprocess, "run", _completed(json.dumps(payload), 0))
    r = mv.verify("a [claim:a].", vault="./v", entailment=True)
    assert r.entailment_checked and r.unsupported == 1
    assert r.checks[0].entailment == "unsupported"


def test_binary_not_found(monkeypatch):
    monkeypatch.delenv("MEMORA_BIN", raising=False)
    monkeypatch.setattr(mv.shutil, "which", lambda name: None)
    with pytest.raises(mv.MemoraNotFound):
        mv.verify("x", vault="./v")


def test_non_json_output_raises(monkeypatch):
    monkeypatch.setenv("MEMORA_BIN", "/bin/echo")
    monkeypatch.setattr(mv.subprocess, "run", _completed("boom: not json", 2, stderr="trace"))
    with pytest.raises(mv.MemoraError):
        mv.verify("x", vault="./v")


def test_flags_and_stdin_are_passed(monkeypatch):
    captured = {}

    def _run(cmd, input=None, capture_output=True, text=True, timeout=None):
        captured["cmd"] = cmd
        captured["input"] = input
        return subprocess.CompletedProcess(cmd, 0, json.dumps({"problems": 0, "checks": []}), "")

    monkeypatch.setenv("MEMORA_BIN", "/bin/memora")
    monkeypatch.setattr(mv.subprocess, "run", _run)
    mv.verify("ans", vault="/v", allow_superseded=True, entailment=True, fail_unsupported=True)
    assert captured["cmd"][:4] == ["/bin/memora", "verify", "--json", "--vault"]
    assert "--allow-superseded" in captured["cmd"]
    assert "--entailment" in captured["cmd"]
    assert "--fail-unsupported" in captured["cmd"]
    assert captured["input"] == "ans"
