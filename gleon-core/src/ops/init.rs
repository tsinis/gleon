//! Initialization operation for gleon workspace.

use crate::config::GleonConfig;
use crate::paths::GleonPaths;
use std::path::PathBuf;
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
            crate::io::IoError::Io(e) => Self::Io(e),
            crate::io::IoError::JsonParse(e) => Self::Io(std::io::Error::other(e)),
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
///
/// # Errors
///
/// Returns an error if the `.gleon` directory tree cannot be created, if the default
/// `gleon.yaml` configuration fails to serialize, or if writing the `.gitignore`,
/// `.env.template`, or `gleon.yaml` scaffold files fails.
#[allow(clippy::too_many_lines)] // TODO(C3): extract shared helpers into ops/common.rs
pub fn init_workspace(context: &crate::context::ResolvedContext) -> Result<InitResult, InitError> {
    use std::io::Write;

    let paths = GleonPaths::new(&context.base_dir);
    let gleon_dir = paths.gleon_dir();
    let blobs_dir = paths.blob_scheme_dir("sha256");
    let runs_dir = paths.runs_latest();

    std::fs::create_dir_all(&blobs_dir)?;
    std::fs::create_dir_all(&runs_dir)?;

    if let Ok(platform_key) = context.platform.to_key() {
        std::fs::create_dir_all(paths.manifests_dir(&platform_key))?;
    } else {
        std::fs::create_dir_all(paths.manifests_root())?;
    }

    // Scaffold .gleon/.gitignore idempotently to prevent committing blobs/ or runs/ artifacts
    let gitignore_path = paths.gitignore();
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
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&gitignore_path)?;
        if !prefix.is_empty() {
            file.write_all(prefix.as_bytes())?;
        }
        file.write_all(to_append.as_bytes())?;
    }

    // Scaffold .gleon/.env.template if it does not exist
    let env_template_path = gleon_dir.join(".env.template");
    let template_content = "# gleon Storage Configuration\n\
        # Copy this file to .env.local and fill in your credentials\n\
        GLEON_STORAGE_URL=\n\
        AWS_ACCESS_KEY_ID=\n\
        AWS_SECRET_ACCESS_KEY=\n\
        # For Cloudflare R2:\n\
        # R2_ACCOUNT_ID=\n";
    let env_create_res = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&env_template_path);
    match env_create_res {
        Ok(mut f) => {
            if let Err(e) = f
                .write_all(template_content.as_bytes())
                .and_then(|()| f.sync_all())
            {
                let _ = std::fs::remove_file(&env_template_path);
                return Err(InitError::Io(e));
            }
            #[cfg(not(windows))]
            if let Ok(d) = std::fs::File::open(&gleon_dir) {
                let _ = d.sync_all();
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(InitError::Io(e)),
    }

    let internal_config = paths.config_file();

    let mut config_created = None;
    let default_config = GleonConfig::default();
    let yaml_content = serde_yaml::to_string(&default_config).map_err(InitError::Yaml)?;

    let config_create_res = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&internal_config);
    match config_create_res {
        Ok(mut f) => {
            if let Err(e) = f
                .write_all(yaml_content.as_bytes())
                .and_then(|()| f.sync_all())
            {
                let _ = std::fs::remove_file(&internal_config);
                return Err(InitError::Io(e));
            }
            #[cfg(not(windows))]
            if let Ok(d) = std::fs::File::open(&gleon_dir) {
                let _ = d.sync_all();
            }
            config_created = Some(internal_config);
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(InitError::Io(e)),
    }

    Ok(InitResult {
        gleon_dir,
        config_created,
    })
}

#[cfg(all(test, not(miri)))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::pedantic,
    clippy::nursery
)]
mod tests {
    use super::*;
    use crate::context::ResolvedContext;

    #[test]
    fn test_init_error_from_io_error() {
        let err1 = crate::io::IoError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "foo"));
        let init_err1: InitError = err1.into();
        assert!(matches!(init_err1, InitError::Io(_)));

        let serde_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let err2 = crate::io::IoError::JsonParse(serde_err);
        let init_err2: InitError = err2.into();
        assert!(matches!(init_err2, InitError::Io(_)));
    }

    #[test]
    fn test_init_workspace_creates_structure_and_config() {
        let temp_dir = tempfile::tempdir().unwrap();
        let ctx = ResolvedContext {
            base_dir: temp_dir.path().to_path_buf(),
            ..ResolvedContext::default()
        };

        let res = init_workspace(&ctx).unwrap();
        assert!(res.gleon_dir.exists());
        assert!(res.gleon_dir.join(".gitignore").exists());
        let expected_config = res.gleon_dir.join("gleon.yaml");
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

    #[test]
    fn test_init_platform_key_error() {
        let temp = tempfile::tempdir().unwrap();
        let mut ctx = ResolvedContext {
            base_dir: temp.path().to_path_buf(),
            ..ResolvedContext::default()
        };
        ctx.platform.os = "invalid/os".to_string();

        let res = init_workspace(&ctx);
        assert!(res.is_ok());

        // manifests should be created without a platform sub-directory
        let manifests_dir = temp.path().join(".gleon").join("manifests");
        assert!(manifests_dir.exists());
    }

    #[test]
    fn test_init_gitignore_append_newline() {
        let temp = tempfile::tempdir().unwrap();
        let gleon_dir = temp.path().join(".gleon");
        std::fs::create_dir_all(&gleon_dir).unwrap();

        // Write .gitignore without trailing newline
        std::fs::write(gleon_dir.join(".gitignore"), "some_ignored_file").unwrap();

        let ctx = ResolvedContext {
            base_dir: temp.path().to_path_buf(),
            ..ResolvedContext::default()
        };
        let res = init_workspace(&ctx);
        assert!(res.is_ok());

        let content = std::fs::read_to_string(gleon_dir.join(".gitignore")).unwrap();
        assert!(content.contains("some_ignored_file\n"));
        assert!(content.contains("runs/"));
    }

    #[test]
    #[cfg(all(unix, not(miri)))]
    fn test_init_read_only_dir_error() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let gleon_dir = temp.path().join(".gleon");
        std::fs::create_dir_all(&gleon_dir).unwrap();

        // Make .gleon read-only so OpenOptions::create_new fails
        let mut perms = std::fs::metadata(&gleon_dir).unwrap().permissions();
        perms.set_mode(0o555);
        std::fs::set_permissions(&gleon_dir, perms.clone()).unwrap();

        let can_write = std::fs::write(gleon_dir.join("test.txt"), "data").is_ok();
        let ctx = ResolvedContext {
            base_dir: temp.path().to_path_buf(),
            ..ResolvedContext::default()
        };
        let res = init_workspace(&ctx);

        // Restore permissions before assertions
        perms.set_mode(0o755);
        std::fs::set_permissions(&gleon_dir, perms).unwrap();

        if !can_write {
            assert!(matches!(res, Err(InitError::Io(_))));
        }
    }
}
