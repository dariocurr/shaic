//! Stable exit codes for scripts and CI.

use shaic_core::Error as CoreError;

use crate::error::CliError;

#[allow(dead_code)]
pub const SUCCESS: i32 = 0;
#[allow(dead_code)]
pub const GENERAL: i32 = 1;
pub const USAGE: i32 = 2;
pub const GIT: i32 = 3;
pub const SECRET: i32 = 4;
pub const CONFIG: i32 = 5;

pub fn from_error(err: &CliError) -> i32 {
    match err {
        CliError::Message(_) => USAGE,
        CliError::Io(_) | CliError::Json(_) => GENERAL,
        CliError::Core(core) => from_core(core),
    }
}

fn from_core(err: &CoreError) -> i32 {
    match err {
        CoreError::SecretDetected(_) => SECRET,
        CoreError::Diverged { .. } | CoreError::UncommittedChanges { .. } | CoreError::Git(_) => {
            GIT
        }
        CoreError::StoreNotInitialized
        | CoreError::Config(_)
        | CoreError::InvalidRemote(_)
        | CoreError::RemoteAlreadySet { .. }
        | CoreError::SchemaTooNew { .. } => CONFIG,
        _ => GENERAL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaic_core::Error as CoreError;

    #[test]
    fn secret_scan_maps_to_secret_exit() {
        assert_eq!(from_core(&CoreError::SecretDetected("x".into())), SECRET);
    }

    #[test]
    fn diverged_maps_to_git_exit() {
        assert_eq!(
            from_core(&CoreError::Diverged {
                store: "/tmp/store".into()
            }),
            GIT
        );
    }
}
