use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Ensure `candidate` (which need not exist yet) resolves to a location within
/// `root`. Rejects any escape via `..` segments, absolute overrides, or a
/// symlink anywhere between `root` and the candidate's parent that points
/// outside `root`. This is the ONLY function materialize::writer trusts to
/// decide whether a write target is safe.
///
/// SIDE EFFECT: this **creates `root`** (and its ancestors) if it doesn't
/// exist. That is not incidental — `root` has to exist to be canonicalized,
/// and callers rely on it, so a first-ever `shaic sync` into an agent
/// directory that isn't there yet succeeds instead of failing. Passing a
/// speculative or user-supplied `root` here will therefore create directories
/// on disk even if the answer turns out to be "rejected".
pub fn ensure_within(root: &Path, candidate: &Path) -> Result<PathBuf> {
    fs::create_dir_all(root).map_err(|source| Error::Io {
        path: root.to_path_buf(),
        source,
    })?;
    let root_canon = fs::canonicalize(root).map_err(|source| Error::Io {
        path: root.to_path_buf(),
        source,
    })?;

    let candidate_abs = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root_canon.join(candidate)
    };

    let resolved = resolve_as_far_as_possible(&candidate_abs)?;
    if !resolved.starts_with(&root_canon) {
        return Err(Error::PathEscape {
            root: root_canon,
            candidate: candidate_abs,
        });
    }

    check_no_escaping_symlinks(&root_canon, &candidate_abs)?;
    Ok(resolved)
}

/// Re-run the ancestor-symlink check right before a filesystem mutation, to
/// shrink the window between `ensure_within`'s original validation and the
/// actual write: an intermediate path component that didn't exist yet at
/// `ensure_within` time (and so couldn't be canonicalized) could in principle
/// be swapped for an escaping symlink afterward. `target` must already be an
/// absolute, `ensure_within`-resolved path.
pub fn revalidate_within(root: &Path, target: &Path) -> Result<()> {
    let root_canon = fs::canonicalize(root).map_err(|source| Error::Io {
        path: root.to_path_buf(),
        source,
    })?;
    check_no_escaping_symlinks(&root_canon, target)
}

/// Refuse to proceed if `path` itself already exists as a symlink — used
/// immediately before the temp-file-then-rename write in materialize::writer,
/// to shrink the window between this check and the filesystem operation.
pub fn reject_if_symlink(path: &Path) -> Result<()> {
    if let Ok(meta) = fs::symlink_metadata(path)
        && meta.file_type().is_symlink()
    {
        return Err(Error::SymlinkEscape(path.to_path_buf()));
    }
    Ok(())
}

/// Canonicalize the deepest existing ancestor, then re-append the remaining
/// (not-yet-existing) components lexically — `fs::canonicalize` itself fails
/// for paths that don't exist yet, so this resolves as much as it can.
fn resolve_as_far_as_possible(path: &Path) -> Result<PathBuf> {
    let mut existing = path.to_path_buf();
    let mut remainder: Vec<std::ffi::OsString> = Vec::new();
    while !existing.exists() {
        match existing.file_name().map(|n| n.to_os_string()) {
            Some(name) => {
                remainder.push(name);
                if !existing.pop() {
                    break;
                }
            }
            None => break,
        }
    }
    let mut resolved = fs::canonicalize(&existing).map_err(|source| Error::Io {
        path: existing.clone(),
        source,
    })?;
    for component in remainder.into_iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

/// Walk every ancestor directory from `candidate`'s parent down to (and
/// including) `root`; if any is itself a symlink, verify it resolves inside
/// `root` — an intermediate symlinked directory is as valid an escape vector
/// as the final path component.
fn check_no_escaping_symlinks(root_canon: &Path, candidate_abs: &Path) -> Result<()> {
    let mut dir = candidate_abs.parent().map(Path::to_path_buf);
    while let Some(d) = dir {
        if d == *root_canon {
            break;
        }
        if !d.starts_with(root_canon) {
            break;
        }
        if let Ok(meta) = fs::symlink_metadata(&d)
            && meta.file_type().is_symlink()
        {
            let target = fs::canonicalize(&d).map_err(|source| Error::Io {
                path: d.clone(),
                source,
            })?;
            if !target.starts_with(root_canon) {
                return Err(Error::SymlinkEscape(d));
            }
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    #[test]
    fn accepts_plain_relative_path() {
        let root = tempfile::tempdir().unwrap();
        let resolved = ensure_within(root.path(), Path::new("skills/foo/SKILL.md")).unwrap();
        assert!(resolved.starts_with(fs::canonicalize(root.path()).unwrap()));
    }

    #[test]
    fn rejects_dotdot_escape() {
        let root = tempfile::tempdir().unwrap();
        let err = ensure_within(root.path(), Path::new("../../etc/passwd"));
        assert!(matches!(err, Err(Error::PathEscape { .. })), "got {err:?}");
    }

    #[test]
    fn rejects_absolute_override() {
        let root = tempfile::tempdir().unwrap();
        let err = ensure_within(root.path(), Path::new("/etc/passwd"));
        assert!(matches!(err, Err(Error::PathEscape { .. })), "got {err:?}");
    }

    #[test]
    #[cfg(unix)]
    fn rejects_escaping_intermediate_symlink() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let link = root.path().join("linked");
        symlink(outside.path(), &link).unwrap();

        // The symlink resolves *before* the containment check, so this is
        // caught as an escaped destination rather than as a symlink — the
        // distinction matters, because it means the check doesn't depend on
        // spotting the link itself.
        let err = ensure_within(root.path(), Path::new("linked/evil.md"));
        assert!(matches!(err, Err(Error::PathEscape { .. })), "got {err:?}");
    }

    #[test]
    #[cfg(unix)]
    fn revalidate_within_catches_a_symlink_planted_after_ensure_within() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();

        // No escape yet: `ensure_within` succeeds since `linked/` doesn't
        // exist and can't be canonicalized as a symlink.
        let target = ensure_within(root.path(), Path::new("linked/evil.md")).unwrap();

        // An attacker plants the escaping symlink in the window between
        // `ensure_within` and the actual write.
        symlink(outside.path(), root.path().join("linked")).unwrap();

        let err = revalidate_within(root.path(), &target);
        assert!(matches!(err, Err(Error::SymlinkEscape(_))), "got {err:?}");
    }

    #[test]
    #[cfg(unix)]
    fn reject_if_symlink_flags_existing_link() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = root.path().join("CLAUDE.md");
        symlink(outside.path().join("nope"), &target).unwrap();
        let err = reject_if_symlink(&target);
        assert!(matches!(err, Err(Error::SymlinkEscape(_))), "got {err:?}");
    }
}
