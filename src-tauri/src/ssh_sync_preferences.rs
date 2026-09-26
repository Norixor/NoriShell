//! Retires legacy SSH sync preference intents without changing local preferences.

use std::{fs, io::Write, path::Path};

use serde_json::Value;
use uuid::Uuid;

pub(crate) struct SshSyncPreferencesService;

impl SshSyncPreferencesService {
    pub(crate) fn new(app_data_directory: &Path) -> Result<Self, String> {
        let path = app_data_directory
            .join("ssh-sync-preferences")
            .join("state.json");
        match fs::read(&path) {
            Ok(bytes) if bytes.len() <= 1024 * 1024 => {
                match serde_json::from_slice::<Value>(&bytes) {
                    Ok(mut checkpoint) => {
                        if let Some(object) = checkpoint.as_object_mut() {
                            if object.remove("pending").is_some()
                                && persist_without_pending(&path, &checkpoint).is_err()
                            {
                                report_cleanup_failure();
                            }
                        } else {
                            report_cleanup_failure();
                        }
                    }
                    Err(_) => report_cleanup_failure(),
                }
            }
            Ok(_) => report_cleanup_failure(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => report_cleanup_failure(),
        }
        Ok(Self)
    }
}

fn persist_without_pending(path: &Path, checkpoint: &Value) -> std::io::Result<()> {
    let directory = path.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    serde_json::to_writer(&mut temporary, checkpoint)?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    temporary.persist(path)?;
    #[cfg(unix)]
    fs::File::open(directory)?.sync_all()?;
    Ok(())
}

fn report_cleanup_failure() {
    eprintln!(
        "ssh sync legacy preferences pending cleanup failed: code=preferences.legacy_pending_cleanup_failed diagnostic_id={}; checkpoint cleanup is unconfirmed and will be rechecked at next startup",
        Uuid::new_v4()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn startup_discards_legacy_pending_and_preserves_local_checkpoint() {
        let directory = tempfile::tempdir().unwrap();
        let checkpoint_directory = directory.path().join("ssh-sync-preferences");
        fs::create_dir(&checkpoint_directory).unwrap();
        let path = checkpoint_directory.join("state.json");
        let current = json!({"product": "NoriShell", "version": 1, "groups": {"application": {"locale": "zh-CN"}}});
        let times = json!({"application": 1_700_000_000_000_i64});
        for ready in [false, true] {
            let checkpoint = json!({
                "current": current,
                "groupUpdateTimes": times,
                "pending": {"id": "legacy", "ready": ready, "desired": {"application": {"locale": "en"}}}
            });
            fs::write(&path, serde_json::to_vec(&checkpoint).unwrap()).unwrap();
            SshSyncPreferencesService::new(directory.path()).unwrap();
            let stored: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            assert!(stored.get("pending").is_none());
            assert_eq!(stored["current"], current);
            assert_eq!(stored["groupUpdateTimes"], times);
        }
    }

    #[test]
    fn malformed_legacy_checkpoint_does_not_block_sync_startup() {
        let directory = tempfile::tempdir().unwrap();
        let checkpoint_directory = directory.path().join("ssh-sync-preferences");
        fs::create_dir(&checkpoint_directory).unwrap();
        let path = checkpoint_directory.join("state.json");
        fs::write(&path, b"not-json").unwrap();
        SshSyncPreferencesService::new(directory.path()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"not-json");
    }
}
