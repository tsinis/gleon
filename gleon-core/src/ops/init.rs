//! Initialization operation for gleon workspace.

use crate::config::GleonConfig;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors that can occur during workspace initialization.
#[derive(Debug, Error)]
pub enum InitError {
    /// IO error during directory or file creation.
    #[error("IO error during initialization: {0}")]
    Io(#[from] std::io::Error),

    /// YAML serialization error when writing default config.
    #[error("YAML serialization error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    /// Manifest error during scaffolding.
    #[error("Manifest error: {0}")]
    Manifest(#[from] crate::manifest::ManifestError),
}

impl From<crate::io::IoError> for InitError {
    fn from(err: crate::io::IoError) -> Self {
        match err {
            crate::io::IoError::Io(e) => InitError::Io(e),
            crate::io::IoError::JsonParse(e) => InitError::Io(std::io::Error::other(e)),
        }
    }
}

/// Result summary of workspace initialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitResult {
    /// Path to the `.gleon` directory.
    pub gleon_dir: PathBuf,
    /// Path to the created `gleon.yaml` config file, if created.
    pub config_created: Option<PathBuf>,
}

/// Initializes the `.gleon` directory structure and default `gleon.yaml` if missing.
pub fn init_workspace(
    context: &crate::context::ResolvedContext,
    base_dir: &Path,
) -> Result<InitResult, InitError> {
    let gleon_dir = base_dir.join(".gleon");
    let blobs_dir = gleon_dir.join("blobs").join("sha256");
    let runs_dir = gleon_dir.join("runs").join("latest");

    std::fs::create_dir_all(&blobs_dir)?;
    std::fs::create_dir_all(&runs_dir)?;

    if let Ok(platform_key) = context.platform.to_key() {
        let manifest_dir = gleon_dir.join("manifests").join(platform_key);
        std::fs::create_dir_all(&manifest_dir)?;
    } else {
        let manifest_dir = gleon_dir.join("manifests");
        std::fs::create_dir_all(&manifest_dir)?;
    }

    // Scaffold .gleon/.gitignore idempotently to prevent committing blobs/ or runs/ artifacts
    let gitignore_path = gleon_dir.join(".gitignore");
    let existing_content = std::fs::read_to_string(&gitignore_path).unwrap_or_default();
    let mut to_append = String::new();

    if !existing_content.lines().any(|l| l.trim() == "blobs/") {
        to_append.push_str("blobs/\n");
    }
    if !existing_content.lines().any(|l| l.trim() == "runs/") {
        to_append.push_str("runs/\n");
    }

    if !to_append.is_empty() {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&gitignore_path)?;
        file.write_all(to_append.as_bytes())?;
    }

    let root_config = base_dir.join("gleon.yaml");
    let internal_config = gleon_dir.join("gleon.yaml");

    let mut config_created = None;
    if !internal_config.exists() && !root_config.exists() {
        let default_config = GleonConfig::default();
        let yaml_content = serde_yaml::to_string(&default_config)?;
        crate::io::save_file_atomically(&root_config, yaml_content.as_bytes())?;
        config_created = Some(root_config);
    }

    Ok(InitResult {
        gleon_dir,
        config_created,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_workspace_creates_structure_and_config() {
        let temp_dir = tempfile::tempdir().unwrap();
        let base_dir = temp_dir.path();
        let ctx = crate::context::ResolvedContext::default();

        let res = init_workspace(&ctx, base_dir).unwrap();
        assert!(res.gleon_dir.exists());
        assert!(res.gleon_dir.join(".gitignore").exists());

        let gitignore = std::fs::read_to_string(res.gleon_dir.join(".gitignore")).unwrap();
        assert!(gitignore.contains("blobs/"));
        assert!(gitignore.contains("runs/"));
    }
}
