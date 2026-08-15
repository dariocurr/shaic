use crate::error::{CliError, Result};

pub fn run() -> Result<()> {
    shaic_tui::run().map_err(|e| CliError::Message(e.to_string()))
}
