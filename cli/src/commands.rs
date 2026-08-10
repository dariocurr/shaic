pub mod agents;
pub mod doctor;
pub mod init;
pub mod item;
pub mod mcp;
pub mod project;
pub mod pull;
pub mod push;
pub mod status;
pub mod sync;
pub mod tui;

use std::path::PathBuf;

use shaic_core::store::Store;

use crate::error::{Context, Result};

pub fn current_project_root() -> Result<PathBuf> {
    std::env::current_dir().context("could not determine current directory")
}

pub fn open_store() -> Result<Store> {
    Store::open(Store::default_path()).context("store not initialized — run `shaic init` first")
}
