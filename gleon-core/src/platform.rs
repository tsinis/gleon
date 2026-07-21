use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;

/// Errors that can occur during platform resolution.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum PlatformError {
    #[error("Cannot apply structured overrides ({0}) to an opaque platform configuration")]
    OpaqueConflict(String),
    #[error("Invalid character or pattern in platform segment: {0}")]
    InvalidSegment(String),
    #[error("Failed to parse platform string: {0}")]
    ParseError(String),
    #[error("Label key '{0}' is reserved — use --{1} flag instead")]
    ReservedLabelKey(String, String),
}

/// Resolved platform identity, used for baseline isolation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlatformInfo {
    /// Operating system (e.g. "macos", "linux", "windows").
    pub os: String,
    /// CPU architecture (e.g. "aarch64", "x86_64").
    pub arch: Option<String>,
    /// Optional renderer identifier (e.g. "flutter-3.22", "chrome-126").
    pub renderer: Option<String>,
    /// Arbitrary key-value labels for additional isolation axes.
    /// Sorted alphabetically by key (BTreeMap guarantees this).
    pub labels: BTreeMap<String, String>,
}

/// A parsed representation of structured platform fields.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PlatformFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renderer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformConfig {
    /// Opaque string — validated and normalized (lowercased) for the storage key.
    Opaque(String),
    /// Structured fields — resolved dynamically.
    Structured(PlatformFields),
}

impl Serialize for PlatformConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            PlatformConfig::Opaque(s) => serializer.serialize_str(s),
            PlatformConfig::Structured(fields) => fields.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for PlatformConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = PlatformConfig;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a string or a map representing structured platform config")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                crate::platform::validate_segment(v)
                    .map(PlatformConfig::Opaque)
                    .map_err(E::custom)
            }

            fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                crate::platform::validate_segment(&v)
                    .map(PlatformConfig::Opaque)
                    .map_err(E::custom)
            }

            fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let fields =
                    PlatformFields::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                Ok(PlatformConfig::Structured(fields))
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

impl PlatformFields {
    /// Parses a key-value comma-separated string or fallback simple string.
    pub fn parse_key_value(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if s.is_empty() {
            return Ok(Self::default());
        }

        let mut fields = Self::default();
        if s.contains('=') {
            for part in s.split(',') {
                let (key, val) = part
                    .split_once('=')
                    .ok_or_else(|| format!("invalid format: no '=' found in '{}'", part))?;
                let key = key.trim();
                let val = val.trim();

                if val.is_empty() {
                    return Err(format!("Empty value for key '{}'", key));
                }

                match key {
                    "os" | "platform" => fields.os = Some(val.to_string()),
                    "arch" | "architecture" => fields.arch = Some(val.to_string()),
                    "renderer" => fields.renderer = Some(val.to_string()),
                    _ => {
                        let labels = fields.labels.get_or_insert_with(BTreeMap::new);
                        labels.insert(key.to_string(), val.to_string());
                    }
                }
            }
        } else if let Some((os, arch)) = s.split_once('-') {
            if arch.contains('-') {
                return Err(format!(
                    "invalid format: ambiguous platform string '{}'. Use 'key=value' comma-separated format for complex platforms",
                    s
                ));
            }
            fields.os = Some(os.to_string());
            fields.arch = Some(arch.to_string());
        } else {
            fields.os = Some(s.to_string());
        }

        Ok(fields)
    }
}

/// Validates that a user-provided segment contains only allowed characters.
/// Returns Ok(lowercased) or descriptive error.
pub fn validate_segment(s: &str) -> Result<String, PlatformError> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(PlatformError::InvalidSegment(
            "Segment cannot be empty".into(),
        ));
    }
    let lowered = trimmed.to_lowercase();
    if lowered == "." || lowered == ".." {
        return Err(PlatformError::InvalidSegment(
            "Segment cannot be '.' or '..' to avoid directory traversal".into(),
        ));
    }
    if lowered
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        Ok(lowered)
    } else {
        let bad_chars: String = s
            .chars()
            .filter(|c| !c.is_ascii_alphanumeric() && *c != '-' && *c != '_' && *c != '.')
            .collect();
        Err(PlatformError::InvalidSegment(format!(
            "'{}' contains invalid characters: '{}'. Use [a-z0-9_.-] only",
            s, bad_chars
        )))
    }
}

impl PlatformInfo {
    /// Generates a deterministic flat key from PlatformInfo fields.
    pub fn to_key(&self) -> Result<String, PlatformError> {
        let mut parts = Vec::new();

        match validate_segment(&self.os) {
            Ok(os) => parts.push(format!("{}:{}", os.len(), os)),
            Err(e) => {
                return Err(PlatformError::InvalidSegment(format!(
                    "OS '{}' is empty or invalid: {}",
                    self.os, e
                )));
            }
        }

        if let Some(ref arch) = self.arch {
            match validate_segment(arch) {
                Ok(clean_arch) => parts.push(format!("{}:{}", clean_arch.len(), clean_arch)),
                Err(e) => {
                    return Err(PlatformError::InvalidSegment(format!(
                        "Architecture '{}' is invalid: {}",
                        arch, e
                    )));
                }
            }
        }

        if let Some(ref renderer) = self.renderer {
            match validate_segment(renderer) {
                Ok(clean_renderer) => {
                    parts.push(format!("{}:{}", clean_renderer.len(), clean_renderer))
                }
                Err(e) => {
                    return Err(PlatformError::InvalidSegment(format!(
                        "Renderer '{}' is invalid: {}",
                        renderer, e
                    )));
                }
            }
        }

        for (k, v) in &self.labels {
            let key = match validate_segment(k) {
                Ok(key) => key,
                Err(e) => {
                    return Err(PlatformError::InvalidSegment(format!(
                        "Label key '{}' is invalid: {}",
                        k, e
                    )));
                }
            };
            let val = match validate_segment(v) {
                Ok(val) => val,
                Err(e) => {
                    return Err(PlatformError::InvalidSegment(format!(
                        "Label value '{}' is invalid for key '{}': {}",
                        v, k, e
                    )));
                }
            };
            parts.push(format!("{}:{}={}:{}", key.len(), key, val.len(), val));
        }

        Ok(parts.join("-"))
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PlatformEnv {
    pub platform: Option<String>,
    pub os: Option<String>,
    pub arch: Option<String>,
    pub renderer: Option<String>,
}

impl PlatformEnv {
    pub fn from_env() -> Self {
        Self {
            platform: std::env::var("GLEON_PLATFORM").ok(),
            os: std::env::var("GLEON_OS").ok(),
            arch: std::env::var("GLEON_ARCH").ok(),
            renderer: std::env::var("GLEON_RENDERER").ok(),
        }
    }
}

pub struct PlatformResolver;

impl PlatformResolver {
    fn check_opaque_conflict(
        cli_os: Option<&str>,
        cli_arch: Option<&str>,
        cli_renderer: Option<&str>,
        cli_labels: &[(String, String)],
        env: &PlatformEnv,
        env_fields: Option<&PlatformFields>,
    ) -> Result<(), PlatformError> {
        let mut overrides = Vec::new();
        if cli_os.is_some() || env.os.is_some() || env_fields.is_some_and(|f| f.os.is_some()) {
            overrides.push("OS");
        }
        if cli_arch.is_some() || env.arch.is_some() || env_fields.is_some_and(|f| f.arch.is_some())
        {
            overrides.push("Architecture");
        }
        if cli_renderer.is_some()
            || env.renderer.is_some()
            || env_fields.is_some_and(|f| f.renderer.is_some())
        {
            overrides.push("Renderer");
        }
        if !cli_labels.is_empty() || env_fields.is_some_and(|f| f.labels.is_some()) {
            overrides.push("Labels");
        }

        if !overrides.is_empty() {
            return Err(PlatformError::OpaqueConflict(overrides.join(", ")));
        }
        Ok(())
    }

    /// Resolves the final platform identity by merging all sources.
    /// Priority per field: env > CLI > config > auto-detect (os/arch only).
    pub fn resolve(
        cli_os: Option<&str>,
        cli_arch: Option<&str>,
        cli_renderer: Option<&str>,
        cli_labels: &[(String, String)],
        cli_platform: Option<&str>,
        env: &PlatformEnv,
        config: Option<&PlatformConfig>,
    ) -> Result<PlatformInfo, PlatformError> {
        // Parse GLEON_PLATFORM if set
        let env_fields = match env
            .platform
            .as_deref()
            .map(PlatformFields::parse_key_value)
            .transpose()
        {
            Ok(fields) => fields,
            Err(e) => return Err(PlatformError::ParseError(e)),
        };

        // 1. Check if cli_platform is specified. It acts as a CLI opaque override.
        if let Some(opaque_val) = cli_platform {
            Self::check_opaque_conflict(
                cli_os,
                cli_arch,
                cli_renderer,
                cli_labels,
                env,
                env_fields.as_ref(),
            )?;

            let validated_opaque = validate_segment(opaque_val)?;
            return Ok(PlatformInfo {
                os: validated_opaque,
                arch: None,
                renderer: None,
                labels: BTreeMap::new(),
            });
        }

        // 2. Check GLEON_PLATFORM env var. If config is Opaque, env.platform overrides it entirely.
        // If config is Structured, env.platform only overrides fields specified in it.
        let active_config =
            if env.platform.is_some() && matches!(config, Some(PlatformConfig::Opaque(_))) {
                None
            } else {
                config
            };

        // 2. Check for Opaque config conflict.
        if let Some(PlatformConfig::Opaque(opaque_val)) = active_config {
            Self::check_opaque_conflict(
                cli_os,
                cli_arch,
                cli_renderer,
                cli_labels,
                env,
                env_fields.as_ref(),
            )?;

            let validated_opaque = validate_segment(opaque_val)?;
            return Ok(PlatformInfo {
                os: validated_opaque,
                arch: None,
                renderer: None,
                labels: BTreeMap::new(),
            });
        }

        // Resolve fields step-by-step
        let raw_os = env
            .os
            .clone()
            .or_else(|| env_fields.as_ref().and_then(|f| f.os.clone()))
            .or_else(|| cli_os.map(String::from))
            .or_else(|| {
                if let Some(PlatformConfig::Structured(fields)) = active_config {
                    fields.os.clone()
                } else {
                    None
                }
            })
            .unwrap_or_else(|| std::env::consts::OS.to_string());
        let resolved_os = validate_segment(&raw_os)?;

        let raw_arch = env
            .arch
            .clone()
            .or_else(|| env_fields.as_ref().and_then(|f| f.arch.clone()))
            .or_else(|| cli_arch.map(String::from))
            .or_else(|| {
                if let Some(PlatformConfig::Structured(fields)) = active_config {
                    fields.arch.clone()
                } else {
                    None
                }
            })
            .unwrap_or_else(|| std::env::consts::ARCH.to_string());
        let resolved_arch = Some(validate_segment(&raw_arch)?);

        let resolved_renderer = env
            .renderer
            .clone()
            .or_else(|| env_fields.as_ref().and_then(|f| f.renderer.clone()))
            .or_else(|| cli_renderer.map(String::from))
            .or_else(|| {
                if let Some(PlatformConfig::Structured(fields)) = active_config {
                    fields.renderer.clone()
                } else {
                    None
                }
            })
            .map(|r| validate_segment(&r))
            .transpose()?;

        // Merge labels
        let mut resolved_labels = BTreeMap::new();
        const RESERVED_KEYS: &[&str] = &["os", "platform", "arch", "architecture", "renderer"];

        let mut insert_label = |k: &str, v: &str| -> Result<(), PlatformError> {
            let valid_key = validate_segment(k)?;
            if RESERVED_KEYS.contains(&valid_key.as_str()) {
                let suggested = match valid_key.as_str() {
                    "architecture" => "arch".to_string(),
                    other => other.to_string(),
                };
                return Err(PlatformError::ReservedLabelKey(valid_key, suggested));
            }
            let valid_val = validate_segment(v)?;
            resolved_labels.insert(valid_key, valid_val);
            Ok(())
        };

        // 1. Config labels
        if let Some(PlatformConfig::Structured(PlatformFields {
            labels: Some(config_labels),
            ..
        })) = active_config
        {
            for (k, v) in config_labels {
                insert_label(k, v)?;
            }
        }

        // 2. CLI labels (override config)
        for (k, v) in cli_labels {
            insert_label(k, v)?;
        }

        // 3. Env labels (override CLI and config)
        if let Some(PlatformFields {
            labels: Some(env_labels),
            ..
        }) = env_fields.as_ref()
        {
            for (k, v) in env_labels {
                insert_label(k, v)?;
            }
        }

        Ok(PlatformInfo {
            os: resolved_os,
            arch: resolved_arch,
            renderer: resolved_renderer,
            labels: resolved_labels,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_key() {
        let info = PlatformInfo {
            os: "MacOS".to_string(),
            arch: Some("aarch64".to_string()),
            renderer: None,
            labels: BTreeMap::new(),
        };
        assert_eq!(info.to_key().unwrap(), "5:macos-7:aarch64");

        let mut labels = BTreeMap::new();
        labels.insert("theme".to_string(), "dark".to_string());
        labels.insert("locale".to_string(), "en_US".to_string());

        let info_rich = PlatformInfo {
            os: "linux".to_string(),
            arch: Some("x86_64".to_string()),
            renderer: Some("flutter-3.22".to_string()),
            labels,
        };
        // Labels are sorted alphabetically: locale, theme
        assert_eq!(
            info_rich.to_key().unwrap(),
            "5:linux-6:x86_64-12:flutter-3.22-6:locale=5:en_us-5:theme=4:dark"
        );
    }

    #[test]
    fn test_parse_key_value() {
        let fields = PlatformFields::parse_key_value(
            "platform=macos,arch=aarch64,renderer=flutter,theme=dark",
        )
        .unwrap();
        assert_eq!(fields.os.as_deref(), Some("macos"));
        assert_eq!(fields.arch.as_deref(), Some("aarch64"));
        assert_eq!(fields.renderer.as_deref(), Some("flutter"));
        assert_eq!(
            fields
                .labels
                .as_ref()
                .unwrap()
                .get("theme")
                .map(|s| s.as_str()),
            Some("dark")
        );

        let fields_simple = PlatformFields::parse_key_value("macos-aarch64").unwrap();
        assert_eq!(fields_simple.os.as_deref(), Some("macos"));
        assert_eq!(fields_simple.arch.as_deref(), Some("aarch64"));

        assert!(PlatformFields::parse_key_value("os=macos,arch=").is_err());
        assert!(PlatformFields::parse_key_value("macos-aarch64-extra").is_err());
    }

    #[test]
    fn test_deserialize_platform_config() {
        let yaml_simple = "\"macos-aarch64\"";
        let config: PlatformConfig = serde_yaml::from_str(yaml_simple).unwrap();
        assert_eq!(config, PlatformConfig::Opaque("macos-aarch64".to_string()));

        let yaml_struct = "
os: linux
arch: x86_64
renderer: chrome
labels:
  theme: dark
";
        let config_struct: PlatformConfig = serde_yaml::from_str(yaml_struct).unwrap();
        assert_eq!(
            config_struct,
            PlatformConfig::Structured(PlatformFields {
                os: Some("linux".to_string()),
                arch: Some("x86_64".to_string()),
                renderer: Some("chrome".to_string()),
                labels: {
                    let mut map = BTreeMap::new();
                    map.insert("theme".to_string(), "dark".to_string());
                    Some(map)
                }
            })
        );
    }

    #[test]
    fn test_platform_config_deserialization_from_value() {
        use serde::Deserialize;
        let val = serde_json::Value::String("custom-opaque".to_string());
        let config = PlatformConfig::deserialize(val).unwrap();
        assert_eq!(config, PlatformConfig::Opaque("custom-opaque".to_string()));
    }

    #[test]
    fn test_platform_config_expecting() {
        use serde::Deserialize;
        let val = serde_json::Value::Number(42.into());
        let err = PlatformConfig::deserialize(val).unwrap_err();
        assert!(
            err.to_string()
                .contains("a string or a map representing structured platform config")
        );
    }

    #[test]
    fn test_resolve_opaque_conflict() {
        let config = PlatformConfig::Opaque("custom-opaque".to_string());
        let env = PlatformEnv::default();

        // No overrides: succeeds
        let res = PlatformResolver::resolve(None, None, None, &[], None, &env, Some(&config));
        assert!(res.is_ok());
        assert_eq!(res.unwrap().os, "custom-opaque");

        // Override architecture: conflict
        let res_conflict =
            PlatformResolver::resolve(None, Some("x86_64"), None, &[], None, &env, Some(&config));
        assert!(res_conflict.is_err());
        assert!(matches!(
            res_conflict.unwrap_err(),
            PlatformError::OpaqueConflict(_)
        ));
    }

    #[test]
    fn test_resolve_precedence() {
        let config = PlatformConfig::Structured(PlatformFields {
            os: Some("config-os".to_string()),
            arch: Some("config-arch".to_string()),
            renderer: Some("config-renderer".to_string()),
            labels: None,
        });

        // 1. Config only (no CLI, no Env) -> uses config
        let empty_env = PlatformEnv::default();
        let res = PlatformResolver::resolve(None, None, None, &[], None, &empty_env, Some(&config))
            .unwrap();
        assert_eq!(res.os, "config-os");
        assert_eq!(res.arch.as_deref(), Some("config-arch"));
        assert_eq!(res.renderer.as_deref(), Some("config-renderer"));

        // 2. CLI overrides Config
        let res = PlatformResolver::resolve(
            Some("cli-os"),
            Some("cli-arch"),
            Some("cli-renderer"),
            &[],
            None,
            &empty_env,
            Some(&config),
        )
        .unwrap();
        assert_eq!(res.os, "cli-os");
        assert_eq!(res.arch.as_deref(), Some("cli-arch"));
        assert_eq!(res.renderer.as_deref(), Some("cli-renderer"));

        // 3. GLEON_PLATFORM (env compound) overrides CLI and Config
        let env_platform = PlatformEnv {
            platform: Some("os=env-plat-os,renderer=env-plat-renderer,theme=dark".to_string()),
            ..Default::default()
        };
        let res = PlatformResolver::resolve(
            Some("cli-os"),
            Some("cli-arch"),
            Some("cli-renderer"),
            &[],
            None,
            &env_platform,
            Some(&config),
        )
        .unwrap();
        assert_eq!(res.os, "env-plat-os");
        assert_eq!(res.arch.as_deref(), Some("cli-arch")); // CLI arch is used since env-platform didn't define arch
        assert_eq!(res.renderer.as_deref(), Some("env-plat-renderer"));
        assert_eq!(res.labels.get("theme").map(|s| s.as_str()), Some("dark"));

        // 4. Specific env variables override GLEON_PLATFORM
        let specific_env = PlatformEnv {
            platform: Some("os=env-plat-os,renderer=env-plat-renderer".to_string()),
            os: Some("specific-env-os".to_string()),
            renderer: Some("specific-env-renderer".to_string()),
            ..Default::default()
        };
        let res = PlatformResolver::resolve(
            Some("cli-os"),
            Some("cli-arch"),
            Some("cli-renderer"),
            &[],
            None,
            &specific_env,
            Some(&config),
        )
        .unwrap();
        assert_eq!(res.os, "specific-env-os");
        assert_eq!(res.renderer.as_deref(), Some("specific-env-renderer"));
    }

    #[test]
    fn test_resolve_opaque_conflict_via_env() {
        let config = PlatformConfig::Opaque("custom".into());
        let env = PlatformEnv {
            os: Some("linux".into()),
            ..Default::default()
        };
        let res = PlatformResolver::resolve(None, None, None, &[], None, &env, Some(&config));
        assert!(matches!(res.unwrap_err(), PlatformError::OpaqueConflict(_)));
    }

    #[test]
    fn test_resolve_opaque_bypassed_by_gleon_platform() {
        let config = PlatformConfig::Opaque("custom".into());
        let env = PlatformEnv {
            platform: Some("os=override".into()),
            ..Default::default()
        };
        let res = PlatformResolver::resolve(None, None, None, &[], None, &env, Some(&config));
        assert!(res.is_ok());
        assert_eq!(res.unwrap().os, "override");
    }

    #[test]
    fn test_reserved_label_key_rejected() {
        let env = PlatformEnv::default();
        let labels = vec![("os".into(), "linux".into())];
        let res = PlatformResolver::resolve(None, None, None, &labels, None, &env, None);
        assert_eq!(
            res.unwrap_err(),
            PlatformError::ReservedLabelKey("os".to_string(), "os".to_string())
        );

        // Test synonym mapping
        let labels_syn = vec![("architecture".into(), "x86_64".into())];
        let res_syn = PlatformResolver::resolve(None, None, None, &labels_syn, None, &env, None);
        assert_eq!(
            res_syn.unwrap_err(),
            PlatformError::ReservedLabelKey("architecture".to_string(), "arch".to_string())
        );
    }

    #[test]
    fn test_validate_segment_invalid() {
        assert!(validate_segment("mac os").is_err());
        assert!(validate_segment("mac/os").is_err());
        assert!(validate_segment("mac!").is_err());
        assert!(validate_segment(".").is_err());
        assert!(validate_segment("..").is_err());
    }

    #[test]
    fn test_validate_segment_empty_and_to_key_errors() {
        // Validate segment empty checks
        assert!(matches!(
            validate_segment("   "),
            Err(PlatformError::InvalidSegment(_))
        ));

        // OS invalid
        let info = PlatformInfo {
            os: "mac os".to_string(),
            arch: None,
            renderer: None,
            labels: BTreeMap::new(),
        };
        assert!(info.to_key().is_err());

        // Arch invalid
        let info = PlatformInfo {
            os: "macos".to_string(),
            arch: Some("x86 64".to_string()),
            renderer: None,
            labels: BTreeMap::new(),
        };
        assert!(info.to_key().is_err());

        // Renderer invalid
        let info = PlatformInfo {
            os: "macos".to_string(),
            arch: None,
            renderer: Some("chrome/126".to_string()),
            labels: BTreeMap::new(),
        };
        assert!(info.to_key().is_err());

        // Label key invalid
        let mut labels = BTreeMap::new();
        labels.insert("theme name".to_string(), "dark".to_string());
        let info = PlatformInfo {
            os: "macos".to_string(),
            arch: None,
            renderer: None,
            labels,
        };
        assert!(info.to_key().is_err());

        // Label value invalid
        let mut labels = BTreeMap::new();
        labels.insert("theme".to_string(), "dark side".to_string());
        let info = PlatformInfo {
            os: "macos".to_string(),
            arch: None,
            renderer: None,
            labels,
        };
        assert!(info.to_key().is_err());
    }

    #[test]
    fn test_platform_config_serialization() {
        let opaque = PlatformConfig::Opaque("custom-opaque".to_string());
        let serialized_opaque = serde_yaml::to_string(&opaque).unwrap();
        assert_eq!(serialized_opaque.trim(), "custom-opaque");

        let structured = PlatformConfig::Structured(PlatformFields {
            os: Some("linux".to_string()),
            arch: Some("x86_64".to_string()),
            renderer: None,
            labels: None,
        });
        let serialized_struct = serde_yaml::to_string(&structured).unwrap();
        assert!(serialized_struct.contains("os: linux"));
        assert!(serialized_struct.contains("arch: x86_64"));
    }

    #[test]
    fn test_platform_config_json_deserialization() {
        let json = "\"custom-opaque\"";
        let config: PlatformConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config, PlatformConfig::Opaque("custom-opaque".to_string()));
    }

    #[test]
    fn test_parse_key_value_edge_cases() {
        let empty = PlatformFields::parse_key_value("").unwrap();
        assert_eq!(empty, PlatformFields::default());

        let simple_no_hyphen = PlatformFields::parse_key_value("macos").unwrap();
        assert_eq!(simple_no_hyphen.os.as_deref(), Some("macos"));
        assert_eq!(simple_no_hyphen.arch, None);
    }

    #[test]
    fn test_to_key_length_prefixed_format() {
        let info = PlatformInfo {
            os: "linux".to_string(),
            arch: Some("x86_64".to_string()),
            renderer: Some("chrome".to_string()),
            labels: {
                let mut map = BTreeMap::new();
                map.insert("theme".to_string(), "dark".to_string());
                map
            },
        };
        // Expect: 5:linux-6:x86_64-6:chrome-5:theme=4:dark
        assert_eq!(
            info.to_key().unwrap(),
            "5:linux-6:x86_64-6:chrome-5:theme=4:dark"
        );
    }

    #[test]
    fn test_opaque_validation_fails_on_invalid() {
        let config = PlatformConfig::Opaque("mac os".to_string());
        let env = PlatformEnv::default();
        let res = PlatformResolver::resolve(None, None, None, &[], None, &env, Some(&config));
        assert!(res.is_err());
        assert!(matches!(res.unwrap_err(), PlatformError::InvalidSegment(_)));
    }

    #[test]
    fn test_reserved_label_case_insensitive_rejected() {
        let env = PlatformEnv::default();
        let labels = vec![("OS".to_string(), "linux".to_string())];
        let res = PlatformResolver::resolve(None, None, None, &labels, None, &env, None);
        assert_eq!(
            res.unwrap_err(),
            PlatformError::ReservedLabelKey("os".to_string(), "os".to_string())
        );

        let labels_mixed = vec![("Platform".to_string(), "macos".to_string())];
        let res_mixed =
            PlatformResolver::resolve(None, None, None, &labels_mixed, None, &env, None);
        assert_eq!(
            res_mixed.unwrap_err(),
            PlatformError::ReservedLabelKey("platform".to_string(), "platform".to_string())
        );
    }

    #[test]
    fn test_partial_env_config_merge() {
        let config = PlatformConfig::Structured(PlatformFields {
            os: Some("linux".to_string()),
            arch: Some("x86_64".to_string()),
            renderer: Some("firefox".to_string()),
            labels: None,
        });

        // env.platform specifies only renderer=chrome. Config os/arch should be preserved.
        let env = PlatformEnv {
            platform: Some("renderer=chrome".to_string()),
            ..Default::default()
        };

        let res =
            PlatformResolver::resolve(None, None, None, &[], None, &env, Some(&config)).unwrap();
        assert_eq!(res.os, "linux");
        assert_eq!(res.arch.as_deref(), Some("x86_64"));
        assert_eq!(res.renderer.as_deref(), Some("chrome"));
    }

    #[test]
    fn test_resolve_opaque_conflict_all_overrides() {
        let config = PlatformConfig::Opaque("custom-opaque".to_string());
        let env = PlatformEnv {
            os: Some("linux".to_string()),
            arch: Some("x86_64".to_string()),
            renderer: Some("chrome".to_string()),
            ..Default::default()
        };
        let labels = vec![("theme".to_string(), "dark".to_string())];
        let res = PlatformResolver::resolve(None, None, None, &labels, None, &env, Some(&config));
        assert!(res.is_err());
        let err = res.unwrap_err().to_string();
        assert!(err.contains("OS"));
        assert!(err.contains("Architecture"));
        assert!(err.contains("Renderer"));
        assert!(err.contains("Labels"));
    }

    #[test]
    fn test_resolve_cli_platform_success() {
        let env = PlatformEnv::default();
        let res =
            PlatformResolver::resolve(None, None, None, &[], Some("custom-opaque"), &env, None)
                .unwrap();
        assert_eq!(res.os, "custom-opaque");
        assert_eq!(res.arch, None);
        assert_eq!(res.renderer, None);
    }

    #[test]
    fn test_resolve_cli_platform_conflict() {
        let env = PlatformEnv::default();
        let res = PlatformResolver::resolve(
            None,
            Some("x86_64"),
            None,
            &[],
            Some("custom-opaque"),
            &env,
            None,
        );
        assert!(res.is_err());
        assert!(matches!(res.unwrap_err(), PlatformError::OpaqueConflict(_)));
    }

    #[test]
    fn test_resolve_cli_platform_conflict_all() {
        let env = PlatformEnv {
            os: Some("linux".to_string()),
            arch: Some("x86_64".to_string()),
            renderer: Some("chrome".to_string()),
            ..Default::default()
        };
        let labels = vec![("theme".to_string(), "dark".to_string())];
        let res =
            PlatformResolver::resolve(None, None, None, &labels, Some("custom-opaque"), &env, None);
        assert!(res.is_err());
        let err = res.unwrap_err().to_string();
        assert!(err.contains("OS"));
        assert!(err.contains("Architecture"));
        assert!(err.contains("Renderer"));
        assert!(err.contains("Labels"));
    }

    #[test]
    fn test_resolve_cli_platform_conflict_with_env_platform() {
        let env = PlatformEnv {
            platform: Some("os=linux,arch=x86_64,renderer=chrome,theme=dark".to_string()),
            ..Default::default()
        };
        let res =
            PlatformResolver::resolve(None, None, None, &[], Some("custom-opaque"), &env, None);
        assert!(res.is_err());
        let err = res.unwrap_err().to_string();
        assert!(err.contains("OS"));
        assert!(err.contains("Architecture"));
        assert!(err.contains("Renderer"));
        assert!(err.contains("Labels"));
    }

    #[test]
    fn test_resolve_cli_platform_success_with_empty_env_platform() {
        let env = PlatformEnv {
            platform: Some("".to_string()),
            ..Default::default()
        };
        let res =
            PlatformResolver::resolve(None, None, None, &[], Some("custom-opaque"), &env, None);
        assert!(res.is_ok());
        let info = res.unwrap();
        assert_eq!(info.os, "custom-opaque");
        assert_eq!(info.arch, None);
        assert_eq!(info.renderer, None);
        assert!(info.labels.is_empty());
    }
}
