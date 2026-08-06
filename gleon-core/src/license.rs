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
    pub static PUBLIC_KEY_BYTES: std::cell::RefCell<[u8; 32]> = const {
        std::cell::RefCell::new([
            198, 122, 238, 222, 114, 183, 214, 45, 12, 191, 109, 14, 127, 240, 71, 98, 250, 48, 199, 168,
            86, 17, 219, 195, 33, 114, 88, 143, 221, 62, 131, 23,
        ])
    };
}

#[cfg(not(test))]
fn get_public_key_bytes() -> [u8; 32] {
    *PUBLIC_KEY_BYTES
}

#[cfg(test)]
fn get_public_key_bytes() -> [u8; 32] {
    PUBLIC_KEY_BYTES.with(|b| *b.borrow())
}

#[derive(Serialize, Deserialize, Debug)]
pub struct LicensePayload {
    pub owner: String,
    pub repo_pattern: String,
    pub expires_at: u64,
    pub license_id: String,
}

enum LicenseValidity {
    Valid,
    GracePeriod { reason: String },
}

#[derive(Debug, PartialEq, Eq)]
pub enum LicenseStatus {
    Valid,
    PublicOrGrantedUse,
    UnlicensedSoft { reason: String },
    UnofficialBuildInPrivateCI,
    ExpiredUnlicensedBinary,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ExecutionContext {
    GitHubActions { repo: String, is_private: bool },
    GenericCI { repo: String },
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

pub fn parse_github_event_payload_is_private(env_provider: &dyn crate::git::EnvProvider) -> bool {
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
        .map(|repo| repo.private)
        .unwrap_or(true) // Fail closed: if event payload exists but fails to parse, treat as private
}

pub fn identify_context(env_provider: &dyn crate::git::EnvProvider) -> ExecutionContext {
    // 1. GitHub Actions
    if let Some(repo) = env_provider.get_var("GITHUB_REPOSITORY") {
        let is_private = parse_github_event_payload_is_private(env_provider);
        return ExecutionContext::GitHubActions { repo, is_private };
    }
    // 2. Try to get project path explicitly provided or from generic CI variables
    let explicit_repo = env_provider
        .get_var("GLEON_PROJECT_PATH")
        .or_else(|| env_provider.get_var("CI_PROJECT_PATH"))
        .or_else(|| env_provider.get_var("TRAVIS_REPO_SLUG"))
        .or_else(|| env_provider.get_var("BITBUCKET_REPO_FULL_NAME"))
        .or_else(|| {
            // CircleCI splits username and reponame
            let user = env_provider.get_var("CIRCLE_PROJECT_USERNAME")?;
            let repo = env_provider.get_var("CIRCLE_PROJECT_REPONAME")?;
            Some(format!("{}/{}", user, repo))
        });

    if let Some(repo) = explicit_repo {
        return ExecutionContext::GenericCI { repo };
    }

    // 3. Fallback for other CIs (CircleCI, Travis, Azure, Buildkite, Drone, TeamCity, Bitbucket, generic "CI=true")
    if env_provider.get_var("CI").is_some()
        || env_provider.get_var("CONTINUOUS_INTEGRATION").is_some()
        || env_provider.get_var("CIRCLECI").is_some()
        || env_provider.get_var("TRAVIS").is_some()
        || env_provider.get_var("GITLAB_CI").is_some()
        || env_provider.get_var("TF_BUILD").is_some()
        || env_provider.get_var("BUILDKITE").is_some()
        || env_provider.get_var("DRONE").is_some()
        || env_provider.get_var("TEAMCITY_VERSION").is_some()
        || env_provider.get_var("BITBUCKET_COMMIT").is_some()
    {
        return ExecutionContext::GenericCI {
            repo: "".to_string(),
        };
    }
    // 4. Local Dev
    ExecutionContext::LocalDev
}

pub struct LicenseGate;

impl LicenseGate {
    pub fn verify(env_provider: &dyn crate::git::EnvProvider) -> LicenseStatus {
        let is_official = option_env!("GLEON_OFFICIAL_SECRET").is_some();
        let build_timestamp_str = option_env!("GLEON_BUILD_TIMESTAMP").unwrap_or("0");
        let build_timestamp: u64 = build_timestamp_str.trim().parse().unwrap_or(0);

        Self::verify_internal(env_provider, is_official, build_timestamp)
    }

    fn verify_internal(
        env_provider: &dyn crate::git::EnvProvider,
        is_official: bool,
        build_timestamp: u64,
    ) -> LicenseStatus {
        let context = identify_context(env_provider);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // If local dev, we always pass silently.
        if let ExecutionContext::LocalDev = context {
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

        let has_valid_license = match env_provider
            .get_var("GLEON_LICENSE_KEY")
            .filter(|k| !k.trim().is_empty())
        {
            Some(key) => Self::verify_key(&key, &context, now),
            None => Err("No GLEON_LICENSE_KEY environment variable provided".to_string()),
        };

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

                // Time-bomb check: > 90 days old (approx 90 * 24 * 60 * 60 = 7776000 seconds)
                if is_valid_official_build && is_private_ci && now > build_timestamp + 7776000 {
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
                if repo.is_empty() {
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
            } else {
                return Err("License has expired".to_string());
            }
        }

        Ok(LicenseValidity::Valid)
    }
}

/// Outcome of enforcing licensing policy.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum EnforcementAction {
    Allow,
    Warn,
    Block,
}

pub fn enforce_policy(
    status: LicenseStatus,
    strict_mode: bool,
    env_provider: &dyn crate::git::EnvProvider,
) -> EnforcementAction {
    match status {
        LicenseStatus::Valid | LicenseStatus::PublicOrGrantedUse => EnforcementAction::Allow,
        LicenseStatus::UnlicensedSoft { reason } => {
            eprintln!("====================================================");
            eprintln!("[GLEON COMPLIANCE NOTICE] Unlicensed production use detected.");
            eprintln!("Reason: {reason}");
            eprintln!("This may fall outside the BSL Additional Use Grant.");
            eprintln!("Get a commercial license at https://gleon.rs");
            eprintln!("====================================================");

            if env_provider.get_var("GITHUB_ACTIONS").is_some() {
                if strict_mode {
                    eprintln!(
                        "::error title=Gleon Compliance::Unlicensed usage detected ({reason})."
                    );
                } else {
                    eprintln!(
                        "::warning title=Gleon Compliance::Unlicensed usage detected ({reason})."
                    );
                }
            }

            if strict_mode {
                EnforcementAction::Block
            } else {
                EnforcementAction::Warn
            }
        }
        LicenseStatus::UnofficialBuildInPrivateCI | LicenseStatus::ExpiredUnlicensedBinary => {
            eprintln!("====================================================");
            eprintln!("[GLEON COMPLIANCE ERROR] Execution blocked.");
            eprintln!(
                "Self-compiled or expired official binaries (>3 months) cannot run in unlicensed private CI."
            );
            eprintln!("Get a valid commercial license at https://gleon.rs");
            eprintln!("====================================================");

            if env_provider.get_var("GITHUB_ACTIONS").is_some() {
                eprintln!(
                    "::error title=Gleon Compliance::Execution blocked. Self-compiled or expired official binaries cannot run in unlicensed private CI."
                );
            }

            EnforcementAction::Block
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockEnv {
        vars: std::collections::HashMap<String, String>,
    }

    impl crate::git::EnvProvider for MockEnv {
        fn get_var(&self, key: &str) -> Option<String> {
            self.vars.get(key).cloned()
        }
    }

    #[test]
    fn test_identify_context_github_actions_public() {
        let mut vars = std::collections::HashMap::new();
        vars.insert("GITHUB_REPOSITORY".to_string(), "foo/bar".to_string());

        let path = std::env::temp_dir().join(format!(
            "github_payload_ctx_{:?}.json",
            std::thread::current().id()
        ));
        std::fs::write(&path, r#"{"repository":{"private":false}}"#).unwrap();
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

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_identify_context_github_actions_missing_payload() {
        let mut vars = std::collections::HashMap::new();
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
    fn test_identify_context_other_ci() {
        let mut vars = std::collections::HashMap::new();
        vars.insert("CIRCLECI".to_string(), "true".to_string());
        let env1 = MockEnv { vars: vars.clone() };
        assert_eq!(
            identify_context(&env1),
            ExecutionContext::GenericCI {
                repo: "".to_string()
            }
        );

        vars.insert("GLEON_PROJECT_PATH".to_string(), "org/repo".to_string());
        let env2 = MockEnv { vars };
        assert_eq!(
            identify_context(&env2),
            ExecutionContext::GenericCI {
                repo: "org/repo".to_string()
            }
        );
    }

    #[test]
    fn test_parse_github_event_payload_is_private() {
        use std::io::Write;
        let temp = tempfile::tempdir().unwrap();
        let payload_path = temp.path().join("event.json");
        let mut file = std::fs::File::create(&payload_path).unwrap();
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
    fn test_enforce_policy_outcomes() {
        let mut vars = std::collections::HashMap::new();
        vars.insert("GITHUB_ACTIONS".to_string(), "true".to_string());
        let env = MockEnv { vars };

        assert_eq!(
            enforce_policy(LicenseStatus::Valid, false, &env),
            EnforcementAction::Allow
        );
        assert_eq!(
            enforce_policy(LicenseStatus::PublicOrGrantedUse, false, &env),
            EnforcementAction::Allow
        );
        assert_eq!(
            enforce_policy(
                LicenseStatus::UnlicensedSoft {
                    reason: "soft test".to_string()
                },
                false,
                &env
            ),
            EnforcementAction::Warn
        );
        assert_eq!(
            enforce_policy(
                LicenseStatus::UnlicensedSoft {
                    reason: "strict test".to_string()
                },
                true,
                &env
            ),
            EnforcementAction::Block
        );
        assert_eq!(
            enforce_policy(LicenseStatus::UnofficialBuildInPrivateCI, false, &env),
            EnforcementAction::Block
        );
        assert_eq!(
            enforce_policy(LicenseStatus::ExpiredUnlicensedBinary, false, &env),
            EnforcementAction::Block
        );
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
        vars.insert("GITHUB_REPOSITORY".to_string(), "foo/bar".to_string());

        let path = std::env::temp_dir().join(format!(
            "github_payload_{:?}.json",
            std::thread::current().id()
        ));
        std::fs::write(&path, r#"{"repository":{"private":false}}"#).unwrap();
        vars.insert(
            "GITHUB_EVENT_PATH".to_string(),
            path.to_string_lossy().into_owned(),
        );

        let env = MockEnv { vars };

        let status = LicenseGate::verify_internal(&env, false, 0);
        assert_eq!(status, LicenseStatus::PublicOrGrantedUse);

        let _ = std::fs::remove_file(path);
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
}
