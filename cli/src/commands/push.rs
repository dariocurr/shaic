use crate::error::Result;

use super::open_store;

pub fn run(allow_secrets: bool) -> Result<()> {
    let store = open_store()?;
    let result = store.push(allow_secrets)?;
    match (result.committed, result.summary) {
        (true, Some(summary)) => println!("pushed: {summary}"),
        _ => println!("nothing to push — store is clean"),
    }
    Ok(())
}
