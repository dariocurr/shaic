use std::path::Path;
use std::process::Command;

use shaic_core::model::{AgentId, Frontmatter, Item, ItemKind, Scope};
use shaic_core::store::Store;

fn init_bare_remote() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let status = Command::new("git")
        .args(["init", "--bare", "-q"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert!(status.success());
    dir
}

/// Local-only fake identity so commits work on CI without a real git config.
fn set_test_git_identity(repo: &Path) {
    for (key, value) in [
        ("user.name", "shaic-test"),
        ("user.email", "shaic-test@example.com"),
        // Keep LF in the working tree so frontmatter parsers see what we wrote.
        ("core.autocrlf", "false"),
    ] {
        let status = Command::new("git")
            .args(["config", key, value])
            .current_dir(repo)
            .status()
            .unwrap();
        assert!(status.success());
    }
}

fn sample_item(name: &str) -> Item {
    Item::new(
        ItemKind::Rule,
        Frontmatter {
            name: name.to_string(),
            description: "a test rule".to_string(),
            applies_to: vec![],
            tags: vec![],
            scope: vec![Scope::Project],
            agents: AgentId::ALL.to_vec(),
        },
        "Body text.".to_string(),
    )
    .unwrap()
}

#[test]
fn push_retries_unpushed_commits_on_clean_working_tree() {
    let remote = init_bare_remote();
    let remote_url = remote.path().to_string_lossy().into_owned();

    let store_dir = tempfile::tempdir().unwrap();
    let store = Store::init(store_dir.path().join("store"), Some(&remote_url)).unwrap();
    set_test_git_identity(store.root());
    store.save_item(&sample_item("first-rule")).unwrap();
    assert!(store.push(false).unwrap().pushed);

    store.save_item(&sample_item("second-rule")).unwrap();
    let status = Command::new("git")
        .args(["add", "-A"])
        .current_dir(store.root())
        .status()
        .unwrap();
    assert!(status.success());
    let status = Command::new("git")
        .args(["commit", "-m", "local-only"])
        .current_dir(store.root())
        .status()
        .unwrap();
    assert!(status.success());

    let status = Command::new("git")
        .args([
            "remote",
            "set-url",
            "origin",
            "/tmp/shaic-no-such-remote-for-test",
        ])
        .current_dir(store.root())
        .status()
        .unwrap();
    assert!(status.success());
    assert!(store.push(false).is_err());

    let status = Command::new("git")
        .args(["remote", "set-url", "origin", &remote_url])
        .current_dir(store.root())
        .status()
        .unwrap();
    assert!(status.success());
    let result = store.push(false).unwrap();
    assert!(result.pushed, "expected retry push of unpushed commits");
    assert!(!result.committed, "working tree was already clean");
}

#[test]
fn push_then_pull_from_a_second_clone_round_trips() {
    let remote = init_bare_remote();
    let remote_url = remote.path().to_string_lossy().into_owned();

    let store1_dir = tempfile::tempdir().unwrap();
    let store1 = Store::init(store1_dir.path().join("store"), Some(&remote_url)).unwrap();
    set_test_git_identity(store1.root());
    store1.save_item(&sample_item("first-rule")).unwrap();
    let push_result = store1.push(false).unwrap();
    assert!(push_result.committed);
    assert!(push_result.pushed);

    let store2_dir = tempfile::tempdir().unwrap();
    let store2 = Store::init(store2_dir.path().join("store"), Some(&remote_url)).unwrap();
    set_test_git_identity(store2.root());
    let items = store2.list_items(ItemKind::Rule).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name(), "first-rule");
}

#[test]
fn divergent_histories_hard_fail_instead_of_merging() {
    let remote = init_bare_remote();
    let remote_url = remote.path().to_string_lossy().into_owned();

    let store1_dir = tempfile::tempdir().unwrap();
    let store1 = Store::init(store1_dir.path().join("store"), Some(&remote_url)).unwrap();
    set_test_git_identity(store1.root());
    store1.save_item(&sample_item("shared-base")).unwrap();
    store1.push(false).unwrap();

    let store2_dir = tempfile::tempdir().unwrap();
    let store2 = Store::init(store2_dir.path().join("store"), Some(&remote_url)).unwrap();
    set_test_git_identity(store2.root());

    // Both clones diverge from the shared base without syncing with each other.
    store1.save_item(&sample_item("from-machine-a")).unwrap();
    store1.push(false).unwrap();

    store2.save_item(&sample_item("from-machine-b")).unwrap();
    let result = store2.push(false);
    assert!(
        matches!(result, Err(shaic_core::Error::Diverged { .. })),
        "expected Diverged, got {result:?}"
    );
}

#[test]
fn pull_refuses_with_uncommitted_local_changes() {
    let remote = init_bare_remote();
    let remote_url = remote.path().to_string_lossy().into_owned();

    let store_dir = tempfile::tempdir().unwrap();
    let store = Store::init(store_dir.path().join("store"), Some(&remote_url)).unwrap();
    store.save_item(&sample_item("uncommitted")).unwrap();

    let result = store.pull(false);
    assert!(
        matches!(result, Err(shaic_core::Error::UncommittedChanges { .. })),
        "expected UncommittedChanges, got {result:?}"
    );
}

#[test]
fn push_and_pull_without_a_remote_fail_with_guidance_not_a_raw_git_error() {
    // Regression: a store `init`ed without `--remote` used to have `push`/
    // `pull` fall through to `git fetch origin`, which fails with a raw,
    // unfriendly message ("fatal: 'origin' does not appear to be a git
    // repository") instead of pointing at the actual fix.
    let store_dir = tempfile::tempdir().unwrap();
    let store = Store::init(store_dir.path().join("store"), None).unwrap();
    store.save_item(&sample_item("local-only")).unwrap();

    let Err(push_err) = store.push(false) else {
        panic!("expected push to fail without a remote configured")
    };
    let push_err = push_err.to_string();
    assert!(
        push_err.contains("shaic init --remote"),
        "push error should point at the fix, got: {push_err:?}"
    );
    assert!(
        !push_err.contains("fatal:"),
        "got raw git error: {push_err:?}"
    );

    let Err(pull_err) = store.pull(false) else {
        panic!("expected pull to fail without a remote configured")
    };
    let pull_err = pull_err.to_string();
    assert!(
        pull_err.contains("shaic init --remote"),
        "pull error should point at the fix, got: {pull_err:?}"
    );
    assert!(
        !pull_err.contains("fatal:"),
        "got raw git error: {pull_err:?}"
    );
}

#[test]
fn pull_scans_incoming_commits_before_fast_forward() {
    let remote = init_bare_remote();
    let remote_url = remote.path().to_string_lossy().into_owned();

    let store1_dir = tempfile::tempdir().unwrap();
    let store1 = Store::init(store1_dir.path().join("store"), Some(&remote_url)).unwrap();
    set_test_git_identity(store1.root());
    store1.save_item(&sample_item("clean-rule")).unwrap();
    store1.push(false).unwrap();

    let store2_dir = tempfile::tempdir().unwrap();
    let store2 = Store::init(store2_dir.path().join("store"), Some(&remote_url)).unwrap();
    set_test_git_identity(store2.root());

    // Bypass shaic's save/push scanners: a credential can still land on the
    // remote via plain git. Pull must refuse it before the FF merge.
    std::fs::write(store1.root().join("leaked.txt"), "AKIAIOSFODNN7EXAMPLE\n").unwrap();
    for args in [
        vec!["add", "leaked.txt"],
        vec!["commit", "-m", "leaked-key"],
        vec!["push", "origin", "HEAD"],
    ] {
        let status = Command::new("git")
            .args(&args)
            .current_dir(store1.root())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    let blocked = store2.pull(false);
    assert!(
        matches!(blocked, Err(shaic_core::Error::SecretDetected(_))),
        "expected SecretDetected, got {blocked:?}"
    );

    let allowed = store2.pull(true).unwrap();
    assert!(allowed.updated);
}
