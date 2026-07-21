//! Configuration and manifest models for gleon.

use crate::platform::PlatformConfig;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

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

    /// Low-level I/O error wrapper.
    #[error("I/O operation failed: {0}")]
    IoError(#[from] crate::io::IoError),

    /// Deserialization error for YAML configuration files.
    #[error("Failed to parse YAML configuration: {0}")]
    YamlParse(#[from] serde_yaml::Error),

    /// Deserialization/serialization error for JSON manifest files.
    #[error("Failed to parse JSON manifest: {0}")]
    JsonParse(#[from] serde_json::Error),

    /// gleon CLI version does not satisfy the required_version.
    #[error("Incompatible version. Required: {0}, Current: {1}")]
    IncompatibleVersion(String, String),

    /// gleon CLI version string is not a valid semver format.
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
#[derive(Debug, Copy, Clone, PartialEq)]
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
            .case_insensitive(true)
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
        Self::new(&raw).map_err(serde::de::Error::custom)
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

/// The root configuration structure for gleon.
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

/// Pre-parsed default version requirement, avoiding `unwrap()` at runtime.
const DEFAULT_VERSION_REQ: &str = ">=0.1.0";

use std::sync::LazyLock;

static DEFAULT_VERSION: LazyLock<semver::VersionReq> = LazyLock::new(|| {
    semver::VersionReq::parse(DEFAULT_VERSION_REQ)
        .expect("DEFAULT_VERSION_REQ must be a valid semver requirement")
});

impl Default for GleonConfig {
    fn default() -> Self {
        let required_version = DEFAULT_VERSION.clone();

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

        // If we are root, writing to 0o000 file will succeed.
        let is_root = std::fs::write(&file_path, "probe").is_ok();
        if is_root {
            return;
        }

        let err = GleonConfig::load_from_file(&file_path).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Io(e) if e.kind() == std::io::ErrorKind::PermissionDenied
        ));
    }

    #[test]
    fn test_load_config_validation_failure() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("invalid_config.yaml");
        std::fs::write(&file_path, "required_version: \">=0.1.0\"\nscreenshots: []").unwrap();

        let err = GleonConfig::load_from_file(&file_path).unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)));
    }
}
