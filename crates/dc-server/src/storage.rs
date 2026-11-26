use anyhow::{Context, Result};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
use std::path::Path;

/// Opens (or creates) the application database and applies migrations.
pub async fn open(data_dir: &Path) -> Result<SqlitePool> {
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("creating {}", data_dir.display()))?;
    let options = SqliteConnectOptions::new()
        .filename(data_dir.join("dentalclarity.db"))
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true);
    let pool = SqlitePool::connect_with(options)
        .await
        .context("opening database")?;
    migrate(&pool).await?;
    Ok(pool)
}

/// An in-memory database for tests.
pub async fn open_in_memory() -> Result<SqlitePool> {
    let pool = SqlitePool::connect("sqlite::memory:").await?;
    migrate(&pool).await?;
    Ok(pool)
}

async fn migrate(pool: &SqlitePool) -> Result<()> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .context("running migrations")
}
