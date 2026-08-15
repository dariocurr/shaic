use crate::error::Result;
use shaic_core::config::Config;
use shaic_core::store::Store;

pub fn run(remote: Option<String>, force: bool) -> Result<()> {
    let store_path = Store::default_path()?;
    Store::init_with_force(&store_path, remote.as_deref(), force)?;

    let mut config = Config::load()?;
    if let Some(url) = &remote {
        config.set_remote(url)?;
        config.save()?;
    }

    match remote {
        Some(url) => println!(
            "store ready at {} (remote: {})",
            store_path.display(),
            shaic_core::store::git::redact_userinfo(&url)
        ),
        None => println!(
            "store ready at {} (no remote set yet — pass --remote or run `shaic init --remote <url>` later)",
            store_path.display()
        ),
    }
    Ok(())
}
