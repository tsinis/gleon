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
    match dotenvy::from_path(&env_local) {
        Ok(()) => {
            debug!("Loaded environment file: {}", env_local.display());
            count += 1;
        }
        Err(dotenvy::Error::Io(ref e)) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            debug!(
                "Failed to load environment file {}: {}",
                env_local.display(),
                e
            );
        }
    }

    // Load .env second (provides defaults for unset variables)
    match dotenvy::from_path(&env_shared) {
        Ok(()) => {
            debug!("Loaded environment file: {}", env_shared.display());
            count += 1;
        }
        Err(dotenvy::Error::Io(ref e)) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            debug!(
                "Failed to load environment file {}: {}",
                env_shared.display(),
                e
            );
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

    #[test]
    fn test_load_dotenv_valid_and_corrupt_files() {
        let temp = tempdir().unwrap();
        let gleon_dir = temp.path().join(".gleon");
        std::fs::create_dir_all(&gleon_dir).unwrap();

        // 1. Valid .env and .env.local
        std::fs::write(gleon_dir.join(".env"), "TEST_VAR_ENV=1\n").unwrap();
        std::fs::write(gleon_dir.join(".env.local"), "TEST_VAR_LOCAL=1\n").unwrap();
        assert_eq!(load_dotenv(temp.path()), 2);

        // 2. Corrupt .env and .env.local (invalid syntax to hit error branches)
        std::fs::write(gleon_dir.join(".env"), "INVALID_LINE_WITHOUT_EQUALS\n").unwrap();
        std::fs::write(
            gleon_dir.join(".env.local"),
            "INVALID_LINE_WITHOUT_EQUALS\n",
        )
        .unwrap();
        assert_eq!(load_dotenv(temp.path()), 0);
    }
}
