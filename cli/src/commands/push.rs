use crate::error::Result;

use super::open_store;

pub fn run(allow_secrets: bool, json: bool) -> Result<()> {
    let store = open_store()?;
    let result = store.push(allow_secrets)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }
    match (result.pushed, result.committed, result.summary) {
        (true, true, Some(summary)) => println!("pushed: {summary}"),
        (true, false, Some(summary)) => println!("pushed previously-unpushed commits: {summary}"),
        (true, _, None) => println!("pushed"),
        (false, _, _) => println!("nothing to push — store is clean"),
    }
    Ok(())
}
