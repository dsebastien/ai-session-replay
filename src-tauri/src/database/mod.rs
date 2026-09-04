use std::path::Path;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

mod bootstrap;
pub(crate) mod repository;

pub use bootstrap::{initialize, plugin};

pub const DATABASE_URL: &str = "sqlite:session-library-v3.sqlite3";
pub const DATABASE_FILE_NAME: &str = "session-library-v3.sqlite3";
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
pub const MAX_CONNECTIONS: u32 = 4;

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

pub fn migrator() -> &'static Migrator {
    &MIGRATOR
}

pub fn plugin_migrations() -> Vec<tauri_plugin_sql::Migration> {
    vec![
        tauri_plugin_sql::Migration {
            version: 1,
            description: "create_indexed_session_library",
            sql: include_str!("../../migrations/0001_create_indexed_session_library.sql"),
            kind: tauri_plugin_sql::MigrationKind::Up,
        },
        tauri_plugin_sql::Migration {
            version: 2,
            description: "add_session_full_text_search",
            sql: include_str!("../../migrations/0002_add_session_full_text_search.sql"),
            kind: tauri_plugin_sql::MigrationKind::Up,
        },
        tauri_plugin_sql::Migration {
            version: 3,
            description: "optimize_session_full_text_search",
            sql: include_str!("../../migrations/0003_optimize_session_full_text_search.sql"),
            kind: tauri_plugin_sql::MigrationKind::Up,
        },
        tauri_plugin_sql::Migration {
            version: 4,
            description: "show_tool_details_by_default",
            sql: include_str!("../../migrations/0004_show_tool_details_by_default.sql"),
            kind: tauri_plugin_sql::MigrationKind::Up,
        },
        tauri_plugin_sql::Migration {
            version: 5,
            description: "add_local_session_titles",
            sql: include_str!("../../migrations/0005_add_local_session_titles.sql"),
            kind: tauri_plugin_sql::MigrationKind::Up,
        },
        tauri_plugin_sql::Migration {
            version: 6,
            description: "set_default_presentation_delay",
            sql: include_str!("../../migrations/0006_set_default_presentation_delay.sql"),
            kind: tauri_plugin_sql::MigrationKind::Up,
        },
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DatabaseBootstrapError {
    #[error("DATABASE_PATH_UNAVAILABLE")]
    PathUnavailable,
    #[error("DATABASE_PRELOAD_MISSING")]
    PreloadMissing,
    #[error("DATABASE_OPEN_FAILED")]
    OpenFailed,
    #[error("DATABASE_STATE_CONFLICT")]
    StateConflict,
}

#[derive(Clone)]
pub struct DatabaseState {
    pool: SqlitePool,
}

impl DatabaseState {
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub(crate) fn repository(&self) -> repository::IndexedSessionRepository {
        repository::IndexedSessionRepository::new(self.pool.clone())
    }
}

pub async fn prepare_repository_state(
    instances: &tauri_plugin_sql::DbInstances,
    path: &Path,
) -> Result<DatabaseState, DatabaseBootstrapError> {
    let previous = {
        let mut pools = instances.0.write().await;
        pools
            .remove(DATABASE_URL)
            .ok_or(DatabaseBootstrapError::PreloadMissing)?
    };

    let pool = match connect(path).await {
        Ok(pool) => pool,
        Err(_) => {
            instances
                .0
                .write()
                .await
                .insert(DATABASE_URL.to_owned(), previous);
            return Err(DatabaseBootstrapError::OpenFailed);
        }
    };

    instances.0.write().await.insert(
        DATABASE_URL.to_owned(),
        tauri_plugin_sql::DbPool::Sqlite(pool.clone()),
    );
    let tauri_plugin_sql::DbPool::Sqlite(previous_pool) = previous;
    previous_pool.close().await;

    Ok(DatabaseState { pool })
}

/// Applies the per-connection safety and durability policy to every pool slot.
/// Source: https://docs.rs/sqlx/0.8.6/sqlx/sqlite/struct.SqliteConnectOptions.html
pub async fn connect(path: &Path) -> Result<SqlitePool, sqlx::Error> {
    let is_memory = path == Path::new(":memory:");
    let options = if is_memory {
        SqliteConnectOptions::new().in_memory(true)
    } else {
        SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
    }
    .foreign_keys(true)
    .journal_mode(SqliteJournalMode::Wal)
    .busy_timeout(BUSY_TIMEOUT)
    .synchronous(SqliteSynchronous::Full);

    SqlitePoolOptions::new()
        .min_connections(1)
        .max_connections(if is_memory { 1 } else { MAX_CONNECTIONS })
        .acquire_timeout(BUSY_TIMEOUT)
        .connect_with(options)
        .await
}

#[cfg(test)]
mod repository_tests;

#[cfg(test)]
mod read_repository_tests;

#[cfg(test)]
mod deletion_repository_tests;

#[cfg(test)]
mod scan_repository_tests;

#[cfg(test)]
mod tests;
