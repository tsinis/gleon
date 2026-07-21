//! I/O utilities for gleon.

use crate::config::ConfigError;
use std::path::Path;

pub fn load_json<T: serde::de::DeserializeOwned, P: AsRef<Path>>(
    path: P,
) -> Result<T, ConfigError> {
    let path = path.as_ref();
    std::fs::File::open(path)
        .map_err(|e| {
            tracing::debug!("Failed to open JSON file at {:?}: {}", path, e);
            ConfigError::Io(e)
        })
        .and_then(|file| {
            let reader = std::io::BufReader::new(file);
            serde_json::from_reader(reader).map_err(|e| {
                tracing::error!("Failed to parse JSON file at {:?}: {}", path, e);
                ConfigError::JsonParse(e)
            })
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
        None => {
            return Err(ConfigError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Cannot resolve parent directory for root path",
            )));
        }
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
    let perms_result = {
        use std::os::unix::fs::PermissionsExt;
        temp_file
            .as_file()
            .metadata()
            .map_err(ConfigError::Io)
            .and_then(|metadata| {
                let mut perms = metadata.permissions();
                if let Ok(existing) = std::fs::metadata(path) {
                    perms.set_mode(existing.permissions().mode());
                }
                temp_file
                    .as_file()
                    .set_permissions(perms)
                    .map_err(ConfigError::Io)
            })
    };
    #[cfg(not(all(unix, not(miri))))]
    let perms_result: Result<(), ConfigError> = Ok(());

    perms_result
        .and_then(|_| temp_file.as_file().sync_all().map_err(ConfigError::Io))
        .and_then(|_| {
            temp_file.persist(path).map_err(|e| {
                tracing::error!("Failed to save JSON atomically to {:?}: {}", path, e);
                ConfigError::Io(e.error)
            })
        })
        .map(|_| {
            if let Ok(dir) = std::fs::File::open(parent) {
                let _ = dir.sync_all(); // Ignore directory fsync errors, especially on Windows
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    #[derive(Serialize)]
    struct Dummy {
        value: String,
    }

    #[test]
    fn test_save_json_atomically_root_path_fails() {
        let dummy = Dummy {
            value: "test".to_string(),
        };
        // Saving to "/" should fail because it has no parent directory
        let result = save_json_atomically(Path::new("/"), &dummy);
        assert!(result.is_err());
        if let Err(ConfigError::Io(err)) = result {
            assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
            assert_eq!(
                err.to_string(),
                "Cannot resolve parent directory for root path"
            );
        } else {
            panic!("Expected ConfigError::Io with InvalidInput");
        }
    }
}
