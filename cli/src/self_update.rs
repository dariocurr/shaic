//! Check for and install GitHub Release binaries. Uses `curl` on Unix and
//! PowerShell on Windows — no extra HTTP crate in the dependency tree.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::error::{Context, Result, bail};

const REPO: &str = "dariocurr/shaic";
const USER_AGENT: &str = "shaic-self-update";

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// GitHub Release asset triple for this build, if we publish one.
pub fn release_target() -> Option<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("x86_64-unknown-linux-gnu")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("aarch64-unknown-linux-gnu")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("aarch64-apple-darwin")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("x86_64-apple-darwin")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("x86_64-pc-windows-msvc")
    } else if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
        Some("aarch64-pc-windows-msvc")
    } else {
        None
    }
}

pub struct UpdateStatus {
    pub current: String,
    pub latest: String,
    pub update_available: bool,
}

pub fn check_update() -> Result<UpdateStatus> {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let release = fetch_latest_release()?;
    let latest = release.tag_name.trim_start_matches('v').to_string();
    Ok(UpdateStatus {
        update_available: version_gt(&latest, &current),
        current,
        latest: release.tag_name,
    })
}

pub fn run_update(yes: bool) -> Result<()> {
    let target = release_target().ok_or_else(|| {
        crate::error::CliError::Message(
            "no prebuilt release for this platform — install from source with `cargo install shaic`"
                .to_string(),
        )
    })?;

    let status = check_update()?;
    if !status.update_available {
        println!("shaic {} is up to date", status.current);
        return Ok(());
    }

    println!("update available: {} → {}", status.current, status.latest);

    eprintln!("downloading release assets…");

    if !yes {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() {
            bail!("stdin is not a terminal — pass --yes to confirm non-interactively");
        }
        print!("install {}? [y/N] ", status.latest);
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !matches!(input.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("cancelled");
            return Ok(());
        }
    }

    let release = fetch_latest_release()?;
    let archive_name = format!("shaic-{target}.{}", archive_ext());
    let checksum_name = format!("{archive_name}.sha256");
    let archive_url = find_asset_url(&release, &archive_name)?;
    let checksum_url = find_asset_url(&release, &checksum_name)?;

    let tmp = tempfile::tempdir().context("create temp dir for download")?;
    let archive_path = tmp.path().join(&archive_name);
    let checksum_path = tmp.path().join(&checksum_name);

    download_to(&checksum_url, &checksum_path)?;
    download_to(&archive_url, &archive_path)?;

    verify_checksum(&checksum_path, &archive_path)?;

    let extracted = extract_archive(&archive_path, tmp.path())?;
    let new_bin = find_binary(&extracted)?;
    replace_current_executable(&new_bin)?;

    println!("updated to {}", status.latest);
    Ok(())
}

fn archive_ext() -> &'static str {
    if cfg!(target_os = "windows") {
        "zip"
    } else {
        "tar.gz"
    }
}

fn fetch_latest_release() -> Result<Release> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = http_get_text(&url)?;
    serde_json::from_slice(&body).context("parse GitHub release JSON")
}

fn find_asset_url(release: &Release, name: &str) -> Result<String> {
    release
        .assets
        .iter()
        .find(|a| a.name == name)
        .map(|a| a.browser_download_url.clone())
        .ok_or_else(|| {
            crate::error::CliError::Message(format!(
                "release {} has no asset {name}",
                release.tag_name
            ))
        })
}

fn verify_checksum(checksum_file: &Path, archive: &Path) -> Result<()> {
    let line = fs::read_to_string(checksum_file).context("read checksum file")?;
    let expected = parse_sha256_line(&line).ok_or_else(|| {
        crate::error::CliError::Message(format!("could not parse checksum line: {}", line.trim()))
    })?;

    use sha2::{Digest, Sha256};
    let bytes = fs::read(archive).context("read downloaded archive")?;
    let digest = format!("{:x}", Sha256::digest(bytes));
    if digest != expected {
        bail!(
            "checksum mismatch for {} (expected {expected}, got {digest})",
            archive.display()
        );
    }
    Ok(())
}

fn parse_sha256_line(line: &str) -> Option<String> {
    let token = line.split_whitespace().next()?;
    if token.len() == 64 && token.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(token.to_ascii_lowercase())
    } else {
        None
    }
}

fn version_gt(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u32> {
        s.trim_start_matches('v')
            .split('.')
            .filter_map(|p| p.parse().ok())
            .collect()
    };
    let latest = parse(latest);
    let current = parse(current);
    let n = latest.len().max(current.len());
    for i in 0..n {
        let l = latest.get(i).copied().unwrap_or(0);
        let c = current.get(i).copied().unwrap_or(0);
        if l != c {
            return l > c;
        }
    }
    false
}

fn extract_archive(archive: &Path, dest: &Path) -> Result<PathBuf> {
    if cfg!(target_os = "windows") {
        let status = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
                    archive.display(),
                    dest.display()
                ),
            ])
            .status()
            .context("spawn powershell to extract zip")?;
        if !status.success() {
            bail!("failed to extract {}", archive.display());
        }
    } else {
        let status = Command::new("tar")
            .args(["-xzf"])
            .arg(archive)
            .arg("-C")
            .arg(dest)
            .status()
            .context("spawn tar to extract archive")?;
        if !status.success() {
            bail!("failed to extract {}", archive.display());
        }
    }
    Ok(dest.to_path_buf())
}

fn find_binary(extracted_dir: &Path) -> Result<PathBuf> {
    let name = if cfg!(windows) { "shaic.exe" } else { "shaic" };
    let direct = extracted_dir.join(name);
    if direct.is_file() {
        return Ok(direct);
    }
    for entry in fs::read_dir(extracted_dir).context("read extract dir")? {
        let entry = entry.context("read extract dir entry")?;
        let path = entry.path();
        if path.file_name().is_some_and(|n| n == name) && path.is_file() {
            return Ok(path);
        }
    }
    bail!("extracted archive did not contain {name}");
}

fn replace_current_executable(new_bin: &Path) -> Result<()> {
    let current = std::env::current_exe().context("resolve current executable path")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(mode) = fs::metadata(new_bin).map(|m| m.permissions().mode()) {
            fs::set_permissions(new_bin, fs::Permissions::from_mode(mode | 0o111))
                .context("mark new binary executable")?;
        }
        if fs::rename(new_bin, &current).is_err() {
            fs::copy(new_bin, &current).context("copy new binary over current")?;
        }
    }

    #[cfg(windows)]
    {
        let backup = current.with_extension("exe.old");
        let _ = fs::remove_file(&backup);
        if fs::rename(&current, &backup).is_ok() {
            if fs::rename(new_bin, &current).is_err() {
                let _ = fs::rename(&backup, &current);
                bail!(
                    "could not replace running executable — close other shaic processes and retry"
                );
            }
            let _ = fs::remove_file(backup);
        } else {
            fs::copy(new_bin, &current).context("copy new binary over current")?;
        }
    }

    Ok(())
}

fn http_get_text(url: &str) -> Result<Vec<u8>> {
    #[cfg(unix)]
    {
        let output = Command::new("curl")
            .args(["-fsSL", "-H", &format!("User-Agent: {USER_AGENT}"), url])
            .output()
            .context("spawn curl")?;
        if !output.status.success() {
            bail!(
                "download failed (curl exit {})",
                output.status.code().unwrap_or(-1)
            );
        }
        Ok(output.stdout)
    }

    #[cfg(windows)]
    {
        let tmp = tempfile::NamedTempFile::new().context("create temp file for download")?;
        let path = tmp.path().to_string_lossy();
        let script = format!(
            "Invoke-WebRequest -Uri '{url}' -OutFile '{path}' -Headers @{{ 'User-Agent' = '{USER_AGENT}' }} -UseBasicParsing"
        );
        let status = Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .status()
            .context("spawn powershell for download")?;
        if !status.success() {
            bail!(
                "download failed (powershell exit {})",
                status.code().unwrap_or(-1)
            );
        }
        fs::read(tmp.path()).context("read downloaded file")
    }
}

fn download_to(url: &str, dest: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let bytes = http_get_text(url)?;
        fs::write(dest, bytes).context(format!("write {}", dest.display()))
    }

    #[cfg(windows)]
    {
        let dest_str = dest.to_string_lossy();
        let script = format!(
            "Invoke-WebRequest -Uri '{url}' -OutFile '{dest_str}' -Headers @{{ 'User-Agent' = '{USER_AGENT}' }} -UseBasicParsing"
        );
        let status = Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .status()
            .context("spawn powershell for download")?;
        if !status.success() {
            bail!(
                "download failed (powershell exit {})",
                status.code().unwrap_or(-1)
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_gt_works() {
        assert!(version_gt("0.1.1", "0.1.0"));
        assert!(version_gt("1.0.0", "0.9.9"));
        assert!(!version_gt("0.1.0", "0.1.0"));
        assert!(!version_gt("0.0.9", "0.1.0"));
    }

    #[test]
    fn parse_sha256_line_works() {
        let hash = "a".repeat(64);
        assert_eq!(
            parse_sha256_line(&format!("{hash}  shaic.tar.gz")),
            Some(hash)
        );
    }
}
