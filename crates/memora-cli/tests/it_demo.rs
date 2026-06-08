use assert_cmd::Command;

/// `memora demo` must run with no vault, no config, no network — and must
/// actually catch the fabricated citations via the real validator.
#[test]
fn demo_runs_offline_and_catches_fabricated_citations() {
    let mut cmd = Command::cargo_bin("memora").expect("memora binary");
    let assert = cmd.arg("demo").env("NO_COLOR", "1").assert().success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();

    assert!(
        stdout.contains("VERIFIED"),
        "shows a verified citation:\n{stdout}"
    );
    assert!(
        stdout.contains("REJECTED"),
        "rejects fabricated citations:\n{stdout}"
    );
    assert!(
        stdout.contains("SUPERSEDED"),
        "flags a superseded claim:\n{stdout}"
    );
    assert!(stdout.contains("1 verified"), "verdict counts:\n{stdout}");
    assert!(stdout.contains("3 rejected"), "verdict counts:\n{stdout}");

    // The fabricated "PostgreSQL" line must not survive into what memora returns.
    let returns = stdout
        .split("What memora actually returns")
        .nth(1)
        .unwrap_or("");
    assert!(
        !returns.contains("PostgreSQL"),
        "fabricated claim must be stripped from the returned answer:\n{stdout}"
    );

    // No "database is locked" noise on a fresh ephemeral db.
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        !stderr.to_lowercase().contains("database is locked"),
        "fresh-db open must not emit lock errors:\n{stderr}"
    );
}
