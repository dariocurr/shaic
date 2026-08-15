use std::process::Command;

use serde::Serialize;

use crate::error::Result;
use crate::self_update;
use shaic_core::adapters;
use shaic_core::config::Config;
use shaic_core::store::Store;

#[derive(Serialize)]
struct DoctorReport {
    version: &'static str,
    platform: &'static str,
    release_target: Option<&'static str>,
    config_path: String,
    checks: Vec<Check>,
}

#[derive(Serialize)]
struct Check {
    name: &'static str,
    level: &'static str,
    message: String,
}

pub fn run(json: bool) -> Result<()> {
    if json {
        return run_json();
    }
    run_human()
}

fn run_human() -> Result<()> {
    println!("shaic doctor");

    #[cfg(unix)]
    {
        // SAFETY: geteuid() has no preconditions and cannot fail.
        let euid = unsafe { libc::geteuid() };
        if euid == 0 {
            println!("  [warn] running as root — shaic should run as an unprivileged user");
        } else {
            println!("  [ok] running unprivileged");
        }
    }

    #[cfg(windows)]
    {
        println!("  [ok] platform: windows");
    }

    if let Some(target) = self_update::release_target() {
        println!("  [ok] release target: {target}");
    } else {
        println!("  [info] no prebuilt release for this platform — use `cargo install shaic`");
    }

    match Command::new("git").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            println!("  [ok] {version}");
        }
        _ => println!("  [warn] git not found on PATH — init/push/pull need git"),
    }

    match Store::default_path() {
        Err(e) => println!("  [warn] {e}"),
        Ok(store_path) => match Store::open(&store_path) {
            Ok(store) => {
                println!("  [ok] store present at {}", store_path.display());
                match store.check_schema() {
                    Ok(()) => println!("  [ok] store schema is readable"),
                    Err(e) => println!("  [warn] store schema: {e}"),
                }
                match shaic_core::store::git::status_porcelain(store.root()) {
                    Ok(status) if status.trim().is_empty() => println!("  [ok] store is clean"),
                    Ok(_) => println!("  [warn] store has uncommitted changes — run `shaic push`"),
                    Err(e) => println!("  [warn] could not read store status: {e}"),
                }
            }
            Err(_) => println!("  [warn] no store yet — run `shaic init`"),
        },
    }

    match Config::load() {
        Ok(config) => match &config.remote {
            Some(url) => println!(
                "  [ok] remote configured: {}",
                shaic_core::store::git::redact_userinfo(url)
            ),
            None => println!("  [warn] no remote configured — run `shaic init --remote <url>`"),
        },
        Err(e) => println!("  [warn] could not read config: {e}"),
    }

    println!(
        "  [info] config file: {}",
        Config::path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|e| format!("unavailable ({e})"))
    );

    match self_update::check_update() {
        Ok(status) if status.update_available => {
            println!(
                "  [info] update available: {} → {} (`shaic self update`)",
                status.current, status.latest
            );
        }
        Ok(status) => {
            println!("  [ok] running latest release ({})", status.current);
        }
        Err(e) => println!("  [info] could not check for updates: {e}"),
    }

    for &agent in adapters::registry() {
        if agent.experimental_read_only() {
            println!(
                "  [info] {} convention is unconfirmed — read-only in this version",
                agent.display_name()
            );
        }
    }

    Ok(())
}

fn run_json() -> Result<()> {
    let mut checks = Vec::new();

    #[cfg(unix)]
    {
        // SAFETY: geteuid() has no preconditions and cannot fail.
        let euid = unsafe { libc::geteuid() };
        if euid == 0 {
            checks.push(Check {
                name: "privilege",
                level: "warn",
                message: "running as root — shaic should run as an unprivileged user".to_string(),
            });
        } else {
            checks.push(Check {
                name: "privilege",
                level: "ok",
                message: "running unprivileged".to_string(),
            });
        }
    }

    #[cfg(windows)]
    {
        checks.push(Check {
            name: "platform",
            level: "ok",
            message: "windows".to_string(),
        });
    }

    if let Some(target) = self_update::release_target() {
        checks.push(Check {
            name: "release_target",
            level: "ok",
            message: target.to_string(),
        });
    } else {
        checks.push(Check {
            name: "release_target",
            level: "info",
            message: "no prebuilt release for this platform — use `cargo install shaic`"
                .to_string(),
        });
    }

    match Command::new("git").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            checks.push(Check {
                name: "git",
                level: "ok",
                message: version,
            });
        }
        _ => checks.push(Check {
            name: "git",
            level: "warn",
            message: "git not found on PATH — init/push/pull need git".to_string(),
        }),
    }

    match Store::default_path() {
        Err(e) => checks.push(Check {
            name: "store",
            level: "warn",
            message: e.to_string(),
        }),
        Ok(store_path) => match Store::open(&store_path) {
            Ok(store) => {
                checks.push(Check {
                    name: "store",
                    level: "ok",
                    message: store_path.display().to_string(),
                });
                match store.check_schema() {
                    Ok(()) => checks.push(Check {
                        name: "store_schema",
                        level: "ok",
                        message: "readable".to_string(),
                    }),
                    Err(e) => checks.push(Check {
                        name: "store_schema",
                        level: "warn",
                        message: e.to_string(),
                    }),
                }
                match shaic_core::store::git::status_porcelain(store.root()) {
                    Ok(status) if status.trim().is_empty() => checks.push(Check {
                        name: "store_clean",
                        level: "ok",
                        message: "clean".to_string(),
                    }),
                    Ok(_) => checks.push(Check {
                        name: "store_clean",
                        level: "warn",
                        message: "uncommitted changes — run `shaic push`".to_string(),
                    }),
                    Err(e) => checks.push(Check {
                        name: "store_clean",
                        level: "warn",
                        message: e.to_string(),
                    }),
                }
            }
            Err(_) => checks.push(Check {
                name: "store",
                level: "warn",
                message: "no store yet — run `shaic init`".to_string(),
            }),
        },
    }

    match Config::load() {
        Ok(config) => match &config.remote {
            Some(url) => checks.push(Check {
                name: "remote",
                level: "ok",
                message: shaic_core::store::git::redact_userinfo(url),
            }),
            None => checks.push(Check {
                name: "remote",
                level: "warn",
                message: "no remote configured — run `shaic init --remote <url>`".to_string(),
            }),
        },
        Err(e) => checks.push(Check {
            name: "config",
            level: "warn",
            message: e.to_string(),
        }),
    }

    match self_update::check_update() {
        Ok(status) if status.update_available => checks.push(Check {
            name: "update",
            level: "info",
            message: format!("update available: {} → {}", status.current, status.latest),
        }),
        Ok(status) => checks.push(Check {
            name: "update",
            level: "ok",
            message: format!("running latest release ({})", status.current),
        }),
        Err(e) => checks.push(Check {
            name: "update",
            level: "info",
            message: format!("could not check for updates: {e}"),
        }),
    }

    for &agent in adapters::registry() {
        if agent.experimental_read_only() {
            checks.push(Check {
                name: "agent",
                level: "info",
                message: format!(
                    "{} convention is unconfirmed — read-only in this version",
                    agent.display_name()
                ),
            });
        }
    }

    let report = DoctorReport {
        version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
        release_target: self_update::release_target(),
        config_path: Config::path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|e| format!("unavailable ({e})")),
        checks,
    };

    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
