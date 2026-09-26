//! Comparison configuration types shared by the engine and its callers.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Comparison mode for visual regression testing.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Pixel-by-pixel color comparison.
    Pixel,
    /// Structural Similarity Index comparison.
    Ssim,
}

/// Dimension value that can be specified either in pixels or as a percentage of the image size.
#[derive(Debug, Copy, Clone, PartialEq)]
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
        enum RawDimension<'a> {
            Integer(u32),
            Str(std::borrow::Cow<'a, str>),
        }

        RawDimension::deserialize(deserializer).and_then(|raw| match raw {
            RawDimension::Integer(px) => Ok(Self::Pixels(px)),
            RawDimension::Str(s) => {
                let trimmed = s.trim();
                trimmed.strip_suffix('%').map_or_else(
                    || {
                        trimmed
                            .parse::<u32>()
                            .map(Dimension::Pixels)
                            .map_err(D::Error::custom)
                    },
                    |pct| {
                        pct.trim()
                            .parse::<f64>()
                            .map_err(D::Error::custom)
                            .and_then(|val| {
                                if (0.0..=100.0).contains(&val) {
                                    Ok(Self::Percent(val))
                                } else {
                                    Err(D::Error::custom(
                                        "percentage must be between 0.0 and 100.0",
                                    ))
                                }
                            })
                    },
                )
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
            Self::Pixels(px) => serializer.serialize_u32(*px),
            Self::Percent(pct) => serializer.collect_str(&format_args!("{pct}%")),
        }
    }
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
    /// Minimum local SSIM [0.0, 1.0] every neighborhood must reach (SSIM mode, see
    /// [`crate::ssim`]).
    #[serde(
        default = "default_min_similarity",
        deserialize_with = "deserialize_ratio"
    )]
    pub min_similarity: f64,
    /// Tolerated deviation in 8-bit channel units beyond the local 3x3 envelope (SSIM mode, see
    /// [`crate::ssim`]).
    #[serde(
        default = "default_color_tolerance",
        deserialize_with = "deserialize_color_tolerance"
    )]
    pub color_tolerance: f64,
}

fn deserialize_color_tolerance<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    f64::deserialize(deserializer).and_then(|val| {
        if val.is_finite() && val >= 0.0 {
            Ok(val)
        } else {
            Err(serde::de::Error::custom(
                "color_tolerance must be a finite, non-negative 8-bit channel amount",
            ))
        }
    })
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
            color_tolerance: default_color_tolerance(),
        }
    }
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

const fn default_threshold() -> f64 {
    0.1
}

const fn default_anti_alias() -> bool {
    true
}

const fn default_min_similarity() -> f64 {
    0.8
}

const fn default_color_tolerance() -> f64 {
    8.0
}
