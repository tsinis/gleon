use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use globset::Glob;
use serde::{Deserialize, Serialize};

#[cfg(not(test))]
const PUBLIC_KEY_BYTES: &[u8; 32] = &[
    198, 122, 238, 222, 114, 183, 214, 45, 12, 191, 109, 14, 127, 240, 71, 98, 250, 48, 199, 168,
    86, 17, 219, 195, 33, 114, 88, 143, 221, 62, 131, 23,
];

#[cfg(test)]
std::thread_local! {
    /// Test-only mutable override of the embedded Ed25519 public key, used so the test suite
    /// can sign license tokens with a key it controls.
    pub static PUBLIC_KEY_BYTES: std::cell::RefCell<[u8; 32]> = const {
        std::cell::RefCell::new([
            198, 122, 238, 222, 114, 183, 214, 45, 12, 191, 109, 14, 127, 240, 71, 98, 250, 48, 199, 168,
            86, 17, 219, 195, 33, 114, 88, 143, 221, 62, 131, 23,
        ])
    };
}

#[cfg(not(test))]
const fn get_public_key_bytes() -> [u8; 32] {
    *PUBLIC_KEY_BYTES
}

#[cfg(test)]
fn get_public_key_bytes() -> [u8; 32] {
    PUBLIC_KEY_BYTES.with(|b| *b.borrow())
}

/// The signed payload embedded in a license key, containing ownership and validity details.
#[derive(Serialize, Deserialize, Debug)]
pub struct LicensePayload {
    /// The name or organization the license was issued to.
    pub owner: String,
    /// Glob pattern matching the repositories this license is valid for.
    pub repo_pattern: String,
    /// Unix timestamp (seconds) after which the license is no longer valid without grace period.
    pub expires_at: u64,
    /// Unique identifier for this license, used for tracking/revocation.
    pub license_id: String,
}

#[derive(Debug)]
enum LicenseValidity {
    Valid,
    GracePeriod { reason: String },
}

/// Outcome of license/compliance verification for the current execution context.
#[derive(Debug, PartialEq, Eq)]
pub enum LicenseStatus {
    /// A valid license was verified for this environment.
    Valid,
    /// The environment is a public repository or otherwise granted use, not requiring a
    /// commercial license.
    PublicOrGrantedUse,
    /// Unlicensed use was detected but is only soft-enforced (e.g. grace period, or
    /// non-strict mode).
    UnlicensedSoft {
        /// Human-readable explanation of why the license was rejected or is in grace period.
        reason: String,
    },
    /// A self-compiled (non-official) binary is running in a private CI without a valid license.
    UnofficialBuildInPrivateCI,
    /// An official binary older than the enforcement window is running unlicensed in private CI.
    ExpiredUnlicensedBinary,
}

/// Identifies the environment gleon is currently executing in, for license enforcement purposes.
#[derive(Debug, PartialEq, Eq)]
pub enum ExecutionContext {
    /// Running inside GitHub Actions.
    GitHubActions {
        /// The `owner/repo` slug.
        repo: String,
        /// Whether the repository is private.
        is_private: bool,
    },
    /// Running inside a recognized non-GitHub CI provider.
    GenericCI {
        /// The best-effort detected repository identifier, or empty if unknown.
        repo: String,
    },
    /// Running outside of any recognized CI environment (local development).
    LocalDev,
}

#[derive(Deserialize)]
struct GithubEventPayload {
    repository: Option<GithubRepository>,
}

#[derive(Deserialize)]
struct GithubRepository {
    private: bool,
}

/// Determines whether the current GitHub Actions repository is private by reading the event
/// payload referenced by the `GITHUB_EVENT_PATH` environment variable.
///
/// Fails closed: if the payload is missing, unreadable, unparsable, or lacks repository
/// information, this returns `true` (private).
pub fn parse_github_event_payload_is_private(env_provider: &dyn crate::env::EnvProvider) -> bool {
    let Some(path) = env_provider.get_var("GITHUB_EVENT_PATH") else {
        return true;
    };

    fs::read_to_string(&path)
        .map_err(|e| tracing::debug!("Failed to read GitHub event payload at {}: {}", path, e))
        .and_then(|content| {
            serde_json::from_str::<GithubEventPayload>(&content)
                .map_err(|e| tracing::warn!("Failed to parse GitHub event payload: {}", e))
        })
        .ok()
        .and_then(|payload| payload.repository)
        .is_none_or(|repo| repo.private) // Fail closed: if event payload exists but fails to parse, treat as private
}

/// Identifies the current execution context (GitHub Actions, another CI provider, or local
/// development) by inspecting well-known environment variables.
pub fn identify_context(env_provider: &dyn crate::env::EnvProvider) -> ExecutionContext {
    use crate::env::get_trimmed_var;

    const OTHER_CI_MARKER_VARS: &[&str] = &[
        "CI",
        "CONTINUOUS_INTEGRATION",
        "CIRCLECI",
        "TRAVIS",
        "GITLAB_CI",
        "TF_BUILD",
        "BUILDKITE",
        "DRONE",
        "TEAMCITY_VERSION",
        "BITBUCKET_COMMIT",
    ];

    // 1. GitHub Actions
    if get_trimmed_var(env_provider, "GITHUB_ACTIONS").as_deref() == Some("true")
        && let Some(repo) = get_trimmed_var(env_provider, "GITHUB_REPOSITORY")
    {
        let is_private = parse_github_event_payload_is_private(env_provider);
        return ExecutionContext::GitHubActions { repo, is_private };
    }

    // 2. Resolve provider repository from trusted CI provider metadata
    let provider_repo = get_trimmed_var(env_provider, "CI_PROJECT_PATH")
        .or_else(|| get_trimmed_var(env_provider, "TRAVIS_REPO_SLUG"))
        .or_else(|| get_trimmed_var(env_provider, "BITBUCKET_REPO_FULL_NAME"))
        .or_else(|| {
            let user = get_trimmed_var(env_provider, "CIRCLE_PROJECT_USERNAME")?;
            let repo = get_trimmed_var(env_provider, "CIRCLE_PROJECT_REPONAME")?;
            Some(format!("{user}/{repo}"))
        });

    let override_repo = get_trimmed_var(env_provider, "GLEON_PROJECT_PATH");

    let repo = provider_repo.unwrap_or_else(|| override_repo.unwrap_or_default());

    // 3. Fallback for other CIs
    if !repo.is_empty()
        || OTHER_CI_MARKER_VARS
            .iter()
            .any(|var| env_provider.has_var(var))
    {
        return ExecutionContext::GenericCI { repo };
    }

    // 4. Local Dev
    ExecutionContext::LocalDev
}

/// Entry point for license verification.
pub struct LicenseGate;

impl LicenseGate {
    /// Verifies the license/compliance status for the current process environment.
    pub fn verify(env_provider: &dyn crate::env::EnvProvider) -> LicenseStatus {
        let is_official = option_env!("GLEON_OFFICIAL_SECRET").is_some();
        let build_timestamp_str = option_env!("GLEON_BUILD_TIMESTAMP").unwrap_or("0");
        let build_timestamp: u64 = build_timestamp_str.trim().parse().unwrap_or(0);

        Self::verify_internal(env_provider, is_official, build_timestamp)
    }

    fn verify_internal(
        env_provider: &dyn crate::env::EnvProvider,
        is_official: bool,
        build_timestamp: u64,
    ) -> LicenseStatus {
        let context = identify_context(env_provider);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // If local dev, we always pass silently.
        if context == ExecutionContext::LocalDev {
            return LicenseStatus::Valid;
        }

        let is_private_ci = match context {
            ExecutionContext::GitHubActions { is_private, .. } => is_private,
            ExecutionContext::GenericCI { .. } => true, // Treat generic CI as potentially private
            ExecutionContext::LocalDev => false,
        };

        // If it's a known public repo on GitHub, it passes silently.
        if let ExecutionContext::GitHubActions {
            is_private: false, ..
        } = context
        {
            return LicenseStatus::PublicOrGrantedUse;
        }

        let has_valid_license = env_provider
            .get_var("GLEON_LICENSE_KEY")
            .filter(|k| !k.trim().is_empty())
            .map_or_else(
                || Err("No GLEON_LICENSE_KEY environment variable provided".to_string()),
                |key| Self::verify_key(&key, &context, now),
            );

        match has_valid_license {
            Ok(LicenseValidity::Valid) => LicenseStatus::Valid,
            Ok(LicenseValidity::GracePeriod { reason }) => LicenseStatus::UnlicensedSoft { reason },
            Err(e) => {
                // An official binary MUST have both the official secret and a valid timestamp (not 0 and not in far future).
                let is_valid_official_build =
                    is_official && build_timestamp > 0 && build_timestamp <= now + 86400;

                if !is_valid_official_build && is_private_ci {
                    return LicenseStatus::UnofficialBuildInPrivateCI;
                }

                // Time-bomb check: > 90 days old (approx 90 * 24 * 60 * 60 = 7_776_000 seconds)
                if is_valid_official_build && is_private_ci && now > build_timestamp + 7_776_000 {
                    return LicenseStatus::ExpiredUnlicensedBinary;
                }

                LicenseStatus::UnlicensedSoft { reason: e }
            }
        }
    }

    fn verify_key(
        key: &str,
        context: &ExecutionContext,
        now: u64,
    ) -> Result<LicenseValidity, String> {
        let mut decoded = None;
        let engines = [
            base64::engine::general_purpose::STANDARD,
            base64::engine::general_purpose::URL_SAFE,
            base64::engine::general_purpose::STANDARD_NO_PAD,
            base64::engine::general_purpose::URL_SAFE_NO_PAD,
        ];
        for engine in engines {
            if let Ok(d) = engine.decode(key) {
                decoded = Some(d);
                break;
            }
        }
        let decoded = decoded.ok_or_else(|| "Invalid base64 encoding".to_string())?;
        if decoded.len() <= 64 {
            return Err("License key payload too short".to_string());
        }

        let (payload_bytes, signature_bytes) = decoded.split_at(decoded.len() - 64);

        let signature = Signature::from_slice(signature_bytes)
            .map_err(|_| "Invalid Ed25519 signature format")?;
        let pub_key_bytes = get_public_key_bytes();
        let pub_key =
            VerifyingKey::from_bytes(&pub_key_bytes).map_err(|_| "Invalid embedded public key")?;

        pub_key
            .verify(payload_bytes, &signature)
            .map_err(|_| "Cryptographic signature verification failed")?;

        let payload: LicensePayload =
            serde_json::from_slice(payload_bytes).map_err(|_| "Invalid license payload JSON")?;

        let repo_to_check = match context {
            ExecutionContext::GitHubActions { repo, .. } => Some(repo),
            ExecutionContext::GenericCI { repo } => {
                if repo.trim().is_empty() {
                    return Err("Repository name could not be automatically detected for this CI. Please set GLEON_PROJECT_PATH environment variable.".to_string());
                }
                Some(repo)
            }
            ExecutionContext::LocalDev => None,
        };

        if let Some(repo) = repo_to_check {
            let matcher = Glob::new(&payload.repo_pattern)
                .map_err(|_| "Invalid license repo pattern")?
                .compile_matcher();
            if !matcher.is_match(repo) {
                return Err(format!(
                    "License pattern '{}' does not match repository '{}'",
                    payload.repo_pattern, repo
                ));
            }
        }

        let fourteen_days = 14 * 24 * 60 * 60;
        if now > payload.expires_at {
            if now <= payload.expires_at + fourteen_days {
                return Ok(LicenseValidity::GracePeriod {
                    reason: "License expired within the last 14 days (grace period)".to_string(),
                });
            }
            return Err("License has expired".to_string());
        }

        Ok(LicenseValidity::Valid)
    }
}

/// Outcome of enforcing licensing policy.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum EnforcementAction {
    /// Execution is allowed to proceed without restriction.
    Allow,
    /// Execution proceeds, but a compliance warning was emitted.
    Warn,
    /// Execution should be blocked due to a compliance violation.
    Block,
}

/// Structured outcome of evaluating license enforcement policy.
///
/// Carries what the caller should *do* (`action`) separately from what it should *print*
/// (`message`, `gha_annotation`), so the library itself performs no I/O side effects — the
/// binary decides how and where to display them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyDecision {
    /// Whether the caller should allow, warn, or block execution.
    pub action: EnforcementAction,
    /// Human-readable compliance notice lines to print to stderr, if any.
    pub message: Vec<String>,
    /// A GitHub Actions workflow command annotation (`::error ...`/`::warning ...`), if
    /// running in GitHub Actions and a notice applies.
    pub gha_annotation: Option<String>,
}

/// Applies enforcement rules for a given [`LicenseStatus`], returning a structured decision
/// describing what action to take and what (if anything) to display — printing is left to
/// the caller.
#[must_use]
pub fn enforce_policy(
    status: LicenseStatus,
    strict_mode: bool,
    env_provider: &dyn crate::env::EnvProvider,
) -> PolicyDecision {
    match status {
        LicenseStatus::Valid | LicenseStatus::PublicOrGrantedUse => PolicyDecision {
            action: EnforcementAction::Allow,
            message: Vec::new(),
            gha_annotation: None,
        },
        LicenseStatus::UnlicensedSoft { reason } => {
            let message = vec![
                "====================================================".to_string(),
                "[GLEON COMPLIANCE NOTICE] Unlicensed production use detected.".to_string(),
                format!("Reason: {reason}"),
                "This may fall outside the BSL Additional Use Grant.".to_string(),
                "Get a commercial license at https://gleon.rs".to_string(),
                "====================================================".to_string(),
            ];
            let gha_annotation = env_provider.has_var("GITHUB_ACTIONS").then(|| {
                if strict_mode {
                    format!("::error title=Gleon Compliance::Unlicensed usage detected ({reason}).")
                } else {
                    format!(
                        "::warning title=Gleon Compliance::Unlicensed usage detected ({reason})."
                    )
                }
            });
            PolicyDecision {
                action: if strict_mode {
                    EnforcementAction::Block
                } else {
                    EnforcementAction::Warn
                },
                message,
                gha_annotation,
            }
        }
        LicenseStatus::UnofficialBuildInPrivateCI | LicenseStatus::ExpiredUnlicensedBinary => {
            let message = vec![
                "====================================================".to_string(),
                "[GLEON COMPLIANCE ERROR] Execution blocked.".to_string(),
                "Self-compiled or expired official binaries (>3 months) cannot run in unlicensed private CI."
                    .to_string(),
                "Get a valid commercial license at https://gleon.rs".to_string(),
                "====================================================".to_string(),
            ];
            let gha_annotation = env_provider.has_var("GITHUB_ACTIONS").then(|| {
                "::error title=Gleon Compliance::Execution blocked. Self-compiled or expired official binaries cannot run in unlicensed private CI."
                    .to_string()
            });
            PolicyDecision {
                action: EnforcementAction::Block,
                message,
                gha_annotation,
            }
        }
    }
}

#[cfg(test)]
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

    struct MockEnv {
        vars: std::collections::HashMap<String, String>,
    }

    impl crate::env::EnvProvider for MockEnv {
        fn get_var(&self, key: &str) -> Option<String> {
            self.vars.get(key).cloned()
        }
    }

    #[test]
    fn test_identify_context_github_actions_public() {
        let mut vars = std::collections::HashMap::new();
        vars.insert("GITHUB_ACTIONS".to_string(), "true".to_string());
        vars.insert("GITHUB_REPOSITORY".to_string(), "foo/bar".to_string());

        let path = std::env::temp_dir().join(format!(
            "github_payload_ctx_{:?}.json",
            std::thread::current().id()
        ));
        fs::write(&path, r#"{"repository":{"private":false}}"#).unwrap();
        vars.insert(
            "GITHUB_EVENT_PATH".to_string(),
            path.to_string_lossy().into_owned(),
        );

        let env = MockEnv { vars };

        let ctx = identify_context(&env);
        assert_eq!(
            ctx,
            ExecutionContext::GitHubActions {
                repo: "foo/bar".to_string(),
                is_private: false
            }
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn test_identify_context_github_actions_missing_payload() {
        let mut vars = std::collections::HashMap::new();
        vars.insert("GITHUB_ACTIONS".to_string(), "true".to_string());
        vars.insert("GITHUB_REPOSITORY".to_string(), "foo/bar".to_string());
        // Do not insert GITHUB_EVENT_PATH
        let env = MockEnv { vars };

        let ctx = identify_context(&env);
        // Should fail-closed to private: true
        assert_eq!(
            ctx,
            ExecutionContext::GitHubActions {
                repo: "foo/bar".to_string(),
                is_private: true
            }
        );
    }

    #[test]
    fn test_identify_context_github_actions_requires_marker() {
        let mut vars = std::collections::HashMap::new();
        // Simulate a fake GITHUB_REPOSITORY set in another CI
        vars.insert("GITHUB_REPOSITORY".to_string(), "foo/bar".to_string());
        // And a generic CI provider variable
        vars.insert("CI_PROJECT_PATH".to_string(), "gitlab/project".to_string());

        // Create a fake public event payload
        let path = std::env::temp_dir().join(format!(
            "github_payload_fake_{:?}.json",
            std::thread::current().id()
        ));
        fs::write(&path, r#"{"repository":{"private":false}}"#).unwrap();
        vars.insert(
            "GITHUB_EVENT_PATH".to_string(),
            path.to_string_lossy().into_owned(),
        );

        let env = MockEnv { vars };

        let ctx = identify_context(&env);
        // It must NOT match GitHubActions and should fall through to GenericCI
        assert_eq!(
            ctx,
            ExecutionContext::GenericCI {
                repo: "gitlab/project".to_string()
            }
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn test_identify_context_gitlab() {
        let mut vars = std::collections::HashMap::new();
        vars.insert("CI_PROJECT_PATH".to_string(), "foo/bar".to_string());
        let env = MockEnv { vars };

        let ctx = identify_context(&env);
        assert_eq!(
            ctx,
            ExecutionContext::GenericCI {
                repo: "foo/bar".to_string()
            }
        );
    }

    #[test]
    fn test_identify_context_local_dev() {
        let env = MockEnv {
            vars: std::collections::HashMap::new(),
        };
        let ctx = identify_context(&env);
        assert_eq!(ctx, ExecutionContext::LocalDev);
    }

    #[test]
    fn test_identify_context_generic_ci_providers_and_precedence() {
        // CI_PROJECT_PATH
        let mut vars = std::collections::HashMap::new();
        vars.insert("CI_PROJECT_PATH".to_string(), "gitlab/project".to_string());
        assert_eq!(
            identify_context(&MockEnv { vars: vars.clone() }),
            ExecutionContext::GenericCI {
                repo: "gitlab/project".to_string()
            }
        );

        // TRAVIS_REPO_SLUG
        let mut vars_travis = std::collections::HashMap::new();
        vars_travis.insert("TRAVIS_REPO_SLUG".to_string(), "travis/project".to_string());
        assert_eq!(
            identify_context(&MockEnv { vars: vars_travis }),
            ExecutionContext::GenericCI {
                repo: "travis/project".to_string()
            }
        );

        // BITBUCKET_REPO_FULL_NAME
        let mut vars_bb = std::collections::HashMap::new();
        vars_bb.insert(
            "BITBUCKET_REPO_FULL_NAME".to_string(),
            "bitbucket/project".to_string(),
        );
        assert_eq!(
            identify_context(&MockEnv { vars: vars_bb }),
            ExecutionContext::GenericCI {
                repo: "bitbucket/project".to_string()
            }
        );

        // CircleCI two-part path
        let mut vars_circle = std::collections::HashMap::new();
        vars_circle.insert(
            "CIRCLE_PROJECT_USERNAME".to_string(),
            "circle_user".to_string(),
        );
        vars_circle.insert(
            "CIRCLE_PROJECT_REPONAME".to_string(),
            "circle_repo".to_string(),
        );
        assert_eq!(
            identify_context(&MockEnv {
                vars: vars_circle.clone()
            }),
            ExecutionContext::GenericCI {
                repo: "circle_user/circle_repo".to_string()
            }
        );

        // CircleCI incomplete (one component blank) falls back
        vars_circle.insert("CIRCLE_PROJECT_REPONAME".to_string(), "   ".to_string());
        vars_circle.insert("CI".to_string(), "true".to_string());
        assert_eq!(
            identify_context(&MockEnv { vars: vars_circle }),
            ExecutionContext::GenericCI {
                repo: String::new()
            }
        );

        // Blank higher-priority GLEON_PROJECT_PATH falls back to provider repo
        let mut vars_blank_override = std::collections::HashMap::new();
        vars_blank_override.insert("GLEON_PROJECT_PATH".to_string(), "   ".to_string());
        vars_blank_override.insert("CI_PROJECT_PATH".to_string(), "fallback/repo".to_string());
        assert_eq!(
            identify_context(&MockEnv {
                vars: vars_blank_override
            }),
            ExecutionContext::GenericCI {
                repo: "fallback/repo".to_string()
            }
        );

        // Override mismatch against provider repo prefers provider repo for security
        let mut vars_mismatch = std::collections::HashMap::new();
        vars_mismatch.insert("GLEON_PROJECT_PATH".to_string(), "spoofed/repo".to_string());
        vars_mismatch.insert("CI_PROJECT_PATH".to_string(), "authentic/repo".to_string());
        assert_eq!(
            identify_context(&MockEnv {
                vars: vars_mismatch
            }),
            ExecutionContext::GenericCI {
                repo: "authentic/repo".to_string()
            }
        );
    }

    #[test]
    fn test_generic_ci_verify_key_empty_and_whitespace_repo_fails() {
        let token = generate_test_license("foo/*", 2000);

        let empty_ctx = ExecutionContext::GenericCI {
            repo: String::new(),
        };
        let res_empty = LicenseGate::verify_key(&token, &empty_ctx, 100);
        assert!(res_empty.is_err());
        assert!(
            res_empty
                .unwrap_err()
                .contains("Repository name could not be automatically detected")
        );

        let whitespace_ctx = ExecutionContext::GenericCI {
            repo: "   ".to_string(),
        };
        let res_ws = LicenseGate::verify_key(&token, &whitespace_ctx, 100);
        assert!(res_ws.is_err());
        assert!(
            res_ws
                .unwrap_err()
                .contains("Repository name could not be automatically detected")
        );
    }

    #[test]
    fn test_parse_github_event_payload_is_private() {
        use std::io::Write;
        let temp = tempfile::tempdir().unwrap();
        let payload_path = temp.path().join("event.json");
        let mut file = fs::File::create(&payload_path).unwrap();
        file.write_all(b"{\"repository\": {\"private\": true}}")
            .unwrap();

        let mut vars = std::collections::HashMap::new();
        vars.insert(
            "GITHUB_EVENT_PATH".to_string(),
            payload_path.to_string_lossy().into_owned(),
        );
        let env = MockEnv { vars };

        assert!(parse_github_event_payload_is_private(&env));
    }

    #[test]
    fn test_parse_github_event_payload_edge_cases() {
        let temp = tempfile::tempdir().unwrap();

        // 1. Non-existent file path returns true (fail closed)
        let mut vars = std::collections::HashMap::new();
        vars.insert(
            "GITHUB_EVENT_PATH".to_string(),
            temp.path()
                .join("missing.json")
                .to_string_lossy()
                .into_owned(),
        );
        assert!(parse_github_event_payload_is_private(&MockEnv {
            vars: vars.clone()
        }));

        // 2. Malformed JSON returns true (fail closed)
        let bad_json_path = temp.path().join("bad.json");
        fs::write(&bad_json_path, "{ invalid json }").unwrap();
        vars.insert(
            "GITHUB_EVENT_PATH".to_string(),
            bad_json_path.to_string_lossy().into_owned(),
        );
        assert!(parse_github_event_payload_is_private(&MockEnv {
            vars: vars.clone()
        }));

        // 3. JSON without repository returns true (fail closed)
        let no_repo_path = temp.path().join("norepo.json");
        fs::write(&no_repo_path, "{}").unwrap();
        vars.insert(
            "GITHUB_EVENT_PATH".to_string(),
            no_repo_path.to_string_lossy().into_owned(),
        );
        assert!(parse_github_event_payload_is_private(&MockEnv { vars }));
    }

    fn generate_test_license(repo_pattern: &str, expires_at: u64) -> String {
        use ed25519_dalek::{Signer, SigningKey};
        let secret = [42u8; 32];
        let signing_key = SigningKey::from_bytes(&secret);
        let public_key = signing_key.verifying_key();

        PUBLIC_KEY_BYTES.with(|b| *b.borrow_mut() = public_key.to_bytes());

        let payload = LicensePayload {
            owner: "test".to_string(),
            repo_pattern: repo_pattern.to_string(),
            expires_at,
            license_id: "test-id".to_string(),
        };

        let payload_bytes = serde_json::to_vec(&payload).unwrap();
        let signature = signing_key.sign(&payload_bytes);

        let mut combined = payload_bytes;
        combined.extend_from_slice(&signature.to_bytes());

        base64::engine::general_purpose::STANDARD.encode(combined)
    }

    #[test]
    fn test_verify_key_valid() {
        let now = 1000;
        let token = generate_test_license("foo/*", now + 100);
        let ctx = ExecutionContext::GenericCI {
            repo: "foo/bar".to_string(),
        };
        let res = LicenseGate::verify_key(&token, &ctx, now).unwrap();
        assert!(matches!(res, LicenseValidity::Valid));
    }

    #[test]
    fn test_verify_key_grace_period() {
        let now = 20 * 24 * 60 * 60;
        let expires_at = now - (5 * 24 * 60 * 60); // Expired 5 days ago (within 14 days)
        let token = generate_test_license("foo/*", expires_at);
        let ctx = ExecutionContext::GenericCI {
            repo: "foo/bar".to_string(),
        };
        let res = LicenseGate::verify_key(&token, &ctx, now).unwrap();
        assert!(matches!(res, LicenseValidity::GracePeriod { .. }));
    }

    #[test]
    fn test_verify_key_expired() {
        let now = 20 * 24 * 60 * 60;
        let expires_at = now - (15 * 24 * 60 * 60); // Expired 15 days ago (> 14 days)
        let token = generate_test_license("foo/*", expires_at);
        let ctx = ExecutionContext::GenericCI {
            repo: "foo/bar".to_string(),
        };
        let res = LicenseGate::verify_key(&token, &ctx, now);
        assert!(res.is_err());
    }

    #[test]
    fn test_verify_key_wrong_repo() {
        let now = 1000;
        let token = generate_test_license("baz/*", now + 100);
        let ctx = ExecutionContext::GenericCI {
            repo: "foo/bar".to_string(),
        };
        let res = LicenseGate::verify_key(&token, &ctx, now);
        assert!(res.is_err());
    }

    #[test]
    fn test_verify_key_error_branches() {
        use ed25519_dalek::{Signer, SigningKey};

        let ctx = ExecutionContext::GenericCI {
            repo: "foo/bar".to_string(),
        };

        // 1. Invalid base64
        let err_b64 = LicenseGate::verify_key("not_valid_b64!@#$", &ctx, 1000);
        assert!(err_b64.is_err());
        assert!(err_b64.unwrap_err().contains("Invalid base64 encoding"));

        // 2. Payload too short (<= 64 bytes)
        let short_b64 = base64::engine::general_purpose::STANDARD.encode([0u8; 32]);
        let err_short = LicenseGate::verify_key(&short_b64, &ctx, 1000);
        assert!(err_short.is_err());
        assert!(err_short.unwrap_err().contains("payload too short"));

        // 3. Cryptographic signature verification failed & Invalid license payload JSON
        // Initialize PUBLIC_KEY_BYTES for current thread
        let _valid_token = generate_test_license("foo/*", 2000);

        // Signature mismatch with valid signature format (signed with a different key)
        let payload = LicensePayload {
            owner: "test".to_string(),
            repo_pattern: "foo/*".to_string(),
            expires_at: 2000,
            license_id: "test-id".to_string(),
        };
        let payload_bytes = serde_json::to_vec(&payload).unwrap();
        let other_key = SigningKey::from_bytes(&[99u8; 32]);
        let sig = other_key.sign(&payload_bytes);
        let mut invalid_sig_payload = payload_bytes;
        invalid_sig_payload.extend_from_slice(&sig.to_bytes());
        let invalid_token = base64::engine::general_purpose::STANDARD.encode(invalid_sig_payload);
        let err_sig_verify = LicenseGate::verify_key(&invalid_token, &ctx, 1000);
        assert!(err_sig_verify.is_err());
        assert!(
            err_sig_verify
                .unwrap_err()
                .contains("Cryptographic signature verification failed")
        );

        // Invalid license payload JSON (signed with matching key but bad JSON)
        let matching_key = SigningKey::from_bytes(&[42u8; 32]);
        let bad_json = b"{ not valid json }";
        let bad_json_sig = matching_key.sign(bad_json);
        let mut bad_json_payload = bad_json.to_vec();
        bad_json_payload.extend_from_slice(&bad_json_sig.to_bytes());
        let bad_json_token = base64::engine::general_purpose::STANDARD.encode(bad_json_payload);
        let err_json = LicenseGate::verify_key(&bad_json_token, &ctx, 1000);
        assert!(err_json.is_err());
        assert!(
            err_json
                .unwrap_err()
                .contains("Invalid license payload JSON")
        );

        // 4. Invalid glob pattern in repo_pattern
        let token_bad_glob = generate_test_license("[invalid", 2000);
        let err_glob = LicenseGate::verify_key(&token_bad_glob, &ctx, 1000);
        assert!(err_glob.is_err());
        assert!(
            err_glob
                .unwrap_err()
                .contains("Invalid license repo pattern")
        );
    }

    #[test]
    fn test_enforce_policy_outcomes() {
        let mut vars = std::collections::HashMap::new();
        vars.insert("GITHUB_ACTIONS".to_string(), "true".to_string());
        let env = MockEnv { vars };

        assert_eq!(
            enforce_policy(LicenseStatus::Valid, false, &env).action,
            EnforcementAction::Allow
        );
        assert_eq!(
            enforce_policy(LicenseStatus::PublicOrGrantedUse, false, &env).action,
            EnforcementAction::Allow
        );

        let soft_warn = enforce_policy(
            LicenseStatus::UnlicensedSoft {
                reason: "soft test".to_string(),
            },
            false,
            &env,
        );
        assert_eq!(soft_warn.action, EnforcementAction::Warn);
        assert!(soft_warn.message.iter().any(|l| l.contains("soft test")));
        assert_eq!(
            soft_warn.gha_annotation.as_deref(),
            Some("::warning title=Gleon Compliance::Unlicensed usage detected (soft test).")
        );

        let soft_block = enforce_policy(
            LicenseStatus::UnlicensedSoft {
                reason: "strict test".to_string(),
            },
            true,
            &env,
        );
        assert_eq!(soft_block.action, EnforcementAction::Block);
        assert_eq!(
            soft_block.gha_annotation.as_deref(),
            Some("::error title=Gleon Compliance::Unlicensed usage detected (strict test).")
        );

        let unofficial = enforce_policy(LicenseStatus::UnofficialBuildInPrivateCI, false, &env);
        assert_eq!(unofficial.action, EnforcementAction::Block);
        assert!(unofficial.gha_annotation.is_some());

        assert_eq!(
            enforce_policy(LicenseStatus::ExpiredUnlicensedBinary, false, &env).action,
            EnforcementAction::Block
        );

        // Non-GitHub environment: no annotation, but still a printable message.
        let env_non_gh = MockEnv {
            vars: std::collections::HashMap::new(),
        };
        let soft_non_gh = enforce_policy(
            LicenseStatus::UnlicensedSoft {
                reason: "soft test non gh".to_string(),
            },
            false,
            &env_non_gh,
        );
        assert_eq!(soft_non_gh.action, EnforcementAction::Warn);
        assert!(!soft_non_gh.message.is_empty());
        assert!(soft_non_gh.gha_annotation.is_none());

        let unofficial_non_gh = enforce_policy(
            LicenseStatus::UnofficialBuildInPrivateCI,
            false,
            &env_non_gh,
        );
        assert_eq!(unofficial_non_gh.action, EnforcementAction::Block);
        assert!(unofficial_non_gh.gha_annotation.is_none());
    }

    #[test]
    fn test_verify_internal_local_dev() {
        let env = MockEnv {
            vars: std::collections::HashMap::new(),
        };
        let status = LicenseGate::verify_internal(&env, false, 0);
        assert_eq!(status, LicenseStatus::Valid);
    }

    #[test]
    fn test_verify_internal_public_ci() {
        let mut vars = std::collections::HashMap::new();
        vars.insert("GITHUB_ACTIONS".to_string(), "true".to_string());
        vars.insert("GITHUB_REPOSITORY".to_string(), "foo/bar".to_string());

        let path = std::env::temp_dir().join(format!(
            "github_payload_{:?}.json",
            std::thread::current().id()
        ));
        fs::write(&path, r#"{"repository":{"private":false}}"#).unwrap();
        vars.insert(
            "GITHUB_EVENT_PATH".to_string(),
            path.to_string_lossy().into_owned(),
        );

        let env = MockEnv { vars };

        let status = LicenseGate::verify_internal(&env, false, 0);
        assert_eq!(status, LicenseStatus::PublicOrGrantedUse);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn test_verify_internal_unofficial_private_ci() {
        let mut vars = std::collections::HashMap::new();
        vars.insert("CI".to_string(), "true".to_string());
        let env = MockEnv { vars };

        let status = LicenseGate::verify_internal(&env, false, 0); // is_official = false
        assert_eq!(status, LicenseStatus::UnofficialBuildInPrivateCI);
    }

    #[test]
    fn test_verify_internal_official_valid_timestamp() {
        let mut vars = std::collections::HashMap::new();
        vars.insert("CI".to_string(), "true".to_string());
        let env = MockEnv { vars };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Valid if timestamp is within last 24h (e.g. now - 1 hour)
        let build_timestamp = now - 3600;

        let status = LicenseGate::verify_internal(&env, true, build_timestamp);
        assert!(matches!(status, LicenseStatus::UnlicensedSoft { .. }));
    }

    #[test]
    fn test_verify_internal_official_expired_timestamp() {
        let mut vars = std::collections::HashMap::new();
        vars.insert("CI".to_string(), "true".to_string());
        let env = MockEnv { vars };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Expired if timestamp is older than 90 days (e.g. now - 100 days)
        let build_timestamp = now.saturating_sub(100 * 24 * 60 * 60);

        let status = LicenseGate::verify_internal(&env, true, build_timestamp);
        assert_eq!(status, LicenseStatus::ExpiredUnlicensedBinary);
    }

    #[test]
    fn test_verify_internal_official_future_timestamp_blocked() {
        let mut vars = std::collections::HashMap::new();
        vars.insert("CI".to_string(), "true".to_string());
        let env = MockEnv { vars };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Invalid if timestamp is > 24h in the future
        let build_timestamp = now + 48 * 3600;

        let status = LicenseGate::verify_internal(&env, true, build_timestamp);
        assert_eq!(status, LicenseStatus::UnofficialBuildInPrivateCI);
    }

    #[test]
    fn test_verify_public_api() {
        let env = MockEnv {
            vars: std::collections::HashMap::new(),
        };
        // Verify public LicenseGate::verify entrypoint
        let status = LicenseGate::verify(&env);
        assert_eq!(status, LicenseStatus::Valid);
    }
}
