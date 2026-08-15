use crate::error::Result;
use crate::self_update;

pub fn run_check() -> Result<()> {
    eprintln!("checking GitHub Releases for updates…");
    match self_update::check_update() {
        Ok(status) if status.update_available => {
            println!(
                "update available: {} → {} (run `shaic self update`)",
                status.current, status.latest
            );
        }
        Ok(status) => {
            println!("shaic {} is up to date", status.current);
        }
        Err(e) => {
            println!("could not check for updates: {e}");
        }
    }
    Ok(())
}

pub fn run_update(yes: bool) -> Result<()> {
    self_update::run_update(yes)
}
