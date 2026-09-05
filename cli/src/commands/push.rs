use shaic_core::operations;

use crate::error::Result;

use super::open_store;

pub fn run(allow_secrets: bool, json: bool) -> Result<()> {
    let store = open_store()?;
    let result = store.push(allow_secrets)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }
    let (message, _) = operations::format_push(&result);
    println!("{message}");
    Ok(())
}
