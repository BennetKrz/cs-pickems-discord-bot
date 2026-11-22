//! `app restore <file>`: replaces the database with a backup. The app must
//! be stopped (`docker compose stop app`).

use std::path::{Path, PathBuf};

use chrono::Utc;

use crate::name::BackupKind;
use crate::snapshot::{self, blocking};
use crate::{BackupConfig, BackupError, BackupInfo};

/// What a restore did.
#[derive(Debug, Clone)]
pub struct RestoreReport {
    pub restored_from: PathBuf,
    pub db_path: PathBuf,
    /// The backup of the database that was replaced.
    pub safety_backup: Option<BackupInfo>,
    /// When the replaced database couldn't be backed up (e.g. it is
    /// corrupt), it was moved here instead.
    pub moved_aside: Option<PathBuf>,
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// The SQLite side files of `db`.
fn side_files(db: &Path) -> [PathBuf; 2] {
    [with_suffix(db, "-wal"), with_suffix(db, "-shm")]
}

/// Replaces the database at `db_path` with the backup `source` (`.db.zst`,
/// or a plain `.db`).
///
/// 1. Unpack `source` next to the database and check its integrity; stop
///    here, with the database untouched, if that fails.
/// 2. Back up the current database (kind `prerestore`). If it can't be
///    backed up, move it aside instead, so it is never lost.
/// 3. Rename the restored file over the database and delete stale
///    `-wal`/`-shm` files, which would otherwise corrupt it.
///
/// The restored database keeps its schema version; the next start
/// migrates it, with a pre-migration backup.
pub async fn restore(
    source: &Path,
    db_path: &Path,
    config: &BackupConfig,
) -> Result<RestoreReport, BackupError> {
    if !tokio::fs::metadata(source)
        .await
        .map(|m| m.is_file())
        .unwrap_or(false)
    {
        return Err(BackupError::Restore(format!(
            "{} is not a file",
            source.display()
        )));
    }
    if let Some(dir) = db_path.parent().filter(|d| !d.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| BackupError::Io {
                action: format!("creating {}", dir.display()),
                source: e,
            })?;
    }

    // 1. Unpack and check.
    let tmp = with_suffix(db_path, ".restore-tmp");
    let _ = tokio::fs::remove_file(&tmp).await;
    let (src, dst) = (source.to_owned(), tmp.clone());
    let checked = async {
        blocking(move || snapshot::unpack(&src, &dst)).await?;
        snapshot::verify(&tmp).await
    }
    .await;
    if let Err(e) = checked {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(BackupError::Restore(format!(
            "{} is not a usable backup, nothing was changed: {e}",
            source.display()
        )));
    }

    // 2. Keep the current database.
    let mut safety_backup = None;
    let mut moved_aside = None;
    if tokio::fs::try_exists(db_path).await.unwrap_or(false) {
        match backup_current(db_path, config).await {
            Ok(info) => safety_backup = Some(info),
            Err(e) => {
                let aside = with_suffix(
                    db_path,
                    &format!(".broken-{}", Utc::now().format("%Y%m%dT%H%M%SZ")),
                );
                tracing::warn!(
                    "cannot back up the current database ({e}); moving it to {}",
                    aside.display()
                );
                if let Err(e) = move_with_side_files(db_path, &aside).await {
                    let _ = tokio::fs::remove_file(&tmp).await;
                    return Err(e);
                }
                moved_aside = Some(aside);
            }
        }
    }

    // 3. Swap in the restored file.
    for side in side_files(db_path) {
        match tokio::fs::remove_file(&side).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                let _ = tokio::fs::remove_file(&tmp).await;
                return Err(BackupError::Io {
                    action: format!("removing {}", side.display()),
                    source: e,
                });
            }
        }
    }
    tokio::fs::rename(&tmp, db_path)
        .await
        .map_err(|e| BackupError::Io {
            action: format!("renaming to {}", db_path.display()),
            source: e,
        })?;

    Ok(RestoreReport {
        restored_from: source.to_owned(),
        db_path: db_path.to_owned(),
        safety_backup,
        moved_aside,
    })
}

async fn backup_current(db_path: &Path, config: &BackupConfig) -> Result<BackupInfo, BackupError> {
    let url = format!("sqlite://{}", db_path.display());
    let pool = store::connect(&url)
        .await
        .map_err(|e| BackupError::Restore(e.to_string()))?;
    let result = snapshot::create(&pool, &config.dir, BackupKind::PreRestore, Utc::now()).await;
    // Closing the last connection checkpoints the WAL into the file.
    pool.close().await;
    let info = result?;
    if let Err(e) = snapshot::prune(&config.dir, config.tz, config.policy).await {
        tracing::warn!("cannot prune old backups: {e}");
    }
    Ok(info)
}

async fn move_with_side_files(db_path: &Path, aside: &Path) -> Result<(), BackupError> {
    let rename = |from: PathBuf, to: PathBuf| async move {
        match tokio::fs::rename(&from, &to).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(BackupError::Io {
                action: format!("moving {} to {}", from.display(), to.display()),
                source: e,
            }),
        }
    };
    rename(db_path.to_owned(), aside.to_owned()).await?;
    for (from, to) in side_files(db_path).into_iter().zip(side_files(aside)) {
        rename(from, to).await?;
    }
    Ok(())
}
