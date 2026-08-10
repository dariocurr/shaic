#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error(transparent)]
    Core(#[from] shaic_core::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Message(String),
}

pub type Result<T> = std::result::Result<T, CliError>;

/// A minimal, dependency-free stand-in for `anyhow::Context`: attach a
/// human-readable "while doing X" prefix to any error on its way out.
pub trait Context<T> {
    fn context(self, msg: impl std::fmt::Display) -> Result<T>;
}

impl<T, E: std::fmt::Display> Context<T> for std::result::Result<T, E> {
    fn context(self, msg: impl std::fmt::Display) -> Result<T> {
        self.map_err(|e| CliError::Message(format!("{msg}: {e}")))
    }
}

/// A minimal, dependency-free stand-in for `anyhow::bail!`.
macro_rules! bail {
    ($($arg:tt)*) => {
        return Err($crate::error::CliError::Message(format!($($arg)*)))
    };
}
pub(crate) use bail;
