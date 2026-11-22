//! Shared helpers: a migrated temp-file database with a few teams, and a
//! way to open a compressed backup.

// Each test binary uses a different subset of these helpers.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use backup::BackupConfig;
use store::SqlitePool;

pub struct TestDb {
    pub pool: SqlitePool,
    pub path: PathBuf,
    pub dir: tempfile::TempDir,
}

impl TestDb {
    pub fn url(&self) -> String {
        url(&self.path)
    }

    /// `backups/` inside the temp directory.
    pub fn config(&self) -> BackupConfig {
        BackupConfig::new(self.dir.path().join("backups"))
    }
}

pub fn url(path: &Path) -> String {
    format!("sqlite://{}", path.display())
}

/// A fresh database migrated with the app's migrations, holding the teams
/// Alpha, Bravo and Charlie.
pub async fn test_db() -> TestDb {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pickem.db");
    let pool = store::open(&url(&path)).await.unwrap();
    for name in ["Alpha", "Bravo", "Charlie"] {
        add_team(&pool, name).await;
    }
    TestDb { pool, path, dir }
}

pub async fn add_team(pool: &SqlitePool, name: &str) {
    sqlx::query("INSERT INTO teams (name) VALUES (?)")
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
}

pub async fn team_names(pool: &SqlitePool) -> Vec<String> {
    sqlx::query_scalar("SELECT name FROM teams ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
}

pub async fn table_exists(pool: &SqlitePool, table: &str) -> bool {
    let n: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name = ?")
            .bind(table)
            .fetch_one(pool)
            .await
            .unwrap();
    n == 1
}

/// Unpacks a `.db.zst` backup into `scratch` and opens it.
pub async fn open_backup(file: &Path, scratch: &Path) -> SqlitePool {
    let out = scratch.join(format!(
        "{}.opened.db",
        file.file_name().unwrap().to_string_lossy()
    ));
    let input = std::fs::File::open(file).unwrap();
    let mut output = std::fs::File::create(&out).unwrap();
    zstd::stream::copy_decode(input, &mut output).unwrap();
    store::connect(&url(&out)).await.unwrap()
}

/// The file names in `dir`, sorted.
pub fn files(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    };
    v.sort();
    v
}
