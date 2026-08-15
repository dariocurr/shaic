//! Cross-platform file helpers. Unix uses explicit modes; Windows relies on
//! the user's ACLs (no `OpenOptionsExt::mode`).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

const PRIVATE_MODE: u32 = 0o600;

fn first_env_path(keys: &[&str]) -> Option<PathBuf> {
    keys.iter().find_map(|key| {
        std::env::var_os(key)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    })
}

/// Home directory for shaic paths. `dirs` 6 on Windows uses known folders and
/// ignores `HOME` / `USERPROFILE`; honor those first so tests and users can
/// isolate from the OS profile.
pub fn home_dir() -> Option<PathBuf> {
    first_env_path(&["HOME", "USERPROFILE"]).or_else(dirs::home_dir)
}

/// Config directory for `config.toml`. Same reason as `home_dir`: honor
/// `XDG_CONFIG_HOME` / `APPDATA` before the Windows known-folder API.
pub fn config_dir() -> Option<PathBuf> {
    first_env_path(&["XDG_CONFIG_HOME", "APPDATA"]).or_else(dirs::config_dir)
}

/// Intersect `requested` with an existing file's mode so rewrites never widen
/// permissions. No-op on platforms without Unix permission bits.
pub fn effective_mode(target: &Path, requested: u32) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match fs::symlink_metadata(target) {
            Ok(meta) if meta.file_type().is_file() => requested & meta.permissions().mode() & 0o777,
            _ => requested,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = target;
        requested
    }
}

pub fn open_create_new(path: &Path, mode: u32) -> Result<fs::File> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(mode);
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
    }
    opts.open(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

pub fn open_create_truncate(path: &Path, mode: u32) -> Result<fs::File> {
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(mode);
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
    }
    opts.open(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Write `contents` to `target` via an exclusive temp file in `parent`, then
/// rename. Cleans up the temp file on failure.
pub fn atomic_write_in_parent(
    parent: &Path,
    target: &Path,
    contents: &str,
    mode: u32,
    tmp_name: &str,
) -> Result<()> {
    let tmp = parent.join(tmp_name);
    let result = (|| {
        {
            let mut f = open_create_new(&tmp, mode)?;
            f.write_all(contents.as_bytes())
                .map_err(|source| Error::Io {
                    path: tmp.clone(),
                    source,
                })?;
        }
        fs::rename(&tmp, target).map_err(|source| Error::Io {
            path: target.to_path_buf(),
            source,
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Like `atomic_write_in_parent`, but creates/truncates the temp file — used
/// when the temp path is derived from the target filename.
pub fn atomic_write_truncate_in_parent(
    _parent: &Path,
    tmp_path: &Path,
    target: &Path,
    contents: &str,
    mode: u32,
) -> Result<()> {
    let result = (|| {
        {
            let mut f = open_create_truncate(tmp_path, mode)?;
            f.write_all(contents.as_bytes())
                .map_err(|source| Error::Io {
                    path: tmp_path.to_path_buf(),
                    source,
                })?;
        }
        fs::rename(tmp_path, target).map_err(|source| Error::Io {
            path: target.to_path_buf(),
            source,
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp_path);
    }
    result
}

/// Config and secret index files: private on Unix, plain atomic write elsewhere.
pub fn write_private_config(path: &Path, contents: &str) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::NoParentDirectory(path.to_path_buf()))?;
    fs::create_dir_all(parent).map_err(|source| Error::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let tmp = parent.join(format!(".shaic-private.{}.tmp", std::process::id()));
    atomic_write_truncate_in_parent(parent, &tmp, path, contents, PRIVATE_MODE)
}
