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
    let branches_dir = gleon_dir.join("branches");
    let runs_dir = gleon_dir.join("runs").join("latest");

    std::fs::create_dir_all(&blobs_dir)?;
    std::fs::create_dir_all(&branches_dir)?;
    std::fs::create_dir_all(&runs_dir)?;

    // Scaffold .gleon/.gitignore to prevent committing runs/ artifacts
    let gitignore_path = gleon_dir.join(".gitignore");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&gitignore_path)
    {
        use std::io::Write;
        let _ = file.write_all(b"runs/\n");
    }

    // Scaffold default manifest_index.json for current branch
    if let Ok(platform_key) = context.platform.to_key() {
        let branch_dir = branches_dir.join(&context.branch).join(&platform_key);
        let index_path = branch_dir.join("manifest_index.json");
        std::fs::create_dir_all(&branch_dir)?;
        let _ = crate::manifest::ManifestIndex::update(&index_path, |_| {});
    }

    let root_config = base_dir.join("gleon.yaml");
    let internal_config = gleon_dir.join("gleon.yaml");

    let mut config_created = None;
    // Check internal config first to avoid scaffolding root if internal exists.
    let root_file = if !internal_config.exists() {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&root_config)
            .ok()
    } else {
        None
    };

    if let Some(mut file) = root_file {
        let default_config = GleonConfig::default();
        let yaml_content = serde_yaml::to_string(&default_config)?;
        use std::io::Write;
        file.write_all(yaml_content.as_bytes())
            .map_err(InitError::Io)?;
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
        let base_path = temp_dir.path();

        let ctx = crate::context::ResolvedContext {
            branch: "main".to_string(),
            platform: crate::platform::PlatformInfo {
                os: "macos".to_string(),
                arch: Some("aarch64".to_string()),
                renderer: None,
                labels: std::collections::BTreeMap::new(),
            },
            config: None,
            target_branch: "main".to_string(),
            base_dir: base_path.to_path_buf(),
        };

        let res = init_workspace(&ctx, base_path).unwrap();

        assert_eq!(res.gleon_dir, base_path.join(".gleon"));
        assert_eq!(res.config_created, Some(base_path.join("gleon.yaml")));

        assert!(base_path.join(".gleon/blobs/sha256").is_dir());
        assert!(base_path.join(".gleon/branches").is_dir());
        assert!(base_path.join(".gleon/runs/latest").is_dir());
        assert!(base_path.join(".gleon/.gitignore").is_file());
        assert_eq!(
            std::fs::read_to_string(base_path.join(".gleon/.gitignore")).unwrap(),
            "runs/\n"
        );
        assert!(base_path.join("gleon.yaml").is_file());
    }

    #[test]
    fn test_init_workspace_skips_config_if_exists() {
        let temp_dir = tempfile::tempdir().unwrap();
        let base_path = temp_dir.path();

        let root_config = base_path.join("gleon.yaml");
        std::fs::write(&root_config, "custom: config").unwrap();

        let ctx = crate::context::ResolvedContext {
            branch: "main".to_string(),
            platform: crate::platform::PlatformInfo {
                os: "macos".to_string(),
                arch: Some("aarch64".to_string()),
                renderer: None,
                labels: std::collections::BTreeMap::new(),
            },
            config: None,
            target_branch: "main".to_string(),
            base_dir: base_path.to_path_buf(),
        };

        let res = init_workspace(&ctx, base_path).unwrap();

        assert_eq!(res.config_created, None);
        assert_eq!(
            std::fs::read_to_string(&root_config).unwrap(),
            "custom: config"
        );
    }

    #[test]
    fn test_init_error_display() {
        let err1 = InitError::Io(std::io::Error::other("io test"));
        assert!(err1.to_string().contains("IO error"));

        let serde_err = serde_yaml::from_str::<GleonConfig>("invalid: yaml:").unwrap_err();
        let err2 = InitError::Yaml(serde_err);
        assert!(err2.to_string().contains("YAML serialization error"));

        let err3 = InitError::Manifest(crate::manifest::ManifestError::Validation(
            "bad manifest".to_string(),
        ));
        assert!(err3.to_string().contains("Manifest error"));
    }
}
