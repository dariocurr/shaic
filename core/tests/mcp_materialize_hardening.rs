use std::collections::BTreeMap;

use shaic_core::adapters::claude_code::ClaudeCode;
use shaic_core::adapters::cursor::Cursor;
use shaic_core::materialize::{McpPlan, apply_mcp, plan_mcp};
use shaic_core::mcp::{EnvValue, McpServer};
use shaic_core::model::Scope;
use shaic_core::store::Store;

fn git_init(dir: &std::path::Path) {
    let status = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success());
}

fn server(name: &str) -> McpServer {
    McpServer::new(
        name.to_string(),
        "npx".to_string(),
        vec!["-y".to_string()],
        BTreeMap::from([(
            "LOG_LEVEL".to_string(),
            EnvValue::Literal("debug".to_string()),
        )]),
        vec![Scope::Project],
    )
    .unwrap()
}

/// Regression coverage for bugs a multi-reviewer pass on the MCP feature
/// found before it ever shipped:
/// - the very first sync for an agent whose config directory doesn't exist
///   yet (Cursor's `.cursor/`, Copilot's `.vscode/`, Windsurf's
///   `~/.codeium/...`) used to hard-fail, because `write_atomic` canonicalized
///   `root` before creating it;
/// - that same `root = target.parent()` shortcut made the ancestor-symlink
///   check a no-op, so a symlinked config directory escaped undetected;
/// - a managed key holding something other than a JSON object (array,
///   string, ...) used to be silently treated as empty and clobbered;
/// - re-applying an already-in-sync plan used to rewrite/reformat the file
///   anyway;
/// - the `.gitignore` write that protects a secret-bearing project config
///   used to bypass `path_guard` entirely, so a symlinked `.gitignore`
///   escaped the project root;
/// - a store file that failed to parse used to drop out of every live-server
///   check, so a typo in one server's TOML could get it deleted from every
///   agent's config on the next sync.
///
/// Sets `HOME` for the process (`Store::state_dir()` lives under it) — must
/// stay the only test in this binary to avoid racing that env var.
#[test]
fn mcp_hardening_missing_dir_symlink_escape_bad_shape_and_noop() {
    let home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HOME", home.path());
    }
    let store = Store::init(home.path().join("store"), None).unwrap();

    // --- First sync into a config dir that doesn't exist yet must succeed,
    // and the resulting file must be mode 0600 since it can hold a secret. ---
    {
        let project = tempfile::tempdir().unwrap();
        let agent = Cursor;
        store.save_mcp_server(&server("fresh")).unwrap();
        let plan = plan_mcp(&agent, &store, Scope::Project, project.path()).unwrap();
        apply_mcp(&agent, &store, &plan, Scope::Project, project.path()).unwrap();

        let mcp_json = project.path().join(".cursor").join("mcp.json");
        assert!(mcp_json.exists(), "should have created .cursor/mcp.json");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&mcp_json).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o600,
                "MCP config file must not be world/group readable"
            );
        }
        store.remove_mcp_server("fresh").unwrap();
    }

    // --- A symlinked config directory must be rejected, not written through. ---
    #[cfg(unix)]
    {
        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), project.path().join(".cursor")).unwrap();
        let agent = Cursor;
        store.save_mcp_server(&server("escapee")).unwrap();
        let plan = plan_mcp(&agent, &store, Scope::Project, project.path()).unwrap();
        let result = apply_mcp(&agent, &store, &plan, Scope::Project, project.path());
        assert!(
            result.is_err(),
            "must not write through a symlinked config dir"
        );
        assert!(
            !outside.path().join("mcp.json").exists(),
            "must not have written into the symlink target"
        );
        store.remove_mcp_server("escapee").unwrap();
    }

    // --- A non-object value at the managed key must be refused, not
    // silently replaced with an empty object. ---
    {
        let project = tempfile::tempdir().unwrap();
        let mcp_json = project.path().join(".mcp.json");
        std::fs::write(&mcp_json, r#"{"mcpServers": ["not", "an", "object"]}"#).unwrap();
        let agent = ClaudeCode;
        store.save_mcp_server(&server("shapemismatch")).unwrap();
        let result = plan_mcp(&agent, &store, Scope::Project, project.path());
        assert!(result.is_err(), "must refuse a non-object mcpServers value");
        let raw = std::fs::read_to_string(&mcp_json).unwrap();
        assert!(raw.contains("not"), "original array must survive untouched");
        store.remove_mcp_server("shapemismatch").unwrap();
    }

    // --- Applying an already-in-sync (empty) plan must not touch the file
    // at all, not even to reformat it. ---
    {
        let project = tempfile::tempdir().unwrap();
        let mcp_json = project.path().join(".mcp.json");
        std::fs::write(&mcp_json, r#"{"mcpServers":{},"other":1}"#).unwrap();
        let before = std::fs::read_to_string(&mcp_json).unwrap();
        let agent = ClaudeCode;
        let plan = plan_mcp(&agent, &store, Scope::Project, project.path()).unwrap();
        assert!(plan.is_empty());
        apply_mcp(&agent, &store, &plan, Scope::Project, project.path()).unwrap();
        let after = std::fs::read_to_string(&mcp_json).unwrap();
        assert_eq!(before, after, "an empty plan must not rewrite the file");
    }

    // --- A store server referencing a secret must get its project-scope
    // target file `.gitignore`d before a resolved value could ever land in
    // the (otherwise git-tracked) MCP config — checked via `apply_mcp`
    // directly with a manually-empty plan, so this never calls `resolve_env`
    // and never touches the OS keychain (this environment's overridden
    // `HOME` has no real keychain to resolve against — see the other
    // integration test's note on the same constraint).
    {
        let project = tempfile::tempdir().unwrap();
        git_init(project.path());
        let secret_server = McpServer::new(
            "has-secret".to_string(),
            "npx".to_string(),
            vec![],
            BTreeMap::from([(
                "TOKEN".to_string(),
                EnvValue::Secret {
                    secret: "shaic-test-secret-never-set-9f2c".to_string(),
                },
            )]),
            vec![Scope::Project],
        )
        .unwrap();
        store.save_mcp_server(&secret_server).unwrap();
        let agent = Cursor;
        let empty_plan = McpPlan::default();

        let applied = apply_mcp(&agent, &store, &empty_plan, Scope::Project, project.path())
            .expect("an empty plan must not touch the keychain, only .gitignore");
        assert_eq!(applied.applied, 0);

        let gitignore = std::fs::read_to_string(project.path().join(".gitignore")).unwrap();
        assert!(
            gitignore.lines().any(|l| l.trim() == ".cursor/mcp.json"),
            "project-scope MCP file holding a secret must be gitignored: {gitignore:?}"
        );
        store.remove_mcp_server("has-secret").unwrap();
    }

    // --- A symlinked `.gitignore` must not let the secret-protection write
    // escape the project root, and must not silently proceed as if the file
    // had been protected. ---
    #[cfg(unix)]
    {
        let project = tempfile::tempdir().unwrap();
        git_init(project.path());
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("victim.conf");
        std::fs::write(&victim, "victim content\n").unwrap();
        std::os::unix::fs::symlink(&victim, project.path().join(".gitignore")).unwrap();

        let secret_server = McpServer::new(
            "leaky".to_string(),
            "npx".to_string(),
            vec![],
            BTreeMap::from([(
                "TOKEN".to_string(),
                EnvValue::Secret {
                    secret: "shaic-test-secret-never-set-leaky".to_string(),
                },
            )]),
            vec![Scope::Project],
        )
        .unwrap();
        store.save_mcp_server(&secret_server).unwrap();
        let agent = Cursor;
        let empty_plan = McpPlan::default();

        let result = apply_mcp(&agent, &store, &empty_plan, Scope::Project, project.path());
        assert!(
            result.is_err(),
            "must not write through a symlinked .gitignore"
        );
        let victim_content = std::fs::read_to_string(&victim).unwrap();
        assert_eq!(
            victim_content, "victim content\n",
            "must not have written into the symlink target"
        );
        store.remove_mcp_server("leaky").unwrap();
    }

    // --- A store file that fails to parse must not be treated as "left the
    // store" — it must stay out of the removal set so it isn't deleted from
    // an agent's config just because of a typo on disk. ---
    {
        let project = tempfile::tempdir().unwrap();
        let agent = ClaudeCode;
        store.save_mcp_server(&server("stable")).unwrap();
        let plan = plan_mcp(&agent, &store, Scope::Project, project.path()).unwrap();
        apply_mcp(&agent, &store, &plan, Scope::Project, project.path()).unwrap();

        let mcp_json = project.path().join(".mcp.json");
        let before = std::fs::read_to_string(&mcp_json).unwrap();
        assert!(before.contains("stable"), "should have synced 'stable'");

        // Corrupt the store file directly (bypassing `save_mcp_server`,
        // which would reject invalid TOML) to simulate a hand-edit typo.
        std::fs::write(
            store.root().join("mcp").join("stable.toml"),
            "not valid toml {{{",
        )
        .unwrap();

        let plan = plan_mcp(&agent, &store, Scope::Project, project.path()).unwrap();
        assert!(
            !plan.removals.contains(&"stable".to_string()),
            "a server that merely failed to parse must not be queued for removal"
        );
        assert!(
            plan.skipped.iter().any(|s| s.contains("stable")),
            "the parse failure must be reported: {:?}",
            plan.skipped
        );
        apply_mcp(&agent, &store, &plan, Scope::Project, project.path()).unwrap();
        let after = std::fs::read_to_string(&mcp_json).unwrap();
        assert!(
            after.contains("stable"),
            "must not have removed 'stable' from the config"
        );
        std::fs::remove_file(store.root().join("mcp").join("stable.toml")).unwrap();
    }

    // --- Claude Code's Global-scope MCP target is `~/.claude.json`, a file
    // this crate does not own — it also holds this machine's Claude Code
    // auth/session/project state. A Global MCP sync must merge only the
    // `mcpServers` key and leave every other top-level key byte-identical. ---
    {
        let claude_json = home.path().join(".claude.json");
        std::fs::write(
            &claude_json,
            r#"{"oauthAccount":"user@example.com","projects":{"/some/path":{"allowedTools":["Bash"]}},"mcpServers":{"old-tool":{"command":"old"}}}"#,
        )
        .unwrap();

        let agent = ClaudeCode;
        let global_server = McpServer::new(
            "global-tool".to_string(),
            "npx".to_string(),
            vec!["-y".to_string()],
            BTreeMap::new(),
            vec![Scope::Global],
        )
        .unwrap();
        store.save_mcp_server(&global_server).unwrap();
        let plan = plan_mcp(&agent, &store, Scope::Global, home.path()).unwrap();
        apply_mcp(&agent, &store, &plan, Scope::Global, home.path()).unwrap();

        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&claude_json).unwrap()).unwrap();
        assert_eq!(
            after["oauthAccount"], "user@example.com",
            "unrelated auth state must survive a Global MCP sync untouched"
        );
        assert_eq!(
            after["projects"]["/some/path"]["allowedTools"][0], "Bash",
            "unrelated project state must survive a Global MCP sync untouched"
        );
        assert!(
            after["mcpServers"].get("global-tool").is_some(),
            "the new server must be merged in: {after:?}"
        );
        assert!(
            after["mcpServers"].get("old-tool").is_some(),
            "a server shaic never wrote (not provenance-tracked) must be left alone, \
             not silently deleted just because it isn't in the store: {after:?}"
        );
        store.remove_mcp_server("global-tool").unwrap();
    }
}
