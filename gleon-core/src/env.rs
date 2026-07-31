//! Environment variable file loading helpers.

use std::path::Path;
use tracing::debug;

/// Loads `.gleon/.env.local` and `.gleon/.env` from `base_dir`.
///
/// Precedence order (highest to lowest):
/// 1. Pre-existing system environment variables.
/// 2. `.gleon/.env.local` (local developer secrets).
/// 3. `.gleon/.env` (shared configuration).
///
/// Returns the total number of environment files successfully loaded.
pub fn load_dotenv(base_dir: &Path) -> usize {
    let root_dir = crate::context::find_config_and_root(base_dir)
        .map(|(_, r)| r)
        .unwrap_or_else(|| base_dir.to_path_buf());

    let gleon_dir = root_dir.join(".gleon");
    let env_local = gleon_dir.join(".env.local");
    let env_shared = gleon_dir.join(".env");

    let mut count = 0;

    // Load .env.local first (overrides .env)
    if env_local.is_file() {
        match dotenvy::from_path(&env_local) {
            Ok(()) => {
                debug!("Loaded environment file: {}", env_local.display());
                count += 1;
            }
            Err(e) => {
                debug!(
                    "Failed to load environment file {}: {}",
                    env_local.display(),
                    e
                );
            }
        }
    }

    // Load .env second (provides defaults for unset variables)
    if env_shared.is_file() {
        match dotenvy::from_path(&env_shared) {
            Ok(()) => {
                debug!("Loaded environment file: {}", env_shared.display());
                count += 1;
            }
            Err(e) => {
                debug!(
                    "Failed to load environment file {}: {}",
                    env_shared.display(),
                    e
                );
            }
        }
    }

    count
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_load_dotenv_missing_dir() {
        let temp = tempdir().unwrap();
        assert_eq!(load_dotenv(temp.path()), 0);
    }
}
