use crate::error::Result;

use super::open_store;

pub fn run() -> Result<()> {
    let store = open_store()?;
    let result = store.pull()?;
    if !result.updated {
        println!("already up to date");
        return Ok(());
    }
    println!("pulled changes:");
    if let Some(stat) = result.diff_stat {
        println!("{stat}");
    }
    Ok(())
}
