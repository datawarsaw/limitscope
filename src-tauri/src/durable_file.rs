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

/// Keeps an exact `<stem>.recovery-<uuid>.json` sibling copy of `raw`. A
/// sibling copy already holding exactly these bytes is reused, so a store
/// that reloads the same degraded bytes on every start does not accumulate
/// duplicates; an existing copy is never overwritten. A new copy must read
/// back byte-for-byte, otherwise it is removed and the caller fails closed.
pub(crate) fn preserve_copy(path: &Path, raw: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("missing store directory"))?;
    let stem = path
        .file_stem()
        .ok_or_else(|| io::Error::other("missing store file name"))?
        .to_string_lossy();
    let prefix = format!("{stem}.recovery-");
    if let Ok(entries) = fs::read_dir(parent) {
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(&prefix)
                && name.ends_with(".json")
                && fs::read(entry.path()).is_ok_and(|bytes| bytes == raw)
            {
                return Ok(());
            }
        }
    }
    let copy = path.with_extension(format!("recovery-{}.json", uuid::Uuid::new_v4()));
    if copy.exists() {
        return Err(io::Error::from(io::ErrorKind::AlreadyExists));
    }
    replace(&copy, raw)?;
    if fs::read(&copy)? != raw {
        let _ = fs::remove_file(&copy);
        return Err(io::Error::other("recovery copy did not verify"));
    }
    Ok(())
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

    #[test]
    fn preserve_copy_reuses_exact_copy_and_never_overwrites_others() {
        let dir =
            std::env::temp_dir().join(format!("limitscope-preserve-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("store-v1.json");
        let other_store = dir.join("other-v1.recovery-x.json");
        let older = dir.join("store-v1.recovery-older.json");
        fs::write(&other_store, b"{ bad").unwrap();
        fs::write(&older, b"older").unwrap();
        let copies = || {
            fs::read_dir(&dir)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|e| e.file_name().to_string_lossy().starts_with("store-v1.recovery-"))
                .count()
        };
        preserve_copy(&path, b"{ bad").unwrap();
        assert_eq!(copies(), 2, "another store's identical copy is not reused");
        preserve_copy(&path, b"{ bad").unwrap();
        preserve_copy(&path, b"older").unwrap();
        assert_eq!(copies(), 2, "exact copies are reused, not duplicated");
        assert_eq!(fs::read(&older).unwrap(), b"older");
        assert_eq!(fs::read(&other_store).unwrap(), b"{ bad");
        fs::remove_dir_all(dir).unwrap();
    }
}
