use crate::error::Result;
use shaic_core::config::Config;
use shaic_core::store::Store;

pub fn run(remote: Option<String>) -> Result<()> {
    Store::init(Store::default_path(), remote.as_deref())?;

    let mut config = Config::load()?;
    if let Some(url) = &remote {
        config.set_remote(url)?;
        config.save()?;
    }

    match remote {
        Some(url) => println!(
            "store ready at {} (remote: {url})",
            Store::default_path().display()
        ),
        None => println!(
            "store ready at {} (no remote set yet — pass --remote or run `shaic init --remote <url>` later)",
            Store::default_path().display()
        ),
    }
    Ok(())
}
