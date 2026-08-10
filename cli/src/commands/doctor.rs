use crate::error::Result;
use shaic_core::adapters;
use shaic_core::config::Config;
use shaic_core::store::Store;

pub fn run() -> Result<()> {
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

    let store_path = Store::default_path();
    match Store::open(&store_path) {
        Ok(store) => {
            println!("  [ok] store present at {}", store_path.display());
            match shaic_core::store::git::status_porcelain(store.root()) {
                Ok(status) if status.trim().is_empty() => println!("  [ok] store is clean"),
                Ok(_) => println!("  [warn] store has uncommitted changes — run `shaic push`"),
                Err(e) => println!("  [warn] could not read store status: {e}"),
            }
        }
        Err(_) => println!("  [warn] no store yet — run `shaic init`"),
    }

    match Config::load() {
        Ok(config) => match &config.remote {
            Some(url) => println!("  [ok] remote configured: {url}"),
            None => println!("  [warn] no remote configured — run `shaic init --remote <url>`"),
        },
        Err(e) => println!("  [warn] could not read config: {e}"),
    }

    for agent in adapters::registry() {
        if agent.experimental_read_only() {
            println!(
                "  [info] {} convention is unconfirmed — read-only in this version",
                agent.display_name()
            );
        }
    }

    Ok(())
}
