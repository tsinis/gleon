//! I/O utilities for gleon.

use std::path::Path;

/// Errors that can occur during I/O operations.
#[derive(Debug, thiserror::Error)]
pub enum IoError {
    /// IO error during file or directory access.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Error deserializing JSON content.
    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),
}

pub fn load_json<T: serde::de::DeserializeOwned, P: AsRef<Path>>(path: P) -> Result<T, IoError> {
    let path = path.as_ref();
    std::fs::File::open(path)
        .map_err(|e| {
            tracing::debug!("Failed to open JSON file at {:?}: {}", path, e);
            IoError::Io(e)
        })
        .and_then(|file| {
            let reader = std::io::BufReader::new(file);
            serde_json::from_reader(reader).map_err(|e| {
                tracing::error!("Failed to parse JSON file at {:?}: {}", path, e);
                IoError::JsonParse(e)
            })
        })
}

/// Loads the JSON (or uses Default if missing), applies the closure, and saves it atomically.
pub fn update_json_atomically<T, P, F, D, E>(path: P, default_fn: D, f: F) -> Result<(), E>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
    P: AsRef<Path>,
    D: FnOnce() -> T,
    F: FnOnce(&mut T) -> Result<(), E>,
    E: From<IoError>,
{
    let path = path.as_ref();
    let mut value = match load_json(path) {
        Ok(val) => val,
        Err(IoError::Io(ref e)) if e.kind() == std::io::ErrorKind::NotFound => default_fn(),
        Err(e) => return Err(E::from(e)),
    };

    f(&mut value)?;
    save_json_atomically(path, &value).map_err(E::from)
}

pub fn save_json_atomically<T: serde::Serialize, P: AsRef<Path>>(
    path: P,
    value: &T,
) -> Result<(), IoError> {
    let path = path.as_ref();
    let parent = match path.parent() {
        Some(p) if p.as_os_str().is_empty() => Path::new("."),
        Some(p) => p,
        None => {
            return Err(IoError::Io(std::io::Error::new(
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
        .map_err(|e| IoError::Io(e.into_error()))?;

    #[cfg(all(unix, not(miri)))]
    let perms_result = {
        use std::os::unix::fs::PermissionsExt;
        temp_file
            .as_file()
            .metadata()
            .map_err(IoError::Io)
            .and_then(|metadata| {
                let mut perms = metadata.permissions();
                if let Ok(existing) = std::fs::metadata(path) {
                    perms.set_mode(existing.permissions().mode());
                }
                temp_file
                    .as_file()
                    .set_permissions(perms)
                    .map_err(IoError::Io)
            })
    };
    #[cfg(not(all(unix, not(miri))))]
    let perms_result: Result<(), IoError> = Ok(());

    perms_result
        .and_then(|_| temp_file.as_file().sync_all().map_err(IoError::Io))
        .and_then(|_| {
            temp_file.persist(path).map_err(|e| {
                tracing::error!("Failed to save JSON atomically to {:?}: {}", path, e);
                IoError::Io(e.error)
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
        if let Err(IoError::Io(err)) = result {
            assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
            assert_eq!(
                err.to_string(),
                "Cannot resolve parent directory for root path"
            );
        } else {
            panic!("Expected IoError::Io with InvalidInput");
        }
    }

    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug, Default)]
    struct TestData {
        count: u32,
    }

    #[test]
    fn test_update_json_atomically_missing_file_uses_default() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("data.json");

        update_json_atomically::<TestData, _, _, _, IoError>(
            &file_path,
            TestData::default,
            |data: &mut TestData| {
                data.count += 5;
                Ok(())
            },
        )
        .unwrap();

        let loaded: TestData = load_json(&file_path).unwrap();
        assert_eq!(loaded, TestData { count: 5 });
    }

    #[test]
    fn test_update_json_atomically_corrupted_file_fails() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("corrupt.json");

        // Write invalid JSON content to simulate file corruption
        std::fs::write(&file_path, "{ invalid json ").unwrap();

        let result = update_json_atomically::<TestData, _, _, _, IoError>(
            &file_path,
            TestData::default,
            |data: &mut TestData| {
                data.count += 5;
                Ok(())
            },
        );

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), IoError::JsonParse(_)));

        // Verify the corrupted file content was NOT overwritten
        let raw_content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(raw_content, "{ invalid json ");
    }
}
