//! Status operation for categorizing workspace screenshots against baseline manifests.

use crate::config::ConfigError;
use crate::context::{ContextError, ResolvedContext};
use crate::manifest::{ManifestError, WorkspaceIndex};
use crate::scanner::{FileScanner, ScannerError};
use serde::Serialize;
use sha2::Digest;

use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors that can occur during status evaluation.
#[derive(Debug, Error)]
pub enum StatusError {
    /// Workspace has not been initialized (`.gleon` missing).
    #[error("gleon workspace is not initialized. Please run 'gleon init' first.")]
    NotInitialized,

    /// Error resolving context.
    #[error("Context resolution error: {0}")]
    Context(#[from] ContextError),

    /// Error scanning files.
    #[error("Scanner error: {0}")]
    Scanner(#[from] ScannerError),

    /// Error loading configuration.
    #[error("Config error: {0}")]
    Config(#[from] ConfigError),

    /// Error loading manifest or manifest index.
    #[error("Manifest error: {0}")]
    Manifest(#[from] ManifestError),

    /// Image processing error.
    #[error("Image error: {0}")]
    Image(#[from] image::ImageError),

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Grouped result of evaluating status across the workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct StatusReport {
    pub added: Vec<PathBuf>,
    pub modified: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
}

impl StatusReport {
    /// Returns true if there are no added, modified, or deleted screenshots.
    pub fn is_clean(&self) -> bool {
        self.added.is_empty() && self.modified.is_empty() && self.deleted.is_empty()
    }

    /// Formats the report as pretty-printed JSON.
    pub fn format_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Formats the report as human-readable text.
    pub fn format_text(&self) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        if self.is_clean() {
            out.push_str("Nothing to report. Workspace is up to date.\n");
            return out;
        }

        if !self.added.is_empty() {
            out.push_str("Added:\n");
            for path in &self.added {
                let _ = writeln!(out, "  {}", path.display());
            }
        }

        if !self.modified.is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str("Modified:\n");
            for path in &self.modified {
                let _ = writeln!(out, "  {}", path.display());
            }
        }

        if !self.deleted.is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str("Deleted:\n");
            for path in &self.deleted {
                let _ = writeln!(out, "  {}", path.display());
            }
        }

        out
    }
}

/// Evaluates status for the workspace at `base_dir`.
pub fn check_status(
    context: &ResolvedContext,
    base_dir: &Path,
) -> Result<StatusReport, StatusError> {
    let gleon_dir = base_dir.join(".gleon");
    if !gleon_dir.exists() {
        return Err(StatusError::NotInitialized);
    }

    let platform_key = match context.platform.to_key() {
        Ok(key) => key,
        Err(e) => return Err(StatusError::Context(ContextError::Platform(e))),
    };

    let manifests_dir = gleon_dir.join("manifests").join(&platform_key);
    let mut workspace_index =
        WorkspaceIndex::load(&manifests_dir).map_err(StatusError::Manifest)?;

    if workspace_index.is_empty()
        && let Some(fallback_key) = context.fallback_platform_key.as_deref()
    {
        let fallback_dir = gleon_dir.join("manifests").join(fallback_key);
        let fb_index = WorkspaceIndex::load(&fallback_dir).map_err(StatusError::Manifest)?;
        if !fb_index.is_empty() {
            tracing::warn!(
                "No manifests found for platform '{}'. Falling back to manifests from platform '{}'.",
                platform_key,
                fallback_key
            );
            workspace_index = fb_index;
        }
    }

    let config = context.config.as_ref().cloned().unwrap_or_default();

    // Scan workspace screenshots
    let test_cases = FileScanner::scan_workspace(&config, base_dir)?;

    use rayon::prelude::*;

    let (mut added, mut modified) = test_cases
        .par_iter()
        .map(
            |case| -> Result<(Option<PathBuf>, Option<PathBuf>), StatusError> {
                let baseline_manifest = workspace_index.get(&case.name);

                let img = &case.image;

                match baseline_manifest {
                    None => Ok((Some(img.relative_path.clone()), None)),
                    Some(manifest) => {
                        let raw_bytes = std::fs::read(&img.absolute_path)?;
                        let is_unchanged = match manifest.hash.scheme() {
                            "sha256" => {
                                let actual_sha256 = hex::encode(sha2::Sha256::digest(&raw_bytes));
                                actual_sha256 == manifest.hash.value()
                            }
                            _ => {
                                let baseline_blob_path = crate::storage::local_blob_path(
                                    &gleon_dir.join("blobs"),
                                    &manifest.hash,
                                );
                                match std::fs::read(&baseline_blob_path) {
                                    Ok(b_bytes) => raw_bytes == b_bytes,
                                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
                                    Err(e) => return Err(StatusError::Io(e)),
                                }
                            }
                        };
                        if is_unchanged {
                            crate::manifest::SingleTestManifest::validate_image_bytes(&raw_bytes)?;
                            Ok((None, None))
                        } else {
                            let matched_zones = case.rule.matched_mask_zones(&img.relative_path);
                            if !matched_zones.is_empty() {
                                let baseline_blob_path = crate::storage::local_blob_path(
                                    &gleon_dir.join("blobs"),
                                    &manifest.hash,
                                );

                                let b_bytes_res = std::fs::read(&baseline_blob_path);
                                let b_bytes = match b_bytes_res {
                                    Ok(b) => b,
                                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                                        return Ok((None, Some(img.relative_path.clone())));
                                    }
                                    Err(e) => return Err(StatusError::Io(e)),
                                };

                                if let Err(e) =
                                    crate::manifest::SingleTestManifest::validate_image_bytes(
                                        &b_bytes,
                                    )
                                {
                                    return Err(StatusError::Io(std::io::Error::new(
                                        std::io::ErrorKind::InvalidData,
                                        format!("Invalid baseline dimensions/format: {}", e),
                                    )));
                                }
                                let b_img = image::load_from_memory(&b_bytes)?;

                                if let Err(e) =
                                    crate::manifest::SingleTestManifest::validate_image_bytes(
                                        &raw_bytes,
                                    )
                                {
                                    return Err(StatusError::Io(std::io::Error::new(
                                        std::io::ErrorKind::InvalidData,
                                        format!("Invalid actual image dimensions/format: {}", e),
                                    )));
                                }
                                let a_img = image::load_from_memory(&raw_bytes)?;

                                let mut b_rgba = b_img.to_rgba8();
                                let mut a_rgba = a_img.to_rgba8();
                                crate::masking::apply_masks(&mut b_rgba, &matched_zones);
                                crate::masking::apply_masks(&mut a_rgba, &matched_zones);
                                if b_rgba == a_rgba {
                                    return Ok((None, None));
                                }
                            }
                            Ok((None, Some(img.relative_path.clone())))
                        }
                    }
                }
            },
        )
        .try_fold(
            || (Vec::new(), Vec::new()),
            |mut acc, item| -> Result<_, StatusError> {
                let (opt_add, opt_mod) = match item {
                    Ok(val) => val,
                    Err(e) => return Err(e),
                };
                if let Some(p) = opt_add {
                    acc.0.push(p);
                }
                if let Some(p) = opt_mod {
                    acc.1.push(p);
                }
                Ok(acc)
            },
        )
        .try_reduce(
            || (Vec::new(), Vec::new()),
            |mut a, b| -> Result<_, StatusError> {
                a.0.extend(b.0);
                a.1.extend(b.1);
                Ok(a)
            },
        )?;

    let seen_test_cases: std::collections::HashSet<&str> =
        test_cases.iter().map(|c| c.name.as_str()).collect();

    let mut deleted = Vec::new();

    // Identify deleted test cases (staged in index but no longer present on disk)
    for staged_name in workspace_index.entries().keys() {
        if !seen_test_cases.contains(staged_name.as_str()) {
            let mut p = String::with_capacity(staged_name.len() + 4);
            p.push_str(staged_name);
            p.push_str(".png");
            deleted.push(PathBuf::from(p));
        }
    }

    added.sort();
    modified.sort();
    deleted.sort();

    Ok(StatusReport {
        added,
        modified,
        deleted,
    })
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

    #[test]
    fn test_status_error_display() {
        let err1 = StatusError::NotInitialized;
        assert!(err1.to_string().contains("not initialized"));

        let err2 = StatusError::Context(ContextError::Platform(
            crate::platform::PlatformError::InvalidSegment("test".to_string()),
        ));
        assert!(err2.to_string().contains("Context resolution error"));

        let err3 = StatusError::Scanner(ScannerError::InvalidTestName {
            name: "bad/name".to_string(),
            reason: "reason".to_string(),
        });
        assert!(err3.to_string().contains("Scanner error"));

        let err4 = StatusError::Config(ConfigError::Validation("bad config".to_string()));
        assert!(err4.to_string().contains("Config error"));

        let err5 = StatusError::Manifest(ManifestError::Validation("bad manifest".to_string()));
        assert!(err5.to_string().contains("Manifest error"));

        let err6 = StatusError::Io(std::io::Error::other("io test"));
        assert!(err6.to_string().contains("IO error"));

        let img_err = image::ImageError::Limits(image::error::LimitError::from_kind(
            image::error::LimitErrorKind::DimensionError,
        ));
        let err7 = StatusError::Image(img_err);
        assert!(err7.to_string().contains("Image error"));
        assert!(std::error::Error::source(&err7).is_some());
    }

    #[test]
    fn test_status_report_format() {
        let report = StatusReport {
            added: vec![PathBuf::from("a.png")],
            modified: vec![PathBuf::from("b.png")],
            deleted: vec![PathBuf::from("c.png")],
        };
        assert!(!report.is_clean());
        let text = report.format_text();
        assert!(text.contains("Added:"));
        assert!(text.contains("a.png"));
        assert!(text.contains("Modified:"));
        assert!(text.contains("b.png"));
        assert!(text.contains("Deleted:"));
        assert!(text.contains("c.png"));

        let clean = StatusReport::default();
        assert!(clean.is_clean());
        assert!(clean.format_text().contains("Nothing to report"));

        let json = report.format_json().unwrap();
        assert!(json.contains("a.png"));
    }

    #[test]
    fn test_status_deleted_dotted_test_name() {
        let report = StatusReport {
            added: vec![],
            modified: vec![],
            deleted: vec![PathBuf::from("auth/user.v2.png")],
        };
        let text = report.format_text();
        assert!(text.contains("auth/user.v2.png"));
    }

    #[test]
    fn test_status_detects_deleted_dotted_test_case() {
        let temp = tempfile::tempdir().unwrap();
        let gleon_dir = temp.path().join(".gleon");
        let ctx = ResolvedContext::default();
        let plat_key = ctx.platform.to_key().unwrap();
        let manifests_dir = gleon_dir.join("manifests").join(&plat_key);
        std::fs::create_dir_all(&manifests_dir).unwrap();

        let hash = crate::manifest::ImageHash::new(
            "sha256",
            "1111111111111111111111111111111111111111111111111111111111111111",
        )
        .unwrap();
        let phash = crate::manifest::ImageHash::new("dhash", "0000000000000000").unwrap();
        let manifest = crate::manifest::SingleTestManifest::new(hash, phash, 1, 1).unwrap();
        manifest
            .save(manifests_dir.join("auth").join("user.v2.json"))
            .unwrap();

        let report = check_status(&ctx, temp.path()).unwrap();
        assert_eq!(report.deleted, vec![PathBuf::from("auth/user.v2.png")]);
    }

    #[test]
    fn test_status_missing_baseline_blob_returns_modified() {
        let temp = tempfile::tempdir().unwrap();
        let gleon_dir = temp.path().join(".gleon");
        let ctx = ResolvedContext {
            base_dir: temp.path().to_path_buf(),
            ..Default::default()
        };
        let plat_key = ctx.platform.to_key().unwrap();
        let manifests_dir = gleon_dir.join("manifests").join(&plat_key);
        std::fs::create_dir_all(&manifests_dir).unwrap();

        // Create actual image on disk
        let screenshots_dir = temp.path().join("screenshots");
        std::fs::create_dir_all(&screenshots_dir).unwrap();
        let img = image::RgbaImage::new(10, 10);
        let actual_path = screenshots_dir.join("login.png");
        img.save(&actual_path).unwrap();

        // Create manifest pointing to a non-existent blob
        let hash = crate::manifest::ImageHash::new(
            "sha256",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap();
        let phash = crate::manifest::ImageHash::new("dhash", "0000000000000000").unwrap();
        let manifest = crate::manifest::SingleTestManifest::new(hash, phash, 10, 10).unwrap();
        manifest
            .save(manifests_dir.join("screenshots").join("login.json"))
            .unwrap();

        let report = check_status(&ctx, temp.path()).unwrap();
        assert_eq!(
            report.modified,
            vec![PathBuf::from("screenshots/login.png")]
        );
    }

    #[test]
    fn test_status_fallback_platform() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join(".gleon")).unwrap();
        let ctx = ResolvedContext {
            base_dir: temp.path().to_path_buf(),
            platform: crate::platform::PlatformInfo {
                os: "unknown_os".to_string(),
                arch: None,
                renderer: None,
                labels: std::collections::BTreeMap::new(),
            },
            ..Default::default()
        };

        let report = check_status(&ctx, temp.path()).unwrap();
        assert!(report.is_clean());
    }

    #[test]
    fn test_status_missing_blob_with_masks() {
        let temp = tempfile::tempdir().unwrap();
        let gleon_dir = temp.path().join(".gleon");
        let ctx_temp = ResolvedContext::default();

        let platform_key = ctx_temp.platform.to_key().unwrap();
        let manifests_dir = gleon_dir.join("manifests").join(&platform_key);
        std::fs::create_dir_all(&manifests_dir).unwrap();

        let mut index = WorkspaceIndex::new();
        let hash_val = "1111111111111111111111111111111111111111111111111111111111111111";
        index
            .save_test(
                &manifests_dir,
                "test",
                &crate::manifest::SingleTestManifest {
                    schema_version: 1,
                    hash: crate::manifest::ImageHash::new("sha256", hash_val).unwrap(),
                    phash: crate::manifest::ImageHash::new("dhash", "2222222222222222").unwrap(),
                    width: 1,
                    height: 1,
                },
            )
            .unwrap();

        let img = image::RgbaImage::new(10, 10);
        img.save(temp.path().join("test.png")).unwrap();

        // Define a mask so status.rs attempts to read the baseline blob
        let ctx = ResolvedContext {
            config: Some(crate::config::GleonConfig {
                screenshots: vec![crate::config::ScreenshotRule {
                    include: vec![crate::config::GlobPattern::new("**/*.png").unwrap()],
                    mode: crate::config::Mode::Pixel,
                    diff: crate::config::DiffConfig::default(),
                    masks: vec![crate::config::MaskRule {
                        path: crate::config::GlobPattern::new("**/*.png").unwrap(),
                        zones: vec![crate::config::Zone {
                            x: 0,
                            y: 0,
                            width: crate::config::Dimension::Pixels(1),
                            height: crate::config::Dimension::Pixels(1),
                        }],
                    }],
                }],
                ..Default::default()
            }),
            ..Default::default()
        };

        // Blob doesn't exist, so we expect Modified (NotFound branch lines 175-178)
        let res = check_status(&ctx, temp.path());
        let res_unwrapped = res.unwrap();
        assert_eq!(res_unwrapped.modified.len(), 1);

        // Now create a CORRUPT baseline blob (lines 181-189)
        let blob_dir = gleon_dir.join("blobs").join("sha256");
        std::fs::create_dir_all(&blob_dir).unwrap();
        std::fs::write(blob_dir.join(hash_val), "fake png data").unwrap();

        let res2 = check_status(&ctx, temp.path());
        assert!(matches!(res2, Err(StatusError::Io(_))));
    }

    #[test]
    fn test_status_invalid_platform_key() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join(".gleon")).unwrap();

        // Construct an invalid platform info that fails to_key() (e.g. empty os and version)
        let ctx = ResolvedContext {
            platform: crate::platform::PlatformInfo {
                os: String::new(),
                arch: None,
                renderer: None,
                labels: std::collections::BTreeMap::new(),
            },
            ..Default::default()
        };
        let res = check_status(&ctx, temp.path());
        assert!(matches!(
            res,
            Err(StatusError::Context(ContextError::Platform(_)))
        ));
    }

    #[test]
    fn test_status_non_sha256_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let gleon_dir = temp.path().join(".gleon");
        let ctx = ResolvedContext::default();

        let platform_key = ctx.platform.to_key().unwrap();
        let manifests_dir = gleon_dir.join("manifests").join(&platform_key);
        std::fs::create_dir_all(&manifests_dir).unwrap();

        let hash_val = "11111111111111111111111111111111";
        let mut index = WorkspaceIndex::new();
        index
            .save_test(
                &manifests_dir,
                "test",
                &crate::manifest::SingleTestManifest {
                    schema_version: 1,
                    hash: crate::manifest::ImageHash::new("sha512", hash_val).unwrap(),
                    phash: crate::manifest::ImageHash::new("dhash", "2222222222222222").unwrap(),
                    width: 10,
                    height: 10,
                },
            )
            .unwrap();

        let img = image::RgbaImage::new(10, 10);
        let mut img_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut img_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        std::fs::write(temp.path().join("test.png"), &img_bytes).unwrap();

        let blob_dir = gleon_dir.join("blobs").join("sha512");
        std::fs::create_dir_all(&blob_dir).unwrap();
        std::fs::write(blob_dir.join(hash_val), &img_bytes).unwrap();

        let res = check_status(&ctx, temp.path()).unwrap();
        assert!(res.is_clean());
    }
}
