use crate::cli::Cli;
use crate::config::{ConfigError, GleonConfig};
use crate::platform::{PlatformEnv, PlatformError, PlatformInfo, PlatformResolver};
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Platform(#[from] PlatformError),
}

#[derive(Debug)]
pub struct ResolvedContext {
    pub config: Option<GleonConfig>,
    pub platform: PlatformInfo,
}

impl ResolvedContext {
    pub fn from_cli(cli: &Cli) -> Result<Self, ContextError> {
        let (config, _config_specified) = if let Some(ref path) = cli.config {
            (Some(GleonConfig::load_from_file(path)?), true)
        } else {
            let default_path = PathBuf::from("gleon.yaml");
            if default_path.exists() {
                (Some(GleonConfig::load_from_file(&default_path)?), false)
            } else {
                (None, false)
            }
        };

        let env = PlatformEnv::from_env();
        let platform = PlatformResolver::resolve(
            cli.os.as_deref(),
            cli.arch.as_deref(),
            cli.renderer.as_deref(),
            &cli.labels,
            &env,
            config.as_ref().and_then(|c| c.platform.as_ref()),
        )?;

        Ok(Self { config, platform })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Commands;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_from_cli_with_config_path() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("my_config.yaml");
        let mut file = File::create(&config_path).unwrap();
        writeln!(
            file,
            "required_version: \">=0.1.0\"\nscreenshots:\n  - include: \"*.png\""
        )
        .unwrap();

        let cli = Cli {
            branch: None,
            os: None,
            arch: None,
            renderer: None,
            labels: vec![],
            verbose: false,
            quiet: false,
            config: Some(config_path),
            command: Commands::Status,
        };

        let context = ResolvedContext::from_cli(&cli).unwrap();
        assert!(context.config.is_some());
    }

    #[test]
    fn test_from_cli_no_config_no_default_file() {
        // Ensure no local gleon.yaml is read/exists in the test execution directory
        let default_path = std::path::PathBuf::from("gleon.yaml");
        let _temp_removed = if default_path.exists() {
            std::fs::remove_file(&default_path).ok();
            true
        } else {
            false
        };

        let cli = Cli {
            branch: None,
            os: None,
            arch: None,
            renderer: None,
            labels: vec![],
            verbose: false,
            quiet: false,
            config: None,
            command: Commands::Status,
        };

        let context = ResolvedContext::from_cli(&cli).unwrap();
        assert!(context.config.is_none());
    }

    #[test]
    fn test_from_cli_no_config_with_default_file() {
        let default_path = std::path::PathBuf::from("gleon.yaml");
        // Save existing one if any to not break developer environment
        let backup_path = std::path::PathBuf::from("gleon.yaml.bak");
        let had_backup = if default_path.exists() {
            std::fs::rename(&default_path, &backup_path).is_ok()
        } else {
            false
        };

        // Create temporary default yaml
        let mut file = File::create(&default_path).unwrap();
        writeln!(
            file,
            "required_version: \">=0.1.0\"\nscreenshots:\n  - include: \"*.png\""
        )
        .unwrap();

        let cli = Cli {
            branch: None,
            os: None,
            arch: None,
            renderer: None,
            labels: vec![],
            verbose: false,
            quiet: false,
            config: None,
            command: Commands::Status,
        };

        let context = ResolvedContext::from_cli(&cli);

        // Cleanup before asserts to avoid leaving temp file in case of failure (Assert-After-Cleanup rule)
        let _ = std::fs::remove_file(&default_path);
        if had_backup {
            let _ = std::fs::rename(&backup_path, &default_path);
        }

        let context = context.unwrap();
        assert!(context.config.is_some());
    }
}
