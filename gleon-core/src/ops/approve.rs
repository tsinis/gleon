//! Baseline approval operation for replacing snapshots with actual run images.

use crate::config::ConfigError;
use crate::context::{ContextError, ResolvedContext};
use crate::engine::phash::compute_phash;
use crate::manifest::{ImageHash, ManifestError, SingleTestManifest, WorkspaceIndex};
use crate::scanner::ScannerError;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors that can occur during baseline approval.
#[derive(Debug, Error)]
pub enum ApproveError {
    /// Workspace has not been initialized (`.gleon` missing).
    #[error("gleon workspace is not initialized. Please run 'gleon init' first.")]
    NotInitialized,

    /// No actual screenshots found to approve at the target path.
    #[error("No actual screenshots found at path '{path}'")]
    NoActualScreenshots { path: PathBuf },

    /// Error resolving context.
    #[error("Context resolution error: {0}")]
    Context(#[from] ContextError),

    /// Error scanning files.
    #[error("Scanner error: {0}")]
    Scanner(#[from] ScannerError),

    /// Error loading configuration.
    #[error("Config error: {0}")]
    Config(#[from] ConfigError),

    /// Error loading or saving manifest.
    #[error("Manifest error: {0}")]
    Manifest(#[from] ManifestError),

    /// Error decoding image file.
    #[error("Image decode error for '{path}'")]
    ImageDecode {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },

    /// Error parsing JSON.
    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<crate::io::IoError> for ApproveError {
    fn from(err: crate::io::IoError) -> Self {
        match err {
            crate::io::IoError::Io(e) => ApproveError::Io(e),
            crate::io::IoError::JsonParse(e) => ApproveError::JsonParse(e),
        }
    }
}

/// Result summary of approving screenshots.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ApproveResult {
    /// List of test case names approved.
    pub approved_test_cases: Vec<String>,
    /// Number of total screenshots approved.
    pub total_approved: usize,
}

/// Executes approval pipeline to promote actual screenshots into baseline manifests and blobs.
pub fn approve_workspace(
    context: &ResolvedContext,
    base_dir: &Path,
    paths: &[PathBuf],
    from_dir: Option<&Path>,
) -> Result<ApproveResult, ApproveError> {
    let gleon_dir = base_dir.join(".gleon");
    if !gleon_dir.exists() {
        return Err(ApproveError::NotInitialized);
    }

    let source_dir = match from_dir {
        Some(d) => {
            if d.is_absolute() {
                d.to_path_buf()
            } else {
                base_dir.join(d)
            }
        }
        None => gleon_dir.join("runs").join("latest").join("actual"),
    };

    if !source_dir.exists() {
        return Err(ApproveError::NoActualScreenshots { path: source_dir });
    }

    let platform_key = match context.platform.to_key() {
        Ok(key) => key,
        Err(e) => return Err(ApproveError::Context(ContextError::Platform(e))),
    };

    let blobs_dir = gleon_dir.join("blobs").join("sha256");
    let manifests_dir = gleon_dir.join("manifests").join(&platform_key);
    std::fs::create_dir_all(&blobs_dir).map_err(ApproveError::Io)?;
    std::fs::create_dir_all(&manifests_dir).map_err(ApproveError::Io)?;

    let mut workspace_index =
        WorkspaceIndex::load(&manifests_dir).map_err(ApproveError::Manifest)?;

    let mut candidate_files = Vec::new();
    let walker = ignore::WalkBuilder::new(&source_dir)
        .standard_filters(false)
        .build();

    for entry in walker.flatten() {
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("png"))
        {
            candidate_files.push(path.to_path_buf());
        }
    }

    if candidate_files.is_empty() {
        return Err(ApproveError::NoActualScreenshots { path: source_dir });
    }

    let mut approved_test_cases_set = std::collections::BTreeSet::new();
    let mut total_approved = 0;

    for file_path in candidate_files {
        let rel_to_source = match file_path.strip_prefix(&source_dir) {
            Ok(r) => r,
            Err(_) => continue,
        };

        let mut raw_test_name = String::new();
        for comp in rel_to_source.with_extension("").components() {
            if let std::path::Component::Normal(c) = comp {
                if !raw_test_name.is_empty() {
                    raw_test_name.push('/');
                }
                raw_test_name.push_str(&c.to_string_lossy());
            }
        }
        let test_name = crate::manifest::normalize_test_name(&raw_test_name).into_owned();

        // Apply path filter if provided (matching on exact component boundaries)
        if !paths.is_empty() {
            let rel_no_ext = rel_to_source.with_extension("");
            let matches_filter = paths
                .iter()
                .any(|p| rel_to_source.starts_with(p) || rel_no_ext.starts_with(p));
            if !matches_filter {
                continue;
            }
        }

        let raw_png_bytes = std::fs::read(&file_path).map_err(ApproveError::Io)?;
        let dynamic_img = image::load_from_memory(&raw_png_bytes).map_err(|source| {
            ApproveError::ImageDecode {
                path: rel_to_source.to_path_buf(),
                source,
            }
        })?;

        let width = dynamic_img.width();
        let height = dynamic_img.height();
        let rgba_img = dynamic_img.to_rgba8();

        let phash_str = compute_phash(&rgba_img);
        let sha256_hex = hex::encode(Sha256::digest(&raw_png_bytes));

        // Save raw blob to .gleon/blobs/sha256/<sha256_hex>
        let blob_path = blobs_dir.join(&sha256_hex);
        crate::io::save_file_atomically(&blob_path, &raw_png_bytes).map_err(ApproveError::from)?;

        let hash = ImageHash::new("sha256", &sha256_hex).map_err(ApproveError::Manifest)?;
        let phash = phash_str
            .parse::<ImageHash>()
            .map_err(ApproveError::Manifest)?;

        let new_manifest =
            SingleTestManifest::new(hash, phash, width, height).map_err(ApproveError::Manifest)?;

        workspace_index
            .save_test(&manifests_dir, &test_name, &new_manifest)
            .map_err(ApproveError::Manifest)?;

        approved_test_cases_set.insert(test_name);
        total_approved += 1;
    }

    Ok(ApproveResult {
        approved_test_cases: approved_test_cases_set.into_iter().collect(),
        total_approved,
    })
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

    #[test]
    fn test_approve_error_display() {
        let err1 = ApproveError::NotInitialized;
        assert!(err1.to_string().contains("not initialized"));

        let err2 = ApproveError::NoActualScreenshots {
            path: PathBuf::from("dummy/path"),
        };
        assert!(err2.to_string().contains("No actual screenshots found"));

        let err3 = ApproveError::Context(ContextError::Platform(
            crate::platform::PlatformError::InvalidSegment("test".to_string()),
        ));
        assert!(err3.to_string().contains("Context resolution error"));

        let err4 = ApproveError::Config(ConfigError::Validation("bad config".to_string()));
        assert!(err4.to_string().contains("Config error"));

        let err5 = ApproveError::Manifest(ManifestError::Validation("bad manifest".to_string()));
        assert!(err5.to_string().contains("Manifest error"));

        let img_err = image::ImageError::Limits(image::error::LimitError::from_kind(
            image::error::LimitErrorKind::DimensionError,
        ));
        let err6 = ApproveError::ImageDecode {
            path: PathBuf::from("a.png"),
            source: img_err,
        };
        assert!(err6.to_string().contains("Image decode error"));
        assert!(std::error::Error::source(&err6).is_some());

        let err7 = ApproveError::Io(std::io::Error::other("io test"));
        assert!(err7.to_string().contains("IO error"));
    }

    #[test]
    fn test_approve_result_derived() {
        let res = ApproveResult {
            approved_test_cases: vec!["test1".to_string()],
            total_approved: 1,
        };
        let cloned = res.clone();
        assert_eq!(res, cloned);
        assert!(!format!("{:?}", res).is_empty());
        let default_res = ApproveResult::default();
        assert_eq!(default_res.total_approved, 0);
    }

    #[test]
    fn test_approve_not_initialized() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = ResolvedContext::default();
        let res = approve_workspace(&ctx, temp.path(), &[], None);
        assert!(matches!(res, Err(ApproveError::NotInitialized)));
    }

    #[test]
    fn test_approve_no_actual_dir() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join(".gleon")).unwrap();
        let ctx = ResolvedContext::default();
        let res = approve_workspace(&ctx, temp.path(), &[], None);
        assert!(matches!(res, Err(ApproveError::NoActualScreenshots { .. })));
    }

    #[test]
    fn test_approve_success_and_filtering() {
        let temp = tempfile::tempdir().unwrap();
        let gleon_dir = temp.path().join(".gleon");
        let actual_dir = gleon_dir.join("runs").join("latest").join("actual");

        let auth_dir = actual_dir.join("auth");
        let settings_dir = actual_dir.join("settings");
        std::fs::create_dir_all(&auth_dir).unwrap();
        std::fs::create_dir_all(&settings_dir).unwrap();

        let img = image::RgbaImage::new(10, 10);
        img.save(auth_dir.join("login.png")).unwrap();
        img.save(settings_dir.join("profile.png")).unwrap();

        let ctx = ResolvedContext::default();

        // 1. Approve only auth test
        let res_filtered =
            approve_workspace(&ctx, temp.path(), &[PathBuf::from("auth")], None).unwrap();

        assert_eq!(res_filtered.total_approved, 1);
        assert_eq!(
            res_filtered.approved_test_cases,
            vec!["auth/login".to_string()]
        );

        // Verify blob and manifest created
        let plat_key = ctx.platform.to_key().unwrap();
        let manifests_dir = gleon_dir.join("manifests").join(&plat_key);
        assert!(manifests_dir.join("auth/login.json").exists());

        // 2. Approve all tests from custom --from directory
        let custom_dir = temp.path().join("custom_actuals");
        let billing_dir = custom_dir.join("billing");
        std::fs::create_dir_all(&billing_dir).unwrap();
        img.save(billing_dir.join("checkout.png")).unwrap();

        let res_custom = approve_workspace(&ctx, temp.path(), &[], Some(&custom_dir)).unwrap();

        assert_eq!(res_custom.total_approved, 1);
        assert_eq!(
            res_custom.approved_test_cases,
            vec!["billing/checkout".to_string()]
        );
        assert!(manifests_dir.join("billing/checkout.json").exists());
    }

    #[test]
    fn test_approve_path_filtering_boundaries() {
        let temp = tempfile::tempdir().unwrap();
        let gleon_dir = temp.path().join(".gleon");
        let actual_dir = gleon_dir.join("runs").join("latest").join("actual");

        let auth_dir = actual_dir.join("auth");
        let author_dir = actual_dir.join("author");
        std::fs::create_dir_all(&auth_dir).unwrap();
        std::fs::create_dir_all(&author_dir).unwrap();

        let img = image::RgbaImage::new(10, 10);
        img.save(auth_dir.join("login.png")).unwrap();
        img.save(author_dir.join("profile.png")).unwrap();

        let ctx = ResolvedContext::default();

        // Filtering by "auth" should match "auth/login.png" but NOT "author/profile.png"
        let res = approve_workspace(&ctx, temp.path(), &[PathBuf::from("auth")], None).unwrap();

        assert_eq!(res.total_approved, 1);
        assert_eq!(res.approved_test_cases, vec!["auth/login".to_string()]);
    }
}
