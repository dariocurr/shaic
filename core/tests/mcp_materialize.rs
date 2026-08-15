use std::collections::BTreeMap;

use shaic_core::adapters::claude_code::ClaudeCode;
use shaic_core::materialize::{WriteAction, apply_mcp, plan_mcp};
use shaic_core::mcp::{EnvValue, McpServer};
use shaic_core::model::Scope;
use shaic_core::store::Store;

fn server(name: &str, command: &str) -> McpServer {
    McpServer::new(
        name.to_string(),
        command.to_string(),
        vec!["-y".to_string()],
        BTreeMap::from([(
            "LOG_LEVEL".to_string(),
            EnvValue::Literal("debug".to_string()),
        )]),
        vec![Scope::Project],
    )
    .unwrap()
}

/// End-to-end `plan_mcp`/`apply_mcp` against a real adapter (Claude Code,
/// project scope, `.mcp.json`): create/update/noop classification, delete
/// safety once an item leaves the store, hand-edit protection, and — the
/// whole point of the JSON-merge design — that unrelated keys already in the
/// target file survive untouched. Only literal env values are used so this
/// test never touches the OS keychain. Sets `HOME` for the process since the
/// provenance manifest lives under `Store::state_dir()`; this must stay the
/// only test in this binary to avoid racing that env var across threads.
#[test]
fn mcp_sync_survives_orchestration_and_protects_hand_edits_and_unrelated_keys() {
    let home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HOME", home.path());
    }
    let project = tempfile::tempdir().unwrap();
    let store = Store::init(home.path().join("store"), None).unwrap();
    let agent = ClaudeCode;
    let mcp_json = project.path().join(".mcp.json");

    // A hand-written file with an unrelated top-level key, as if the user
    // already had other MCP servers configured by hand.
    std::fs::write(
        &mcp_json,
        r#"{"mcpServers": {"hand-written": {"command": "manual"}}}"#,
    )
    .unwrap();

    // --- Create ---
    store
        .save_mcp_server(&server("temp-server", "npx"))
        .unwrap();
    let plan = plan_mcp(&agent, &store, Scope::Project, project.path()).unwrap();
    assert_eq!(plan.writes.len(), 1);
    assert_eq!(plan.writes[0].action, WriteAction::Create);
    apply_mcp(&agent, &store, &plan, Scope::Project, project.path()).unwrap();

    let raw = std::fs::read_to_string(&mcp_json).unwrap();
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(value["mcpServers"]["temp-server"]["command"], "npx");
    assert_eq!(
        value["mcpServers"]["temp-server"]["env"]["LOG_LEVEL"],
        "debug"
    );
    // The hand-written entry must have survived the merge untouched.
    assert_eq!(value["mcpServers"]["hand-written"]["command"], "manual");

    // --- NoOp on an unchanged re-plan ---
    let plan2 = plan_mcp(&agent, &store, Scope::Project, project.path()).unwrap();
    assert_eq!(plan2.writes[0].action, WriteAction::NoOp);
    assert!(plan2.is_empty());

    // --- Update once the store definition changes ---
    store
        .save_mcp_server(&server("temp-server", "npx-updated"))
        .unwrap();
    let plan3 = plan_mcp(&agent, &store, Scope::Project, project.path()).unwrap();
    assert_eq!(plan3.writes[0].action, WriteAction::Update);
    apply_mcp(&agent, &store, &plan3, Scope::Project, project.path()).unwrap();
    let raw = std::fs::read_to_string(&mcp_json).unwrap();
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(value["mcpServers"]["temp-server"]["command"], "npx-updated");

    // --- Untouched, shaic-written entry is safe to delete once the item
    // leaves the store ---
    store.remove_mcp_server("temp-server").unwrap();
    let plan4 = plan_mcp(&agent, &store, Scope::Project, project.path()).unwrap();
    assert_eq!(plan4.removals, vec!["temp-server".to_string()]);
    apply_mcp(&agent, &store, &plan4, Scope::Project, project.path()).unwrap();
    let raw = std::fs::read_to_string(&mcp_json).unwrap();
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(value["mcpServers"].get("temp-server").is_none());
    assert_eq!(value["mcpServers"]["hand-written"]["command"], "manual");

    // --- Hand-edited entry at the same name must survive removal from the
    // store — the entire reason the manifest exists. ---
    store
        .save_mcp_server(&server("kept-server", "npx"))
        .unwrap();
    let plan5 = plan_mcp(&agent, &store, Scope::Project, project.path()).unwrap();
    apply_mcp(&agent, &store, &plan5, Scope::Project, project.path()).unwrap();

    let raw = std::fs::read_to_string(&mcp_json).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    value["mcpServers"]["kept-server"] = serde_json::json!({"command": "hand-edited"});
    std::fs::write(&mcp_json, serde_json::to_string_pretty(&value).unwrap()).unwrap();

    store.remove_mcp_server("kept-server").unwrap();
    let plan6 = plan_mcp(&agent, &store, Scope::Project, project.path()).unwrap();
    assert!(
        plan6.removals.is_empty(),
        "hand-edited MCP entry must not be proposed for removal"
    );
    apply_mcp(&agent, &store, &plan6, Scope::Project, project.path()).unwrap();
    let raw = std::fs::read_to_string(&mcp_json).unwrap();
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        value["mcpServers"]["kept-server"]["command"], "hand-edited",
        "hand-edited entry must survive apply()"
    );
}
