//! Durable replacement for application-owned files. Callers serialize mutations
//! and replacement together; atomic replacement alone does not order snapshots.
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
};

pub(crate) fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("missing store directory"))?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".limitscope-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Preserve unreadable/unknown schemas in place; malformed JSON gets a durable
/// recovery copy before a caller may initialize defaults.
pub(crate) fn prepare_json(
    path: &Path,
    versions: &[u64],
    healthy: impl FnOnce(&serde_json::Value) -> bool,
) -> bool {
    let raw = match fs::read(path) {
        Ok(raw) => raw,
        Err(error) => return error.kind() == io::ErrorKind::NotFound,
    };
    match serde_json::from_slice::<serde_json::Value>(&raw) {
        Ok(value)
            if value
                .get("version")
                .and_then(serde_json::Value::as_u64)
                .is_some_and(|version| versions.contains(&version)) =>
        {
            healthy(&value)
                || replace(
                    &path.with_extension(format!("recovery-{}.json", uuid::Uuid::new_v4())),
                    &raw,
                )
                .is_ok()
        }
        Ok(_) => false,
        Err(_) => replace(
            &path.with_extension(format!("recovery-{}.json", uuid::Uuid::new_v4())),
            &raw,
        )
        .is_ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn semantic_recovery_preserves_original_and_future_schema_blocks_writes() {
        let dir =
            std::env::temp_dir().join(format!("limitscope-recovery-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("store.json");
        let raw = br#"{"version":1,"enabled":"unknown"}"#;
        fs::write(&path, raw).unwrap();
        assert!(prepare_json(&path, &[1], |_| false));
        assert!(fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.path() != path && fs::read(entry.path()).unwrap() == raw));
        let future = br#"{"version":999,"enabled":true}"#;
        fs::write(&path, future).unwrap();
        assert!(!prepare_json(&path, &[1], |_| true));
        assert_eq!(fs::read(&path).unwrap(), future);
        fs::remove_dir_all(dir).unwrap();
    }
}
