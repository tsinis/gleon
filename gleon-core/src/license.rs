use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use globset::Glob;
use serde::{Deserialize, Serialize};

const PUBLIC_KEY_BYTES: &[u8; 32] = &[
    198, 122, 238, 222, 114, 183, 214, 45, 12, 191, 109, 14, 127, 240, 71, 98, 250, 48, 199, 168,
    86, 17, 219, 195, 33, 114, 88, 143, 221, 62, 131, 23,
];

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
    env_provider
        .get_var("GITHUB_EVENT_PATH")
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|content| serde_json::from_str::<GithubEventPayload>(&content).ok())
        .and_then(|payload| payload.repository)
        .map(|repo| repo.private)
        .unwrap_or(false)
}

pub fn identify_context(env_provider: &dyn crate::git::EnvProvider) -> ExecutionContext {
    // 1. GitHub Actions
    if let Some(repo) = env_provider.get_var("GITHUB_REPOSITORY") {
        let is_private = parse_github_event_payload_is_private(env_provider);
        return ExecutionContext::GitHubActions { repo, is_private };
    }
    // 2. GitLab / Generic CI
    if let Some(repo) = env_provider.get_var("CI_PROJECT_PATH") {
        return ExecutionContext::GenericCI { repo };
    }
    // 3. Local Dev
    ExecutionContext::LocalDev
}

pub struct LicenseGate;

impl LicenseGate {
    pub fn verify(env_provider: &dyn crate::git::EnvProvider) -> LicenseStatus {
        let context = identify_context(env_provider);
        let is_official = option_env!("GLEON_OFFICIAL_SECRET").is_some();
        let build_timestamp_str = option_env!("GLEON_BUILD_TIMESTAMP").unwrap_or("0");
        let build_timestamp: u64 = build_timestamp_str.trim().parse().unwrap_or(0);

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

        let has_valid_license = match env_provider.get_var("GLEON_LICENSE_KEY") {
            Some(key) => Self::verify_key(&key, &context, now),
            None => Err("No GLEON_LICENSE_KEY environment variable provided".to_string()),
        };

        match has_valid_license {
            Ok(LicenseValidity::Valid) => LicenseStatus::Valid,
            Ok(LicenseValidity::GracePeriod { reason }) => LicenseStatus::UnlicensedSoft { reason },
            Err(e) => {
                // An official binary MUST have both the official secret and a valid non-zero build timestamp.
                let is_valid_official_build = is_official && build_timestamp > 0;

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
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(key)
            .map_err(|_| "Invalid base64 encoding")?;
        if decoded.len() <= 64 {
            return Err("License key payload too short".to_string());
        }

        let (payload_bytes, signature_bytes) = decoded.split_at(decoded.len() - 64);

        let signature = Signature::from_slice(signature_bytes)
            .map_err(|_| "Invalid Ed25519 signature format")?;
        let pub_key = VerifyingKey::from_bytes(PUBLIC_KEY_BYTES)
            .map_err(|_| "Invalid embedded public key")?;

        pub_key
            .verify(payload_bytes, &signature)
            .map_err(|_| "Cryptographic signature verification failed")?;

        let payload: LicensePayload =
            serde_json::from_slice(payload_bytes).map_err(|_| "Invalid license payload JSON")?;

        let repo_to_check = match context {
            ExecutionContext::GitHubActions { repo, .. } => Some(repo),
            ExecutionContext::GenericCI { repo } => Some(repo),
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

pub fn enforce_policy(
    status: LicenseStatus,
    strict_mode: bool,
    env_provider: &dyn crate::git::EnvProvider,
) {
    match status {
        LicenseStatus::Valid | LicenseStatus::PublicOrGrantedUse => {}
        LicenseStatus::UnlicensedSoft { reason } => {
            eprintln!("====================================================");
            eprintln!("[GLEON COMPLIANCE NOTICE] Unlicensed production use detected.");
            eprintln!("Reason: {reason}");
            eprintln!("This may fall outside the BSL Additional Use Grant.");
            eprintln!("Get a commercial license at https://gleon.rs");
            eprintln!("====================================================");

            if env_provider.get_var("GITHUB_ACTIONS").is_some() {
                println!("::warning title=Gleon Compliance::Unlicensed usage detected ({reason}).");
            }

            if strict_mode {
                std::process::exit(42);
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
            std::process::exit(42);
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
        let env = MockEnv { vars };

        let ctx = identify_context(&env);
        assert_eq!(
            ctx,
            ExecutionContext::GitHubActions {
                repo: "foo/bar".to_string(),
                is_private: false
            }
        );
    }

    #[test]
    fn test_identify_context_generic_ci() {
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
}
