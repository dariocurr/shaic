use shaic_core::adapters::claude_code::ClaudeCode;
use shaic_core::adapters::cursor::Cursor;
use shaic_core::materialize::{apply_mcp, plan_mcp, reconcile_mcp};
use shaic_core::mcp::McpServer;
use shaic_core::model::Scope;
use shaic_core::store::Store;

/// Sets `HOME` for the process (the per-server provenance manifest lives
/// under `Store::state_dir()`, resolved from the real `HOME` otherwise,
/// which would mean racing every other test in this binary — and the
/// developer's actual `~/.shaic/state/` — over the same file) — must stay
/// the only test in this binary to avoid that race.
#[test]
fn bidirectional_mcp_sync() {
    let home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HOME", home.path());
    }

    // --- The actual feature request: add/edit an MCP server directly in
    // one agent's own config (no `shaic mcp add`/`edit` involved), and have
    // it show up in every other agent the next time `sync` runs — not just
    // stay invisible to the store, and not get clobbered by the store's
    // stale copy of that agent's file. ---
    {
        let store_dir = tempfile::tempdir().unwrap();
        let store = Store::init(store_dir.path().join("store"), None).unwrap();
        let project = tempfile::tempdir().unwrap();

        // Simulate a user hand-adding an MCP server directly in Cursor's own
        // config, bypassing `shaic mcp add` entirely.
        let cursor_mcp = project.path().join(".cursor").join("mcp.json");
        std::fs::create_dir_all(cursor_mcp.parent().unwrap()).unwrap();
        std::fs::write(
            &cursor_mcp,
            r#"{"mcpServers":{"shared-tool":{"command":"npx","args":["-y","shared-tool"]}}}"#,
        )
        .unwrap();

        // A `sync`-style apply for Cursor pulls "shared-tool" into the store...
        let cursor = Cursor;
        let report = reconcile_mcp(&cursor, &store, Scope::Project, project.path()).unwrap();
        assert_eq!(report.pulled, vec!["shared-tool".to_string()]);
        assert!(
            report.rejected.is_empty(),
            "unexpected rejects: {:?}",
            report.rejected
        );
        let (servers, _) = store.list_mcp_servers().unwrap();
        assert!(servers.iter().any(|s| s.name == "shared-tool"));

        // ...and from there, a normal materialize for a completely different
        // agent (Claude Code) picks it up and writes it out, with no manual
        // `shaic mcp add` step in between.
        let claude = ClaudeCode;
        let plan = plan_mcp(&claude, &store, Scope::Project, project.path()).unwrap();
        assert!(
            plan.changed_writes().any(|w| w.name == "shared-tool"),
            "expected Claude Code's plan to include the server pulled from Cursor: {:?}",
            plan.writes
        );
        apply_mcp(&claude, &store, &plan, Scope::Project, project.path()).unwrap();
        let claude_mcp = std::fs::read_to_string(project.path().join(".mcp.json")).unwrap();
        assert!(
            claude_mcp.contains("shared-tool"),
            "Claude Code's .mcp.json should now include the server hand-added in Cursor: {claude_mcp}"
        );
    }

    // --- Reconciling must not touch an entry that's already identical to
    // what the store already has — otherwise every `sync` would report
    // "pulled" for every server on every run, even when nothing changed. ---
    {
        let store_dir = tempfile::tempdir().unwrap();
        let store = Store::init(store_dir.path().join("store"), None).unwrap();
        let project = tempfile::tempdir().unwrap();

        let claude = ClaudeCode;
        let server = McpServer::new(
            "stable".to_string(),
            "npx".to_string(),
            vec![],
            std::collections::BTreeMap::new(),
            vec![Scope::Project],
        )
        .unwrap();
        store.save_mcp_server(&server).unwrap();
        let plan = plan_mcp(&claude, &store, Scope::Project, project.path()).unwrap();
        apply_mcp(&claude, &store, &plan, Scope::Project, project.path()).unwrap();

        let report = reconcile_mcp(&claude, &store, Scope::Project, project.path()).unwrap();
        assert!(
            report.pulled.is_empty(),
            "reconciling an already-in-sync agent should pull nothing: {:?}",
            report.pulled
        );
    }
}
