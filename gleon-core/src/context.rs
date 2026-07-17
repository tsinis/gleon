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
