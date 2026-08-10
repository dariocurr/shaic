pub mod git;
pub mod layout;
mod mcp;

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::{Item, ItemKind};
use crate::security::{frontmatter_limits, secret_scan};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct StoreMeta {
    pub schema_version: u32,
    pub created_at_unix: u64,
}

pub struct PushResult {
    pub committed: bool,
    pub summary: Option<String>,
}

pub struct PullResult {
    pub updated: bool,
    pub diff_stat: Option<String>,
}

pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn default_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".shaic")
            .join("store")
    }

    /// Per-machine materialization state (provenance manifests). Lives outside
    /// the git-tracked store — never committed, pushed, or pulled.
    pub fn state_dir() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".shaic")
            .join("state")
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        if !root.join(".git").exists() {
            return Err(Error::StoreNotInitialized);
        }
        Ok(Store { root })
    }

    /// Create a fresh store (clone if `remote` given, else `git init`), or, if
    /// one already exists at `root`, just update its remote when `remote` is
    /// given — `shaic init` is safe to re-run.
    pub fn init(root: impl Into<PathBuf>, remote: Option<&str>) -> Result<Self> {
        let root = root.into();
        if let Some(url) = remote {
            git::validate_remote_url(url)?;
        }

        if root.join(".git").exists() {
            if let Some(url) = remote {
                git::set_remote(&root, url)?;
            }
            return Ok(Store { root });
        }

        match remote {
            Some(url) => git::clone(url, &root)?,
            None => {
                std::fs::create_dir_all(&root).map_err(|source| Error::Io {
                    path: root.clone(),
                    source,
                })?;
                git::init(&root)?;
            }
        }

        let meta_path = root.join(".shaic-store.toml");
        if !meta_path.exists() {
            let meta = StoreMeta {
                schema_version: SCHEMA_VERSION,
                created_at_unix: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            };
            let toml = toml::to_string_pretty(&meta).map_err(|e| Error::Config(e.to_string()))?;
            std::fs::write(&meta_path, toml).map_err(|source| Error::Io {
                path: meta_path,
                source,
            })?;
        }

        Ok(Store { root })
    }

    pub fn check_schema(&self) -> Result<()> {
        let meta_path = self.root.join(".shaic-store.toml");
        if !meta_path.exists() {
            return Ok(());
        }
        let raw = std::fs::read_to_string(&meta_path).map_err(|source| Error::Io {
            path: meta_path.clone(),
            source,
        })?;
        let meta: StoreMeta = toml::from_str(&raw).map_err(|e| Error::Config(e.to_string()))?;
        if meta.schema_version > SCHEMA_VERSION {
            return Err(Error::SchemaTooNew {
                found: meta.schema_version,
                supported: SCHEMA_VERSION,
            });
        }
        Ok(())
    }

    pub fn save_item(&self, item: &Item) -> Result<()> {
        self.check_schema()?;
        let path = layout::item_path(&self.root, item.kind, item.name());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| Error::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let fm_yaml = serde_yaml_ng::to_string(&item.frontmatter)
            .map_err(|e| Error::FrontmatterParse(e.to_string()))?;
        frontmatter_limits::validate_raw(&fm_yaml)?;
        let contents = render_with_frontmatter(&fm_yaml, &item.body);
        std::fs::write(&path, contents).map_err(|source| Error::Io { path, source })
    }

    pub fn load_item(&self, kind: ItemKind, name: &str) -> Result<Item> {
        self.check_schema()?;
        crate::model::validate_name(name)?;
        let path = layout::item_path(&self.root, kind, name);
        let raw = std::fs::read_to_string(&path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        let (fm_raw, body) = split_frontmatter(&raw)?;
        let frontmatter = frontmatter_limits::parse_lenient(fm_raw)?;
        Item::new(kind, frontmatter, body.to_string())
    }

    pub fn remove_item(&self, kind: ItemKind, name: &str) -> Result<()> {
        crate::model::validate_name(name)?;
        let path = layout::item_path(&self.root, kind, name);
        match kind {
            ItemKind::Skill => {
                if let Some(dir) = path.parent() {
                    std::fs::remove_dir_all(dir).map_err(|source| Error::Io {
                        path: dir.to_path_buf(),
                        source,
                    })?;
                }
            }
            _ => {
                std::fs::remove_file(&path).map_err(|source| Error::Io { path, source })?;
            }
        }
        Ok(())
    }

    pub fn list_items(&self, kind: ItemKind) -> Result<Vec<Item>> {
        self.check_schema()?;
        let dir = self.root.join(layout::kind_dir(kind));
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut items = Vec::new();
        for entry in walkdir::WalkDir::new(&dir).max_depth(2).follow_links(false) {
            let entry = entry.map_err(|e| Error::Git(e.to_string()))?;
            let is_item_file = match kind {
                ItemKind::Skill => entry.file_name() == "SKILL.md",
                _ => entry.path().extension().is_some_and(|e| e == "md"),
            };
            if !entry.file_type().is_file() || !is_item_file {
                continue;
            }
            // A skill's name is its containing directory (`skills/<name>/SKILL.md`);
            // every other kind's is the file stem.
            let name = match kind {
                ItemKind::Skill => entry.path().parent().and_then(|dir| dir.file_name()),
                _ => entry.path().file_stem(),
            }
            .and_then(|n| n.to_str())
            .unwrap_or_default();
            if crate::model::validate_name(name).is_err() {
                continue;
            }
            items.push(self.load_item(kind, name)?);
        }
        Ok(items)
    }

    pub fn push(&self, allow_secrets: bool) -> Result<PushResult> {
        if !git::has_remote(&self.root) {
            return Err(Error::Config(
                "no remote configured for the store yet — run `shaic init --remote <url>` first"
                    .to_string(),
            ));
        }
        let status = git::status_porcelain(&self.root)?;
        if status.trim().is_empty() {
            return Ok(PushResult {
                committed: false,
                summary: None,
            });
        }
        git::add_all(&self.root)?;
        let staged = git::diff_cached(&self.root)?;
        secret_scan::scan_or_reject(&staged, allow_secrets)?;
        let stat = git::diff_cached_stat(&self.root)?;
        let summary = summarize_stat(&stat);
        git::commit(&self.root, &summary)?;
        let branch = git::current_branch(&self.root)?;
        git::fetch(&self.root)?;
        if git::remote_branch_exists(&self.root, &branch)? {
            git::merge_ff_only(&self.root, &branch)?;
        }
        git::push(&self.root, &branch)?;
        Ok(PushResult {
            committed: true,
            summary: Some(summary),
        })
    }

    pub fn pull(&self) -> Result<PullResult> {
        if !git::has_remote(&self.root) {
            return Err(Error::Config(
                "no remote configured for the store yet — run `shaic init --remote <url>` first"
                    .to_string(),
            ));
        }
        let status = git::status_porcelain(&self.root)?;
        if !status.trim().is_empty() {
            return Err(Error::UncommittedChanges {
                store: self.root.clone(),
            });
        }
        let branch = git::current_branch(&self.root)?;
        let before = git::rev_parse(&self.root, "HEAD").unwrap_or_default();
        git::fetch(&self.root)?;
        if git::remote_branch_exists(&self.root, &branch)? {
            git::merge_ff_only(&self.root, &branch)?;
        }
        let after = git::rev_parse(&self.root, "HEAD").unwrap_or_default();
        if before == after {
            return Ok(PullResult {
                updated: false,
                diff_stat: None,
            });
        }
        let stat = git::diff_stat(&self.root, &before, &after).ok();
        Ok(PullResult {
            updated: true,
            diff_stat: stat,
        })
    }
}

fn summarize_stat(stat: &str) -> String {
    let files_changed = stat.lines().filter(|l| l.contains('|')).count();
    if files_changed == 0 {
        "shaic: update store".to_string()
    } else {
        format!("shaic: update {files_changed} item(s)")
    }
}

/// Parse a full item file (frontmatter + body) as authored by a human via
/// `$EDITOR` — strict, since this is the "write" path (a typo in a field name
/// should fail loudly here rather than being silently dropped).
pub fn parse_item(kind: ItemKind, raw: &str) -> Result<Item> {
    let (fm_raw, body) = split_frontmatter(raw)?;
    let frontmatter = frontmatter_limits::parse_strict(fm_raw)?;
    Item::new(kind, frontmatter, body.to_string())
}

/// Starter template opened in `$EDITOR` for `shaic item add`.
pub fn item_template(name: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: \napplies_to: []\ntags: []\nscope: [global, project]\n# agents: [claude-code]  # omit for every agent; restrict an agent-specific item\n---\n\nDescribe it here.\n"
    )
}

/// Render an existing item back into the same frontmatter+body text opened in
/// `$EDITOR` for `shaic item edit` — shared by the CLI and TUI so there's one
/// "how do we round-trip an item through a human editor" implementation.
pub fn render_for_edit(item: &Item) -> String {
    let fm_yaml = serde_yaml_ng::to_string(&item.frontmatter).unwrap_or_default();
    render_with_frontmatter(&fm_yaml, &item.body)
}

fn render_with_frontmatter(fm_yaml: &str, body: &str) -> String {
    format!("---\n{}\n---\n{}", fm_yaml.trim_end(), body)
}

fn split_frontmatter(content: &str) -> Result<(&str, &str)> {
    let rest = content.strip_prefix("---\n").ok_or_else(|| {
        Error::FrontmatterParse("expected content to start with '---'".to_string())
    })?;
    let marker = "\n---\n";
    let idx = rest
        .find(marker)
        .ok_or_else(|| Error::FrontmatterParse("unterminated frontmatter block".to_string()))?;
    let (fm, after) = rest.split_at(idx);
    Ok((fm, &after[marker.len()..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Frontmatter;

    fn sample_item(name: &str) -> Item {
        Item::new(
            ItemKind::Rule,
            Frontmatter {
                name: name.to_string(),
                description: "a test rule".to_string(),
                applies_to: vec![],
                tags: vec![],
                scope: vec![crate::model::Scope::Project],
                agents: crate::model::AgentId::ALL.to_vec(),
            },
            "Body text.".to_string(),
        )
        .unwrap()
    }

    #[test]
    fn save_and_load_roundtrips() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::init(tmp.path(), None).unwrap();
        let item = sample_item("no-any-in-ts");
        store.save_item(&item).unwrap();
        let loaded = store.load_item(ItemKind::Rule, "no-any-in-ts").unwrap();
        assert_eq!(loaded.name(), "no-any-in-ts");
        assert_eq!(loaded.body.trim(), "Body text.");
    }

    #[test]
    fn list_items_finds_saved_items() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::init(tmp.path(), None).unwrap();
        store.save_item(&sample_item("a")).unwrap();
        store.save_item(&sample_item("b")).unwrap();
        let items = store.list_items(ItemKind::Rule).unwrap();
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn remove_item_deletes_file() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::init(tmp.path(), None).unwrap();
        store.save_item(&sample_item("temp")).unwrap();
        store.remove_item(ItemKind::Rule, "temp").unwrap();
        assert!(store.load_item(ItemKind::Rule, "temp").is_err());
    }
}
