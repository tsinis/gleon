//! Parser for Git merge conflict markers in per-test JSON manifest files.

use crate::manifest::single::SingleTestManifest;
use thiserror::Error;

/// Errors occurring during parsing of conflicted manifest JSON files.
#[derive(Debug, Error)]
pub enum ConflictParseError {
    /// Conflict marker `<<<<<<<` was missing.
    #[error("Missing conflict start marker '<<<<<<<'")]
    MissingStartMarker,

    /// Conflict marker `======` was missing.
    #[error("Missing conflict separator marker '======='")]
    MissingSeparatorMarker,

    /// Conflict marker `>>>>>>>` was missing.
    #[error("Missing conflict end marker '>>>>>>>'")]
    MissingEndMarker,

    /// Invalid marker sequence or layout.
    #[error("Invalid conflict marker sequence")]
    InvalidSequence,

    /// Failed to parse `ours` manifest JSON segment.
    #[error("Failed to parse 'ours' manifest JSON: {0}")]
    InvalidOursJson(#[source] serde_json::Error),

    /// Invalid `ours` manifest validation.
    #[error("Invalid 'ours' manifest: {0}")]
    InvalidOursManifest(#[source] crate::manifest::ManifestError),

    /// Failed to parse `theirs` manifest JSON segment.
    #[error("Failed to parse 'theirs' manifest JSON: {0}")]
    InvalidTheirsJson(#[source] serde_json::Error),

    /// Invalid `theirs` manifest validation.
    #[error("Invalid 'theirs' manifest: {0}")]
    InvalidTheirsManifest(#[source] crate::manifest::ManifestError),

    /// Failed to parse `ancestor` manifest JSON segment.
    #[error("Failed to parse 'ancestor' manifest JSON: {0}")]
    InvalidAncestorJson(#[source] serde_json::Error),

    /// Invalid `ancestor` manifest validation.
    #[error("Invalid 'ancestor' manifest: {0}")]
    InvalidAncestorManifest(#[source] crate::manifest::ManifestError),
}

/// Represents a parsed Git merge conflict inside a per-test JSON manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictManifest {
    /// The `ours` (HEAD / current branch) manifest state.
    pub ours: SingleTestManifest,
    /// The `theirs` (incoming branch) manifest state.
    pub theirs: SingleTestManifest,
    /// The common ancestor manifest state (if 3-way diff3 format was used).
    pub ancestor: Option<SingleTestManifest>,
    /// Raw JSON string of `ours`.
    pub ours_raw: String,
    /// Raw JSON string of `theirs`.
    pub theirs_raw: String,
    /// Raw JSON string of `ancestor` (if present).
    pub ancestor_raw: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConflictState {
    Before,
    Ours,
    Ancestor,
    Theirs,
    After,
}

/// Parses a per-test JSON string containing Git conflict markers (`<<<<<<<`, `=======`, `>>>>>>>`).
///
/// # Errors
/// Returns [`ConflictParseError`] if markers are missing, out of order, or if JSON segments cannot be deserialized.
pub fn parse_conflict_manifest(content: &str) -> Result<ConflictManifest, ConflictParseError> {
    let mut ours_raw = String::new();
    let mut theirs_raw = String::new();
    let mut ancestor_raw = String::new();

    let mut state = ConflictState::Before;

    let mut has_start = false;
    let mut has_ancestor = false;
    let mut has_sep = false;
    let mut has_end = false;

    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("<<<<<<<") {
            if has_start {
                return Err(ConflictParseError::InvalidSequence);
            }
            has_start = true;
            state = ConflictState::Ours;
        } else if trimmed.starts_with("|||||||") {
            if !has_start || has_ancestor || has_sep {
                return Err(ConflictParseError::InvalidSequence);
            }
            has_ancestor = true;
            state = ConflictState::Ancestor;
        } else if trimmed.starts_with("=======") {
            if !has_start || has_sep {
                return Err(ConflictParseError::InvalidSequence);
            }
            has_sep = true;
            state = ConflictState::Theirs;
        } else if trimmed.starts_with(">>>>>>>") {
            if !has_start {
                return Err(ConflictParseError::MissingStartMarker);
            }
            if !has_sep {
                return Err(ConflictParseError::MissingSeparatorMarker);
            }
            if has_end {
                return Err(ConflictParseError::InvalidSequence);
            }
            has_end = true;
            state = ConflictState::After;
        } else {
            match state {
                ConflictState::Ours => {
                    if !ours_raw.is_empty() {
                        ours_raw.push('\n');
                    }
                    ours_raw.push_str(line);
                }
                ConflictState::Ancestor => {
                    if !ancestor_raw.is_empty() {
                        ancestor_raw.push('\n');
                    }
                    ancestor_raw.push_str(line);
                }
                ConflictState::Theirs => {
                    if !theirs_raw.is_empty() {
                        theirs_raw.push('\n');
                    }
                    theirs_raw.push_str(line);
                }
                ConflictState::Before | ConflictState::After => {}
            }
        }
    }

    if !has_start {
        return Err(ConflictParseError::MissingStartMarker);
    }
    if !has_sep {
        return Err(ConflictParseError::MissingSeparatorMarker);
    }
    if !has_end {
        return Err(ConflictParseError::MissingEndMarker);
    }

    let ancestor_opt = if has_ancestor {
        Some(ancestor_raw)
    } else {
        None
    };

    let ours: SingleTestManifest = match serde_json::from_str(&ours_raw) {
        Ok(m) => m,
        Err(e) => return Err(ConflictParseError::InvalidOursJson(e)),
    };

    let theirs: SingleTestManifest = match serde_json::from_str(&theirs_raw) {
        Ok(m) => m,
        Err(e) => return Err(ConflictParseError::InvalidTheirsJson(e)),
    };

    let ancestor = if let Some(ref raw) = ancestor_opt {
        match serde_json::from_str::<SingleTestManifest>(raw) {
            Ok(m) => Some(m),
            Err(e) => return Err(ConflictParseError::InvalidAncestorJson(e)),
        }
    } else {
        None
    };

    if let Err(e) = ours.validate() {
        return Err(ConflictParseError::InvalidOursManifest(e));
    }

    if let Err(e) = theirs.validate() {
        return Err(ConflictParseError::InvalidTheirsManifest(e));
    }

    if let Some(ref anc) = ancestor {
        anc.validate()
            .map_err(ConflictParseError::InvalidAncestorManifest)?;
    }

    Ok(ConflictManifest {
        ours,
        theirs,
        ancestor,
        ours_raw,
        theirs_raw,
        ancestor_raw: ancestor_opt,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_2way_conflict() {
        let content = include_str!("../../tests/fixtures/conflict_2way.json");

        let conflict = parse_conflict_manifest(content).expect("Failed to parse 2-way conflict");
        assert_eq!(
            conflict.ours.hash.to_string(),
            "sha256:1111111111111111111111111111111111111111111111111111111111111111"
        );
        assert_eq!(
            conflict.theirs.hash.to_string(),
            "sha256:2222222222222222222222222222222222222222222222222222222222222222"
        );
        assert!(conflict.ancestor.is_none());
    }

    #[test]
    fn test_parse_3way_conflict() {
        let content = include_str!("../../tests/fixtures/conflict_3way.json");

        let conflict = parse_conflict_manifest(content).expect("Failed to parse 3-way conflict");
        assert_eq!(
            conflict.ours.hash.to_string(),
            "sha256:1111111111111111111111111111111111111111111111111111111111111111"
        );
        assert_eq!(
            conflict.ancestor.as_ref().unwrap().hash.to_string(),
            "sha256:0000000000000000000000000000000000000000000000000000000000000000"
        );
        assert_eq!(
            conflict.theirs.hash.to_string(),
            "sha256:2222222222222222222222222222222222222222222222222222222222222222"
        );
    }

    #[test]
    fn test_missing_markers() {
        assert!(matches!(
            parse_conflict_manifest("no conflict markers here"),
            Err(ConflictParseError::MissingStartMarker)
        ));

        let missing_sep = "<<<<<<< HEAD\n{}\n>>>>>>> branch";
        assert!(matches!(
            parse_conflict_manifest(missing_sep),
            Err(ConflictParseError::MissingSeparatorMarker)
        ));

        let missing_end = "<<<<<<< HEAD\n{}\n=======\n{}";
        assert!(matches!(
            parse_conflict_manifest(missing_end),
            Err(ConflictParseError::MissingEndMarker)
        ));
    }

    #[test]
    fn test_invalid_sequences() {
        let double_start = "<<<<<<< HEAD\n<<<<<<<\n=======\n>>>>>>>";
        assert!(matches!(
            parse_conflict_manifest(double_start),
            Err(ConflictParseError::InvalidSequence)
        ));

        let bad_ancestor = "|||||||\n<<<<<<<\n=======\n>>>>>>>";
        assert!(matches!(
            parse_conflict_manifest(bad_ancestor),
            Err(ConflictParseError::InvalidSequence)
        ));

        let bad_sep = "=======\n<<<<<<<\n>>>>>>>";
        assert!(matches!(
            parse_conflict_manifest(bad_sep),
            Err(ConflictParseError::InvalidSequence)
        ));

        let double_end = "<<<<<<< HEAD\n=======\n>>>>>>>\n>>>>>>>";
        assert!(matches!(
            parse_conflict_manifest(double_end),
            Err(ConflictParseError::InvalidSequence)
        ));
    }

    #[test]
    fn test_invalid_json_and_manifest_validation() {
        let valid = "{\"schema_version\":1,\"hash\":\"sha256:1111111111111111111111111111111111111111111111111111111111111111\",\"phash\":\"dhash:0000000000000000\",\"width\":10,\"height\":10}";
        let valid2 = "{\"schema_version\":1,\"hash\":\"sha256:2222222222222222222222222222222222222222222222222222222222222222\",\"phash\":\"dhash:0000000000000000\",\"width\":10,\"height\":10}";

        let bad_ours_json = format!("<<<<<<< HEAD\ninvalid_json\n=======\n{valid}\n>>>>>>> branch");
        assert!(matches!(
            parse_conflict_manifest(&bad_ours_json),
            Err(ConflictParseError::InvalidOursJson(_))
        ));

        let bad_theirs_json =
            format!("<<<<<<< HEAD\n{valid}\n=======\ninvalid_json\n>>>>>>> branch");
        assert!(matches!(
            parse_conflict_manifest(&bad_theirs_json),
            Err(ConflictParseError::InvalidTheirsJson(_))
        ));

        let bad_ancestor_json = format!(
            "<<<<<<< HEAD\n{valid}\n|||||||\ninvalid_json\n=======\n{valid2}\n>>>>>>> branch"
        );
        assert!(matches!(
            parse_conflict_manifest(&bad_ancestor_json),
            Err(ConflictParseError::InvalidAncestorJson(_))
        ));

        let invalid_ours = "{\"schema_version\":1,\"hash\":\"sha256:1111\",\"phash\":\"dhash:0000000000000000\",\"width\":10,\"height\":10}";
        let invalid_ours_manifest =
            format!("<<<<<<< HEAD\n{invalid_ours}\n=======\n{valid2}\n>>>>>>> branch");
        assert!(matches!(
            parse_conflict_manifest(&invalid_ours_manifest),
            Err(ConflictParseError::InvalidOursManifest(_))
        ));

        let invalid_theirs = "{\"schema_version\":1,\"hash\":\"sha256:2222\",\"phash\":\"dhash:0000000000000000\",\"width\":10,\"height\":10}";
        let invalid_theirs_manifest =
            format!("<<<<<<< HEAD\n{valid}\n=======\n{invalid_theirs}\n>>>>>>> branch");
        assert!(matches!(
            parse_conflict_manifest(&invalid_theirs_manifest),
            Err(ConflictParseError::InvalidTheirsManifest(_))
        ));

        let invalid_ancestor = "{\"schema_version\":1,\"hash\":\"sha256:0000\",\"phash\":\"dhash:0000000000000000\",\"width\":10,\"height\":10}";
        let invalid_ancestor_manifest = format!(
            "<<<<<<< HEAD\n{valid}\n|||||||\n{invalid_ancestor}\n=======\n{valid2}\n>>>>>>> branch"
        );
        assert!(matches!(
            parse_conflict_manifest(&invalid_ancestor_manifest),
            Err(ConflictParseError::InvalidAncestorManifest(_))
        ));
    }
}
