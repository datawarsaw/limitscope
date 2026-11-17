//! Consistent bounded SQLite snapshot, held only in memory.
//! Read-only source access uses SQLite reader locks (including WAL coordination).
//! No SQL writes or checkpoint commands are issued to the source.
use rusqlite::{
    backup::{Backup, StepResult},
    Connection, OpenFlags,
};
use std::{
    path::Path,
    time::{Duration, Instant},
};

pub(crate) fn snapshot(path: &Path) -> Result<Connection, String> {
    let source = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| "Could not open source database read-only".to_string())?;
    source
        .busy_timeout(Duration::from_millis(100))
        .map_err(|_| "Could not bound SQLite wait")?;
    let page_size: i64 = source
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .map_err(|_| "Could not read SQLite page size")?;
    let mut destination =
        Connection::open_in_memory().map_err(|_| "Could not allocate SQLite snapshot")?;
    {
        let backup = Backup::new(&source, &mut destination)
            .map_err(|_| "Could not start SQLite snapshot")?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if Instant::now() >= deadline {
                return Err("SQLite snapshot timed out; progress retained".into());
            }
            match backup.step(256).map_err(|_| "SQLite snapshot rejected")? {
                StepResult::Done => break,
                StepResult::More => {
                    if i64::from(backup.progress().pagecount).saturating_mul(page_size)
                        > 256 * 1024 * 1024
                    {
                        return Err("SQLite snapshot exceeds resource limit".into());
                    }
                }
                StepResult::Busy | StepResult::Locked => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                _ => return Err("SQLite snapshot incomplete".into()),
            }
        }
    }
    let check: String = destination
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|_| "SQLite snapshot integrity check failed")?;
    if check != "ok" {
        return Err("SQLite snapshot integrity rejected".into());
    }
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(std::path::PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn fixture() -> Fixture {
        let path =
            std::env::temp_dir().join(format!("limitscope-snapshot-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Fixture(path)
    }
    #[test]
    fn wal_checkpoint_and_rotation_produce_only_complete_transactions() {
        let dir = fixture();
        let path = dir.0.join("source.db");
        let mut writer = Connection::open(&path).unwrap();
        writer.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE pair(id INTEGER PRIMARY KEY, revision INTEGER); INSERT INTO pair VALUES(1,0),(2,0);").unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        std::thread::scope(|scope| {
            let barrier2 = barrier.clone();
            scope.spawn(move || {
                barrier2.wait();
                for revision in 1..=100 {
                    let tx = writer.transaction().unwrap();
                    tx.execute("UPDATE pair SET revision=?1", [revision])
                        .unwrap();
                    tx.commit().unwrap();
                    writer
                        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                        .unwrap();
                }
            });
            barrier.wait();
            let mut verified = 0;
            for _ in 0..50 {
                if let Ok(copy) = snapshot(&path) {
                    let revisions: i64 = copy
                        .query_row("SELECT COUNT(DISTINCT revision) FROM pair", [], |row| {
                            row.get(0)
                        })
                        .unwrap();
                    assert_eq!(revisions, 1);
                    verified += 1;
                }
            }
            assert!(verified > 0);
        });
        assert_eq!(
            std::fs::read_dir(&dir.0)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().contains("snapshot"))
                .count(),
            0
        );
    }
    #[test]
    fn malformed_source_is_rejected_without_modifying_bytes() {
        let dir = fixture();
        let path = dir.0.join("source.db");
        std::fs::write(&path, b"not sqlite").unwrap();
        assert!(snapshot(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"not sqlite");
    }
}
