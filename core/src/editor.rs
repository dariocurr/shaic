use std::path::PathBuf;

use crate::error::{Error, Result};

/// Open `initial` content in `$EDITOR` (falling back to `vi`), block until the
/// editor exits, and return the edited content. Shared by the CLI and TUI so
/// there's exactly one "how do we invoke an external editor" code path.
pub fn edit_in_editor(initial: &str) -> Result<String> {
    let editor_cmd = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let mut parts = editor_cmd.split_whitespace();
    let editor = parts.next().unwrap_or("vi").to_string();
    let extra_args: Vec<&str> = parts.collect();
    let tmp = tempfile::Builder::new()
        .suffix(".md")
        .tempfile()
        .map_err(|source| Error::Io {
            path: PathBuf::new(),
            source,
        })?;
    std::fs::write(tmp.path(), initial).map_err(|source| Error::Io {
        path: tmp.path().to_path_buf(),
        source,
    })?;

    let status = std::process::Command::new(&editor)
        .args(&extra_args)
        .arg(tmp.path())
        .status()
        .map_err(|source| Error::Io {
            path: PathBuf::from(&editor),
            source,
        })?;
    if !status.success() {
        return Err(Error::Config(format!(
            "editor {editor:?} exited with a non-zero status; aborting"
        )));
    }
    std::fs::read_to_string(tmp.path()).map_err(|source| Error::Io {
        path: tmp.path().to_path_buf(),
        source,
    })
}
