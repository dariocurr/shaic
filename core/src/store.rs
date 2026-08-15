pub mod git;
pub mod layout;
mod mcp;

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::{Item, ItemKind};
use crate::security::{frontmatter_limits, path_guard, secret_scan};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct StoreMeta {
    pub schema_version: u32,
    pub created_at_unix: u64,
}

#[derive(Debug, serde::Serialize)]
pub struct PushResult {
    /// True when this call created a new local commit.
    pub committed: bool,
    /// True when this call successfully ran `git push` (including retrying
    /// previously-unpushed commits on a clean working tree).
    pub pushed: bool,
    pub summary: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct PullResult {
    pub updated: bool,
    pub diff_stat: Option<String>,
}

/// `(name, message)` — `name` is set only when the skipped file's own name
/// was valid (see `Store::list_items_with_skips`).
pub type SkippedItem = (String, String);

#[derive(Debug)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn default_path() -> Result<PathBuf> {
        Ok(crate::platform::home_dir()
            .ok_or_else(|| {
                Error::Config(
                    "no home directory — cannot locate ~/.shaic/store (set HOME or USERPROFILE)"
                        .to_string(),
                )
            })?
            .join(".shaic")
            .join("store"))
    }

    /// Per-machine materialization state (provenance manifests). Lives outside
    /// the git-tracked store — never committed, pushed, or pulled.
    pub fn state_dir() -> PathBuf {
        crate::platform::home_dir()
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

    /// Create a fresh store (clone if `remote` given, else `git init`).
    /// Re-running against an existing store is a no-op unless `remote` is
    /// given; then see `init_with_force`.
    pub fn init(root: impl Into<PathBuf>, remote: Option<&str>) -> Result<Self> {
        Self::init_with_force(root, remote, false)
    }

    /// Like `init`, but a `--force` re-init may replace an already-configured
    /// origin. Without `force`, pointing an existing store at a *different*
    /// remote is refused — the next `shaic push` would otherwise publish the
    /// store to an attacker-supplied URL with no extra confirmation.
    pub fn init_with_force(
        root: impl Into<PathBuf>,
        remote: Option<&str>,
        force: bool,
    ) -> Result<Self> {
        let root = root.into();
        if let Some(url) = remote {
            git::validate_remote_url(url)?;
        }

        if root.join(".git").exists() {
            if let Some(url) = remote {
                if git::has_remote(&root) {
                    let current = git::origin_url(&root)?;
                    if current != url && !force {
                        return Err(Error::RemoteAlreadySet {
                            current,
                            requested: url.to_string(),
                        });
                    }
                }
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
        let meta: StoreMeta = toml::from_str(&raw).map_err(|e| Error::Toml {
            path: meta_path.clone(),
            message: e.to_string(),
        })?;
        if meta.schema_version > SCHEMA_VERSION {
            return Err(Error::SchemaTooNew {
                found: meta.schema_version,
                supported: SCHEMA_VERSION,
            });
        }
        Ok(())
    }

    /// Write an item into the canonical store.
    ///
    /// Runs the secret-scan tripwire on the rendered file, the same way
    /// `save_mcp_server` does: this is not only the `shaic item add/edit`
    /// path, it's also where `reconcile_items` pulls content straight out of
    /// an agent's own file (`CLAUDE.md`, `.cursor/rules/*.md`, ...). A
    /// credential pasted into one of those by hand would otherwise be adopted
    /// into the git-tracked store — and the push-time scan is a much worse
    /// place to discover it, since by then it's already committed locally.
    pub fn save_item(&self, item: &Item) -> Result<()> {
        self.check_schema()?;
        let path = layout::item_path(&self.root, item.kind, item.name());
        let target = guarded_path(&self.root, &path)?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|source| Error::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let fm_yaml = serde_yaml_ng::to_string(&item.frontmatter)
            .map_err(|e| Error::FrontmatterParse(e.to_string()))?;
        frontmatter_limits::validate_raw(&fm_yaml)?;
        let contents = render_with_frontmatter(&fm_yaml, &item.body);
        secret_scan::scan_or_reject(&contents, false)?;
        path_guard::reject_if_symlink(&target)?;
        path_guard::revalidate_within(&self.root, &target)?;
        std::fs::write(&target, contents).map_err(|source| Error::Io {
            path: target,
            source,
        })
    }

    pub fn load_item(&self, kind: ItemKind, name: &str) -> Result<Item> {
        Ok(self.load_item_with_warnings(kind, name)?.0)
    }

    /// As `load_item`, plus any lenient-parse warnings (unknown frontmatter
    /// fields from a newer shaic). Callers that can surface notes — reconcile,
    /// `status` — should use this so the user sees what was dropped.
    pub fn load_item_with_warnings(
        &self,
        kind: ItemKind,
        name: &str,
    ) -> Result<(Item, Vec<String>)> {
        self.check_schema()?;
        crate::model::validate_name(name)?;
        let path = layout::item_path(&self.root, kind, name);
        let target = guarded_path(&self.root, &path)?;
        let raw = std::fs::read_to_string(&target).map_err(|source| Error::Io {
            path: target.clone(),
            source,
        })?;
        let (fm_raw, body) = split_frontmatter(&raw)?;
        let (frontmatter, warnings) = frontmatter_limits::parse_lenient(fm_raw)?;
        Ok((Item::new(kind, frontmatter, body.to_string())?, warnings))
    }

    pub fn remove_item(&self, kind: ItemKind, name: &str) -> Result<()> {
        self.check_schema()?;
        crate::model::validate_name(name)?;
        let path = layout::item_path(&self.root, kind, name);
        match kind {
            ItemKind::Skill => {
                let dir = path
                    .parent()
                    .ok_or_else(|| Error::NoParentDirectory(path.clone()))?;
                let target = guarded_path(&self.root, dir)?;
                path_guard::reject_if_symlink(&target)?;
                path_guard::revalidate_within(&self.root, &target)?;
                std::fs::remove_dir_all(&target).map_err(|source| Error::Io {
                    path: target,
                    source,
                })?;
            }
            _ => {
                let target = guarded_path(&self.root, &path)?;
                path_guard::reject_if_symlink(&target)?;
                path_guard::revalidate_within(&self.root, &target)?;
                std::fs::remove_file(&target).map_err(|source| Error::Io {
                    path: target,
                    source,
                })?;
            }
        }
        Ok(())
    }

    /// Every readable item of `kind`, dropping the ones that aren't.
    ///
    /// Delegates to `list_items_with_skips` and throws the skip list away, for
    /// the callers that only want to show what's there.
    pub fn list_items(&self, kind: ItemKind) -> Result<Vec<Item>> {
        Ok(self.list_items_with_skips(kind)?.0)
    }

    /// Items of `kind` plus one `(name, message)` entry per file that had to
    /// be skipped, mirroring `list_mcp_servers`'s contract exactly.
    ///
    /// A single corrupt item file used to fail this call outright, which took
    /// down `shaic status`/`sync` for *every* agent, scope and kind — one
    /// unparseable rule made the whole tool unusable until it was found by
    /// hand. Skipping and reporting keeps the blast radius at the one file.
    /// `name` is set only when the file's own name was valid, so a caller can
    /// tell "a known item failed to parse" apart from "a junk filename", and
    /// not mistake the former for an item that left the store.
    pub fn list_items_with_skips(&self, kind: ItemKind) -> Result<(Vec<Item>, Vec<SkippedItem>)> {
        self.check_schema()?;
        let dir = self.root.join(layout::kind_dir(kind));
        if !dir.exists() {
            return Ok((Vec::new(), Vec::new()));
        }
        let mut items = Vec::new();
        let mut skipped = Vec::new();
        for entry in walkdir::WalkDir::new(&dir).max_depth(2).follow_links(false) {
            let entry = entry.map_err(|e| Error::WalkDir {
                path: dir.clone(),
                message: e.to_string(),
            })?;
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
                skipped.push((
                    String::new(),
                    format!(
                        "{} — {name:?} isn't a valid item name",
                        entry.path().display()
                    ),
                ));
                continue;
            }
            match self.load_item(kind, name) {
                Ok(item) => items.push(item),
                Err(e) => skipped.push((name.to_string(), format!("{kind:?} {name:?} — {e}"))),
            }
        }
        Ok((items, skipped))
    }

    /// Commit whatever is in the working tree and publish it.
    ///
    /// `allow_secrets` overrides the secret-scan tripwire *for this call only*:
    /// it is never recorded anywhere, so a commit waved through once is
    /// scanned again by the next `push` that has to publish it. That's the
    /// point — the override is meant to unblock one deliberate push, not to
    /// permanently exempt the history it created.
    pub fn push(&self, allow_secrets: bool) -> Result<PushResult> {
        if !git::has_remote(&self.root) {
            return Err(Error::Config(
                "no remote configured for the store yet — run `shaic init --remote <url>` first"
                    .to_string(),
            ));
        }
        let status = git::status_porcelain(&self.root)?;
        let mut committed = false;
        let mut summary = None;
        if !status.trim().is_empty() {
            git::add_all(&self.root)?;
            let staged = git::diff_cached(&self.root)?;
            if let Err(e) = secret_scan::scan_or_reject(&staged, allow_secrets) {
                // `add -A` already staged the offending file. Leaving it in
                // the index means a later plain `git commit`/`shaic push`
                // sweeps the credential in without the user ever seeing this
                // error again, so undo the staging before surfacing it.
                git::reset_mixed(&self.root)?;
                return Err(e);
            }
            let stat = git::diff_cached_stat(&self.root)?;
            summary = Some(summarize_stat(&stat));
            git::commit(
                &self.root,
                summary.as_deref().unwrap_or("shaic: update store"),
            )?;
            committed = true;
        }

        let branch = git::current_branch(&self.root)?;
        git::fetch(&self.root)?;
        let remote_exists = git::remote_branch_exists(&self.root, &branch)?;
        if remote_exists {
            git::merge_ff_only(&self.root, &branch)?;
        }

        let ahead = if remote_exists {
            git::commits_ahead(&self.root, &branch)?
        } else {
            // No origin/<branch> yet — any local history still needs a push
            // (covers "commit succeeded, push failed" with a clean tree).
            git::commit_count_head(&self.root)?
        };
        if !committed && ahead == 0 {
            return Ok(PushResult {
                committed: false,
                pushed: false,
                summary: None,
            });
        }

        // Scan what this push would actually publish, not just what this call
        // happened to stage. A clean working tree with unpushed commits — a
        // previous push that used the override, or one whose commit succeeded
        // and whose network push failed — used to reach `git push` with no
        // scan at all.
        let base = git::outgoing_base(&branch, remote_exists);
        let outgoing = git::diff_range(&self.root, &base, "HEAD")?;
        secret_scan::scan_or_reject(&outgoing, allow_secrets)?;

        git::push(&self.root, &branch)?;
        Ok(PushResult {
            committed,
            pushed: true,
            summary: summary.or_else(|| Some(format!("{ahead} unpushed commit(s)"))),
        })
    }

    pub fn pull(&self, allow_secrets: bool) -> Result<PullResult> {
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
            // Scan what would land *before* merging it in. After an FF merge
            // the secret is already in HEAD; refusing then would leave the
            // store holding a credential the user never agreed to keep.
            let incoming = git::diff_range(&self.root, "HEAD", &format!("origin/{branch}"))?;
            secret_scan::scan_or_reject(&incoming, allow_secrets)?;
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

/// Resolve `path` (absolute under the store root) through `path_guard` so a
/// symlinked skill dir / mcp file planted in the store can't escape it.
fn guarded_path(store_root: &Path, path: &Path) -> Result<PathBuf> {
    let relative = path
        .strip_prefix(store_root)
        .map_err(|_| Error::PathEscape {
            root: store_root.to_path_buf(),
            candidate: path.to_path_buf(),
        })?;
    path_guard::ensure_within(store_root, relative)
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

/// Split a `---\n<yaml>\n---\n<body>` document. Accepts LF and CRLF so Windows
/// checkouts with `core.autocrlf=true` still load.
fn split_frontmatter(content: &str) -> Result<(&str, &str)> {
    split_frontmatter_lf_or_crlf(content).ok_or_else(|| {
        Error::FrontmatterParse(
            "expected a --- frontmatter block (LF or CRLF line endings)".to_string(),
        )
    })
}

fn split_frontmatter_lf_or_crlf(content: &str) -> Option<(&str, &str)> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let rest = content
        .strip_prefix("---\r\n")
        .or_else(|| content.strip_prefix("---\n"))?;
    if let Some(idx) = rest.find("\r\n---\r\n") {
        let (fm, after) = rest.split_at(idx);
        return Some((fm, &after["\r\n---\r\n".len()..]));
    }
    let marker = "\n---\n";
    let idx = rest.find(marker)?;
    let (fm, after) = rest.split_at(idx);
    Some((fm, &after[marker.len()..]))
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
    fn load_item_accepts_crlf_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::init(tmp.path(), None).unwrap();
        let path = store.root().join("rules").join("crlf-rule.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "---\r\nname: crlf-rule\r\ndescription: windows checkout\r\napplies_to: []\r\ntags: []\r\nscope: [project]\r\nagents: [claude-code]\r\n---\r\nBody from CRLF.\r\n",
        )
        .unwrap();
        let loaded = store.load_item(ItemKind::Rule, "crlf-rule").unwrap();
        assert_eq!(loaded.name(), "crlf-rule");
        assert_eq!(loaded.body.trim(), "Body from CRLF.");
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

    #[test]
    fn save_item_rejects_a_pasted_credential() {
        // The reconcile path feeds `save_item` content straight out of an
        // agent's own file, so this is the boundary that keeps a credential
        // typed into `CLAUDE.md` out of the git-tracked store.
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::init(tmp.path(), None).unwrap();
        let mut item = sample_item("leaky");
        item.body = "export AWS_ACCESS_KEY_ID=AKIAABCDEFGHIJKLMNOP\n".to_string();
        assert!(matches!(
            store.save_item(&item),
            Err(Error::SecretDetected(_))
        ));
        assert!(store.load_item(ItemKind::Rule, "leaky").is_err());
    }

    #[test]
    fn list_items_skips_a_corrupt_file_instead_of_failing_the_whole_call() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::init(tmp.path(), None).unwrap();
        store.save_item(&sample_item("good")).unwrap();
        std::fs::write(
            store
                .root()
                .join(layout::kind_dir(ItemKind::Rule))
                .join("broken.md"),
            "no frontmatter here at all\n",
        )
        .unwrap();

        let (items, skipped) = store.list_items_with_skips(ItemKind::Rule).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name(), "good");
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].0, "broken");
        // The whole point: one bad file no longer takes `status`/`sync` down.
        assert_eq!(store.list_items(ItemKind::Rule).unwrap().len(), 1);
    }

    fn git_in(repo: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    fn store_with_remote() -> (tempfile::TempDir, tempfile::TempDir, Store) {
        let remote = tempfile::tempdir().unwrap();
        git_in(remote.path(), &["init", "--bare", "-q"]);
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(
            dir.path().join("store"),
            Some(&remote.path().to_string_lossy()),
        )
        .unwrap();
        git_in(store.root(), &["config", "user.name", "shaic-test"]);
        git_in(
            store.root(),
            &["config", "user.email", "shaic-test@example.com"],
        );
        (remote, dir, store)
    }

    #[test]
    fn push_leaves_nothing_staged_when_the_scan_rejects() {
        let (_remote, _dir, store) = store_with_remote();
        std::fs::write(
            store.root().join("notes.md"),
            "password = \"hunter2hunter2hunter2\"\n",
        )
        .unwrap();

        assert!(matches!(store.push(false), Err(Error::SecretDetected(_))));
        assert!(
            git::diff_cached(store.root()).unwrap().trim().is_empty(),
            "a rejected credential must not be left in the index for the next commit to sweep up"
        );
    }

    #[test]
    fn push_scans_unpushed_commits_on_a_clean_working_tree() {
        let (_remote, _dir, store) = store_with_remote();
        store.save_item(&sample_item("clean")).unwrap();
        assert!(store.push(false).unwrap().pushed);

        // Committed behind shaic's back — the working tree is clean, so the
        // staged-diff scan sees nothing at all.
        std::fs::write(
            store.root().join("notes.md"),
            "password = \"hunter2hunter2hunter2\"\n",
        )
        .unwrap();
        git_in(store.root(), &["add", "-A"]);
        git_in(store.root(), &["commit", "-qm", "behind shaic's back"]);

        assert!(
            matches!(store.push(false), Err(Error::SecretDetected(_))),
            "the outgoing patch must be scanned even when there is nothing to commit"
        );
        assert!(
            store.push(true).unwrap().pushed,
            "the override still has to work for a deliberate push"
        );
    }

    #[test]
    fn re_init_refuses_to_silently_repoint_origin() {
        let remote_a = tempfile::tempdir().unwrap();
        let remote_b = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init", "--bare", "-q"])
            .current_dir(remote_a.path())
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["init", "--bare", "-q"])
            .current_dir(remote_b.path())
            .status()
            .unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join("store");
        Store::init(&store_path, Some(&remote_a.path().to_string_lossy())).unwrap();
        let err = Store::init(&store_path, Some(&remote_b.path().to_string_lossy())).unwrap_err();
        assert!(matches!(err, Error::RemoteAlreadySet { .. }));
        Store::init_with_force(&store_path, Some(&remote_b.path().to_string_lossy()), true)
            .unwrap();
        assert_eq!(
            crate::store::git::origin_url(&store_path).unwrap(),
            remote_b.path().to_string_lossy().to_string()
        );
    }
}
