use std::collections::BTreeMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::model::{AgentId, ContentForm, Scope};
use crate::platform;
use crate::security::path_guard;
use crate::store::Store;

pub const BEGIN_MARKER: &str = "<!-- shaic:begin -->";
pub const END_MARKER: &str = "<!-- shaic:end -->";

/// Every marker is `<!-- shaic:` + zero or more backslashes + `begin`/`end` +
/// ` -->`. Zero backslashes is a *real* marker; one or more is an escaped
/// mention of one inside a body.
const MARKER_PREFIX: &str = "<!-- shaic:";
const MARKER_SUFFIX: &str = " -->";
const MARKER_WORDS: &[&str] = &["begin", "end"];

/// If a marker token starts at `at`, its backslash depth and total byte
/// length. `None` for anything else, including a prefix that isn't followed
/// by a recognized word.
fn marker_at(s: &str, at: usize) -> Option<(usize, usize)> {
    let after_prefix = s.get(at..)?.strip_prefix(MARKER_PREFIX)?;
    let depth = after_prefix.len() - after_prefix.trim_start_matches('\\').len();
    let word = MARKER_WORDS
        .iter()
        .find(|w| after_prefix[depth..].starts_with(&format!("{w}{MARKER_SUFFIX}")))?;
    Some((
        depth,
        MARKER_PREFIX.len() + depth + word.len() + MARKER_SUFFIX.len(),
    ))
}

/// Rewrite every marker token in `region`, changing its backslash depth by
/// `delta`, and copy everything else through untouched.
fn reindent_markers(region: &str, delta: i32) -> String {
    let mut out = String::with_capacity(region.len());
    let mut i = 0;
    while i < region.len() {
        match marker_at(region, i) {
            Some((depth, len)) => {
                let new_depth = (depth as i32 + delta).max(0) as usize;
                out.push_str(MARKER_PREFIX);
                for _ in 0..new_depth {
                    out.push('\\');
                }
                out.push_str(&region[i + MARKER_PREFIX.len() + depth..i + len]);
                i += len;
            }
            None => {
                // Byte-wise would split a multi-byte char; step one char.
                let ch_len = region[i..]
                    .chars()
                    .next()
                    .map(char::len_utf8)
                    .unwrap_or(1)
                    .max(1);
                out.push_str(&region[i..i + ch_len]);
                i += ch_len;
            }
        }
    }
    out
}

/// Escape marker mentions in a body so `find(END_MARKER)` can't truncate
/// mid-region.
///
/// A backslash is added to *every* marker token, not just unescaped ones,
/// which is what makes the transform injective and therefore reversible —
/// the same class of fix as `adapters::common::escape_heading_lines`.
/// Escaping only real markers while unescaping any depth meant a body that
/// deliberately contained the literal text `<!-- shaic:\begin -->` came back
/// as a *real* marker, silently rewriting content (and, worse, planting a
/// marker inside a managed region). Now `<!-- shaic:begin -->` ->
/// `<!-- shaic:\begin -->` -> `<!-- shaic:begin -->` and
/// `<!-- shaic:\begin -->` -> `<!-- shaic:\\begin -->` ->
/// `<!-- shaic:\begin -->`, at every depth.
fn escape_markers_in_region(region: &str) -> String {
    reindent_markers(region, 1)
}

fn unescape_markers_in_region(region: &str) -> String {
    reindent_markers(region, -1)
}

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
/// themselves) end up misread as part of an item's body. Marker text that
/// appeared in the original body is unescaped so round-trips stay faithful.
pub fn managed_region(content: &str) -> Option<String> {
    let start = content.find(BEGIN_MARKER)? + BEGIN_MARKER.len();
    let end = content[start..].find(END_MARKER)? + start;
    Some(unescape_markers_in_region(content[start..end].trim()))
}

/// Read the current managed region (between markers) from `path`, if the
/// file and markers exist.
fn read_managed_region(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    managed_region(&content)
}

/// Splice `region` into whatever `path` already contains: replace an existing
/// marker block in place, or append a new one at the end. Content outside the
/// markers is preserved byte-for-byte. Creates the file with just the region
/// if it doesn't exist yet. Marker substrings inside `region` are escaped so
/// they cannot close the block early.
fn splice_managed_region(existing: Option<&str>, region: &str) -> String {
    let escaped = escape_markers_in_region(region.trim());
    let block = format!("{BEGIN_MARKER}\n{escaped}\n{END_MARKER}");
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
/// anything that might hold a resolved credential — narrowed by
/// `never_loosen` against whatever the file already has.
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
        .ok_or_else(|| Error::NoParentDirectory(target.to_path_buf()))?;
    fs::create_dir_all(parent).map_err(|source| Error::Io {
        path: parent.to_path_buf(),
        source,
    })?;

    let mode = platform::effective_mode(target, mode);
    let tmp_path = parent.join(format!(
        ".shaic-tmp-{}-{}",
        std::process::id(),
        fastrand_ish()
    ));
    let result = (|| {
        {
            let mut f = platform::open_create_new(&tmp_path, mode)?;
            f.write_all(final_contents.as_bytes())
                .map_err(|source| Error::Io {
                    path: tmp_path.clone(),
                    source,
                })?;
        }
        path_guard::reject_if_symlink(target)?;
        path_guard::revalidate_within(root, target)?;
        fs::rename(&tmp_path, target).map_err(|source| Error::Io {
            path: target.to_path_buf(),
            source,
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
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

/// What a manifest entry's digest covers.
///
/// The distinction is the whole reason single-file agents can be tracked at
/// all: for `ContentForm::SingleFile` shaic owns only the text between the
/// markers, and the user legitimately edits everything around it, so hashing
/// the whole file would report "not mine" the instant they added a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackedContent {
    /// The entire file (directory-form items) or the entire serialized value
    /// (one MCP server entry inside a shared config file).
    Whole,
    /// Only the region between `BEGIN_MARKER` and `END_MARKER`.
    ManagedRegion,
}

impl TrackedContent {
    /// Tagging the digest with what it covers means a `Whole` digest can
    /// never accidentally satisfy a `ManagedRegion` question (or vice versa)
    /// — a same-shaped hex string compared against the wrong thing would be
    /// a licence to delete.
    fn prefix(self) -> &'static str {
        match self {
            TrackedContent::Whole => "whole:",
            TrackedContent::ManagedRegion => "region:",
        }
    }

    pub fn for_form(form: ContentForm) -> Self {
        match form {
            ContentForm::Directory => TrackedContent::Whole,
            ContentForm::SingleFile => TrackedContent::ManagedRegion,
        }
    }
}

/// SHA-256, lowercase hex, prefixed with what the digest covers.
///
/// Deliberately not `std::collections::hash_map::DefaultHasher`, which this
/// used to be. std explicitly does not guarantee `DefaultHasher`'s output is
/// stable across releases, so a routine toolchain upgrade would silently
/// invalidate every entry in every manifest — after which shaic could never
/// again recognize (and therefore never clean up) a file it had written. 64
/// bits was also thin for what this decides, which is "may I delete this
/// file?".
fn digest(tracked: TrackedContent, contents: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(contents.as_bytes());
    format!("{}{:x}", tracked.prefix(), hasher.finalize())
}

/// Whether `raw` is a digest this build knows how to compare against.
///
/// Anything else — an integer left by the pre-SHA-256 format, a truncated
/// line, a shape a future version introduces — is treated as *not tracked*.
/// That is the conservative direction on purpose: an unrecognized entry means
/// shaic refuses to delete a file it may well have written (the user cleans
/// it up by hand, once), whereas guessing the other way means deleting a file
/// that might be theirs. Never delete wrongly beats never leave a stale file.
fn is_recognized_digest(raw: &str) -> bool {
    [TrackedContent::Whole, TrackedContent::ManagedRegion]
        .iter()
        .any(|tracked| {
            raw.strip_prefix(tracked.prefix()).is_some_and(|hex| {
                hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
            })
        })
}

/// Current manifest format. Bumped whenever an entry's *meaning* changes, so
/// the reader can tell "written by a shaic that meant something else" apart
/// from "written by this one" instead of misreading old data as current.
const MANIFEST_VERSION: u32 = 2;

/// Per-machine record of what shaic itself last wrote, keyed by relative path
/// (or, for MCP, by server name), so a rename/removal can be told apart from
/// a hand-authored file with the same name. Lives under `Store::state_dir()`
/// — never inside the git-tracked store, never synced.
#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    /// See `MANIFEST_VERSION`. `#[serde(default)]` (to 0) so a v1 manifest,
    /// which had no such field, parses and is then rejected by `load` as
    /// "older format" rather than failing to deserialize at all.
    #[serde(default)]
    version: u32,
    /// Relative path -> `digest(...)`. A `String` rather than v1's `i64`,
    /// which existed only to squeeze a `u64` hash through TOML's signed
    /// integer type.
    #[serde(default)]
    entries: BTreeMap<String, String>,
}

impl Default for Manifest {
    fn default() -> Self {
        Manifest {
            version: MANIFEST_VERSION,
            entries: BTreeMap::new(),
        }
    }
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

    /// Load, degrading to an empty manifest for anything this build can't
    /// read with confidence: a missing file, an unparseable one (a crash or
    /// full disk mid-write), or one written in a different format version.
    ///
    /// Empty is always the safe degradation, and that property is worth
    /// stating plainly because everything else here depends on it: an empty
    /// manifest makes every path look *untracked*, and untracked means shaic
    /// refuses to delete it. Losing the manifest can therefore only ever
    /// leave stale files behind — it can never cause a deletion.
    ///
    /// Migration from v1 (`i64` `DefaultHasher` values, no `version` field)
    /// is exactly that degradation: those entries are unreadable as digests,
    /// so every path a v1 manifest tracked becomes untracked. The user may
    /// have to remove one generation of orphaned agent files by hand; in
    /// exchange, no upgrade path can ever delete a file shaic didn't write.
    /// Individual entries whose digest shape isn't recognized are dropped for
    /// the same reason, rather than lingering forever unmatched.
    pub fn load(path: &Path) -> Manifest {
        let parsed: Option<Manifest> = fs::read_to_string(path)
            .ok()
            .and_then(|raw| toml::from_str(&raw).ok());
        match parsed {
            Some(mut manifest) if manifest.version == MANIFEST_VERSION => {
                manifest
                    .entries
                    .retain(|_, digest| is_recognized_digest(digest));
                manifest
            }
            _ => Manifest::default(),
        }
    }

    /// Written via temp-file-then-rename so a crash or a full disk mid-write
    /// can never leave a truncated, unparseable manifest on disk — see
    /// `load` for why that degradation is safe but still worth avoiding.
    ///
    /// Mode `0o600`, matching the sibling secrets index in the same state
    /// directory. The manifest holds no secret values, but it does enumerate
    /// every agent config path on this machine plus a digest of each one's
    /// contents — a map of what to tamper with, and an oracle for confirming
    /// guessed file contents. There is also no reason for any other user to
    /// read it, and a consistent mode across `state_dir()` is one less thing
    /// to reason about.
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
        let result = write_private_then_rename(&tmp_path, path, &toml);
        if result.is_err() {
            let _ = fs::remove_file(&tmp_path);
        }
        result
    }

    /// Record that shaic just wrote `contents` at `relative_path`. `tracked`
    /// says whether that is the whole file or only the managed region — see
    /// `TrackedContent`.
    pub fn record(&mut self, relative_path: &str, tracked: TrackedContent, contents: &str) {
        self.entries
            .insert(relative_path.to_string(), digest(tracked, contents));
    }

    pub fn forget(&mut self, relative_path: &str) {
        self.entries.remove(relative_path);
    }

    /// Whether what's on disk at `relative_path` is still exactly what shaic
    /// last wrote there. `contents` must be the whole file for
    /// `TrackedContent::Whole` and the extracted managed region for
    /// `TrackedContent::ManagedRegion`; a digest recorded under one is never
    /// accepted for the other.
    pub fn owns(&self, relative_path: &str, tracked: TrackedContent, contents: &str) -> bool {
        self.entries
            .get(relative_path)
            .is_some_and(|recorded| *recorded == digest(tracked, contents))
    }

    /// A path is safe to delete only if the manifest tracks it as a *whole*
    /// file shaic wrote and the on-disk content still matches — anything else
    /// (untracked, hand-edited since, or tracked only as a managed region
    /// inside a file the user also owns) is left alone. The managed-region
    /// case can never satisfy this by construction, which is the point:
    /// `.cursorrules` and friends are the user's files and shaic removes
    /// content from them by splicing an empty region, never by unlinking.
    pub fn safe_to_delete(&self, relative_path: &str, on_disk_contents: &str) -> bool {
        self.owns(relative_path, TrackedContent::Whole, on_disk_contents)
    }

    pub fn tracked_paths(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }
}

/// `fs::write` + rename, but with an explicit `0o600` on creation rather than
/// the process umask's idea of a default.
fn write_private_then_rename(tmp_path: &Path, path: &Path, contents: &str) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::NoParentDirectory(path.to_path_buf()))?;
    platform::atomic_write_truncate_in_parent(parent, tmp_path, path, contents, 0o600)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splice_escapes_end_marker_inside_region_so_it_cannot_truncate() {
        let region = format!("docs mention {END_MARKER} in prose");
        let out = splice_managed_region(None, &region);
        let round_trip = managed_region(&out).expect("markers present");
        assert_eq!(round_trip, region.trim());
        assert!(
            !out[BEGIN_MARKER.len()..].contains(END_MARKER) || out.matches(END_MARKER).count() == 1,
            "escaped body must not introduce a second real end marker: {out}"
        );
    }

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
    fn escaped_marker_text_in_a_body_survives_a_round_trip_unchanged() {
        // The escape has to be injective: a body that deliberately contains
        // the *escaped* spelling must come back as that same text, not be
        // promoted into a real marker (which would also plant a marker inside
        // a managed region).
        for body in [
            format!("mentions {BEGIN_MARKER} in prose"),
            format!("mentions {END_MARKER} in prose"),
            "already escaped: <!-- shaic:\\begin -->".to_string(),
            "already escaped: <!-- shaic:\\end -->".to_string(),
            "twice escaped: <!-- shaic:\\\\end -->".to_string(),
            "not a marker: <!-- shaic:middle -->".to_string(),
            "unicode ✂ then <!-- shaic:\\begin --> tail".to_string(),
        ] {
            let out = splice_managed_region(None, &body);
            assert_eq!(
                managed_region(&out).as_deref(),
                Some(body.trim()),
                "body {body:?} did not round-trip through {out:?}"
            );
            assert_eq!(
                out.matches(END_MARKER).count(),
                1,
                "exactly one real end marker must survive escaping: {out}"
            );
        }
    }

    #[test]
    fn manifest_round_trips_a_hex_digest_through_toml() {
        // v1 stored an `i64` purely to squeeze a `u64` hash through TOML's
        // signed integer type. A hex string has no such hazard — this pins
        // that the digest survives save/load byte-for-byte, and that the
        // version field is written and accepted.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.toml");
        let mut m = Manifest::default();
        m.record("skills/a.md", TrackedContent::Whole, "content");
        m.save(&path).unwrap();

        let loaded = Manifest::load(&path);
        assert!(loaded.safe_to_delete("skills/a.md", "content"));
        assert_eq!(loaded.version, MANIFEST_VERSION);
    }

    #[test]
    #[cfg(unix)]
    fn manifest_is_written_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.toml");
        Manifest::default().save(&path).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "manifest must match the sibling secrets index");
    }

    #[test]
    fn a_v1_manifest_degrades_to_untracked_rather_than_being_misread() {
        // The whole migration contract: an older manifest must never make
        // shaic delete something, so every path it tracked reads as untracked.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.toml");
        fs::write(&path, "[entries]\n\"skills/a.md\" = -42\n").unwrap();
        let loaded = Manifest::load(&path);
        assert!(!loaded.safe_to_delete("skills/a.md", "content"));
        assert_eq!(loaded.tracked_paths().count(), 0);
    }

    #[test]
    fn an_unrecognized_digest_shape_is_dropped_not_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.toml");
        fs::write(
            &path,
            "version = 2\n[entries]\n\"skills/a.md\" = \"whole:not-hex\"\n\"skills/b.md\" = \"AABB\"\n",
        )
        .unwrap();
        let loaded = Manifest::load(&path);
        assert_eq!(loaded.tracked_paths().count(), 0);
    }

    #[test]
    fn manifest_tracks_and_forgets() {
        let mut m = Manifest::default();
        m.record("skills/a.md", TrackedContent::Whole, "content");
        assert!(m.safe_to_delete("skills/a.md", "content"));
        assert!(!m.safe_to_delete("skills/a.md", "different content"));
        m.forget("skills/a.md");
        assert!(!m.safe_to_delete("skills/a.md", "content"));
    }

    #[test]
    fn a_managed_region_entry_is_never_safe_to_delete_as_a_whole_file() {
        // `.cursorrules` is the user's file; shaic owns only the region
        // inside it. Tracking that region must never authorize unlinking the
        // file, no matter what it happens to contain.
        let mut m = Manifest::default();
        m.record(".cursorrules", TrackedContent::ManagedRegion, "region body");
        assert!(m.owns(".cursorrules", TrackedContent::ManagedRegion, "region body"));
        assert!(!m.safe_to_delete(".cursorrules", "region body"));
    }

    #[test]
    #[cfg(unix)]
    fn a_rewrite_never_loosens_an_existing_files_permissions() {
        use std::os::unix::fs::PermissionsExt;
        // `~/.claude.json` is the motivating case: a user who tightened it
        // must not find it widened because shaic merged in one MCP server.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("config.json");
        fs::write(&target, "{}").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o400)).unwrap();

        write_atomic(dir.path(), &target, "{\"a\":1}", 0o600).unwrap();
        let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o400, "the stricter existing mode must be kept");

        // A fresh file still gets exactly what was asked for.
        let fresh = dir.path().join("fresh.json");
        write_atomic(dir.path(), &fresh, "{}", 0o600).unwrap();
        let fresh_mode = fs::metadata(&fresh).unwrap().permissions().mode() & 0o777;
        assert_eq!(fresh_mode, 0o600);
    }
}
