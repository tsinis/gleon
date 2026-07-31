//! Environment variable file loading helpers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::debug;

fn find_gleon_dir(base_dir: &Path) -> PathBuf {
    let mut current = base_dir.to_path_buf();
    loop {
        let candidate = current.join(".gleon");
        if candidate.is_dir() {
            return candidate;
        }
        if !current.pop() {
            break;
        }
    }
    base_dir.join(".gleon")
}

/// Parses `.gleon/.env` and `.gleon/.env.local` into a key-value map.
///
/// Precedence order: `.env.local` keys override `.env` keys.
/// Does NOT mutate the process-global environment.
pub fn load_dotenv(base_dir: &Path) -> HashMap<String, String> {
    let gleon_dir = find_gleon_dir(base_dir);
    let env_shared = gleon_dir.join(".env");
    let env_local = gleon_dir.join(".env.local");

    let mut map = HashMap::new();

    // 1. Read shared .env first
    match dotenvy::from_path_iter(&env_shared) {
        Ok(iter) => {
            debug!("Parsed environment file: {}", env_shared.display());
            for (k, v) in iter.flatten() {
                map.insert(k, v);
            }
        }
        Err(e) => {
            debug!("Skipping environment file {}: {}", env_shared.display(), e);
        }
    }

    // 2. Read local .env.local second (overwriting shared keys)
    match dotenvy::from_path_iter(&env_local) {
        Ok(iter) => {
            debug!("Parsed environment file: {}", env_local.display());
            for (k, v) in iter.flatten() {
                map.insert(k, v);
            }
        }
        Err(e) => {
            debug!("Skipping environment file {}: {}", env_local.display(), e);
        }
    }

    map
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_load_dotenv_missing_dir() {
        let temp = tempdir().unwrap();
        assert!(load_dotenv(temp.path()).is_empty());
    }

    #[test]
    fn test_load_dotenv_valid_and_corrupt_files() {
        let temp = tempdir().unwrap();
        let gleon_dir = temp.path().join(".gleon");
        std::fs::create_dir_all(&gleon_dir).unwrap();

        // 1. Valid .env and .env.local with shared variable to test precedence
        std::fs::write(
            gleon_dir.join(".env"),
            "TEST_VAR_ENV=1\nTEST_SHARED=from_env\n",
        )
        .unwrap();
        std::fs::write(
            gleon_dir.join(".env.local"),
            "TEST_VAR_LOCAL=1\nTEST_SHARED=from_local\n",
        )
        .unwrap();

        let env_map = load_dotenv(temp.path());
        assert_eq!(env_map.get("TEST_VAR_ENV").map(String::as_str), Some("1"));
        assert_eq!(env_map.get("TEST_VAR_LOCAL").map(String::as_str), Some("1"));
        assert_eq!(
            env_map.get("TEST_SHARED").map(String::as_str),
            Some("from_local")
        );

        // 2. Corrupt .env and .env.local (invalid syntax)
        std::fs::write(gleon_dir.join(".env"), "INVALID_LINE_WITHOUT_EQUALS\n").unwrap();
        std::fs::write(
            gleon_dir.join(".env.local"),
            "INVALID_LINE_WITHOUT_EQUALS\n",
        )
        .unwrap();
        let corrupt_map = load_dotenv(temp.path());
        assert!(corrupt_map.is_empty());
    }
}
