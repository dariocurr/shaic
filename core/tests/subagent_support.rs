use shaic_core::adapters::Agent;
use shaic_core::adapters::claude_code::ClaudeCode;
use shaic_core::adapters::cursor::Cursor;
use shaic_core::adapters::opencode::OpenCode;
use shaic_core::materialize::{apply, plan_materialize, reconcile_items};
use shaic_core::model::{AgentId, Frontmatter, Item, ItemKind, Scope};
use shaic_core::store::Store;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

/// Tests mutate HOME / XDG_CONFIG_HOME; lock so parallel #[test]s do not race.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn subagent(name: &str, agents: Vec<AgentId>, tools: &[&str], body: &str) -> Item {
    Item::new(
        ItemKind::Subagent,
        Frontmatter {
            name: name.to_string(),
            description: "review code".to_string(),
            applies_to: vec![],
            tags: vec![],
            scope: vec![Scope::Project],
            agents,
            tools: tools.iter().map(|s| (*s).to_string()).collect(),
            native: BTreeMap::new(),
        },
        body.to_string(),
    )
    .unwrap()
}
fn subagent_scoped(
    name: &str,
    agents: Vec<AgentId>,
    scopes: Vec<Scope>,
    tools: &[&str],
    body: &str,
) -> Item {
    Item::new(
        ItemKind::Subagent,
        Frontmatter {
            name: name.to_string(),
            description: "review code".to_string(),
            applies_to: vec![],
            tags: vec![],
            scope: scopes,
            agents,
            tools: tools.iter().map(|s| (*s).to_string()).collect(),
            native: BTreeMap::new(),
        },
        body.to_string(),
    )
    .unwrap()
}

fn with_hooks(mut item: Item) -> Item {
    let mut hooks = serde_yaml_ng::Mapping::new();
    hooks.insert(
        serde_yaml_ng::Value::String("PreToolUse".into()),
        serde_yaml_ng::Value::String("echo check".into()),
    );
    let mut overlay = serde_yaml_ng::Mapping::new();
    overlay.insert(
        serde_yaml_ng::Value::String("hooks".into()),
        serde_yaml_ng::Value::Mapping(hooks),
    );
    overlay.insert(
        serde_yaml_ng::Value::String("isolation".into()),
        serde_yaml_ng::Value::String("worktree".into()),
    );
    item.frontmatter.native.insert(
        AgentId::ClaudeCode.as_str().to_string(),
        serde_yaml_ng::Value::Mapping(overlay),
    );
    item
}

/// Subagent materialize + import behaviours that must hold together:
/// Claude round-trips hooks; Cursor skips when Claude markdown covers the id;
/// OpenCode-only still writes Cursor; OpenCode discovers both `agent/` and
/// `agents/`; same-id import refuses without `--force`.
#[test]
fn subagent_sync_cursor_skip_and_opencode_discover() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let home = tempfile::tempdir().unwrap();
    let xdg = home.path().join(".config");
    std::fs::create_dir_all(&xdg).unwrap();
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var("XDG_CONFIG_HOME", &xdg);
    }
    let project = tempfile::tempdir().unwrap();
    let store = Store::init(home.path().join("store"), None).unwrap();
    let claude = ClaudeCode;
    let cursor = Cursor;
    let opencode = OpenCode;

    // --- Claude + tools + native hooks: Cursor must not write a shadow ---
    let reviewer = with_hooks(subagent(
        "security-auditor",
        ItemKind::SUBAGENT_AGENTS.to_vec(),
        &["Read", "Grep"],
        "Audit for vulns.",
    ));
    store.save_item(&reviewer).unwrap();

    let claude_plan = plan_materialize(&claude, &store, Scope::Project, project.path()).unwrap();
    apply(&claude, &claude_plan, Scope::Project, project.path()).unwrap();
    let claude_path = project.path().join(".claude/agents/security-auditor.md");
    assert!(claude_path.exists());
    let claude_raw = std::fs::read_to_string(&claude_path).unwrap();
    assert!(
        claude_raw.contains("hooks:"),
        "Claude sync must keep hooks overlay: {claude_raw}"
    );
    assert!(claude_raw.contains("isolation: worktree"));
    assert!(claude_raw.contains("tools:"));

    let cursor_plan = plan_materialize(&cursor, &store, Scope::Project, project.path()).unwrap();
    assert!(
        !cursor_plan.writes.iter().any(|w| w
            .relative_path
            .to_string_lossy()
            .contains("security-auditor")),
        "Cursor must skip subagent when Claude markdown covers it: {:?}",
        cursor_plan.writes
    );
    assert!(
        !project
            .path()
            .join(".cursor/agents/security-auditor.md")
            .exists()
    );

    // --- OpenCode-only name: Cursor writes, Claude does not ---
    let oc_only = subagent(
        "opencode-helper",
        vec![AgentId::OpenCode, AgentId::Cursor],
        &["Read", "Bash"],
        "Help in OpenCode.",
    );
    store.save_item(&oc_only).unwrap();

    let oc_plan = plan_materialize(&opencode, &store, Scope::Project, project.path()).unwrap();
    apply(&opencode, &oc_plan, Scope::Project, project.path()).unwrap();
    assert!(
        project
            .path()
            .join(".opencode/agents/opencode-helper.md")
            .exists()
    );

    let cursor_plan = plan_materialize(&cursor, &store, Scope::Project, project.path()).unwrap();
    apply(&cursor, &cursor_plan, Scope::Project, project.path()).unwrap();
    let cursor_path = project.path().join(".cursor/agents/opencode-helper.md");
    assert!(
        cursor_path.exists(),
        "OpenCode-only subagent must still get a Cursor markdown file"
    );
    let cursor_raw = std::fs::read_to_string(&cursor_path).unwrap();
    assert!(
        !cursor_raw.contains("readonly: true"),
        "Bash in tools ⇒ not readonly: {cursor_raw}"
    );

    // --- Discover singular OpenCode `agent/` dir ---
    let singular = project.path().join(".opencode/agent");
    std::fs::create_dir_all(&singular).unwrap();
    std::fs::write(
        singular.join("legacy-agent.md"),
        "---\ndescription: from singular\nmode: subagent\n---\n\nLegacy body.\n",
    )
    .unwrap();
    let discovered =
        opencode.reconcile_existing(ItemKind::Subagent, Scope::Project, project.path());
    assert!(
        discovered.iter().any(|i| i.name() == "legacy-agent"),
        "must discover `.opencode/agent/`: {:?}",
        discovered
            .iter()
            .map(|i| i.name().to_string())
            .collect::<Vec<_>>()
    );

    // --- Same-id import refuses without force (hand-written, untracked) ---
    // `discover_unowned` skips shaic-manifest files, so write a new id that
    // already exists in the store but is not on the Claude manifest.
    store
        .save_item(&subagent(
            "dup-id",
            ItemKind::SUBAGENT_AGENTS.to_vec(),
            &["Read"],
            "Store copy.",
        ))
        .unwrap();
    let hand = project.path().join(".claude/agents/dup-id.md");
    std::fs::create_dir_all(hand.parent().unwrap()).unwrap();
    std::fs::write(
        &hand,
        "---\nname: dup-id\ndescription: hand\nhooks:\n  PreToolUse: echo x\n---\n\nHand body.\n",
    )
    .unwrap();
    let report = reconcile_items(
        &claude,
        &store,
        ItemKind::Subagent,
        Scope::Project,
        project.path(),
        false,
    )
    .unwrap();
    assert!(
        report
            .rejected
            .iter()
            .any(|(n, r)| { n == "dup-id" && r.contains("refuse overwrite") }),
        "expected refuse: {:?}",
        report.rejected
    );

    // --- force replaces; Claude hooks land in native ---
    let report = reconcile_items(
        &claude,
        &store,
        ItemKind::Subagent,
        Scope::Project,
        project.path(),
        true,
    )
    .unwrap();
    assert!(
        report.pulled.contains(&"dup-id".to_string()),
        "force import should pull: pulled={:?} rejected={:?}",
        report.pulled,
        report.rejected
    );
    let loaded = store.load_item(ItemKind::Subagent, "dup-id").unwrap();
    let native_yaml = serde_yaml_ng::to_string(
        loaded
            .frontmatter
            .native
            .get("claude-code")
            .expect("claude-code native overlay"),
    )
    .unwrap();
    assert!(
        native_yaml.contains("hooks"),
        "force re-import must keep hooks in native: {native_yaml}"
    );
    assert_eq!(loaded.body.trim(), "Hand body.");

    // --- When Claude appears for a previously Cursor-only shadow, delete it ---
    let shadow = subagent(
        "soon-claude",
        vec![AgentId::Cursor],
        &["Read"],
        "Cursor first.",
    );
    store.save_item(&shadow).unwrap();
    let plan = plan_materialize(&cursor, &store, Scope::Project, project.path()).unwrap();
    apply(&cursor, &plan, Scope::Project, project.path()).unwrap();
    let shadow_path = project.path().join(".cursor/agents/soon-claude.md");
    assert!(shadow_path.exists());

    let with_claude = subagent(
        "soon-claude",
        ItemKind::SUBAGENT_AGENTS.to_vec(),
        &["Read"],
        "Now Claude too.",
    );
    store.save_item(&with_claude).unwrap();
    let plan = plan_materialize(&cursor, &store, Scope::Project, project.path()).unwrap();
    assert!(
        plan.deletes
            .iter()
            .any(|d| path_ends_with(&d.relative_path, Path::new(".cursor/agents/soon-claude.md"))),
        "Cursor shadow must be queued for delete: {:?}",
        plan.deletes
    );
    apply(&cursor, &plan, Scope::Project, project.path()).unwrap();
    assert!(!shadow_path.exists(), "shadow Cursor file must be gone");
}

fn path_ends_with(path: &Path, suffix: &Path) -> bool {
    path == suffix
        || path
            .components()
            .rev()
            .zip(suffix.components().rev())
            .all(|(a, b)| a == b)
            && path.components().count() >= suffix.components().count()
}

/// Codex TOML alone must not skip Cursor; on-disk Claude.md (Cursor-only agents) must.
#[test]
fn cursor_skip_claude_md_on_disk_not_codex_toml() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HOME", home.path());
    }
    let project = tempfile::tempdir().unwrap();
    let store = Store::init(home.path().join("store"), None).unwrap();
    let cursor = Cursor;

    // Codex-only artifact must not skip Cursor write.
    let codex_only = subagent(
        "codex-twin",
        vec![AgentId::Cursor],
        &["Read", "Grep"],
        "Cursor copy of Codex agent.",
    );
    store.save_item(&codex_only).unwrap();
    std::fs::create_dir_all(project.path().join(".codex/agents")).unwrap();
    std::fs::write(
        project.path().join(".codex/agents/codex-twin.toml"),
        "name = \"codex-twin\"\ndescription = \"x\"\ndeveloper_instructions = \"y\"\n",
    )
    .unwrap();
    let plan = plan_materialize(&cursor, &store, Scope::Project, project.path()).unwrap();
    apply(&cursor, &plan, Scope::Project, project.path()).unwrap();
    let cursor_path = project.path().join(".cursor/agents/codex-twin.md");
    assert!(
        cursor_path.exists(),
        "Codex TOML must not skip Cursor markdown write"
    );
    let raw = std::fs::read_to_string(&cursor_path).unwrap();
    assert!(
        raw.contains("readonly: true"),
        "Read/Grep-only must emit readonly: {raw}"
    );

    // Hand-placed Claude markdown + Cursor-only targeting → skip (disk branch).
    let disk_skip = subagent(
        "hand-claude",
        vec![AgentId::Cursor],
        &["Read"],
        "Would shadow Claude.",
    );
    store.save_item(&disk_skip).unwrap();
    // First write Cursor copy so it is tracked.
    let plan = plan_materialize(&cursor, &store, Scope::Project, project.path()).unwrap();
    apply(&cursor, &plan, Scope::Project, project.path()).unwrap();
    let shadow = project.path().join(".cursor/agents/hand-claude.md");
    assert!(shadow.exists());

    std::fs::create_dir_all(project.path().join(".claude/agents")).unwrap();
    std::fs::write(
        project.path().join(".claude/agents/hand-claude.md"),
        "---\nname: hand-claude\ndescription: richer\n---\n\nClaude body.\n",
    )
    .unwrap();
    let plan = plan_materialize(&cursor, &store, Scope::Project, project.path()).unwrap();
    assert!(
        !plan
            .writes
            .iter()
            .any(|w| w.relative_path.to_string_lossy().contains("hand-claude")),
        "on-disk Claude.md must skip Cursor write: {:?}",
        plan.writes
    );
    assert!(
        plan.deletes
            .iter()
            .any(|d| path_ends_with(&d.relative_path, Path::new(".cursor/agents/hand-claude.md"))),
        "tracked Cursor shadow must delete when Claude.md appears: {:?}",
        plan.deletes
    );
    apply(&cursor, &plan, Scope::Project, project.path()).unwrap();
    assert!(!shadow.exists());
}

/// Force import is last-wins: empty incoming native clears that agent's overlay;
/// disk tools replace store tools (including widening/narrowing).
#[test]
fn force_import_replaces_tools_and_clears_native_when_absent() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HOME", home.path());
    }
    let project = tempfile::tempdir().unwrap();
    let store = Store::init(home.path().join("store"), None).unwrap();
    let claude = ClaudeCode;

    let mut item = subagent(
        "keep-hooks",
        ItemKind::SUBAGENT_AGENTS.to_vec(),
        &["Read"],
        "Store body.",
    );
    let mut overlay = serde_yaml_ng::Mapping::new();
    overlay.insert(
        serde_yaml_ng::Value::String("isolation".into()),
        serde_yaml_ng::Value::String("worktree".into()),
    );
    item.frontmatter.native.insert(
        AgentId::ClaudeCode.as_str().to_string(),
        serde_yaml_ng::Value::Mapping(overlay),
    );
    // Other agent overlay must survive a Claude force-import.
    let mut oc = serde_yaml_ng::Mapping::new();
    oc.insert(
        serde_yaml_ng::Value::String("color".into()),
        serde_yaml_ng::Value::String("blue".into()),
    );
    item.frontmatter.native.insert(
        AgentId::OpenCode.as_str().to_string(),
        serde_yaml_ng::Value::Mapping(oc),
    );
    store.save_item(&item).unwrap();

    let hand = project.path().join(".claude/agents/keep-hooks.md");
    std::fs::create_dir_all(hand.parent().unwrap()).unwrap();
    std::fs::write(
        &hand,
        "---\nname: keep-hooks\ndescription: hand\ntools: Read, Grep\n---\n\nNew body.\n",
    )
    .unwrap();

    let report = reconcile_items(
        &claude,
        &store,
        ItemKind::Subagent,
        Scope::Project,
        project.path(),
        true,
    )
    .unwrap();
    assert!(report.pulled.contains(&"keep-hooks".to_string()));
    let loaded = store.load_item(ItemKind::Subagent, "keep-hooks").unwrap();
    assert_eq!(loaded.body.trim(), "New body.");
    assert_eq!(
        loaded.frontmatter.tools,
        vec!["Read".to_string(), "Grep".to_string()]
    );
    assert!(
        !loaded.frontmatter.native.contains_key("claude-code"),
        "force with no Claude extras must clear native.claude-code: {:?}",
        loaded.frontmatter.native
    );
    assert!(
        loaded.frontmatter.native.contains_key("opencode"),
        "other agents' native must survive: {:?}",
        loaded.frontmatter.native
    );
}

#[test]
fn global_subagent_paths_and_handwritten_cursor_shadow_warn() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let home = tempfile::tempdir().unwrap();
    let xdg = home.path().join(".config");
    std::fs::create_dir_all(&xdg).unwrap();
    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var("XDG_CONFIG_HOME", &xdg);
    }
    let project = tempfile::tempdir().unwrap();
    let store = Store::init(home.path().join("store"), None).unwrap();
    let claude = ClaudeCode;
    let opencode = OpenCode;
    let cursor = Cursor;

    let item = with_hooks(subagent_scoped(
        "global-bot",
        ItemKind::SUBAGENT_AGENTS.to_vec(),
        vec![Scope::Global, Scope::Project],
        &["Read"],
        "Global body.",
    ));
    store.save_item(&item).unwrap();

    let plan = plan_materialize(&claude, &store, Scope::Global, project.path()).unwrap();
    apply(&claude, &plan, Scope::Global, project.path()).unwrap();
    assert!(
        home.path().join(".claude/agents/global-bot.md").exists(),
        "Global Claude agents under ~/.claude/agents/"
    );

    let plan = plan_materialize(&opencode, &store, Scope::Global, project.path()).unwrap();
    apply(&opencode, &plan, Scope::Global, project.path()).unwrap();
    assert!(
        xdg.join("opencode/agents/global-bot.md").exists(),
        "Global OpenCode agents under ~/.config/opencode/agents/"
    );

    // Hand-written Cursor shadow of Claude for a Cursor-only store item.
    let name = "hand-shadow";
    store
        .save_item(&subagent(
            name,
            vec![AgentId::Cursor],
            &["Read"],
            "Cursor only in store.",
        ))
        .unwrap();
    std::fs::create_dir_all(project.path().join(".claude/agents")).unwrap();
    std::fs::write(
        project.path().join(format!(".claude/agents/{name}.md")),
        "---\nname: hand-shadow\ndescription: claude\n---\n\nClaude.\n",
    )
    .unwrap();
    std::fs::create_dir_all(project.path().join(".cursor/agents")).unwrap();
    std::fs::write(
        project.path().join(format!(".cursor/agents/{name}.md")),
        "---\nname: hand-shadow\ndescription: hand\n---\n\nHand.\n",
    )
    .unwrap();
    let plan = plan_materialize(&cursor, &store, Scope::Project, project.path()).unwrap();
    assert!(
        plan.warnings
            .iter()
            .any(|w| w.contains("hand-shadow") && w.contains("shadow")),
        "expected hand-written shadow warning: {:?}",
        plan.warnings
    );
    assert!(
        project
            .path()
            .join(format!(".cursor/agents/{name}.md"))
            .exists(),
        "must not delete untracked Cursor shadow"
    );
}

#[test]
fn codex_toml_subagent_round_trip() {
    use shaic_core::adapters::codex::Codex;

    let _env = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HOME", home.path());
    }
    let project = tempfile::tempdir().unwrap();
    let store = Store::init(home.path().join("store"), None).unwrap();
    let codex = Codex;
    let cursor = Cursor;

    let item = subagent(
        "codex-bot",
        vec![AgentId::Codex, AgentId::Cursor],
        &["Read", "Grep"],
        "Review in Codex.",
    );
    store.save_item(&item).unwrap();

    let plan = plan_materialize(&codex, &store, Scope::Project, project.path()).unwrap();
    apply(&codex, &plan, Scope::Project, project.path()).unwrap();
    let path = project.path().join(".codex/agents/codex-bot.toml");
    assert!(path.exists());
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.contains("sandbox_mode = \"read-only\""), "{raw}");
    assert!(raw.contains("developer_instructions"), "{raw}");

    // Codex TOML must not skip Cursor markdown.
    let plan = plan_materialize(&cursor, &store, Scope::Project, project.path()).unwrap();
    apply(&cursor, &plan, Scope::Project, project.path()).unwrap();
    assert!(project.path().join(".cursor/agents/codex-bot.md").exists());
}
