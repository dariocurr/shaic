use crate::error::Result;

use super::open_store;

pub fn run(allow_secrets: bool, json: bool) -> Result<()> {
    let store = open_store()?;
    let result = store.pull(allow_secrets)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }
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
