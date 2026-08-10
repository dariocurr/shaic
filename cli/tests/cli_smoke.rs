use std::path::Path;
use std::process::Command as StdCommand;

use assert_cmd::Command;

fn init_bare_remote() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let status = StdCommand::new("git")
        .args(["init", "--bare", "-q"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert!(status.success());
    dir
}

/// Fake identity for the temp `$HOME` used by this smoke test — CI runners
/// (and isolated HOMEs) have no `user.name`/`user.email`, and shaic must not
/// invent one for real users.
fn write_test_gitconfig(home: &Path) {
    std::fs::write(
        home.join(".gitconfig"),
        "[user]\n\tname = shaic-test\n\temail = shaic-test@example.com\n",
    )
    .unwrap();
}

/// A fake `$EDITOR` that overwrites whatever file it's given with fixed,
/// valid frontmatter+body content — lets the CLI's `skill add`/`edit` flow
/// (which normally opens a real editor) run non-interactively in CI.
fn fake_editor() -> tempfile::TempPath {
    let script = tempfile::Builder::new().suffix(".sh").tempfile().unwrap();
    std::fs::write(
        script.path(),
        "#!/bin/sh\ncat > \"$1\" <<'BODY'\n---\nname: ci-rule\ndescription: added by CI smoke test\napplies_to: []\ntags: []\nscope: [global, project]\n---\n\nAlways write tests.\nBODY\n",
    )
    .unwrap();
    let mut perms = std::fs::metadata(script.path()).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(script.path(), perms).unwrap();
    script.into_temp_path()
}

#[test]
fn init_skill_add_sync_push_end_to_end() {
    let home = tempfile::tempdir().unwrap();
    write_test_gitconfig(home.path());
    let remote = init_bare_remote();
    let remote_url = remote.path().to_string_lossy().into_owned();
    let project = tempfile::tempdir().unwrap();
    let editor = fake_editor();

    Command::cargo_bin("shaic")
        .unwrap()
        .env("HOME", home.path())
        .args(["init", "--remote", &remote_url])
        .assert()
        .success();

    Command::cargo_bin("shaic")
        .unwrap()
        .env("HOME", home.path())
        .env("EDITOR", &editor)
        .args(["item", "add", "ci-rule", "--kind", "rule"])
        .assert()
        .success();

    Command::cargo_bin("shaic")
        .unwrap()
        .env("HOME", home.path())
        .args(["item", "list"])
        .assert()
        .success()
        .stdout(predicates::str::contains("ci-rule"));

    Command::cargo_bin("shaic")
        .unwrap()
        .env("HOME", home.path())
        .current_dir(project.path())
        .args(["project", "add"])
        .arg(project.path())
        .assert()
        .success();

    Command::cargo_bin("shaic")
        .unwrap()
        .env("HOME", home.path())
        .current_dir(project.path())
        .args(["sync", "--agent", "claude-code", "--project", "--yes"])
        .assert()
        .success();

    let claude_md = project.path().join(".claude").join("CLAUDE.md");
    let content = std::fs::read_to_string(&claude_md).expect("CLAUDE.md should have been written");
    assert!(content.contains("Always write tests."));

    Command::cargo_bin("shaic")
        .unwrap()
        .env("HOME", home.path())
        .args(["push"])
        .assert()
        .success();

    // A second, independent clone should see the same content after `pull`.
    let home2 = tempfile::tempdir().unwrap();
    write_test_gitconfig(home2.path());
    Command::cargo_bin("shaic")
        .unwrap()
        .env("HOME", home2.path())
        .args(["init", "--remote", &remote_url])
        .assert()
        .success();
    Command::cargo_bin("shaic")
        .unwrap()
        .env("HOME", home2.path())
        .args(["item", "list"])
        .assert()
        .success()
        .stdout(predicates::str::contains("ci-rule"));
}
