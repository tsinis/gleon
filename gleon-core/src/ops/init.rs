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
    if !existing_content.lines().any(|l| l.trim() == ".env") {
        to_append.push_str(".env\n");
    }
    if !existing_content.lines().any(|l| l.trim() == ".env.local") {
        to_append.push_str(".env.local\n");
    }
    if !existing_content.lines().any(|l| l.trim() == "credentials") {
        to_append.push_str("credentials\n");
    }

    if !to_append.is_empty() {
        let prefix = if !existing_content.is_empty() && !existing_content.ends_with('\n') {
            "\n"
        } else {
            ""
        };
        let full_append = format!("{prefix}{to_append}");
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&gitignore_path)?;
        file.write_all(full_append.as_bytes())?;
    }

    // Scaffold .gleon/.env.template if it does not exist
    let env_template_path = gleon_dir.join(".env.template");
    if !env_template_path.exists() {
        let template_content = "# Gleon Storage Configuration\n\
            # Copy this file to .env.local and fill in your credentials\n\
            GLEON_STORAGE_URL=\n\
            AWS_ACCESS_KEY_ID=\n\
            AWS_SECRET_ACCESS_KEY=\n\
            # For Cloudflare R2:\n\
            # R2_ACCOUNT_ID=\n";
        crate::io::save_file_atomically(&env_template_path, template_content.as_bytes())
            .map_err(InitError::from)?;
    }

    let root_config = base_dir.join("gleon.yaml");
    let internal_config = gleon_dir.join("gleon.yaml");

    let mut config_created = None;
    if !internal_config.exists() && !root_config.exists() {
        let default_config = GleonConfig::default();
        let yaml_content = serde_yaml::to_string(&default_config).map_err(InitError::Yaml)?;
        crate::io::save_file_atomically(&root_config, yaml_content.as_bytes())
            .map_err(InitError::from)?;
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
        let expected_config = base_dir.join("gleon.yaml");
        assert_eq!(res.config_created, Some(expected_config.clone()));
        assert!(expected_config.exists());

        let gitignore = std::fs::read_to_string(res.gleon_dir.join(".gitignore")).unwrap();
        assert!(gitignore.contains("blobs/"));
        assert!(gitignore.contains("runs/"));
        assert!(gitignore.contains(".env.local"));
        assert!(gitignore.contains("credentials"));

        let env_template = res.gleon_dir.join(".env.template");
        assert!(env_template.exists());
        let template_str = std::fs::read_to_string(env_template).unwrap();
        assert!(template_str.contains("GLEON_STORAGE_URL="));
    }
}
