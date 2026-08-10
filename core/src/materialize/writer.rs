use std::collections::BTreeMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::{AgentId, ContentForm, Scope};
use crate::security::path_guard;
use crate::store::Store;

pub const BEGIN_MARKER: &str = "<!-- shaic:begin -->";
pub const END_MARKER: &str = "<!-- shaic:end -->";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteAction {
    Create,
    Update,
    NoOp,
}

/// The region between `BEGIN_MARKER`/`END_MARKER` in `content`, if both are
/// present — never any hand-written text outside the managed block. Callers
/// that reconcile agent-on-disk content back into the store must scope to
/// this, not the whole file, or hand-written notes (and even the markers
/// themselves) end up misread as part of an item's body.
pub fn managed_region(content: &str) -> Option<&str> {
    let start = content.find(BEGIN_MARKER)? + BEGIN_MARKER.len();
    let end = content[start..].find(END_MARKER)? + start;
    Some(content[start..end].trim())
}

/// Read the current managed region (between markers) from `path`, if the
/// file and markers exist.
fn read_managed_region(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    managed_region(&content).map(str::to_string)
}

/// Splice `region` into whatever `path` already contains: replace an existing
/// marker block in place, or append a new one at the end. Content outside the
/// markers is preserved byte-for-byte. Creates the file with just the region
/// if it doesn't exist yet.
fn splice_managed_region(existing: Option<&str>, region: &str) -> String {
    let block = format!("{BEGIN_MARKER}\n{}\n{END_MARKER}", region.trim());
    match existing {
        None => format!("{block}\n"),
        Some(existing) => match existing
            .find(BEGIN_MARKER)
            .and_then(|start| Some((start, existing[start..].find(END_MARKER)?)))
        {
            Some((start, end_offset)) => {
                let end = start + end_offset + END_MARKER.len();
                format!("{}{}{}", &existing[..start], block, &existing[end..])
            }
            _ => {
                let mut out = existing.trim_end().to_string();
                if !out.is_empty() {
                    out.push_str("\n\n");
                }
                out.push_str(&block);
                out.push('\n');
                out
            }
        },
    }
}

/// Read-only comparison: does writing `new_contents` (or region, for
/// single-file agents) to `target` actually change anything?
pub fn classify(target: &Path, form: ContentForm, new_contents: &str) -> WriteAction {
    match form {
        ContentForm::Directory => match fs::read_to_string(target) {
            Ok(existing) if existing == new_contents => WriteAction::NoOp,
            Ok(_) => WriteAction::Update,
            Err(_) => WriteAction::Create,
        },
        ContentForm::SingleFile => match read_managed_region(target) {
            Some(region) if region == new_contents.trim() => WriteAction::NoOp,
            Some(_) => WriteAction::Update,
            None => WriteAction::Create,
        },
    }
}

/// Atomically write `final_contents` to `target`: create a temp file in the
/// same parent directory with `O_EXCL`-style `create_new`, then rename into
/// place. Re-checks for an escaping symlink (both the leaf and the whole
/// ancestor chain back to `root`) immediately before `create_dir_all` and
/// again immediately before the rename, to shrink the TOCTOU window as far as
/// std's APIs allow. `mode` is the temp file's permission bits (carried
/// through the rename) — `0o644` for agent-owned content, `0o600` for
/// anything that might hold a resolved credential.
pub(super) fn write_atomic(
    root: &Path,
    target: &Path,
    final_contents: &str,
    mode: u32,
) -> Result<()> {
    path_guard::reject_if_symlink(target)?;
    path_guard::revalidate_within(root, target)?;
    let parent = target
        .parent()
        .ok_or_else(|| Error::Git("write target has no parent directory".to_string()))?;
    fs::create_dir_all(parent).map_err(|source| Error::Io {
        path: parent.to_path_buf(),
        source,
    })?;

    let tmp_path = parent.join(format!(
        ".shaic-tmp-{}-{}",
        std::process::id(),
        fastrand_ish()
    ));
    let result = write_and_rename(&tmp_path, target, root, final_contents, mode);
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

fn write_and_rename(
    tmp_path: &Path,
    target: &Path,
    root: &Path,
    final_contents: &str,
    mode: u32,
) -> Result<()> {
    {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(tmp_path)
            .map_err(|source| Error::Io {
                path: tmp_path.to_path_buf(),
                source,
            })?;
        f.write_all(final_contents.as_bytes())
            .map_err(|source| Error::Io {
                path: tmp_path.to_path_buf(),
                source,
            })?;
    }

    path_guard::reject_if_symlink(target)?;
    path_guard::revalidate_within(root, target)?;
    fs::rename(tmp_path, target).map_err(|source| Error::Io {
        path: target.to_path_buf(),
        source,
    })
}

/// Cheap non-cryptographic uniqueness for temp file names — collisions only
/// matter for avoiding an accidental clash, not as a security boundary.
fn fastrand_ish() -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::time::SystemTime::now().hash(&mut hasher);
    std::thread::current().id().hash(&mut hasher);
    hasher.finish()
}

/// Validate `relative_path` is safely contained in `root`, classify, and
/// (if it's not a no-op) perform the write. This is the only function in the
/// whole workspace permitted to write into an agent-owned directory.
pub fn write_item(
    root: &Path,
    relative_path: &Path,
    form: ContentForm,
    contents: &str,
) -> Result<(PathBuf, WriteAction)> {
    let target = path_guard::ensure_within(root, relative_path)?;
    let action = classify(&target, form, contents);
    if action == WriteAction::NoOp {
        return Ok((target, action));
    }
    let final_contents = match form {
        ContentForm::Directory => contents.to_string(),
        ContentForm::SingleFile => {
            let existing = fs::read_to_string(&target).ok();
            splice_managed_region(existing.as_deref(), contents)
        }
    };
    write_atomic(root, &target, &final_contents, 0o644)?;
    Ok((target, action))
}

fn hash_content(s: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

/// Per-machine record of which `ContentForm::Directory` files shaic itself
/// last wrote, keyed by relative path, so a rename/removal can be told apart
/// from a hand-authored file with the same name. Lives under
/// `Store::state_dir()` — never inside the git-tracked store, never synced.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Manifest {
    // Stored as `i64` (a lossless bit-reinterpretation of the `u64` content
    // hash) because TOML's integer type is signed 64-bit — a raw `u64` with
    // its top bit set fails to serialize with "out-of-range value for u64
    // type".
    entries: BTreeMap<String, i64>,
}

impl Manifest {
    pub fn path_for(agent: AgentId, scope: Scope) -> PathBuf {
        Store::state_dir().join(format!("{}-{:?}.toml", agent.as_str(), scope).to_lowercase())
    }

    /// Separate manifest file for MCP servers, keyed by server *name* rather
    /// than file path — same provenance mechanism (record a content hash,
    /// only ever delete what's still exactly what shaic last wrote), applied
    /// to JSON object keys within one shared file instead of whole files.
    pub fn mcp_path_for(agent: AgentId, scope: Scope) -> PathBuf {
        Store::state_dir().join(format!("{}-{:?}-mcp.toml", agent.as_str(), scope).to_lowercase())
    }

    pub fn load(path: &Path) -> Manifest {
        fs::read_to_string(path)
            .ok()
            .and_then(|raw| toml::from_str(&raw).ok())
            .unwrap_or_default()
    }

    /// Written via temp-file-then-rename so a crash or a full disk mid-write
    /// can never leave a truncated, unparseable manifest on disk — that would
    /// silently `unwrap_or_default()` back to empty in `load` and make every
    /// path it tracked look untracked (so no longer safe to delete, but
    /// otherwise harmless — see `load`'s doc comment).
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| Error::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let toml = toml::to_string_pretty(self).map_err(|e| Error::Config(e.to_string()))?;
        let tmp_path = path.with_file_name(format!(
            "{}.tmp-{}",
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("manifest"),
            std::process::id()
        ));
        fs::write(&tmp_path, toml).map_err(|source| Error::Io {
            path: tmp_path.clone(),
            source,
        })?;
        let result = fs::rename(&tmp_path, path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        });
        if result.is_err() {
            let _ = fs::remove_file(&tmp_path);
        }
        result
    }

    pub fn record(&mut self, relative_path: &str, contents: &str) {
        self.entries
            .insert(relative_path.to_string(), hash_content(contents) as i64);
    }

    pub fn forget(&mut self, relative_path: &str) {
        self.entries.remove(relative_path);
    }

    /// A path is safe to delete only if the manifest currently tracks it
    /// *and* the on-disk content still matches shaic's last recorded write —
    /// anything else (untracked, or hand-edited since) is left alone.
    pub fn safe_to_delete(&self, relative_path: &str, on_disk_contents: &str) -> bool {
        self.entries
            .get(relative_path)
            .is_some_and(|&h| h == hash_content(on_disk_contents) as i64)
    }

    pub fn tracked_paths(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splice_creates_markers_in_new_file() {
        let out = splice_managed_region(None, "hello");
        assert!(out.contains(BEGIN_MARKER));
        assert!(out.contains("hello"));
    }

    #[test]
    fn splice_preserves_content_outside_markers() {
        let existing = format!(
            "# My notes\n\nHand-written stuff.\n\n{BEGIN_MARKER}\nold\n{END_MARKER}\n\nMore notes."
        );
        let out = splice_managed_region(Some(&existing), "new region");
        assert!(out.contains("Hand-written stuff."));
        assert!(out.contains("More notes."));
        assert!(out.contains("new region"));
        assert!(!out.contains("old"));
    }

    #[test]
    fn splice_ignores_end_marker_text_mentioned_before_the_real_block() {
        // A hand-written doc that merely *mentions* the end marker (e.g. as an
        // example) must not confuse the search for the real block: end marker
        // must be found after begin marker's position, not anywhere in the file.
        let existing = format!(
            "# Notes\n\nExample: `{END_MARKER}` closes a managed block.\n\n{BEGIN_MARKER}\nold\n{END_MARKER}\n"
        );
        let out = splice_managed_region(Some(&existing), "new region");
        assert_eq!(
            out.matches(BEGIN_MARKER).count(),
            1,
            "must not duplicate the block"
        );
        assert!(out.contains("new region"));
        assert!(!out.contains("old"));
        assert!(out.contains("Example:"));
    }

    #[test]
    fn classify_reports_noop_when_region_matches() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("CLAUDE.md");
        fs::write(&path, format!("{BEGIN_MARKER}\nsame\n{END_MARKER}")).unwrap();
        assert_eq!(
            classify(&path, ContentForm::SingleFile, "same"),
            WriteAction::NoOp
        );
    }

    #[test]
    fn manifest_round_trips_hash_with_high_bit_set() {
        // TOML integers are signed 64-bit; a raw `u64` content hash with its
        // top bit set (i.e. > i64::MAX, about half of all possible hashes)
        // must still serialize/deserialize losslessly through the manifest.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.toml");
        let mut m = Manifest::default();
        m.entries.insert("skills/a.md".to_string(), u64::MAX as i64);
        m.save(&path).unwrap();
        let loaded = Manifest::load(&path);
        assert_eq!(loaded.entries.get("skills/a.md"), Some(&(u64::MAX as i64)));
    }

    #[test]
    fn manifest_tracks_and_forgets() {
        let mut m = Manifest::default();
        m.record("skills/a.md", "content");
        assert!(m.safe_to_delete("skills/a.md", "content"));
        assert!(!m.safe_to_delete("skills/a.md", "different content"));
        m.forget("skills/a.md");
        assert!(!m.safe_to_delete("skills/a.md", "content"));
    }
}
