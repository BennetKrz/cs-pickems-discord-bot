//! Taking one backup: `VACUUM INTO` a temp file, check it, compress it,
//! then rename it into place. Plus listing, pruning and copying backups.

use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, DurationRound, Utc};
use chrono_tz::Tz;
use sqlx::Connection;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection, SqliteJournalMode};
use store::SqlitePool;

use crate::name::{self, BackupKind};
use crate::retention::{self, Policy};
use crate::{BackupError, BackupInfo};

/// zstd level: fast on a Pi and still about 4:1 on SQLite pages.
const ZSTD_LEVEL: i32 = 3;

fn io_err(action: impl Into<String>) -> impl FnOnce(io::Error) -> BackupError {
    let action = action.into();
    move |source| BackupError::Io { action, source }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Writes a verified, compressed backup of `pool`'s database into `dir`.
///
/// `VACUUM INTO` reads one consistent snapshot in a read transaction. In
/// WAL mode, writers carry on meanwhile, so requests are not blocked.
pub(crate) async fn create(
    pool: &SqlitePool,
    dir: &Path,
    kind: BackupKind,
    now: DateTime<Utc>,
) -> Result<BackupInfo, BackupError> {
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(io_err(format!("creating {}", dir.display())))?;

    // Whole seconds, as in the file name; never overwrite an existing backup.
    let mut at = now.duration_trunc(Duration::seconds(1)).unwrap_or(now);
    let mut file_name = name::format(at, kind);
    while tokio::fs::try_exists(dir.join(&file_name))
        .await
        .unwrap_or(false)
    {
        at += Duration::seconds(1);
        file_name = name::format(at, kind);
    }
    let path = dir.join(&file_name);
    let raw = with_suffix(&path, ".vacuum.tmp");
    let packed = with_suffix(&path, ".tmp");

    let result = async {
        remove_if_exists(&raw).await?;
        let raw_str = raw
            .to_str()
            .ok_or_else(|| BackupError::Path(raw.display().to_string()))?;
        sqlx::query("VACUUM INTO ?")
            .bind(raw_str)
            .execute(pool)
            .await
            .map_err(BackupError::Db)?;
        verify(&raw).await?;
        let (src, dst) = (raw.clone(), packed.clone());
        blocking(move || compress(&src, &dst)).await?;
        tokio::fs::rename(&packed, &path)
            .await
            .map_err(io_err(format!("renaming to {}", path.display())))?;
        sync_dir(dir).await;
        let size = tokio::fs::metadata(&path)
            .await
            .map_err(io_err(format!("reading {}", path.display())))?
            .len();
        Ok(BackupInfo {
            path: path.clone(),
            file_name: file_name.clone(),
            kind,
            created_at: at,
            size,
        })
    }
    .await;

    let _ = tokio::fs::remove_file(&raw).await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&packed).await;
    }
    result
}

/// Opens a plain SQLite file and runs `PRAGMA integrity_check` on it. Sets
/// the file to rollback-journal mode first, so it stands alone without
/// `-wal`/`-shm` files.
pub(crate) async fn verify(path: &Path) -> Result<(), BackupError> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .journal_mode(SqliteJournalMode::Delete);
    let mut conn = SqliteConnection::connect_with(&options)
        .await
        .map_err(|e| BackupError::Integrity(format!("cannot open {}: {e}", path.display())))?;
    let rows: Result<Vec<String>, sqlx::Error> = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_all(&mut conn)
        .await;
    let _ = conn.close().await;
    match rows {
        Ok(rows) if rows == ["ok"] => Ok(()),
        Ok(rows) => Err(BackupError::Integrity(
            rows.into_iter().take(5).collect::<Vec<_>>().join("; "),
        )),
        Err(e) => Err(BackupError::Integrity(e.to_string())),
    }
}

/// Runs blocking file work off the async runtime.
pub(crate) async fn blocking(
    f: impl FnOnce() -> Result<(), BackupError> + Send + 'static,
) -> Result<(), BackupError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| BackupError::Io {
            action: "running a file task".to_owned(),
            source: io::Error::other(e),
        })?
}

fn compress(src: &Path, dst: &Path) -> Result<(), BackupError> {
    let input = std::fs::File::open(src).map_err(io_err(format!("opening {}", src.display())))?;
    let mut out =
        std::fs::File::create(dst).map_err(io_err(format!("creating {}", dst.display())))?;
    zstd::stream::copy_encode(io::BufReader::new(input), &mut out, ZSTD_LEVEL)
        .map_err(io_err(format!("compressing into {}", dst.display())))?;
    out.sync_all()
        .map_err(io_err(format!("syncing {}", dst.display())))
}

/// Unpacks a `.zst` backup (or copies a plain `.db`) to `dst`.
pub(crate) fn unpack(src: &Path, dst: &Path) -> Result<(), BackupError> {
    let input = std::fs::File::open(src).map_err(io_err(format!("opening {}", src.display())))?;
    let mut out =
        std::fs::File::create(dst).map_err(io_err(format!("creating {}", dst.display())))?;
    let is_zst = src.extension().is_some_and(|e| e == "zst");
    if is_zst {
        zstd::stream::copy_decode(io::BufReader::new(input), &mut out)
            .map_err(io_err(format!("decompressing {}", src.display())))?;
    } else {
        io::copy(&mut io::BufReader::new(input), &mut out)
            .map_err(io_err(format!("copying {}", src.display())))?;
    }
    out.sync_all()
        .map_err(io_err(format!("syncing {}", dst.display())))
}

/// Copies a finished backup into `dir` (e.g. a NAS share): temp file, then
/// rename, so a half-copied file is never taken for a backup.
pub(crate) async fn copy_to(info: &BackupInfo, dir: &Path) -> Result<(), BackupError> {
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(io_err(format!("creating {}", dir.display())))?;
    let target = dir.join(&info.file_name);
    let tmp = with_suffix(&target, ".tmp");
    let (src, dst) = (info.path.clone(), tmp.clone());
    let copied = blocking(move || {
        std::fs::copy(&src, &dst).map_err(io_err(format!("copying to {}", dst.display())))?;
        std::fs::File::open(&dst)
            .and_then(|f| f.sync_all())
            .map_err(io_err(format!("syncing {}", dst.display())))
    })
    .await;
    if let Err(e) = copied {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e);
    }
    tokio::fs::rename(&tmp, &target)
        .await
        .map_err(io_err(format!("renaming to {}", target.display())))?;
    sync_dir(dir).await;
    Ok(())
}

/// The finished backups in `dir`, oldest first. A missing directory has
/// none.
pub(crate) async fn list(dir: &Path) -> Result<Vec<BackupInfo>, BackupError> {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_err(format!("listing {}", dir.display()))(e)),
    };
    let mut out = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(io_err(format!("listing {}", dir.display())))?
    {
        let Some(file_name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some((created_at, kind)) = name::parse(&file_name) else {
            continue;
        };
        let Ok(meta) = entry.metadata().await else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        out.push(BackupInfo {
            path: entry.path(),
            file_name,
            kind,
            created_at,
            size: meta.len(),
        });
    }
    out.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then(a.file_name.cmp(&b.file_name))
    });
    Ok(out)
}

/// Deletes the backups in `dir` that `policy` doesn't keep; returns their
/// file names.
pub(crate) async fn prune(dir: &Path, tz: Tz, policy: Policy) -> Result<Vec<String>, BackupError> {
    let backups = list(dir).await?;
    let keys: Vec<_> = backups.iter().map(|b| (b.created_at, b.kind)).collect();
    let mut deleted = Vec::new();
    for i in retention::to_delete(&keys, tz, policy) {
        let b = &backups[i];
        tokio::fs::remove_file(&b.path)
            .await
            .map_err(io_err(format!("deleting {}", b.path.display())))?;
        deleted.push(b.file_name.clone());
    }
    Ok(deleted)
}

/// Deletes temp files that an interrupted backup left behind in `dir`.
pub(crate) async fn clean_tmp(dir: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with(name::PREFIX) && name.ends_with(".tmp") {
            match tokio::fs::remove_file(entry.path()).await {
                Ok(()) => tracing::info!(file = name, "removed a leftover backup temp file"),
                Err(e) => tracing::warn!(file = name, "cannot remove a backup temp file: {e}"),
            }
        }
    }
}

async fn remove_if_exists(path: &Path) -> Result<(), BackupError> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_err(format!("removing {}", path.display()))(e)),
    }
}

/// Makes a rename durable. Best effort: not every file system supports
/// syncing a directory (e.g. some network shares).
async fn sync_dir(dir: &Path) {
    let dir = dir.to_owned();
    let _ = tokio::task::spawn_blocking(move || std::fs::File::open(&dir)?.sync_all()).await;
}
