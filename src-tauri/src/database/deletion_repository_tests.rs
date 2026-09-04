use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::repository::{
    IndexedSessionRepository, PersistRevisionOutcome, PersistSessionRevision, PersistedEntry,
    PersistedRevision, PersistedSession, PersistedSourceLocation, RepositoryError,
};
use super::{DatabaseState, connect, migrator};
use crate::indexed_library::{
    ContentAvailability, ContentAvailabilityV1, IndexedSessionSourceV1, ReasoningAvailability,
    SuppressedSourceListRequestV1,
};

static DELETION_DATABASE_NONCE: AtomicU64 = AtomicU64::new(0);

struct TempDeletionDatabase {
    root: PathBuf,
    path: PathBuf,
    source_path: PathBuf,
}

impl TempDeletionDatabase {
    fn new() -> Self {
        let nonce = DELETION_DATABASE_NONCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "ai-session-replay-deletion-repository-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("temporary deletion directory should be created");
        let path = root.join("library.sqlite3");
        let source_path = root.join("session.jsonl");
        fs::write(&source_path, b"synthetic source").expect("synthetic source should be written");
        Self {
            root,
            path,
            source_path,
        }
    }
}

impl Drop for TempDeletionDatabase {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

async fn migrated_repository(path: &Path) -> IndexedSessionRepository {
    let pool = connect(path)
        .await
        .expect("temporary deletion database should connect");
    migrator()
        .run(&pool)
        .await
        .expect("temporary deletion database should migrate");
    DatabaseState { pool }.repository()
}

fn utf16le(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn sample_write(path: &Path) -> PersistSessionRevision {
    PersistSessionRevision {
        session: PersistedSession {
            id: "session_1".to_owned(),
            source: IndexedSessionSourceV1::Codex,
            vendor_session_id: "vendor-session-1".to_owned(),
            title: "Synthetic deletion session".to_owned(),
            created_at_ms: Some(900),
            source_version: Some("1.0".to_owned()),
            collection: Some("active".to_owned()),
            display_filename: Some("session.jsonl".to_owned()),
        },
        revision: PersistedRevision {
            id: "revision_1".to_owned(),
            content_hash: [1; 32],
            indexed_at_ms: 1_000,
            source_created_at_ms: Some(900),
            source_updated_at_ms: Some(950),
            duration_ms: 1_500,
            diagnostic_count: 0,
            content_availability: ContentAvailabilityV1 {
                reasoning: ReasoningAvailability::Unavailable,
                tool_details: ContentAvailability::Unavailable,
            },
            entries: vec![PersistedEntry::User {
                entry_key: "entry_1".to_owned(),
                ordinal: 0,
                at_ms: 0,
                text: "Synthetic prompt".to_owned(),
            }],
        },
        source_location: PersistedSourceLocation {
            canonical_path_utf16le: utf16le(path),
            source_identity_hash: [2; 32],
            root_identity: vec![3; 24],
            file_identity: vec![4; 24],
            file_size_bytes: 16,
            modified_at_ms: 950,
            last_seen_generation: None,
        },
        observed_at_ms: 1_000,
    }
}

#[test]
fn library_deletion_is_atomic_source_preserving_and_restorable_after_restart() {
    tauri::async_runtime::block_on(async {
        let database = TempDeletionDatabase::new();
        let repository = migrated_repository(&database.path).await;
        repository
            .persist_revision(&sample_write(&database.source_path))
            .await
            .expect("fixture should persist");
        let source = repository
            .source_deletion_record("session_1")
            .await
            .expect("backend source record should be available");
        assert_eq!(source.session_id, "session_1");
        assert_eq!(source.vendor_session_id, "vendor-session-1");
        assert_eq!(source.collection, "active");
        assert_eq!(
            source.canonical_path_utf16le,
            utf16le(&database.source_path)
        );
        assert_eq!(source.source_identity_hash, [2; 32]);
        assert_eq!(source.root_identity, vec![3; 24]);
        assert_eq!(source.file_identity, vec![4; 24]);

        let tombstone = repository
            .delete_indexed_session("session_1", "suppression_1", false, 1_100, None)
            .await
            .expect("library deletion should succeed");

        assert_eq!(tombstone.suppression_id, "suppression_1");
        assert!(!tombstone.source_deleted);
        assert!(database.source_path.exists());
        for table in [
            "sessions",
            "source_locations",
            "session_revisions",
            "revision_entries",
            "entry_selection_overrides",
            "session_preferences",
            "session_search_documents",
            "session_search",
        ] {
            let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(repository.pool())
                .await
                .expect("cascade count should be readable");
            assert_eq!(count, 0, "{table} should be empty");
        }
        repository.close().await;

        let reopened = migrated_repository(&database.path).await;
        let page = reopened
            .list_suppressed_sources(&SuppressedSourceListRequestV1 {
                schema_version: 1,
                cursor: None,
                page_size: 20,
            })
            .await
            .expect("tombstone should survive restart");
        assert_eq!(page.items, vec![tombstone]);

        let restored = reopened
            .restore_suppressed_source("suppression_1")
            .await
            .expect("suppression should restore");
        assert_eq!(restored.suppression_id, "suppression_1");
        assert!(
            reopened
                .list_suppressed_sources(&SuppressedSourceListRequestV1 {
                    schema_version: 1,
                    cursor: None,
                    page_size: 20,
                })
                .await
                .expect("empty page should read")
                .items
                .is_empty()
        );
        assert!(database.source_path.exists());
    });
}

#[test]
fn deletion_rollback_and_identity_guards_keep_the_last_known_good_session() {
    tauri::async_runtime::block_on(async {
        let database = TempDeletionDatabase::new();
        let repository = migrated_repository(&database.path).await;
        repository
            .persist_revision(&sample_write(&database.source_path))
            .await
            .expect("fixture should persist");

        assert_eq!(
            repository.mark_source_missing("session_1", &[9; 32]).await,
            Err(RepositoryError::SourceStateChanged)
        );
        assert_eq!(
            repository
                .delete_indexed_session("session_1", "suppression_1", true, 1_100, Some(&[9; 32]),)
                .await,
            Err(RepositoryError::SourceStateChanged)
        );

        sqlx::query(
            "CREATE TRIGGER reject_session_delete BEFORE DELETE ON sessions
             BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END",
        )
        .execute(repository.pool())
        .await
        .expect("failure trigger should install");
        assert_eq!(
            repository
                .delete_indexed_session("session_1", "suppression_1", false, 1_100, None)
                .await,
            Err(RepositoryError::WriteFailed)
        );
        let session_count: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
            .fetch_one(repository.pool())
            .await
            .expect("session count should read");
        let suppression_count: i64 = sqlx::query_scalar("SELECT count(*) FROM suppressed_sources")
            .fetch_one(repository.pool())
            .await
            .expect("suppression count should read");
        assert_eq!((session_count, suppression_count), (1, 0));
    });
}

#[test]
fn suppression_blocks_atomic_reimport_by_private_or_vendor_identity() {
    tauri::async_runtime::block_on(async {
        let database = TempDeletionDatabase::new();
        let repository = migrated_repository(&database.path).await;
        let original = sample_write(&database.source_path);
        repository
            .persist_revision(&original)
            .await
            .expect("fixture should persist");
        repository
            .delete_indexed_session("session_1", "suppression_1", false, 1_100, None)
            .await
            .expect("library deletion should succeed");

        let mut same_source = original.clone();
        same_source.session.id = "session_2".to_owned();
        same_source.revision.id = "revision_2".to_owned();
        assert_eq!(
            repository.persist_revision(&same_source).await,
            Ok(PersistRevisionOutcome::Suppressed)
        );

        let mut same_vendor = same_source;
        same_vendor.session.id = "session_3".to_owned();
        same_vendor.revision.id = "revision_3".to_owned();
        same_vendor.source_location.source_identity_hash = [8; 32];
        assert_eq!(
            repository.persist_revision(&same_vendor).await,
            Ok(PersistRevisionOutcome::Suppressed)
        );
        let session_count: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
            .fetch_one(repository.pool())
            .await
            .expect("session count should read");
        assert_eq!(session_count, 0);
    });
}

#[test]
fn suppression_listing_is_bounded_and_cursor_scoped() {
    tauri::async_runtime::block_on(async {
        let repository = migrated_repository(Path::new(":memory:")).await;
        for (id, time) in [("suppression_1", 100), ("suppression_2", 200)] {
            sqlx::query(
                "INSERT INTO suppressed_sources (
                    id, source, source_identity_hash, vendor_session_id,
                    source_deleted, suppressed_at_ms
                 ) VALUES (?, 'codex', ?, ?, 0, ?)",
            )
            .bind(id)
            .bind(
                if id == "suppression_1" {
                    [1; 32]
                } else {
                    [2; 32]
                }
                .as_slice(),
            )
            .bind(format!("vendor-{id}"))
            .bind(time)
            .execute(repository.pool())
            .await
            .expect("tombstone should insert");
        }

        let first = repository
            .list_suppressed_sources(&SuppressedSourceListRequestV1 {
                schema_version: 1,
                cursor: None,
                page_size: 1,
            })
            .await
            .expect("first page should read");
        assert_eq!(first.items[0].suppression_id, "suppression_2");
        assert_eq!(first.next_cursor.as_deref(), Some("suppression_2"));
        let second = repository
            .list_suppressed_sources(&SuppressedSourceListRequestV1 {
                schema_version: 1,
                cursor: first.next_cursor,
                page_size: 1,
            })
            .await
            .expect("second page should read");
        assert_eq!(second.items[0].suppression_id, "suppression_1");
        assert_eq!(second.next_cursor, None);

        assert_eq!(
            repository
                .list_suppressed_sources(&SuppressedSourceListRequestV1 {
                    schema_version: 1,
                    cursor: Some("missing_cursor".to_owned()),
                    page_size: 1,
                })
                .await,
            Err(RepositoryError::InvalidInput)
        );
        assert_eq!(
            repository.restore_suppressed_source("missing").await,
            Err(RepositoryError::SuppressionNotFound)
        );
    });
}
