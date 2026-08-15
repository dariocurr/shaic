use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use shaic_core::adapters::codex::Codex;
use shaic_core::materialize::{WriteAction, apply_mcp, plan_mcp, reconcile_mcp};
use shaic_core::mcp::{EnvValue, McpServer};
use shaic_core::model::{AgentId, Scope};
use shaic_core::store::Store;

/// Both tests mutate `HOME` — must not run in parallel with each other or
/// with any other test in this binary that does the same.
fn home_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn github_http_server() -> McpServer {
    McpServer {
        name: "github".to_string(),
        command: String::new(),
        args: vec![],
        env: BTreeMap::new(),
        url: Some("https://api.githubcopilot.com/mcp/".to_string()),
        bearer_token_env_var: Some(EnvValue::Literal("GITHUB_PAT".to_string())),
        scope: vec![Scope::Global],
        agents: vec![AgentId::Codex],
    }
}

#[test]
fn dual_transport_reconcile_preserves_stdio_when_codex_pulls_http() {
    let _guard = home_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HOME", home.path());
    }

    let store = Store::init(home.path().join("store"), None).unwrap();
    let mut dual = github_http_server();
    dual.command = "npx".to_string();
    dual.args = vec![
        "-y".to_string(),
        "@modelcontextprotocol/server-github".to_string(),
    ];
    dual.agents = vec![AgentId::Codex, AgentId::Cursor];
    store.save_mcp_server(&dual).unwrap();

    let codex_config = home.path().join(".codex").join("config.toml");
    std::fs::create_dir_all(codex_config.parent().unwrap()).unwrap();
    std::fs::write(
        &codex_config,
        r#"
[mcp_servers.github]
url = "https://api.githubcopilot.com/mcp/"
bearer_token_env_var = "GITHUB_PAT"
"#,
    )
    .unwrap();

    let report = reconcile_mcp(&Codex, &store, Scope::Global, home.path()).unwrap();
    assert!(report.rejected.is_empty(), "{:?}", report.rejected);

    let loaded = store.load_mcp_server("github").unwrap();
    assert_eq!(
        loaded.command, "npx",
        "stdio must survive Codex HTTP reconcile"
    );
    assert!(loaded.has_http());
    assert_eq!(loaded.agents, vec![AgentId::Codex, AgentId::Cursor]);
}

#[test]
fn codex_github_mcp_sync_writes_bearer_env_var_name_not_token() {
    let _guard = home_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HOME", home.path());
    }

    let store = Store::init(home.path().join("store"), None).unwrap();
    store.save_mcp_server(&github_http_server()).unwrap();

    let codex_config = home.path().join(".codex").join("config.toml");
    std::fs::create_dir_all(codex_config.parent().unwrap()).unwrap();
    std::fs::write(&codex_config, "model = \"gpt-5\"\n").unwrap();

    let agent = Codex;
    let plan = plan_mcp(&agent, &store, Scope::Global, home.path()).unwrap();
    assert_eq!(plan.writes.len(), 1);
    assert_eq!(plan.writes[0].action, WriteAction::Create);
    apply_mcp(&agent, &store, &plan, Scope::Global, home.path()).unwrap();

    let raw = std::fs::read_to_string(&codex_config).unwrap();
    assert!(raw.contains("[mcp_servers.github]"));
    assert!(raw.contains("https://api.githubcopilot.com/mcp/"));
    assert!(raw.contains("bearer_token_env_var"));
    assert!(raw.contains("GITHUB_PAT"));
    assert!(
        !raw.contains("ghp_"),
        "token value must never land in config.toml"
    );
    assert!(raw.contains("model = \"gpt-5\""));
}

#[test]
fn codex_github_mcp_reconcile_pulls_http_server_into_store() {
    let _guard = home_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HOME", home.path());
    }

    let store = Store::init(home.path().join("store"), None).unwrap();
    let codex_config = home.path().join(".codex").join("config.toml");
    std::fs::create_dir_all(codex_config.parent().unwrap()).unwrap();
    std::fs::write(
        &codex_config,
        r#"
[mcp_servers.github]
url = "https://api.githubcopilot.com/mcp/"
bearer_token_env_var = "GITHUB_PAT"
"#,
    )
    .unwrap();

    let agent = Codex;
    let report = reconcile_mcp(&agent, &store, Scope::Global, home.path()).unwrap();
    assert!(
        report.rejected.is_empty(),
        "unexpected rejects: {:?}",
        report.rejected
    );
    assert_eq!(report.pulled, vec!["github".to_string()]);

    let server = store.load_mcp_server("github").unwrap();
    assert!(server.has_http());
    match server.bearer_token_env_var {
        Some(EnvValue::Secret { secret }) => assert_eq!(secret, "GITHUB_PAT"),
        other => panic!("expected secret ref, got {other:?}"),
    }
}
