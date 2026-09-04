use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sqlx::migrate::Migrator;
use sqlx::{Row, SqlitePool};

use super::{
    DATABASE_URL, DatabaseBootstrapError, MAX_CONNECTIONS, connect, migrator,
    prepare_repository_state,
};

static TEMP_NONCE: AtomicU64 = AtomicU64::new(0);

struct TempDatabase {
    root: PathBuf,
    path: PathBuf,
}

impl TempDatabase {
    fn new() -> Self {
        let nonce = TEMP_NONCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "ai-session-replay-database-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("temporary database directory should be created");
        let path = root.join("library.sqlite3");
        Self { root, path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for TempDatabase {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

async fn migrated_memory_pool() -> SqlitePool {
    let pool = connect(Path::new(":memory:"))
        .await
        .expect("in-memory database should connect");
    migrator()
        .run(&pool)
        .await
        .expect("initial migration should succeed");
    pool
}

async fn insert_session(pool: &SqlitePool, id: &str) {
    sqlx::query(
        "INSERT INTO sessions (
            id, source, vendor_session_id, title, source_present,
            first_indexed_at_ms, last_seen_at_ms
         ) VALUES (?, 'codex', ?, 'Synthetic session', 1, 1000, 1000)",
    )
    .bind(id)
    .bind(format!("vendor-{id}"))
    .execute(pool)
    .await
    .expect("synthetic session should insert");
}

async fn insert_revision(pool: &SqlitePool, session_id: &str, revision_id: &str) {
    sqlx::query(
        "INSERT INTO session_revisions (
            id, session_id, content_hash, indexed_at_ms, entry_count, duration_ms,
            diagnostic_count, reasoning_availability, tool_detail_availability
         ) VALUES (?, ?, zeroblob(32), 1000, 1, 0, 0, 'unavailable', 'unavailable')",
    )
    .bind(revision_id)
    .bind(session_id)
    .execute(pool)
    .await
    .expect("synthetic revision should insert");
}

async fn insert_preferences(
    pool: &SqlitePool,
    session_id: &str,
    show_tool_calls: i64,
    show_tool_details: i64,
    entry_delay_ms: i64,
    playback_speed: f64,
    color: &str,
) -> Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error> {
    sqlx::query(
        "INSERT INTO session_preferences (
            session_id, show_tool_calls, show_tool_details, show_reasoning,
            entry_delay_ms, playback_speed, background_color, surface_color,
            text_color, muted_color, accent_color, success_color, error_color,
            font_family, font_size_px, line_height, updated_at_ms
         ) VALUES (?, ?, ?, 1, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'JetBrains Mono', 18, 1.5, 1000)",
    )
    .bind(session_id)
    .bind(show_tool_calls)
    .bind(show_tool_details)
    .bind(entry_delay_ms)
    .bind(playback_speed)
    .bind(color)
    .bind(color)
    .bind(color)
    .bind(color)
    .bind(color)
    .bind(color)
    .bind(color)
    .execute(pool)
    .await
}

#[test]
fn file_database_uses_the_approved_connection_policy() {
    tauri::async_runtime::block_on(async {
        let database = TempDatabase::new();
        let pool = connect(database.path())
            .await
            .expect("temporary database should connect");

        let mut connections = Vec::new();
        for _ in 0..MAX_CONNECTIONS {
            let mut connection = pool.acquire().await.expect("pool slot should be available");
            let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
                .fetch_one(&mut *connection)
                .await
                .expect("foreign key policy should be readable");
            let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
                .fetch_one(&mut *connection)
                .await
                .expect("journal mode should be readable");
            let busy_timeout: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
                .fetch_one(&mut *connection)
                .await
                .expect("busy timeout should be readable");
            let synchronous: i64 = sqlx::query_scalar("PRAGMA synchronous")
                .fetch_one(&mut *connection)
                .await
                .expect("synchronous policy should be readable");

            assert_eq!(foreign_keys, 1);
            assert_eq!(journal_mode, "wal");
            assert_eq!(busy_timeout, 5_000);
            assert_eq!(synchronous, 2);
            connections.push(connection);
        }
        assert!(pool.try_acquire().is_none());

        drop(connections);
        pool.close().await;
    });
}

#[test]
fn fresh_database_contains_only_expected_strict_application_tables() {
    tauri::async_runtime::block_on(async {
        let pool = migrated_memory_pool().await;

        let rows = sqlx::query(
            "SELECT name, strict FROM pragma_table_list \
             WHERE schema = 'main' AND type = 'table' \
             AND name NOT LIKE 'sqlite_%' AND name <> '_sqlx_migrations'",
        )
        .fetch_all(&pool)
        .await
        .expect("table metadata should be readable");

        let actual = rows
            .into_iter()
            .map(|row| {
                let name: String = row.get("name");
                let strict: i64 = row.get("strict");
                (name, strict)
            })
            .collect::<BTreeSet<_>>();

        let expected = [
            "entry_selection_overrides",
            "revision_entries",
            "scan_runs",
            "session_preferences",
            "session_revisions",
            "session_search_documents",
            "sessions",
            "source_diagnostics",
            "source_locations",
            "suppressed_sources",
        ]
        .into_iter()
        .map(|name| (name.to_owned(), 1))
        .collect::<BTreeSet<_>>();

        assert_eq!(actual, expected);

        let search_table: (String, i64) = sqlx::query_as(
            "SELECT type, strict FROM pragma_table_list
             WHERE schema = 'main' AND name = 'session_search'",
        )
        .fetch_one(&pool)
        .await
        .expect("full-text search table should exist");
        assert_eq!(search_table, ("virtual".to_owned(), 0));
        let delete_trigger: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM sqlite_schema
             WHERE type = 'trigger'
               AND name IN ('session_search_documents_insert', 'session_search_documents_delete', 'session_search_documents_update')",
        )
        .fetch_one(&pool)
        .await
        .expect("full-text cleanup trigger should be readable");
        assert_eq!(delete_trigger, 3);
        let custom_title_column: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pragma_table_info('sessions') WHERE name = 'custom_title'",
        )
        .fetch_one(&pool)
        .await
        .expect("custom title column should be readable");
        assert_eq!(custom_title_column, 1);
    });
}

#[test]
fn session_schema_rejects_invalid_types_enums_bounds_and_duplicates() {
    tauri::async_runtime::block_on(async {
        let pool = migrated_memory_pool().await;
        insert_session(&pool, "session_1").await;

        let wrong_type = sqlx::query(
            "INSERT INTO sessions (
                id, source, vendor_session_id, title, source_present,
                first_indexed_at_ms, last_seen_at_ms
             ) VALUES ('session_2', 'codex', 'vendor-2', 'Title', X'01', 1, 1)",
        )
        .execute(&pool)
        .await;
        assert!(wrong_type.is_err());

        let unknown_source = sqlx::query(
            "INSERT INTO sessions (
                id, source, vendor_session_id, title, source_present,
                first_indexed_at_ms, last_seen_at_ms
             ) VALUES ('session_3', 'vscode', 'vendor-3', 'Title', 1, 1, 1)",
        )
        .execute(&pool)
        .await;
        assert!(unknown_source.is_err());

        let oversized_title = sqlx::query(
            "INSERT INTO sessions (
                id, source, vendor_session_id, title, source_present,
                first_indexed_at_ms, last_seen_at_ms
             ) VALUES ('session_4', 'codex', 'vendor-4', ?, 1, 1, 1)",
        )
        .bind("x".repeat(513))
        .execute(&pool)
        .await;
        assert!(oversized_title.is_err());

        let duplicate_source_identity = sqlx::query(
            "INSERT INTO sessions (
                id, source, vendor_session_id, title, source_present,
                first_indexed_at_ms, last_seen_at_ms
             ) VALUES ('session_5', 'codex', 'vendor-session_1', 'Title', 1, 1, 1)",
        )
        .execute(&pool)
        .await;
        assert!(duplicate_source_identity.is_err());
    });
}

#[test]
fn schema_rejects_embedded_nul_in_identifiers_and_colors() {
    tauri::async_runtime::block_on(async {
        let pool = migrated_memory_pool().await;

        let nul_id = sqlx::query(
            "INSERT INTO sessions (
                id, source, vendor_session_id, title, source_present,
                first_indexed_at_ms, last_seen_at_ms
             ) VALUES (?, 'codex', 'vendor-nul-id', 'Title', 1, 1, 1)",
        )
        .bind("session_1\0hidden")
        .execute(&pool)
        .await;
        assert!(nul_id.is_err());

        insert_session(&pool, "nul_color").await;
        assert!(
            insert_preferences(&pool, "nul_color", 1, 0, 1000, 1.0, "#123456\0hidden")
                .await
                .is_err()
        );
    });
}

#[test]
fn revision_schema_enforces_parent_idempotency_and_current_ownership() {
    tauri::async_runtime::block_on(async {
        let pool = migrated_memory_pool().await;

        let orphan = sqlx::query(
            "INSERT INTO session_revisions (
                id, session_id, content_hash, indexed_at_ms, entry_count, duration_ms,
                diagnostic_count, reasoning_availability, tool_detail_availability
             ) VALUES ('revision_orphan', 'missing', zeroblob(32), 1, 1, 0, 0,
                'unavailable', 'unavailable')",
        )
        .execute(&pool)
        .await;
        assert!(orphan.is_err());

        insert_session(&pool, "session_1").await;
        insert_revision(&pool, "session_1", "revision_1").await;

        let duplicate_content = sqlx::query(
            "INSERT INTO session_revisions (
                id, session_id, content_hash, indexed_at_ms, entry_count, duration_ms,
                diagnostic_count, reasoning_availability, tool_detail_availability
             ) VALUES ('revision_duplicate', 'session_1', zeroblob(32), 2, 1, 0, 0,
                'unavailable', 'unavailable')",
        )
        .execute(&pool)
        .await;
        assert!(duplicate_content.is_err());

        insert_session(&pool, "session_2").await;
        insert_revision(&pool, "session_2", "revision_2").await;
        let foreign_current = sqlx::query(
            "UPDATE sessions SET current_revision_id = 'revision_2' WHERE id = 'session_1'",
        )
        .execute(&pool)
        .await;
        assert!(foreign_current.is_err());
    });
}

#[test]
fn entry_schema_enforces_parent_kind_payload_order_and_bounds() {
    tauri::async_runtime::block_on(async {
        let pool = migrated_memory_pool().await;
        insert_session(&pool, "session_1").await;
        insert_revision(&pool, "session_1", "revision_1").await;

        let orphan = sqlx::query(
            "INSERT INTO revision_entries
                (revision_id, entry_key, ordinal, at_ms, kind, body_text)
             VALUES ('missing', 'entry_1', 0, 0, 'user', 'hello')",
        )
        .execute(&pool)
        .await;
        assert!(orphan.is_err());

        let impossible_payload = sqlx::query(
            "INSERT INTO revision_entries
                (revision_id, entry_key, ordinal, at_ms, kind, body_text)
             VALUES ('revision_1', 'entry_1', 0, 0, 'tool-call', 'not a tool payload')",
        )
        .execute(&pool)
        .await;
        assert!(impossible_payload.is_err());

        let missing_available_detail = sqlx::query(
            "INSERT INTO revision_entries (
                revision_id, entry_key, ordinal, at_ms, kind, tool_name,
                tool_status, summary_text, tool_detail_availability
             ) VALUES (
                'revision_1', 'entry_without_detail', 1, 0, 'tool-call', 'shell',
                'succeeded', 'Synthetic tool call', 'available'
             )",
        )
        .execute(&pool)
        .await;
        assert!(missing_available_detail.is_err());

        sqlx::query(
            "INSERT INTO revision_entries
                (revision_id, entry_key, ordinal, at_ms, kind, body_text)
             VALUES ('revision_1', 'entry_1', 0, 0, 'user', 'hello')",
        )
        .execute(&pool)
        .await
        .expect("valid user entry should insert");

        let duplicate_ordinal = sqlx::query(
            "INSERT INTO revision_entries
                (revision_id, entry_key, ordinal, at_ms, kind, body_text)
             VALUES ('revision_1', 'entry_2', 0, 1, 'assistant', 'response')",
        )
        .execute(&pool)
        .await;
        assert!(duplicate_ordinal.is_err());

        let excessive_timestamp = sqlx::query(
            "INSERT INTO revision_entries
                (revision_id, entry_key, ordinal, at_ms, kind, body_text)
             VALUES ('revision_1', 'entry_3', 1, 604800001, 'user', 'late')",
        )
        .execute(&pool)
        .await;
        assert!(excessive_timestamp.is_err());
    });
}

#[test]
fn preference_schema_enforces_visibility_timing_and_appearance_bounds() {
    tauri::async_runtime::block_on(async {
        let pool = migrated_memory_pool().await;
        for id in ["valid", "visibility", "delay", "speed", "color"] {
            insert_session(&pool, id).await;
        }

        insert_preferences(&pool, "valid", 1, 1, 250, 0.25, "#A1b2C3")
            .await
            .expect("boundary preferences should insert");
        assert!(
            insert_preferences(&pool, "visibility", 0, 1, 1000, 1.0, "#123456")
                .await
                .is_err()
        );
        assert!(
            insert_preferences(&pool, "delay", 1, 0, 249, 1.0, "#123456")
                .await
                .is_err()
        );
        assert!(
            insert_preferences(&pool, "speed", 1, 0, 1000, 4.01, "#123456")
                .await
                .is_err()
        );
        assert!(
            insert_preferences(&pool, "color", 1, 0, 1000, 1.0, "#12ZZZZ")
                .await
                .is_err()
        );
    });
}

#[test]
fn scan_schema_enforces_state_counts_timestamps_and_safe_error_codes() {
    tauri::async_runtime::block_on(async {
        let pool = migrated_memory_pool().await;
        sqlx::query(
            "INSERT INTO scan_runs (
                generation, status, started_at_ms, completed_at_ms,
                discovered_count, processed_count, indexed_count, unchanged_count,
                failed_count, error_code
             ) VALUES (1, 'running', 1000, NULL, 3, 1, 1, 0, 0, NULL)",
        )
        .execute(&pool)
        .await
        .expect("valid running scan should insert");

        for invalid in [
            "INSERT INTO scan_runs VALUES (2, 'completed', 1000, NULL, 1, 1, 1, 0, 0, NULL)",
            "INSERT INTO scan_runs VALUES (3, 'running', 1000, NULL, 1, 2, 0, 0, 0, NULL)",
            "INSERT INTO scan_runs VALUES (4, 'failed', 1000, 1001, 1, 1, 0, 0, 1, 'raw path leaked')",
            "INSERT INTO scan_runs VALUES (5, 'completed', 1000, 999, 0, 0, 0, 0, 0, NULL)",
        ] {
            assert!(sqlx::query(invalid).execute(&pool).await.is_err());
        }
    });
}

#[test]
fn private_source_schema_requires_bounded_blobs_and_unique_identities() {
    tauri::async_runtime::block_on(async {
        let pool = migrated_memory_pool().await;
        insert_session(&pool, "session_1").await;
        insert_session(&pool, "session_2").await;

        sqlx::query(
            "INSERT INTO source_locations (
                session_id, canonical_path_utf16le, source_identity_hash,
                root_identity, file_identity, file_size_bytes, modified_at_ms
             ) VALUES ('session_1', X'43003A00', zeroblob(32), X'01', X'02', 10, 1000)",
        )
        .execute(&pool)
        .await
        .expect("valid private source identity should insert");

        let odd_utf16_path = sqlx::query(
            "INSERT INTO source_locations (
                session_id, canonical_path_utf16le, source_identity_hash,
                root_identity, file_identity, file_size_bytes, modified_at_ms
             ) VALUES ('session_2', X'43003A', randomblob(32), X'01', X'02', 10, 1000)",
        )
        .execute(&pool)
        .await;
        assert!(odd_utf16_path.is_err());

        sqlx::query(
            "INSERT INTO suppressed_sources (
                id, source, source_identity_hash, vendor_session_id,
                source_deleted, suppressed_at_ms
             ) VALUES ('suppression_1', 'codex', randomblob(32), 'vendor-1', 0, 1000)",
        )
        .execute(&pool)
        .await
        .expect("valid suppression should insert");

        let invalid_hash = sqlx::query(
            "INSERT INTO suppressed_sources (
                id, source, source_identity_hash, vendor_session_id,
                source_deleted, suppressed_at_ms
             ) VALUES ('suppression_2', 'codex', X'01', 'vendor-2', 0, 1000)",
        )
        .execute(&pool)
        .await;
        assert!(invalid_hash.is_err());
    });
}

#[test]
fn migration_is_idempotent_and_matches_the_registered_plugin_migration() {
    tauri::async_runtime::block_on(async {
        let pool = migrated_memory_pool().await;
        migrator()
            .run(&pool)
            .await
            .expect("re-running the migration should be a no-op");

        let applied_versions: Vec<i64> =
            sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
                .fetch_all(&pool)
                .await
                .expect("migration history should be readable");
        assert_eq!(applied_versions, vec![1, 2, 3, 4, 5, 6]);

        let plugin_migrations = super::plugin_migrations();
        let sqlx_migrations = migrator().iter().collect::<Vec<_>>();
        assert_eq!(plugin_migrations.len(), 6);
        for (plugin, sqlx) in plugin_migrations.iter().zip(sqlx_migrations) {
            assert_eq!(plugin.version, sqlx.version);
            assert_eq!(plugin.sql, sqlx.sql);
        }
    });
}

#[test]
fn tool_detail_default_migration_enables_existing_sessions() {
    tauri::async_runtime::block_on(async {
        let database = TempDatabase::new();
        let migration_directory = database.root().join("v3-migration");
        fs::create_dir(&migration_directory).expect("migration directory should be created");
        for (name, sql) in [
            (
                "0001_create_indexed_session_library.sql",
                include_str!("../../migrations/0001_create_indexed_session_library.sql"),
            ),
            (
                "0002_add_session_full_text_search.sql",
                include_str!("../../migrations/0002_add_session_full_text_search.sql"),
            ),
            (
                "0003_optimize_session_full_text_search.sql",
                include_str!("../../migrations/0003_optimize_session_full_text_search.sql"),
            ),
        ] {
            fs::write(migration_directory.join(name), sql)
                .expect("prior migration should be written");
        }
        let v3_migrator = Migrator::new(migration_directory.as_path())
            .await
            .expect("version-three migrator should resolve");
        let pool = connect(database.path())
            .await
            .expect("database should connect");
        v3_migrator
            .run(&pool)
            .await
            .expect("version three should migrate");
        insert_session(&pool, "session_existing").await;
        insert_preferences(&pool, "session_existing", 1, 0, 1_000, 1.0, "#123456")
            .await
            .expect("existing preferences should insert");

        migrator()
            .run(&pool)
            .await
            .expect("latest migration should run");

        let show_tool_details: i64 = sqlx::query_scalar(
            "SELECT show_tool_details FROM session_preferences WHERE session_id = 'session_existing'",
        )
        .fetch_one(&pool)
        .await
        .expect("migrated preference should be readable");
        assert_eq!(show_tool_details, 1);
        pool.close().await;
    });
}

#[test]
fn entry_delay_default_migration_updates_only_the_previous_default() {
    tauri::async_runtime::block_on(async {
        let database = TempDatabase::new();
        let migration_directory = database.root().join("v5-migration");
        fs::create_dir(&migration_directory).expect("migration directory should be created");
        for (name, sql) in [
            (
                "0001_create_indexed_session_library.sql",
                include_str!("../../migrations/0001_create_indexed_session_library.sql"),
            ),
            (
                "0002_add_session_full_text_search.sql",
                include_str!("../../migrations/0002_add_session_full_text_search.sql"),
            ),
            (
                "0003_optimize_session_full_text_search.sql",
                include_str!("../../migrations/0003_optimize_session_full_text_search.sql"),
            ),
            (
                "0004_show_tool_details_by_default.sql",
                include_str!("../../migrations/0004_show_tool_details_by_default.sql"),
            ),
            (
                "0005_add_local_session_titles.sql",
                include_str!("../../migrations/0005_add_local_session_titles.sql"),
            ),
        ] {
            fs::write(migration_directory.join(name), sql)
                .expect("prior migration should be written");
        }
        let v5_migrator = Migrator::new(migration_directory.as_path())
            .await
            .expect("version-five migrator should resolve");
        let pool = connect(database.path())
            .await
            .expect("database should connect");
        v5_migrator
            .run(&pool)
            .await
            .expect("version five should migrate");

        insert_session(&pool, "session_previous_default").await;
        insert_preferences(
            &pool,
            "session_previous_default",
            1,
            1,
            1_000,
            1.0,
            "#123456",
        )
        .await
        .expect("previous default should insert");
        insert_session(&pool, "session_custom_delay").await;
        insert_preferences(&pool, "session_custom_delay", 1, 1, 2_500, 1.0, "#123456")
            .await
            .expect("custom preference should insert");

        migrator()
            .run(&pool)
            .await
            .expect("latest migration should run");

        let delays: Vec<(String, i64)> = sqlx::query_as(
            "SELECT session_id, entry_delay_ms FROM session_preferences ORDER BY session_id",
        )
        .fetch_all(&pool)
        .await
        .expect("migrated preferences should be readable");
        assert_eq!(
            delays,
            vec![
                ("session_custom_delay".to_owned(), 2_500),
                ("session_previous_default".to_owned(), 5_000),
            ]
        );
        pool.close().await;
    });
}

#[test]
fn full_text_migration_backfills_existing_current_revisions() {
    tauri::async_runtime::block_on(async {
        let database = TempDatabase::new();
        let migration_directory = database.root().join("v1-migration");
        fs::create_dir(&migration_directory).expect("migration directory should be created");
        fs::write(
            migration_directory.join("0001_create_indexed_session_library.sql"),
            include_str!("../../migrations/0001_create_indexed_session_library.sql"),
        )
        .expect("version-one migration should be written");
        let v1_migrator = Migrator::new(migration_directory.as_path())
            .await
            .expect("version-one migrator should resolve");
        let pool = connect(database.path())
            .await
            .expect("database should connect");
        v1_migrator
            .run(&pool)
            .await
            .expect("version one should migrate");

        insert_session(&pool, "session_existing").await;
        insert_revision(&pool, "session_existing", "revision_existing").await;
        sqlx::query(
            "INSERT INTO revision_entries
                (revision_id, entry_key, ordinal, at_ms, kind, body_text)
             VALUES ('revision_existing', 'entry_existing', 0, 0, 'user',
                     'backfilled conversation sentinel')",
        )
        .execute(&pool)
        .await
        .expect("existing entry should insert");
        sqlx::query(
            "UPDATE sessions SET current_revision_id = 'revision_existing'
             WHERE id = 'session_existing'",
        )
        .execute(&pool)
        .await
        .expect("current revision should update");

        migrator()
            .run(&pool)
            .await
            .expect("full-text migration should apply");
        let body_match: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM session_search
             WHERE session_search MATCH '\"backfilled\"*'",
        )
        .fetch_one(&pool)
        .await
        .expect("backfilled content should be searchable");
        let title_match: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM session_search
             WHERE session_search MATCH '\"synthetic\"*'",
        )
        .fetch_one(&pool)
        .await
        .expect("backfilled title should be searchable");
        assert_eq!((body_match, title_match), (1, 1));
        pool.close().await;
    });
}

#[test]
fn migrated_file_database_reopens_without_losing_data() {
    tauri::async_runtime::block_on(async {
        let database = TempDatabase::new();

        let first_pool = connect(database.path())
            .await
            .expect("temporary database should connect");
        migrator()
            .run(&first_pool)
            .await
            .expect("initial migration should succeed");
        insert_session(&first_pool, "session_before_restart").await;
        first_pool.close().await;

        let reopened_pool = connect(database.path())
            .await
            .expect("existing database should reopen");
        migrator()
            .run(&reopened_pool)
            .await
            .expect("startup migration should remain idempotent");
        let title: String =
            sqlx::query_scalar("SELECT title FROM sessions WHERE id = 'session_before_restart'")
                .fetch_one(&reopened_pool)
                .await
                .expect("persisted session should remain readable after restart");
        assert_eq!(title, "Synthetic session");

        reopened_pool.close().await;
    });
}

#[test]
fn failed_upgrade_rolls_back_and_leaves_latest_version_usable() {
    tauri::async_runtime::block_on(async {
        let database = TempDatabase::new();
        let migration_directory = database.root().join("migrations");
        fs::create_dir(&migration_directory)
            .expect("temporary migration directory should be created");
        fs::write(
            migration_directory.join("0001_create_indexed_session_library.sql"),
            include_str!("../../migrations/0001_create_indexed_session_library.sql"),
        )
        .expect("initial migration fixture should be written");
        fs::write(
            migration_directory.join("0002_add_session_full_text_search.sql"),
            include_str!("../../migrations/0002_add_session_full_text_search.sql"),
        )
        .expect("full-text migration fixture should be written");
        fs::write(
            migration_directory.join("0003_optimize_session_full_text_search.sql"),
            include_str!("../../migrations/0003_optimize_session_full_text_search.sql"),
        )
        .expect("optimized full-text migration fixture should be written");
        fs::write(
            migration_directory.join("0004_intentionally_failing.sql"),
            "CREATE TABLE rollback_probe (id INTEGER PRIMARY KEY) STRICT;\n\
             INSERT INTO rollback_probe (id) VALUES (1);\n\
             INSERT INTO rollback_probe (id) VALUES (1);",
        )
        .expect("failing migration fixture should be written");

        let pool = connect(database.path())
            .await
            .expect("temporary database should connect");
        migrator()
            .run(&pool)
            .await
            .expect("version one should migrate");

        let failing_migrator = Migrator::new(migration_directory.as_path())
            .await
            .expect("test migrations should resolve");
        assert!(failing_migrator.run(&pool).await.is_err());

        let probe_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name = 'rollback_probe'",
        )
        .fetch_one(&pool)
        .await
        .expect("schema should remain readable");
        assert_eq!(probe_count, 0);

        insert_session(&pool, "session_after_rollback").await;
        pool.close().await;
    });
}

#[test]
fn corrupt_database_is_rejected_without_replacing_the_file() {
    tauri::async_runtime::block_on(async {
        let database = TempDatabase::new();
        let corrupt_bytes = b"synthetic invalid sqlite content";
        fs::write(database.path(), corrupt_bytes).expect("corrupt fixture should be written");

        assert!(connect(database.path()).await.is_err());
        assert_eq!(
            fs::read(database.path()).expect("corrupt fixture should remain readable"),
            corrupt_bytes
        );
    });
}

#[test]
fn database_from_a_newer_schema_version_is_rejected() {
    tauri::async_runtime::block_on(async {
        let pool = migrated_memory_pool().await;
        sqlx::query(
            "INSERT INTO _sqlx_migrations
                (version, description, success, checksum, execution_time)
             VALUES (7, 'future migration', 1, zeroblob(32), 0)",
        )
        .execute(&pool)
        .await
        .expect("future migration marker should insert");

        let error = migrator()
            .run(&pool)
            .await
            .expect_err("newer schema should not be accepted");
        assert!(matches!(
            error,
            sqlx::migrate::MigrateError::VersionMissing(7)
        ));
    });
}

#[test]
fn deleting_a_session_cascades_all_session_owned_rows() {
    tauri::async_runtime::block_on(async {
        let pool = migrated_memory_pool().await;
        insert_session(&pool, "session_1").await;
        insert_revision(&pool, "session_1", "revision_1").await;
        sqlx::query(
            "INSERT INTO revision_entries
                (revision_id, entry_key, ordinal, at_ms, kind, body_text)
             VALUES ('revision_1', 'entry_1', 0, 0, 'user', 'hello')",
        )
        .execute(&pool)
        .await
        .expect("entry should insert");
        sqlx::query(
            "INSERT INTO entry_selection_overrides
                (session_id, stable_entry_key, selected, updated_at_ms)
             VALUES ('session_1', 'entry_1', 0, 1000)",
        )
        .execute(&pool)
        .await
        .expect("selection override should insert");
        sqlx::query(
            "UPDATE sessions SET current_revision_id = 'revision_1' WHERE id = 'session_1'",
        )
        .execute(&pool)
        .await
        .expect("current revision should update");

        sqlx::query("DELETE FROM sessions WHERE id = 'session_1'")
            .execute(&pool)
            .await
            .expect("session delete should cascade");

        let remaining_rows: i64 = sqlx::query_scalar(
            "SELECT
                (SELECT count(*) FROM sessions)
                + (SELECT count(*) FROM session_revisions)
                + (SELECT count(*) FROM revision_entries)
                + (SELECT count(*) FROM entry_selection_overrides)",
        )
        .fetch_one(&pool)
        .await
        .expect("session-owned row count should be readable");
        assert_eq!(remaining_rows, 0);
    });
}

#[test]
fn production_configuration_preloads_sql_without_exposing_it_to_the_webview() {
    let config: serde_json::Value = serde_json::from_str(include_str!("../../tauri.conf.json"))
        .expect("Tauri configuration should be valid JSON");
    assert_eq!(
        config["plugins"]["sql"]["preload"],
        serde_json::json!([DATABASE_URL])
    );
    assert_eq!(
        config["bundle"]["windows"]["webviewInstallMode"]["type"],
        "offlineInstaller"
    );

    let capability: serde_json::Value =
        serde_json::from_str(include_str!("../../capabilities/main.json"))
            .expect("main capability should be valid JSON");
    let permissions = capability["permissions"]
        .as_array()
        .expect("permissions should be an array");
    assert!(permissions.iter().all(|permission| {
        permission
            .as_str()
            .is_none_or(|permission| !permission.starts_with("sql:"))
    }));
    assert!(
        !permissions
            .iter()
            .any(|permission| permission == "dialog:allow-open")
    );
    assert!(
        permissions
            .iter()
            .any(|permission| permission == "core:window:allow-set-fullscreen")
    );

    let package: serde_json::Value = serde_json::from_str(include_str!("../../../package.json"))
        .expect("package manifest should be valid JSON");
    assert!(package["dependencies"]["@tauri-apps/plugin-sql"].is_null());
    assert!(package["devDependencies"]["@tauri-apps/plugin-sql"].is_null());
}

#[test]
fn hardened_repository_pool_replaces_and_closes_the_preloaded_plugin_pool() {
    tauri::async_runtime::block_on(async {
        let database = TempDatabase::new();
        let instances = tauri_plugin_sql::DbInstances::default();
        let preloaded_pool = connect(Path::new(":memory:"))
            .await
            .expect("synthetic preloaded pool should connect");
        instances.0.write().await.insert(
            DATABASE_URL.to_owned(),
            tauri_plugin_sql::DbPool::Sqlite(preloaded_pool.clone()),
        );

        let state = prepare_repository_state(&instances, database.path())
            .await
            .expect("repository pool should replace the preloaded pool");

        assert!(preloaded_pool.is_closed());
        let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(state.pool())
            .await
            .expect("replacement pool should be usable");
        assert_eq!(foreign_keys, 1);
        assert!(matches!(
            instances.0.read().await.get(DATABASE_URL),
            Some(tauri_plugin_sql::DbPool::Sqlite(_))
        ));

        state.pool().close().await;
    });
}

#[test]
fn repository_preparation_returns_path_free_errors_for_missing_preload_and_open_failure() {
    tauri::async_runtime::block_on(async {
        let missing_preload_database = TempDatabase::new();
        let missing_preload = prepare_repository_state(
            &tauri_plugin_sql::DbInstances::default(),
            missing_preload_database.path(),
        )
        .await;
        assert!(matches!(
            missing_preload,
            Err(DatabaseBootstrapError::PreloadMissing)
        ));
        assert!(!missing_preload_database.path().exists());

        let invalid_database = TempDatabase::new();
        fs::create_dir(invalid_database.path())
            .expect("invalid database fixture should be a directory");
        let instances = tauri_plugin_sql::DbInstances::default();
        let preloaded_pool = connect(Path::new(":memory:"))
            .await
            .expect("synthetic preloaded pool should connect");
        instances.0.write().await.insert(
            DATABASE_URL.to_owned(),
            tauri_plugin_sql::DbPool::Sqlite(preloaded_pool.clone()),
        );
        let open_failure = prepare_repository_state(&instances, invalid_database.path()).await;
        let error = match open_failure {
            Err(error) => error,
            Ok(_) => panic!("directory path should not open as a database"),
        };
        assert_eq!(error, DatabaseBootstrapError::OpenFailed);
        assert!(
            !error
                .to_string()
                .contains(&invalid_database.path().display().to_string())
        );
        assert!(matches!(
            instances.0.read().await.get(DATABASE_URL),
            Some(tauri_plugin_sql::DbPool::Sqlite(_))
        ));
        assert!(!preloaded_pool.is_closed());
        preloaded_pool.close().await;
    });
}
