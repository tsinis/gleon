//! I/O utilities for gleon.

use crate::config::ConfigError;
use std::path::Path;

pub fn load_json<T: serde::de::DeserializeOwned, P: AsRef<Path>>(
    path: P,
) -> Result<T, ConfigError> {
    let path = path.as_ref();
    let file = std::fs::File::open(path).map_err(|e| {
        tracing::debug!("Failed to open JSON file at {:?}: {}", path, e);
        ConfigError::Io(e)
    })?;
    let reader = std::io::BufReader::new(file);
    serde_json::from_reader(reader).map_err(|e| {
        tracing::error!("Failed to parse JSON file at {:?}: {}", path, e);
        ConfigError::JsonParse(e)
    })
}

pub fn save_json_atomically<T: serde::Serialize, P: AsRef<Path>>(
    path: P,
    value: &T,
) -> Result<(), ConfigError> {
    let path = path.as_ref();
    let parent = match path.parent() {
        Some(p) if p.as_os_str().is_empty() => Path::new("."),
        Some(p) => p,
        None => Path::new("."),
    };
    std::fs::create_dir_all(parent)?;

    let file_name = path.file_name().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid file name")
    })?;

    let temp_file = tempfile::Builder::new()
        .prefix(file_name)
        .suffix(".tmp")
        .tempfile_in(parent)?;

    use std::io::Write;
    let mut writer = std::io::BufWriter::new(temp_file);
    serde_json::to_writer_pretty(&mut writer, value)?;
    writer.flush()?;
    let temp_file = writer
        .into_inner()
        .map_err(|e| ConfigError::Io(e.into_error()))?;

    #[cfg(all(unix, not(miri)))]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = temp_file
            .as_file()
            .metadata()
            .map_err(ConfigError::Io)?
            .permissions();
        if let Ok(existing) = std::fs::metadata(path) {
            perms.set_mode(existing.permissions().mode());
        }
        temp_file
            .as_file()
            .set_permissions(perms)
            .map_err(ConfigError::Io)?;
    }

    temp_file.as_file().sync_all().map_err(ConfigError::Io)?;

    temp_file
        .persist(path)
        .map_err(|e| {
            tracing::error!("Failed to save JSON atomically to {:?}: {}", path, e);
            ConfigError::Io(e.error)
        })
        .map(|_| {
            if let Ok(dir) = std::fs::File::open(parent) {
                let _ = dir.sync_all(); // Ignore directory fsync errors, especially on Windows
            }
        })
}
