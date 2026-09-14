//! Standalone entitlement key generator for Gleon.
//!
//! Generates and signs Ed25519 license tokens offline for commercial visual regression
//! entitlements.

use std::fmt;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use clap::{Parser, Subcommand};
use ed25519_dalek::SigningKey;
use gleon_core::license::{LicenseGate, LicensePayload, generate_license_token};
use zeroize::{Zeroize, Zeroizing};

/// CLI arguments for the standalone `keygen` binary.
#[derive(Parser, Debug)]
#[command(
    name = "keygen",
    about = "Generate signed Ed25519 entitlement keys for Gleon",
    version
)]
struct KeygenArgs {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Generate a new Ed25519 keypair for signing licenses
    GenerateKeypair {
        /// Path to save the generated secret key (0600 permissions).
        #[arg(long, help = "Output path for the 32-byte hex secret key")]
        out: PathBuf,
    },
    /// Sign a new license entitlement
    Sign {
        /// Key ID to use for signing. Defaults to 1.
        #[arg(long, default_value = "1", help = "Key ID (kid) to embed in token")]
        kid: u8,

        /// Name of the organization or owner to license.
        #[arg(long, help = "Organization or owner name for the entitlement")]
        org: String,

        /// Glob pattern matching repositories the license applies to.
        #[arg(long, help = "Repository glob pattern (e.g. 'acme/*')")]
        repo_pattern: String,

        /// Expiration duration in days from the current time. Max 3650 days (10 years).
        #[arg(long, help = "Number of days until the license expires")]
        expires_days: u64,

        /// Optional custom license identifier. If omitted, a unique ID is auto-generated.
        #[arg(long, help = "Unique license ID (defaults to lic_<timestamp>_<hex>)")]
        license_id: Option<String>,

        /// 32-byte Ed25519 signing key in hex, base64, or file path prefixed with '@'.
        /// Note: Prefer using `GLEON_SIGNING_KEY`, '@<path>', or '-' (stdin) to avoid leaking secrets in process lists.
        #[arg(
            long,
            env = "GLEON_SIGNING_KEY",
            hide_env_values = true,
            help = "32-byte secret key (hex, base64, @<path>, or - for stdin)"
        )]
        secret_key: SecretKeyWrapper,

        /// Optional path to an audit log file (JSONL format) to append generation details.
        #[arg(long, help = "Path to append audit log (JSONL)")]
        audit_log: Option<PathBuf>,

        /// Optional public key (32 bytes hex) to validate against during self-check (for offline test keys).
        #[arg(long, hide = true, env = "GLEON_KEYGEN_TEST_PUBKEY")]
        test_pubkey: Option<String>,
    },
}

#[derive(Clone)]
struct SecretKeyWrapper(String);

impl fmt::Debug for SecretKeyWrapper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecretKeyWrapper([REDACTED])")
    }
}

impl std::str::FromStr for SecretKeyWrapper {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_string()))
    }
}

impl Drop for SecretKeyWrapper {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

fn validate_identifier(value: &str, field_name: &str) -> anyhow::Result<()> {
    if value.trim().is_empty() {
        anyhow::bail!("--{field_name} cannot be empty or whitespace");
    }
    if value != value.trim() {
        anyhow::bail!("--{field_name} cannot contain leading or trailing whitespace");
    }
    if value.len() > 128 {
        anyhow::bail!("--{field_name} exceeds maximum allowed length of 128 characters");
    }
    for ch in value.chars() {
        if !ch.is_ascii_alphanumeric() && ch != '_' && ch != '.' && ch != '-' {
            anyhow::bail!(
                "Invalid character in --{field_name}: '{ch}'. Allowed characters are [a-zA-Z0-9_.-]"
            );
        }
    }
    Ok(())
}

fn parse_secret_key(raw: &str) -> anyhow::Result<SigningKey> {
    let trimmed = raw.trim();

    if trimmed == "-" {
        use std::io::Read;
        let mut content = String::new();
        std::io::stdin()
            .read_to_string(&mut content)
            .map_err(|e| anyhow::anyhow!("Failed to read secret key from stdin: {e}"))?;
        let key = parse_secret_key(&content);
        content.zeroize();
        return key;
    }

    if let Some(path) = trimmed.strip_prefix('@') {
        let mut content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read secret key file '{path}': {e}"))?;
        let key = parse_secret_key(&content);
        content.zeroize();
        return key;
    }

    if trimmed.len() == 64 {
        let mut arr = [0u8; 32];
        if hex::decode_to_slice(trimmed, &mut arr).is_ok() {
            let key = SigningKey::from_bytes(&arr);
            arr.zeroize();
            return Ok(key);
        }
        arr.zeroize();
    }

    if let Ok(bytes) = base64::prelude::BASE64_STANDARD.decode(trimmed) {
        let mut bytes = bytes;
        if let Ok(arr) = <[u8; 32]>::try_from(bytes.as_slice()) {
            let mut arr = arr;
            let key = SigningKey::from_bytes(&arr);
            arr.zeroize();
            bytes.zeroize();
            return Ok(key);
        }
        bytes.zeroize();
    }

    anyhow::bail!(
        "Invalid secret key format. Expected 32-byte key encoded as 64-character hex, base64 string, or @<path>, or -."
    )
}

fn generate_keypair(out: &std::path::Path) -> anyhow::Result<()> {
    let mut secret_bytes = [0u8; 32];
    getrandom::fill(&mut secret_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to read secure random bytes from OS: {e}"))?;
    let signing_key = SigningKey::from_bytes(&secret_bytes);
    let public_key = signing_key.verifying_key();

    let mut secret_hex = Zeroizing::new(hex::encode(secret_bytes));
    let public_hex = hex::encode(public_key.to_bytes());

    secret_bytes.zeroize();

    if let Some(parent) = out.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| {
            anyhow::anyhow!("Failed to create parent directory for output file: {e}")
        })?;
    }

    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }

    let mut file = opts
        .open(out)
        .map_err(|e| anyhow::anyhow!("Failed to open output file: {e}"))?;

    let write_res = writeln!(file, "{}", secret_hex.as_str());
    secret_hex.zeroize();
    write_res?;

    file.sync_all()
        .map_err(|e| anyhow::anyhow!("Failed to sync output file to disk: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(out, std::fs::Permissions::from_mode(0o600));
    }

    eprintln!("Successfully generated keypair.");
    eprintln!("Secret key saved to: {}", out.display());
    eprintln!("Public key (hex): {public_hex}");

    Ok(())
}

#[derive(Clone, Copy)]
struct SignOptions<'a> {
    kid: u8,
    org: &'a str,
    repo_pattern: &'a str,
    expires_days: u64,
    license_id: Option<&'a str>,
    secret_key: &'a SecretKeyWrapper,
    audit_log: Option<&'a std::path::Path>,
    test_pubkey: Option<&'a str>,
}

fn sign(opts: SignOptions<'_>) -> anyhow::Result<()> {
    let SignOptions {
        kid,
        org,
        repo_pattern,
        expires_days,
        license_id,
        secret_key,
        audit_log,
        test_pubkey,
    } = opts;

    if expires_days == 0 {
        anyhow::bail!("--expires-days must be greater than zero");
    }
    if expires_days > 3650 {
        anyhow::bail!("--expires-days cannot exceed 3650 days (10 years)");
    }

    validate_identifier(org, "org")?;

    if let Some(id) = license_id {
        validate_identifier(id, "license-id")?;
    }

    if repo_pattern.trim().is_empty() {
        anyhow::bail!("--repo-pattern cannot be empty or whitespace");
    }
    if repo_pattern != repo_pattern.trim() {
        anyhow::bail!("--repo-pattern cannot contain leading or trailing whitespace");
    }
    if repo_pattern.contains('\n') || repo_pattern.contains('\r') {
        anyhow::bail!("--repo-pattern cannot contain newline characters");
    }
    if let Err(e) = globset::GlobBuilder::new(repo_pattern)
        .case_insensitive(true)
        .build()
    {
        anyhow::bail!("--repo-pattern is an invalid glob pattern: {e}");
    }

    let signing_key = parse_secret_key(&secret_key.0)?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| anyhow::anyhow!("System time before UNIX EPOCH: {e}"))?
        .as_secs();

    let expires_at = now.saturating_add(expires_days.saturating_mul(86_400));

    let final_license_id = if let Some(id) = license_id {
        id.to_string()
    } else {
        let mut buf = [0u8; 4];
        getrandom::fill(&mut buf).map_err(|e| {
            anyhow::anyhow!("Failed to generate secure random license ID suffix: {e}")
        })?;
        let rand_suffix = u32::from_le_bytes(buf);
        format!("lic_{now}_{rand_suffix:08x}")
    };

    let payload = LicensePayload {
        v: 1,
        owner: org.to_string(),
        repo_pattern: repo_pattern.to_string(),
        expires_at,
        license_id: final_license_id,
    };

    let token = generate_license_token(kid, &payload, &signing_key)
        .map_err(|e| anyhow::anyhow!("Failed to serialize license token: {e}"))?;

    // Self-check 1: Ensure public key matches expected public key (OFFICIAL_PUBLIC_KEYS or test_pubkey)
    let expected_pubkey = if let Some(test_pub_str) = test_pubkey {
        let mut bytes = [0u8; 32];
        hex::decode_to_slice(test_pub_str.trim(), &mut bytes)
            .map_err(|e| anyhow::anyhow!("Invalid --test-pubkey hex: {e}"))?;
        bytes
    } else {
        gleon_core::license::get_public_key(kid)
            .ok_or_else(|| anyhow::anyhow!("Self-check failed: Unknown key ID (kid: {kid})"))?
    };

    if expected_pubkey != signing_key.verifying_key().to_bytes() {
        anyhow::bail!(
            "Self-check failed: The generated public key does not match OFFICIAL_PUBLIC_KEYS for kid {kid}."
        );
    }

    // Self-check 2: Full round-trip verification of the generated token
    LicenseGate::verify_token_integrity_with_public_key(&token, &expected_pubkey)
        .map_err(|e| anyhow::anyhow!("Self-check failed: {e}"))?;

    if let Some(path) = audit_log {
        write_audit_log(path, now, kid, &payload)?;
    }

    println!("{token}");
    Ok(())
}

fn write_audit_log(
    path: &std::path::Path,
    timestamp: u64,
    kid: u8,
    payload: &LicensePayload,
) -> anyhow::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("Failed to create parent directory for audit log: {e}"))?;
    }

    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts
        .open(path)
        .map_err(|e| anyhow::anyhow!("Failed to open audit log file: {e}"))?;

    let audit_entry = serde_json::json!({
        "timestamp": timestamp,
        "action": "sign_license",
        "v": payload.v,
        "kid": kid,
        "owner": payload.owner,
        "repo_pattern": payload.repo_pattern,
        "expires_at": payload.expires_at,
        "license_id": payload.license_id
    });

    writeln!(file, "{audit_entry}")?;
    file.sync_all()
        .map_err(|e| anyhow::anyhow!("Failed to sync audit log to disk: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }

    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args = KeygenArgs::parse();

    match args.command {
        Commands::GenerateKeypair { out } => generate_keypair(&out),
        Commands::Sign {
            kid,
            org,
            repo_pattern,
            expires_days,
            ref license_id,
            ref secret_key,
            ref audit_log,
            ref test_pubkey,
        } => sign(SignOptions {
            kid,
            org: &org,
            repo_pattern: &repo_pattern,
            expires_days,
            license_id: license_id.as_deref(),
            secret_key,
            audit_log: audit_log.as_deref(),
            test_pubkey: test_pubkey.as_deref(),
        }),
    }
}
