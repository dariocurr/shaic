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

fn with_isolated_home<'a>(cmd: &'a mut Command, home: &Path) -> &'a mut Command {
    let appdata = home.join("AppData/Roaming");
    let local = home.join("AppData/Local");
    let xdg = home.join(".config");
    std::fs::create_dir_all(&appdata).unwrap();
    std::fs::create_dir_all(&local).unwrap();
    std::fs::create_dir_all(&xdg).unwrap();
    cmd.env("HOME", home)
        .env("USERPROFILE", home)
        .env("APPDATA", &appdata)
        .env("LOCALAPPDATA", &local)
        .env("XDG_CONFIG_HOME", &xdg);
    if let Some(s) = home.to_str() {
        if let Some((drive, rest)) = s.split_once(':') {
            if drive.len() == 1 {
                cmd.env("HOMEDRIVE", format!("{drive}:"));
                cmd.env("HOMEPATH", rest);
            }
        }
    }
    cmd
}

/// Fake identity for an isolated home — CI runners often have no git user.
fn write_test_gitconfig(home: &Path) {
    std::fs::create_dir_all(home).unwrap();
    std::fs::write(
        home.join(".gitconfig"),
        "[user]\n\tname = shaic-test\n\temail = shaic-test@example.com\n",
    )
    .unwrap();
}

const EDITOR_BODY: &str = "---\nname: ci-rule\ndescription: added by CI smoke test\napplies_to: []\ntags: []\nscope: [global, project]\n---\n\nAlways write tests.\n";

struct FakeEditor {
    script: tempfile::TempPath,
    _payload: tempfile::TempPath,
}

impl AsRef<std::ffi::OsStr> for FakeEditor {
    fn as_ref(&self) -> &std::ffi::OsStr {
        self.script.as_os_str()
    }
}

fn fake_editor() -> FakeEditor {
    let payload = tempfile::Builder::new().suffix(".md").tempfile().unwrap();
    std::fs::write(payload.path(), EDITOR_BODY).unwrap();
    let payload = payload.into_temp_path();

    #[cfg(unix)]
    let script = {
        let script = tempfile::Builder::new().suffix(".sh").tempfile().unwrap();
        std::fs::write(
            script.path(),
            format!("#!/bin/sh\ncp '{}' \"$1\"\n", payload.display()),
        )
        .unwrap();
        let mut perms = std::fs::metadata(script.path()).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(script.path(), perms).unwrap();
        script.into_temp_path()
    };

    #[cfg(windows)]
    let script = {
        let script = tempfile::Builder::new().suffix(".cmd").tempfile().unwrap();
        std::fs::write(
            script.path(),
            format!(
                "@echo off\r\ncopy /Y \"{}\" \"%~1\" >nul\r\nexit /b 0\r\n",
                payload.display()
            ),
        )
        .unwrap();
        script.into_temp_path()
    };

    FakeEditor {
        script,
        _payload: payload,
    }
}

#[test]
fn help_and_version_run() {
    Command::cargo_bin("shaic")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicates::str::contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn init_skill_add_sync_push_end_to_end() {
    let home = tempfile::tempdir().unwrap();
    write_test_gitconfig(home.path());
    let remote = init_bare_remote();
    let remote_url = remote.path().to_string_lossy().into_owned();
    let project = tempfile::tempdir().unwrap();
    let editor = fake_editor();

    with_isolated_home(
        Command::cargo_bin("shaic")
            .unwrap()
            .args(["init", "--remote", &remote_url]),
        home.path(),
    )
    .assert()
    .success();

    with_isolated_home(
        Command::cargo_bin("shaic")
            .unwrap()
            .env("EDITOR", &editor)
            .args(["item", "add", "ci-rule", "--kind", "rule"]),
        home.path(),
    )
    .assert()
    .success();

    with_isolated_home(
        Command::cargo_bin("shaic").unwrap().args(["item", "list"]),
        home.path(),
    )
    .assert()
    .success()
    .stdout(predicates::str::contains("ci-rule"));

    with_isolated_home(
        Command::cargo_bin("shaic")
            .unwrap()
            .current_dir(project.path())
            .args(["project", "add"])
            .arg(project.path()),
        home.path(),
    )
    .assert()
    .success();

    with_isolated_home(
        Command::cargo_bin("shaic")
            .unwrap()
            .current_dir(project.path())
            .args(["sync", "--agent", "claude-code", "--project", "--yes"]),
        home.path(),
    )
    .assert()
    .success();

    let claude_md = project.path().join(".claude").join("CLAUDE.md");
    let content = std::fs::read_to_string(&claude_md).expect("CLAUDE.md should have been written");
    assert!(content.contains("Always write tests."));

    with_isolated_home(
        Command::cargo_bin("shaic").unwrap().args(["push"]),
        home.path(),
    )
    .assert()
    .success();

    let home2 = tempfile::tempdir().unwrap();
    write_test_gitconfig(home2.path());
    with_isolated_home(
        Command::cargo_bin("shaic")
            .unwrap()
            .args(["init", "--remote", &remote_url]),
        home2.path(),
    )
    .assert()
    .success();
    with_isolated_home(
        Command::cargo_bin("shaic").unwrap().args(["item", "list"]),
        home2.path(),
    )
    .assert()
    .success()
    .stdout(predicates::str::contains("ci-rule"));
}

#[test]
fn import_without_yes_refuses_when_stdin_is_not_a_tty() {
    let home = tempfile::tempdir().unwrap();
    write_test_gitconfig(home.path());
    let remote = init_bare_remote();
    let remote_url = remote.path().to_string_lossy().into_owned();

    with_isolated_home(
        Command::cargo_bin("shaic")
            .unwrap()
            .args(["init", "--remote", &remote_url]),
        home.path(),
    )
    .assert()
    .success();

    with_isolated_home(
        Command::cargo_bin("shaic").unwrap().args([
            "import",
            "--agent",
            "claude-code",
            "--project",
        ]),
        home.path(),
    )
    .assert()
    .failure()
    .stderr(predicates::str::contains("pass --yes"));
}

#[test]
fn sync_without_yes_refuses_when_there_are_changes() {
    let home = tempfile::tempdir().unwrap();
    write_test_gitconfig(home.path());
    let remote = init_bare_remote();
    let remote_url = remote.path().to_string_lossy().into_owned();
    let project = tempfile::tempdir().unwrap();
    let editor = fake_editor();

    with_isolated_home(
        Command::cargo_bin("shaic")
            .unwrap()
            .args(["init", "--remote", &remote_url]),
        home.path(),
    )
    .assert()
    .success();

    with_isolated_home(
        Command::cargo_bin("shaic")
            .unwrap()
            .env("EDITOR", &editor)
            .args(["item", "add", "ci-rule", "--kind", "rule"]),
        home.path(),
    )
    .assert()
    .success();

    with_isolated_home(
        Command::cargo_bin("shaic")
            .unwrap()
            .current_dir(project.path())
            .args(["sync", "--agent", "claude-code", "--project"]),
        home.path(),
    )
    .assert()
    .failure()
    .stderr(predicates::str::contains("pass --yes"));
}

#[test]
fn doctor_and_self_check_run() {
    let home = tempfile::tempdir().unwrap();
    write_test_gitconfig(home.path());

    with_isolated_home(
        Command::cargo_bin("shaic").unwrap().arg("doctor"),
        home.path(),
    )
    .assert()
    .success()
    .stdout(predicates::str::contains("shaic doctor"));

    with_isolated_home(
        Command::cargo_bin("shaic").unwrap().args(["self", "check"]),
        home.path(),
    )
    .assert()
    .success();

    with_isolated_home(
        Command::cargo_bin("shaic")
            .unwrap()
            .args(["doctor", "--json"]),
        home.path(),
    )
    .assert()
    .success()
    .stdout(predicates::str::contains("\"checks\""));
}

#[test]
fn status_json_after_init_reports_store() {
    let home = tempfile::tempdir().unwrap();
    write_test_gitconfig(home.path());
    let remote = init_bare_remote();
    let remote_url = remote.path().to_string_lossy().into_owned();

    with_isolated_home(
        Command::cargo_bin("shaic")
            .unwrap()
            .args(["init", "--remote", &remote_url]),
        home.path(),
    )
    .assert()
    .success();

    with_isolated_home(
        Command::cargo_bin("shaic")
            .unwrap()
            .args(["status", "--json"]),
        home.path(),
    )
    .assert()
    .success()
    .stdout(predicates::str::contains("\"uncommitted_changes\""));
}

#[test]
fn sync_without_store_exits_config_code() {
    let home = tempfile::tempdir().unwrap();
    write_test_gitconfig(home.path());

    with_isolated_home(
        Command::cargo_bin("shaic")
            .unwrap()
            .args(["sync", "--yes", "--all"]),
        home.path(),
    )
    .assert()
    .failure()
    .code(5);
}
