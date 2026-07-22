//! Status operation for categorizing workspace screenshots against baseline manifests.

use crate::config::ConfigError;
use crate::context::{ContextError, ResolvedContext};
use crate::manifest::{Manifest, ManifestError, ManifestIndex};
use crate::scanner::{FileScanner, ScannerError};
use serde::Serialize;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors that can occur during status evaluation.
#[derive(Debug, Error)]
pub enum StatusError {
    /// Workspace has not been initialized (`.gleon` missing).
    #[error("Gleon workspace is not initialized. Please run 'gleon init' first.")]
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

    let platform_key = context
        .platform
        .to_key()
        .map_err(|e| StatusError::Context(ContextError::Platform(e)))?;

    let index_path = gleon_dir
        .join("branches")
        .join(&context.branch)
        .join(&platform_key)
        .join("manifest_index.json");

    let manifest_index = match ManifestIndex::load(&index_path) {
        Ok(idx) => Some(idx),
        Err(ManifestError::Io(crate::io::IoError::Io(e)))
            if e.kind() == std::io::ErrorKind::NotFound =>
        {
            None
        }
        Err(e) => return Err(StatusError::Manifest(e)),
    };

    let config = context.config.as_ref().cloned().unwrap_or_default();

    // Scan workspace screenshots
    let test_cases = FileScanner::scan_workspace(&config, base_dir)?;

    let mut added = Vec::new();
    let mut modified = Vec::new();
    let mut deleted = Vec::new();

    // Map test manifests to their entries
    let mut baseline_entries = std::collections::BTreeMap::<String, (u32, u32, String)>::new();

    if let Some(ref index) = manifest_index {
        for manifest_hash in index.test_manifests.values() {
            let manifest_blob_path = gleon_dir
                .join("blobs")
                .join(manifest_hash.scheme())
                .join(manifest_hash.value());

            let manifest = Manifest::load(&manifest_blob_path)?;
            for (rel_path, entry) in manifest.entries {
                baseline_entries.insert(
                    rel_path,
                    (entry.width, entry.height, entry.hash.value().to_string()),
                );
            }
        }
    }

    for case in test_cases {
        for img in case.images {
            let rel_path = img.relative_path;
            let rel_path_str = FileScanner::normalize_path_str(&rel_path);

            match baseline_entries.remove(rel_path_str.as_ref()) {
                None => {
                    added.push(rel_path);
                }
                Some((_w, _h, baseline_sha256)) => {
                    let matched_zones = case.rule.matched_mask_zones(&rel_path);
                    let png_bytes = if !matched_zones.is_empty() {
                        let dynamic_img = image::open(&img.absolute_path)?;
                        let mut rgba = dynamic_img.to_rgba8();
                        crate::masking::apply_masks(&mut rgba, &matched_zones);
                        let mut encoded = Vec::new();
                        let mut cursor = std::io::Cursor::new(&mut encoded);
                        rgba.write_to(&mut cursor, image::ImageFormat::Png)
                            .map_err(std::io::Error::other)?;
                        encoded
                    } else {
                        std::fs::read(&img.absolute_path)?
                    };

                    use sha2::{Digest, Sha256};
                    let computed_sha256 = hex::encode(Sha256::digest(&png_bytes));

                    if computed_sha256 != baseline_sha256 {
                        modified.push(rel_path);
                    }
                }
            }
        }
    }

    // Any entry remaining in baseline_entries was not found in the workspace, so it is Deleted
    for (rel_path_str, _) in baseline_entries {
        deleted.push(PathBuf::from(rel_path_str));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_text_clean() {
        let report = StatusReport::default();
        assert!(report.is_clean());
        assert_eq!(
            report.format_text(),
            "Nothing to report. Workspace is up to date.\n"
        );
    }

    #[test]
    fn test_format_text_with_changes() {
        let report = StatusReport {
            added: vec![PathBuf::from("a.png")],
            modified: vec![PathBuf::from("b.png")],
            deleted: vec![PathBuf::from("c.png")],
        };
        assert!(!report.is_clean());
        let formatted = report.format_text();
        assert!(formatted.contains("Added:\n  a.png"));
        assert!(formatted.contains("Modified:\n  b.png"));
        assert!(formatted.contains("Deleted:\n  c.png"));
    }
}
