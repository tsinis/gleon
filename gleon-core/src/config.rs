//! Configuration and manifest models for Gleon.

use crate::platform::PlatformConfig;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors that can occur during configuration loading or manifest operations.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// Configuration file not found.
    #[error("Configuration file not found: {0}")]
    NotFound(PathBuf),

    /// I/O error during file read or write.
    #[error("Failed to read/write file: {0}")]
    Io(#[from] std::io::Error),

    /// Deserialization error for YAML configuration files.
    #[error("Failed to parse YAML configuration: {0}")]
    YamlParse(#[from] serde_yaml::Error),

    /// Deserialization/serialization error for JSON manifest files.
    #[error("Failed to parse JSON manifest: {0}")]
    JsonParse(#[from] serde_json::Error),

    /// Gleon CLI version does not satisfy the required_version.
    #[error("Incompatible version. Required: {0}, Current: {1}")]
    IncompatibleVersion(String, String),

    /// Gleon CLI version string is not a valid semver format.
    #[error("Invalid version format: {0}")]
    InvalidVersionFormat(String),

    /// Configuration is semantically invalid (e.g. empty screenshots list).
    #[error("Invalid configuration: {0}")]
    Validation(String),
}

/// Comparison mode for visual regression testing.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Pixel-by-pixel color comparison.
    Pixel,
    /// Structural Similarity Index comparison.
    Ssim,
}

/// Dimension value that can be specified either in pixels or as a percentage of the image size.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Dimension {
    /// Absolute size in pixels.
    Pixels(u32),
    /// Relative size as a percentage [0.0, 100.0].
    Percent(f64),
}

impl<'de> Deserialize<'de> for Dimension {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error;

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum RawDimension {
            Integer(u32),
            Str(String),
        }

        RawDimension::deserialize(deserializer).and_then(|raw| match raw {
            RawDimension::Integer(px) => Ok(Dimension::Pixels(px)),
            RawDimension::Str(s) => {
                let trimmed = s.trim();
                if let Some(pct) = trimmed.strip_suffix('%') {
                    pct.trim()
                        .parse::<f64>()
                        .map_err(D::Error::custom)
                        .and_then(|val| {
                            if (0.0..=100.0).contains(&val) {
                                Ok(Dimension::Percent(val))
                            } else {
                                Err(D::Error::custom("percentage must be between 0.0 and 100.0"))
                            }
                        })
                } else {
                    trimmed
                        .parse::<u32>()
                        .map(Dimension::Pixels)
                        .map_err(D::Error::custom)
                }
            }
        })
    }
}

impl Serialize for Dimension {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Dimension::Pixels(px) => serializer.serialize_u32(*px),
            Dimension::Percent(pct) => serializer.serialize_str(&format!("{}%", pct)),
        }
    }
}

/// A compiled glob pattern for fast file matching, serialized as a simple string.
#[derive(Debug, Clone)]
pub struct GlobPattern(pub globset::Glob);

impl GlobPattern {
    /// Create a new `GlobPattern` from a raw string.
    pub fn new(raw: &str) -> Result<Self, globset::Error> {
        globset::GlobBuilder::new(raw)
            .literal_separator(true)
            .build()
            .map(Self)
    }

    /// Get the raw string representation.
    pub fn as_str(&self) -> &str {
        self.0.glob()
    }

    /// Access the compiled `globset::Glob`.
    pub fn as_glob(&self) -> &globset::Glob {
        &self.0
    }
}

impl PartialEq for GlobPattern {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}
impl Eq for GlobPattern {}

impl<'de> Deserialize<'de> for GlobPattern {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        let compiled = globset::GlobBuilder::new(&raw)
            .literal_separator(true)
            .build()
            .map_err(serde::de::Error::custom)?;
        Ok(GlobPattern(compiled))
    }
}

impl Serialize for GlobPattern {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

/// A strongly-typed image comparison hash, serialized as a `scheme:value` string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ImageHash {
    /// The hashing scheme/algorithm (e.g. "sha256", "phash", "dhash", "ssim").
    pub scheme: String,
    /// The hex or alphanumeric representation of the hash.
    pub value: String,
}

impl ImageHash {
    /// Constructs a new ImageHash.
    pub fn new(scheme: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            scheme: scheme.into(),
            value: value.into(),
        }
    }
}

impl<'de> Deserialize<'de> for ImageHash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).and_then(|s| {
            let (scheme, value) = s
                .split_once(':')
                .ok_or_else(|| serde::de::Error::custom("Hash must be in 'scheme:value' format"))?;

            if scheme.is_empty() {
                return Err(serde::de::Error::custom("Hash scheme cannot be empty"));
            }
            if value.is_empty() {
                return Err(serde::de::Error::custom("Hash value cannot be empty"));
            }

            if !value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return Err(serde::de::Error::custom(
                    "Hash value contains invalid characters",
                ));
            }

            Ok(ImageHash {
                scheme: scheme.to_string(),
                value: value.to_string(),
            })
        })
    }
}

impl Serialize for ImageHash {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = format!("{}:{}", self.scheme, self.value);
        serializer.serialize_str(&s)
    }
}

impl std::fmt::Display for ImageHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.scheme, self.value)
    }
}

/// The root configuration structure for Gleon.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GleonConfig {
    /// The required version range of the CLI to run this configuration.
    pub required_version: semver::VersionReq,
    /// The platform identifier for which these rules apply (e.g. macos-aarch64).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<PlatformConfig>,
    /// List of screenshot match rules.
    pub screenshots: Vec<ScreenshotRule>,
    /// Globs of paths to exclude from testing.
    #[serde(default, with = "item_or_vec")]
    pub exclude: Vec<GlobPattern>,
}

/// Helper module for serde to deserialize a single item or a list of items into a `Vec<T>`.
pub mod item_or_vec {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<T, S>(vec: &Vec<T>, serializer: S) -> Result<S::Ok, S::Error>
    where
        T: Serialize,
        S: Serializer,
    {
        if vec.len() == 1 {
            vec[0].serialize(serializer)
        } else {
            vec.serialize(serializer)
        }
    }

    pub fn deserialize<'de, T, D>(deserializer: D) -> Result<Vec<T>, D::Error>
    where
        T: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum ItemOrVec<T> {
            Item(T),
            Vec(Vec<T>),
        }

        ItemOrVec::deserialize(deserializer).map(|res| match res {
            ItemOrVec::Item(s) => vec![s],
            ItemOrVec::Vec(v) => v,
        })
    }
}

/// A rule specifying how to match and process screenshots.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ScreenshotRule {
    /// Glob patterns representing the files to include.
    #[serde(with = "item_or_vec")]
    pub include: Vec<GlobPattern>,
    /// Diffing mode (pixel or ssim).
    #[serde(default = "default_mode")]
    pub mode: Mode,
    /// Specific diffing configuration.
    #[serde(default)]
    pub diff: DiffConfig,
    /// Optional zones to mask out (ignore) during verification.
    #[serde(default)]
    pub masks: Vec<MaskRule>,
}

/// Configuration parameters for the diff engine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DiffConfig {
    /// Pixel comparison threshold [0.0, 1.0].
    #[serde(default = "default_threshold", deserialize_with = "deserialize_ratio")]
    pub threshold: f64,
    /// Whether to apply anti-aliasing detection.
    #[serde(default = "default_anti_alias")]
    pub anti_alias: bool,
    /// Minimum required similarity ratio [0.0, 1.0] (for SSIM).
    #[serde(
        default = "default_min_similarity",
        deserialize_with = "deserialize_ratio"
    )]
    pub min_similarity: f64,
}

fn deserialize_ratio<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    f64::deserialize(deserializer).and_then(|val| {
        if (0.0..=1.0).contains(&val) {
            Ok(val)
        } else {
            Err(serde::de::Error::custom(
                "Value must be between 0.0 and 1.0",
            ))
        }
    })
}

impl Default for DiffConfig {
    fn default() -> Self {
        Self {
            threshold: default_threshold(),
            anti_alias: default_anti_alias(),
            min_similarity: default_min_similarity(),
        }
    }
}

/// A mask rule specifying which regions of an image to ignore.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaskRule {
    /// Image path pattern this mask applies to.
    pub path: GlobPattern,
    /// List of bounding zones to ignore.
    pub zones: Vec<Zone>,
}

/// A bounding zone to ignore.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Zone {
    /// The X coordinate of the top-left corner.
    pub x: u32,
    /// The Y coordinate of the top-left corner.
    pub y: u32,
    /// Width of the zone.
    pub width: Dimension,
    /// Height of the zone.
    pub height: Dimension,
}

fn default_mode() -> Mode {
    Mode::Pixel
}

fn default_threshold() -> f64 {
    0.1
}

fn default_anti_alias() -> bool {
    true
}

fn default_min_similarity() -> f64 {
    0.95
}

impl GleonConfig {
    /// Load configuration from a YAML file.
    ///
    /// Performs post-deserialization validation to catch semantically invalid
    /// configurations that serde alone cannot enforce (e.g. empty screenshot rules).
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        tracing::debug!("Loading configuration from {:?}", path);

        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tracing::error!("Configuration file not found at {:?}", path);
                return Err(ConfigError::NotFound(path.to_path_buf()));
            }
            Err(error) => return Err(ConfigError::Io(error)),
        };
        let reader = std::io::BufReader::new(file);
        let config: GleonConfig = serde_yaml::from_reader(reader)?;
        config.validate()?;
        Ok(config)
    }

    /// Verifies if the current CLI version satisfies the configuration's required_version.
    pub fn verify_version(&self, current_version: &str) -> Result<(), ConfigError> {
        let current = semver::Version::parse(current_version)
            .map_err(|_| ConfigError::InvalidVersionFormat(current_version.to_string()))?;

        if !self.required_version.matches(&current) {
            return Err(ConfigError::IncompatibleVersion(
                self.required_version.to_string(),
                current_version.to_string(),
            ));
        }
        Ok(())
    }

    /// Validates semantic invariants that serde attributes cannot express.
    fn validate(&self) -> Result<(), ConfigError> {
        if self.screenshots.is_empty() {
            return Err(ConfigError::Validation(
                "'screenshots' must contain at least one rule".to_string(),
            ));
        }
        for (i, rule) in self.screenshots.iter().enumerate() {
            if rule.include.is_empty() {
                return Err(ConfigError::Validation(format!(
                    "screenshots[{i}].include must contain at least one glob pattern"
                )));
            }
            if !(0.0..=1.0).contains(&rule.diff.threshold) {
                return Err(ConfigError::Validation(format!(
                    "screenshots[{i}].diff.threshold must be between 0.0 and 1.0 (got {})",
                    rule.diff.threshold
                )));
            }
            if !(0.0..=1.0).contains(&rule.diff.min_similarity) {
                return Err(ConfigError::Validation(format!(
                    "screenshots[{i}].diff.min_similarity must be between 0.0 and 1.0 (got {})",
                    rule.diff.min_similarity
                )));
            }
            for (j, mask) in rule.masks.iter().enumerate() {
                for (k, zone) in mask.zones.iter().enumerate() {
                    match zone.width {
                        Dimension::Percent(pct) if !(0.0..=100.0).contains(&pct) => {
                            return Err(ConfigError::Validation(format!(
                                "screenshots[{i}].masks[{j}].zones[{k}].width percentage must be between 0.0 and 100.0 (got {pct}%)"
                            )));
                        }
                        _ => {}
                    }
                    match zone.height {
                        Dimension::Percent(pct) if !(0.0..=100.0).contains(&pct) => {
                            return Err(ConfigError::Validation(format!(
                                "screenshots[{i}].masks[{j}].zones[{k}].height percentage must be between 0.0 and 100.0 (got {pct}%)"
                            )));
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }
}

/// The manifest file structure representing previous run results.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub schema_version: u64,
    pub hash_algo: String,
    pub pixel_format: String,
    pub generator_version: String,
    pub entries: BTreeMap<String, ManifestEntry>,
}

/// Metadata entry for a single verified screenshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntry {
    pub hash: ImageHash,
    pub width: u32,
    pub height: u32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub created_by: String,
    pub source_commit: String,
}

/// The index mapping test paths to their respective manifest hashes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ManifestIndex {
    pub schema_version: u64,
    pub test_manifests: BTreeMap<String, String>,
}

impl Manifest {
    /// Load a manifest from a JSON file.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        tracing::debug!("Loading manifest from {:?}", path);

        let file = std::fs::File::open(path).map_err(|e| {
            tracing::debug!("Failed to open manifest at {:?}: {}", path, e);
            ConfigError::Io(e)
        })?;
        let reader = std::io::BufReader::new(file);
        let manifest: Manifest = serde_json::from_reader(reader).map_err(|e| {
            tracing::error!("Failed to parse JSON manifest at {:?}: {}", path, e);
            ConfigError::JsonParse(e)
        })?;
        Ok(manifest)
    }

    /// Save a manifest to a JSON file atomically.
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), ConfigError> {
        let path = path.as_ref();
        tracing::info!("Saving manifest to {:?}", path);

        // Determine temporary file path in the same folder to guarantee atomic rename capability
        let parent = path.parent().unwrap_or_else(|| Path::new("."));

        if !parent.exists() {
            tracing::debug!("Creating parent directories for manifest: {:?}", parent);
            std::fs::create_dir_all(parent)?;
        }

        let file_name = path.file_name().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid file name")
        })?;

        // Create a unique temporary file in the same directory to avoid concurrency issues and symlink attacks.
        let temp_file = tempfile::Builder::new()
            .prefix(file_name)
            .suffix(".tmp")
            .tempfile_in(parent)?;

        // Write to temporary file with buffered I/O.
        // We transfer ownership of temp_file to BufWriter to avoid a redundant flush on drop.
        use std::io::Write;
        let mut writer = std::io::BufWriter::new(temp_file);
        serde_json::to_writer_pretty(&mut writer, self)?;
        writer.flush()?;
        let temp_file = writer
            .into_inner()
            .map_err(|e| ConfigError::Io(e.into_error()))?;

        // If the target path already exists, preserve its existing permissions.
        // Otherwise, retain the temporary file's default secure permissions (e.g., 0o600).
        #[cfg(all(unix, not(miri)))]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = temp_file
                .as_file()
                .metadata()
                .map_err(ConfigError::Io)?
                .permissions();
            if let Ok(existing) = std::fs::metadata(path) {
                perms.set_mode(existing.permissions().mode());
            }
            temp_file
                .as_file()
                .set_permissions(perms)
                .map_err(ConfigError::Io)?;
        }

        temp_file.as_file().sync_all().map_err(ConfigError::Io)?;

        // Atomically persist (rename) the temporary file to the final destination.
        temp_file.persist(path).map_err(|e| {
            tracing::error!("Failed to save manifest atomically to {:?}: {}", path, e);
            ConfigError::Io(e.error)
        })?;

        if let Some(dir) = path.parent().and_then(|p| std::fs::File::open(p).ok()) {
            let _ = dir.sync_all();
        }

        tracing::debug!("Manifest saved successfully to {:?}", path);
        Ok(())
    }
}

impl ManifestIndex {
    /// Load a manifest index from a JSON file.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        tracing::debug!("Loading manifest index from {:?}", path);

        let file = std::fs::File::open(path).map_err(|e| {
            tracing::debug!("Failed to open manifest index at {:?}: {}", path, e);
            ConfigError::Io(e)
        })?;
        let reader = std::io::BufReader::new(file);
        let index: ManifestIndex = serde_json::from_reader(reader).map_err(|e| {
            tracing::error!("Failed to parse JSON manifest index at {:?}: {}", path, e);
            ConfigError::JsonParse(e)
        })?;
        Ok(index)
    }

    /// Save a manifest index to a JSON file atomically.
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), ConfigError> {
        let path = path.as_ref();
        tracing::info!("Saving manifest index to {:?}", path);

        let parent = path.parent().unwrap_or_else(|| Path::new("."));

        if !parent.exists() {
            tracing::debug!(
                "Creating parent directories for manifest index: {:?}",
                parent
            );
            std::fs::create_dir_all(parent)?;
        }

        let file_name = path.file_name().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid file name")
        })?;

        let temp_file = tempfile::Builder::new()
            .prefix(file_name)
            .suffix(".tmp")
            .tempfile_in(parent)?;

        use std::io::Write;
        let mut writer = std::io::BufWriter::new(temp_file);
        serde_json::to_writer_pretty(&mut writer, self)?;
        writer.flush()?;
        let temp_file = writer
            .into_inner()
            .map_err(|e| ConfigError::Io(e.into_error()))?;

        #[cfg(all(unix, not(miri)))]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = temp_file
                .as_file()
                .metadata()
                .map_err(ConfigError::Io)?
                .permissions();
            if let Ok(existing) = std::fs::metadata(path) {
                perms.set_mode(existing.permissions().mode());
            }
            temp_file
                .as_file()
                .set_permissions(perms)
                .map_err(ConfigError::Io)?;
        }

        temp_file.as_file().sync_all().map_err(ConfigError::Io)?;

        temp_file.persist(path).map_err(|e| {
            tracing::error!(
                "Failed to save manifest index atomically to {:?}: {}",
                path,
                e
            );
            ConfigError::Io(e.error)
        })?;

        if let Some(dir) = path.parent().and_then(|p| std::fs::File::open(p).ok()) {
            let _ = dir.sync_all();
        }

        tracing::debug!("Manifest index saved successfully to {:?}", path);
        Ok(())
    }
}

/// Pre-parsed default version requirement, avoiding `unwrap()` at runtime.
///
/// This is safe because the string `">=0.1.0"` is a valid semver requirement
/// and is verified by the `test_default_version_req_is_valid` unit test.
const DEFAULT_VERSION_REQ: &str = ">=0.1.0";

impl Default for GleonConfig {
    fn default() -> Self {
        static DEFAULT_VERSION: std::sync::OnceLock<semver::VersionReq> =
            std::sync::OnceLock::new();

        // SAFETY rationale: DEFAULT_VERSION_REQ is a compile-time constant
        // validated by unit tests. Using expect() inside OnceLock to satisfy
        // clippy::unwrap_used while keeping the panic message clear.
        #[allow(clippy::expect_used)]
        let required_version = DEFAULT_VERSION
            .get_or_init(|| {
                semver::VersionReq::parse(DEFAULT_VERSION_REQ)
                    .expect("DEFAULT_VERSION_REQ must be a valid semver requirement")
            })
            .clone();

        Self {
            required_version,
            platform: None,
            screenshots: vec![ScreenshotRule {
                #[allow(clippy::expect_used)]
                include: vec![
                    GlobPattern::new("**/*.png").expect("Default glob pattern must be valid"),
                ],
                mode: Mode::Pixel,
                diff: DiffConfig::default(),
                masks: vec![],
            }],
            exclude: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use semver::VersionReq;
    use tempfile::tempdir;

    #[test]
    fn test_default_yaml_snapshot() {
        let config = GleonConfig::default();
        let generated_yaml = serde_yaml::to_string(&config).unwrap();

        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fixture_path = manifest_dir.join("tests/fixtures/default_config.yaml");
        let expected_yaml = std::fs::read_to_string(fixture_path).unwrap();

        assert_eq!(generated_yaml.trim(), expected_yaml.trim());
    }

    #[test]
    fn test_image_hash_prefixed_serialization() {
        let hash = ImageHash::new(
            "sha256",
            "0000000000000000000000000000000000000000000000000000000000000000",
        );
        let json = serde_json::to_string(&hash).unwrap();
        assert_eq!(
            json,
            "\"sha256:0000000000000000000000000000000000000000000000000000000000000000\""
        );

        let deserialized: ImageHash = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, hash);

        // Test different scheme
        let pixel_hash = ImageHash::new("pixel", "a1b2c3d4");
        let pixel_json = serde_json::to_string(&pixel_hash).unwrap();
        assert_eq!(pixel_json, "\"pixel:a1b2c3d4\"");
        let deserialized_pixel: ImageHash = serde_json::from_str(&pixel_json).unwrap();
        assert_eq!(deserialized_pixel, pixel_hash);

        // Missing prefix should fail
        let bad_json = "\"0000000000000000000000000000000000000000000000000000000000000000\"";
        assert!(serde_json::from_str::<ImageHash>(bad_json).is_err());
    }

    #[test]
    fn test_manifest_json_snapshot() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("manifest.json");

        let mut entries = BTreeMap::new();

        let _ = entries.insert(
            "src/login.png".to_string(),
            ManifestEntry {
                hash: ImageHash::new(
                    "sha256",
                    "0000000000000000000000000000000000000000000000000000000000000000",
                ),
                width: 800,
                height: 600,
                created_at: chrono::DateTime::from_timestamp(1710000000, 0).unwrap(),
                created_by: "Test User <test@example.com>".to_string(),
                source_commit: "abcdef1234567890".to_string(),
            },
        );

        let manifest = Manifest {
            schema_version: 1,
            hash_algo: "sha256".to_string(),
            pixel_format: "rgba".to_string(),
            generator_version: "1.0.0".to_string(),
            entries,
        };

        manifest.save(&file_path).unwrap();

        let generated_json = std::fs::read_to_string(&file_path).unwrap();

        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fixture_path = manifest_dir.join("tests/fixtures/default_manifest.json");
        let expected_json = std::fs::read_to_string(fixture_path).unwrap();

        assert_eq!(generated_json.trim(), expected_json.trim());
    }

    #[test]
    fn test_manifest_index_serialization() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("manifest_index.json");

        let mut test_manifests = BTreeMap::new();
        test_manifests.insert(
            "tests/ui/login".to_string(),
            "sha256:1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef".to_string(),
        );

        let index = ManifestIndex {
            schema_version: 1,
            test_manifests,
        };

        index.save(&file_path).unwrap();

        let loaded_index = ManifestIndex::load(&file_path).unwrap();
        assert_eq!(index, loaded_index);
    }

    #[test]
    fn test_load_config_success() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fixture_path = manifest_dir.join("tests/fixtures/gleon.yaml");
        let config = GleonConfig::load_from_file(fixture_path).unwrap();

        assert_eq!(
            config.required_version,
            VersionReq::parse(">=0.1.0").unwrap()
        );
        assert_eq!(
            config.platform,
            Some(PlatformConfig::Opaque("macos-aarch64".to_string()))
        );
        assert_eq!(config.exclude[0].as_str(), "**/ignored/**");
        assert_eq!(config.screenshots.len(), 1);

        let rule = &config.screenshots[0];
        assert_eq!(rule.include[0].as_str(), "src/login.png");
        assert_eq!(rule.mode, Mode::Ssim);
        assert_eq!(rule.diff.threshold, 0.05);
        assert!(!rule.diff.anti_alias);
        assert_eq!(rule.diff.min_similarity, 0.98);
        assert_eq!(rule.masks.len(), 1);

        let mask = &rule.masks[0];
        assert_eq!(mask.path.as_str(), "mask1");
        assert_eq!(mask.zones.len(), 1);

        let zone = &mask.zones[0];
        assert_eq!(zone.x, 10);
        assert_eq!(zone.y, 20);
        assert_eq!(zone.width, Dimension::Pixels(100));
        assert_eq!(zone.height, Dimension::Percent(20.0));
    }

    #[test]
    fn test_load_config_unknown_fields_rejected() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fixture_path = manifest_dir.join("tests/fixtures/gleon_unknown_fields.yaml");
        let result = GleonConfig::load_from_file(fixture_path);
        assert!(
            result.is_err(),
            "Unknown fields should be rejected with deny_unknown_fields"
        );
    }

    #[test]
    fn test_load_config_not_found() {
        let path = PathBuf::from("nonexistent_config_file.yaml");
        let result = GleonConfig::load_from_file(&path);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ConfigError::NotFound(p) if p == path));
    }

    #[test]
    fn test_load_config_invalid_semver() {
        let invalid_yaml = "required_version: \"invalid_semver\"\nscreenshots: []";
        let result: Result<GleonConfig, _> = serde_yaml::from_str(invalid_yaml);
        assert!(result.is_err());
    }

    #[test]
    fn test_load_config_invalid_dimension() {
        // Test non-numeric dimension
        let invalid_dim_yaml = "
required_version: \">=0.1.0\"
screenshots:
  - include: \"test.png\"
    masks:
      - path: \"mask\"
        zones:
          - x: 0
            y: 0
            width: \"not_a_number\"
            height: 10
";
        let result: Result<GleonConfig, _> = serde_yaml::from_str(invalid_dim_yaml);
        assert!(result.is_err());

        // Test out of range percentage (> 100%)
        let invalid_pct_yaml = "
required_version: \">=0.1.0\"
screenshots:
  - include: \"test.png\"
    masks:
      - path: \"mask\"
        zones:
          - x: 0
            y: 0
            width: 10
            height: \"105%\"
";
        let result2: Result<GleonConfig, _> = serde_yaml::from_str(invalid_pct_yaml);
        assert!(result2.is_err());
    }

    #[test]
    fn test_load_config_invalid_ratio() {
        let invalid_yaml = "
required_version: \">=0.1.0\"
screenshots:
  - include: \"test.png\"
    diff:
      threshold: 1.5
";
        let result: Result<GleonConfig, _> = serde_yaml::from_str(invalid_yaml);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_version() {
        let config = GleonConfig::default(); // default has ">=0.1.0"

        // Valid versions
        assert!(config.verify_version("0.1.0").is_ok());
        assert!(config.verify_version("1.0.0").is_ok());

        // Invalid versions
        let err = config.verify_version("0.0.9").unwrap_err();
        assert!(matches!(
            err,
            ConfigError::IncompatibleVersion(req, cur) if req == ">=0.1.0" && cur == "0.0.9"
        ));

        // Malformed current version string
        let err2 = config.verify_version("not-a-semver").unwrap_err();
        assert!(matches!(
            err2,
            ConfigError::InvalidVersionFormat(cur) if cur == "not-a-semver"
        ));
    }

    #[test]
    fn test_default_version_req_is_valid() {
        assert!(semver::VersionReq::parse(DEFAULT_VERSION_REQ).is_ok());
    }

    #[test]
    fn test_validation_empty_screenshots() {
        let yaml = "required_version: \">=0.1.0\"\nscreenshots: []";
        let config: GleonConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigError::Validation(msg) if msg.contains("screenshots")
        ));
    }

    #[test]
    fn test_validation_empty_include() {
        let yaml = "
required_version: \">=0.1.0\"
screenshots:
  - include: []
";
        let config: GleonConfig = serde_yaml::from_str(yaml).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigError::Validation(msg) if msg.contains("include")
        ));
    }

    #[test]
    fn test_validation_invalid_threshold() {
        let mut config = GleonConfig::default();
        config.screenshots[0].diff.threshold = 1.5;
        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigError::Validation(msg) if msg.contains("threshold must be between 0.0 and 1.0")
        ));
    }

    #[test]
    fn test_validation_invalid_min_similarity() {
        let mut config = GleonConfig::default();
        config.screenshots[0].diff.min_similarity = -0.1;
        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigError::Validation(msg) if msg.contains("min_similarity must be between 0.0 and 1.0")
        ));
    }

    #[test]
    fn test_validation_invalid_mask_percentages() {
        // Test invalid width percentage
        let mut config = GleonConfig::default();
        config.screenshots[0].masks = vec![MaskRule {
            path: GlobPattern::new("src/test.png").unwrap(),
            zones: vec![Zone {
                x: 0,
                y: 0,
                width: Dimension::Percent(150.0),
                height: Dimension::Pixels(100),
            }],
        }];
        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigError::Validation(msg) if msg.contains("width percentage must be between 0.0 and 100.0")
        ));

        // Test invalid height percentage
        let mut config2 = GleonConfig::default();
        config2.screenshots[0].masks = vec![MaskRule {
            path: GlobPattern::new("src/test.png").unwrap(),
            zones: vec![Zone {
                x: 0,
                y: 0,
                width: Dimension::Pixels(100),
                height: Dimension::Percent(-10.0),
            }],
        }];
        let result2 = config2.validate();
        assert!(result2.is_err());
        assert!(matches!(
            result2.unwrap_err(),
            ConfigError::Validation(msg) if msg.contains("height percentage must be between 0.0 and 100.0")
        ));
    }

    #[test]
    fn test_image_hash_invalid_format_deserialization() {
        // Missing colon
        let bad_json_1 =
            "\"sha2560000000000000000000000000000000000000000000000000000000000000000\"";
        assert!(serde_json::from_str::<ImageHash>(bad_json_1).is_err());

        // Empty scheme
        let bad_json_2 = "\":00000000\"";
        assert!(serde_json::from_str::<ImageHash>(bad_json_2).is_err());

        // Empty value
        let bad_json_3 = "\"sha256:\"";
        assert!(serde_json::from_str::<ImageHash>(bad_json_3).is_err());

        // Invalid characters
        let bad_json_4 = "\"sha256:invalid?hash\"";
        assert!(serde_json::from_str::<ImageHash>(bad_json_4).is_err());
    }

    #[test]
    fn test_image_hash_display() {
        let hash = ImageHash::new(
            "sha256",
            "0000000000000000000000000000000000000000000000000000000000000000",
        );
        let formatted = format!("{}", hash);
        assert_eq!(
            formatted,
            "sha256:0000000000000000000000000000000000000000000000000000000000000000"
        );
    }

    #[test]
    fn test_manifest_roundtrip() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("manifest.json");

        let mut screenshots = BTreeMap::new();
        let _ = screenshots.insert(
            "src/login.png".to_string(),
            ManifestEntry {
                hash: ImageHash::new(
                    "sha256",
                    "0000000000000000000000000000000000000000000000000000000000000000",
                ),
                width: 800,
                height: 600,
                created_at: chrono::DateTime::from_timestamp(1710000000, 0).unwrap(),
                created_by: "Test User <test@example.com>".to_string(),
                source_commit: "abcdef123".to_string(),
            },
        );

        let manifest = Manifest {
            schema_version: 1,
            hash_algo: "sha256".to_string(),
            pixel_format: "rgba".to_string(),
            generator_version: "1.0.0".to_string(),
            entries: screenshots,
        };

        manifest.save(&file_path).unwrap();
        let loaded = Manifest::load(&file_path).unwrap();
        assert_eq!(manifest, loaded);
    }

    #[test]
    fn test_manifest_atomic_save_success() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("manifest.json");

        let manifest = Manifest {
            schema_version: 1,
            hash_algo: "sha256".to_string(),
            pixel_format: "rgba".to_string(),
            generator_version: "1.0.0".to_string(),
            entries: BTreeMap::new(),
        };

        // Save first time
        manifest.save(&file_path).unwrap();
        assert!(file_path.exists());

        // Update and save again
        let mut screenshots = BTreeMap::new();
        let _ = screenshots.insert(
            "test.png".to_string(),
            ManifestEntry {
                hash: ImageHash::new(
                    "sha256",
                    "0000000000000000000000000000000000000000000000000000000000000000",
                ),
                width: 800,
                height: 600,
                created_at: chrono::DateTime::from_timestamp(456, 0).unwrap(),
                created_by: "Test User <test@example.com>".to_string(),
                source_commit: "abcdef123".to_string(),
            },
        );
        let updated = Manifest {
            schema_version: 2,
            hash_algo: "sha256".to_string(),
            pixel_format: "rgba".to_string(),
            generator_version: "1.0.0".to_string(),
            entries: screenshots,
        };
        updated.save(&file_path).unwrap();

        let loaded = Manifest::load(&file_path).unwrap();
        assert_eq!(loaded.schema_version, 2);
    }

    #[test]
    #[cfg(all(unix, not(miri)))]
    fn test_manifest_atomic_save_failure_preserves_original() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("manifest.json");

        let original = Manifest {
            schema_version: 1,
            hash_algo: "sha256".to_string(),
            pixel_format: "rgba".to_string(),
            generator_version: "1.0.0".to_string(),
            entries: BTreeMap::new(),
        };
        original.save(&file_path).unwrap();

        // Make the directory read-only to make the tempfile creation fail.
        let mut perms = std::fs::metadata(dir.path()).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(dir.path(), perms).unwrap();

        // Verify if the write permission check works (so we are not root)
        let test_file = dir.path().join("test_write.txt");
        let probe_succeeded = std::fs::write(&test_file, b"test").is_err();

        let mut save_result = None;
        if probe_succeeded {
            let updated = Manifest {
                schema_version: 2,
                hash_algo: "sha256".to_string(),
                pixel_format: "rgba".to_string(),
                generator_version: "1.0.0".to_string(),
                entries: BTreeMap::new(),
            };
            save_result = Some(updated.save(&file_path));
        }

        // Restore permissions so tempdir can clean up
        let mut perms = std::fs::metadata(dir.path()).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        let _ = std::fs::set_permissions(dir.path(), perms);

        if probe_succeeded {
            let result = save_result.unwrap();
            assert!(
                result.is_err(),
                "Expected save to fail due to read-only directory"
            );

            // Verify original file remains untouched
            let loaded = Manifest::load(&file_path).unwrap();
            assert_eq!(loaded.schema_version, 1);
        }
    }

    #[test]
    fn test_manifest_io_error_on_save() {
        let dir = tempdir().unwrap();
        let non_directory = dir.path().join("not-a-directory");
        std::fs::write(&non_directory, b"blocker").unwrap();
        let invalid_path = non_directory.join("manifest.json");
        let manifest = Manifest {
            schema_version: 1,
            hash_algo: "sha256".to_string(),
            pixel_format: "rgba".to_string(),
            generator_version: "1.0.0".to_string(),
            entries: BTreeMap::new(),
        };
        let result = manifest.save(&invalid_path);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ConfigError::Io(_)));
    }

    #[test]
    fn test_dimension_deserialization_and_serialization() {
        // Test integer pixels
        let d1: Dimension = serde_yaml::from_str("100").unwrap();
        assert_eq!(d1, Dimension::Pixels(100));
        assert_eq!(serde_yaml::to_string(&d1).unwrap().trim(), "100");

        // Test string pixels
        let d2: Dimension = serde_yaml::from_str("\"150\"").unwrap();
        assert_eq!(d2, Dimension::Pixels(150));
        assert_eq!(serde_yaml::to_string(&d2).unwrap().trim(), "150");

        // Test valid percentage
        let d3: Dimension = serde_yaml::from_str("\"50%\"").unwrap();
        assert_eq!(d3, Dimension::Percent(50.0));
        assert_eq!(serde_yaml::to_string(&d3).unwrap().trim(), "50%");

        // Test invalid negative percentage
        let d_neg_pct: Result<Dimension, _> = serde_yaml::from_str("\"-5%\"");
        assert!(d_neg_pct.is_err());

        // Test invalid excessive percentage
        let d_exc_pct: Result<Dimension, _> = serde_yaml::from_str("\"105%\"");
        assert!(d_exc_pct.is_err());

        // Test invalid format
        let d_invalid: Result<Dimension, _> = serde_yaml::from_str("\"not_a_number\"");
        assert!(d_invalid.is_err());

        // Test invalid float inside percentage
        let d_invalid_pct_float: Result<Dimension, _> = serde_yaml::from_str("\"abc%\"");
        assert!(d_invalid_pct_float.is_err());
    }

    #[test]
    fn test_image_hash_deserialization_errors() {
        // Non-string type (number)
        let res_num: Result<ImageHash, _> = serde_json::from_str("123");
        assert!(res_num.is_err());

        // Invalid hex characters (64 chars long)
        let invalid_hex_str = format!("\"{}\"", "g".repeat(64));
        let res_hex: Result<ImageHash, _> = serde_json::from_str(&invalid_hex_str);
        assert!(res_hex.is_err());
    }

    #[test]
    fn test_item_or_vec_errors() {
        #[derive(Deserialize, Serialize, Debug, PartialEq)]
        struct TestItem {
            #[serde(with = "item_or_vec")]
            values: Vec<String>,
        }
        // Passing an invalid type (number) where string/vec is expected
        let res: Result<TestItem, _> = serde_yaml::from_str("values: 123");
        assert!(res.is_err());
    }

    #[test]
    fn test_diff_config_invalid_type() {
        // threshold is a string, not a float
        let invalid_yaml = "
required_version: \">=0.1.0\"
screenshots:
  - include: \"test.png\"
    diff:
      threshold: \"not-a-float\"
";
        let result: Result<GleonConfig, _> = serde_yaml::from_str(invalid_yaml);
        assert!(result.is_err());
    }

    #[test]
    fn test_item_or_vec_parsing() {
        #[derive(Debug, Serialize, Deserialize, PartialEq)]
        struct TestStruct {
            #[serde(with = "item_or_vec")]
            values: Vec<String>,
        }

        // Test single string
        let s1: TestStruct = serde_yaml::from_str("values: \"hello\"").unwrap();
        assert_eq!(s1.values, vec!["hello".to_string()]);
        // Serializes as a single string
        assert_eq!(serde_yaml::to_string(&s1).unwrap().trim(), "values: hello");

        // Test list of strings
        let s2: TestStruct = serde_yaml::from_str("values: [\"hello\", \"world\"]").unwrap();
        assert_eq!(s2.values, vec!["hello".to_string(), "world".to_string()]);
        // Serializes as a list of strings
        assert_eq!(
            serde_yaml::to_string(&s2).unwrap().trim(),
            "values:\n- hello\n- world"
        );
    }

    #[test]
    fn test_glob_pattern_validation() {
        // 1. Literal path (no wildcards)
        let lit_pat: GlobPattern = serde_yaml::from_str("\"test/pic.png\"").unwrap();
        assert_eq!(lit_pat.as_str(), "test/pic.png");
        let matcher = lit_pat.as_glob().compile_matcher();
        assert!(matcher.is_match("test/pic.png"));
        assert!(!matcher.is_match("test/other.png"));
        assert!(!matcher.is_match("test/pic.png.bak"));

        // 2. Wildcard pattern
        let wild_pat: GlobPattern = serde_yaml::from_str("\"test/*.png\"").unwrap();
        assert_eq!(wild_pat.as_str(), "test/*.png");
        let matcher = wild_pat.as_glob().compile_matcher();
        assert!(matcher.is_match("test/pic.png"));
        assert!(matcher.is_match("test/other.png"));
        assert!(!matcher.is_match("test/dir/pic.png"));

        // 3. Double wildcard pattern
        let double_wild_pat: GlobPattern = serde_yaml::from_str("\"test/**/*.png\"").unwrap();
        let matcher = double_wild_pat.as_glob().compile_matcher();
        assert!(matcher.is_match("test/dir/pic.png"));

        // 4. Invalid pattern (unclosed character class)
        let invalid: Result<GlobPattern, _> = serde_yaml::from_str("\"test/[a-z\"");
        assert!(invalid.is_err());

        // 5. Invalid pattern via new() directly
        let invalid_new = GlobPattern::new("test/[a-z");
        assert!(invalid_new.is_err());
    }

    #[test]
    fn test_manifest_load_corrupted() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("corrupted_manifest.json");
        std::fs::write(&file_path, "{ invalid json }").unwrap();

        let result = Manifest::load(&file_path);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ConfigError::JsonParse(_)));
    }

    #[test]
    fn test_diff_config_invalid_ratio_bounds() {
        // Negative threshold
        let yaml_neg_thresh = "
required_version: \">=0.1.0\"
screenshots:
  - include: \"test.png\"
    diff:
      threshold: -0.1
";
        let res1: Result<GleonConfig, _> = serde_yaml::from_str(yaml_neg_thresh);
        assert!(res1.is_err());

        // Negative similarity
        let yaml_neg_sim = "
required_version: \">=0.1.0\"
screenshots:
  - include: \"test.png\"
    diff:
      min_similarity: -0.05
";
        let res2: Result<GleonConfig, _> = serde_yaml::from_str(yaml_neg_sim);
        assert!(res2.is_err());
    }

    #[test]
    fn test_config_defaults_applied() {
        let minimal_yaml = "
required_version: \">=0.1.0\"
screenshots:
  - include: \"test.png\"
";
        let config: GleonConfig = serde_yaml::from_str(minimal_yaml).unwrap();
        // Check structural defaults
        assert_eq!(config.platform, None);
        assert!(config.exclude.is_empty());
        assert_eq!(config.screenshots.len(), 1);

        let rule = &config.screenshots[0];
        assert_eq!(rule.mode, Mode::Pixel); // default mode
        assert_eq!(rule.masks, Vec::<MaskRule>::new()); // default empty masks

        // Check nested DiffConfig defaults
        assert_eq!(rule.diff.threshold, 0.1);
        assert!(rule.diff.anti_alias);
        assert_eq!(rule.diff.min_similarity, 0.95);
    }

    #[test]
    fn test_config_roundtrip() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fixture_path = manifest_dir.join("tests/fixtures/gleon.yaml");
        let original_config = GleonConfig::load_from_file(fixture_path).unwrap();

        // Serialize to YAML string
        let serialized = serde_yaml::to_string(&original_config).unwrap();

        // Deserialize back
        let round_tripped_config: GleonConfig = serde_yaml::from_str(&serialized).unwrap();

        // Validate equality
        assert_eq!(original_config, round_tripped_config);
    }

    #[test]
    #[cfg(all(unix, not(miri)))]
    fn test_load_config_permission_denied() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let file_path = dir.path().join("unreadable_config.yaml");
        std::fs::write(&file_path, "required_version: \">=0.1.0\"").unwrap();

        std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o000)).unwrap();

        let result = GleonConfig::load_from_file(&file_path);

        // If we are running as root, the load might succeed despite 0o000 permissions.
        if result.is_ok() {
            return;
        }

        let err = result.unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Io(e) if e.kind() == std::io::ErrorKind::PermissionDenied
        ));
    }

    #[test]
    fn test_manifest_save_invalid_filename() {
        let manifest = Manifest {
            schema_version: 1,
            hash_algo: "sha256".to_string(),
            pixel_format: "rgba".to_string(),
            generator_version: "1.0.0".to_string(),
            entries: BTreeMap::new(),
        };
        let err = manifest.save(Path::new("/")).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Io(e) if e.kind() == std::io::ErrorKind::InvalidInput
        ));
    }

    #[test]
    fn test_manifest_save_persist_failure() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("manifest_dir");
        std::fs::create_dir(&file_path).unwrap(); // make it a directory

        let manifest = Manifest {
            schema_version: 1,
            hash_algo: "sha256".to_string(),
            pixel_format: "rgba".to_string(),
            generator_version: "1.0.0".to_string(),
            entries: BTreeMap::new(),
        };

        // Try to save to a path which is a directory (persist fails)
        let err = manifest.save(&file_path).unwrap_err();
        assert!(matches!(err, ConfigError::Io(_)));
    }

    #[test]
    fn test_manifest_load_not_found() {
        let path = PathBuf::from("nonexistent_manifest_file.json");
        let err = Manifest::load(&path).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Io(e) if e.kind() == std::io::ErrorKind::NotFound
        ));
    }

    #[test]
    fn test_manifest_save_creates_parent_dir() {
        let dir = tempdir().unwrap();
        let nested_path = dir.path().join("subdir/nested/manifest.json");

        let manifest = Manifest {
            schema_version: 1,
            hash_algo: "sha256".to_string(),
            pixel_format: "rgba".to_string(),
            generator_version: "1.0.0".to_string(),
            entries: BTreeMap::new(),
        };
        manifest.save(&nested_path).unwrap();

        assert!(nested_path.exists());
        let loaded = Manifest::load(&nested_path).unwrap();
        assert_eq!(loaded.schema_version, 1);
    }

    #[test]
    fn test_load_config_validation_failure() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("invalid_config.yaml");
        std::fs::write(&file_path, "required_version: \">=0.1.0\"\nscreenshots: []").unwrap();

        let err = GleonConfig::load_from_file(&file_path).unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)));
    }

    #[test]
    fn test_manifest_save_create_dir_all_failure() {
        let dir = tempdir().unwrap();
        let blocker_file = dir.path().join("blocker-file");
        std::fs::write(&blocker_file, b"data").unwrap();

        let nested_path = blocker_file.join("subdir/manifest.json");
        let manifest = Manifest {
            schema_version: 1,
            hash_algo: "sha256".to_string(),
            pixel_format: "rgba".to_string(),
            generator_version: "1.0.0".to_string(),
            entries: BTreeMap::new(),
        };
        let err = manifest.save(&nested_path).unwrap_err();
        assert!(matches!(err, ConfigError::Io(_)));
    }
}
