use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::repository::{
    IndexedSessionRepository, PersistSessionRevision, PersistedEntry, PersistedRevision,
    PersistedSession, PersistedSourceLocation, PersistedToolDetail, RepositoryError,
};
use super::{DatabaseState, connect, migrator};
use crate::indexed_library::{
    ContentAvailability, ContentAvailabilityV1, IndexedSessionListRequestV1,
    IndexedSessionSortOrderV1, IndexedSessionSourceV1, IndexedToolStatus, ReasoningAvailability,
    SessionEntryV1,
};

static READ_DATABASE_NONCE: AtomicU64 = AtomicU64::new(0);

struct TempDatabase {
    root: PathBuf,
    path: PathBuf,
}

impl TempDatabase {
    fn new() -> Self {
        let nonce = READ_DATABASE_NONCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "ai-session-replay-read-repository-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("temporary database directory should be created");
        let path = root.join("library.sqlite3");
        Self { root, path }
    }
}

impl Drop for TempDatabase {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

async fn migrated_repository(path: &Path) -> IndexedSessionRepository {
    let pool = connect(path).await.expect("database should connect");
    migrator()
        .run(&pool)
        .await
        .expect("migration should succeed");
    DatabaseState { pool }.repository()
}

fn utf16le(value: &str) -> Vec<u8> {
    value.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn sample_write(
    session_id: &str,
    revision_id: &str,
    source: IndexedSessionSourceV1,
    title: &str,
    indexed_at_ms: u64,
    identity_byte: u8,
) -> PersistSessionRevision {
    PersistSessionRevision {
        session: PersistedSession {
            id: session_id.to_owned(),
            source,
            vendor_session_id: format!("vendor-{session_id}"),
            title: title.to_owned(),
            created_at_ms: Some(indexed_at_ms.saturating_sub(100)),
            source_version: None,
            collection: Some("synthetic".to_owned()),
            display_filename: Some(format!("{session_id}.jsonl")),
        },
        revision: PersistedRevision {
            id: revision_id.to_owned(),
            content_hash: [identity_byte; 32],
            indexed_at_ms,
            source_created_at_ms: None,
            source_updated_at_ms: None,
            duration_ms: 4_000,
            diagnostic_count: 1,
            content_availability: ContentAvailabilityV1 {
                reasoning: ReasoningAvailability::Available,
                tool_details: ContentAvailability::Partial,
            },
            entries: vec![
                PersistedEntry::User {
                    entry_key: format!("{session_id}_user"),
                    ordinal: 0,
                    at_ms: 0,
                    text: "Synthetic <script>alert(1)</script> prompt".to_owned(),
                },
                PersistedEntry::Assistant {
                    entry_key: format!("{session_id}_assistant"),
                    ordinal: 1,
                    at_ms: 1_000,
                    markdown: "**Synthetic** response".to_owned(),
                },
                PersistedEntry::Reasoning {
                    entry_key: format!("{session_id}_reasoning"),
                    ordinal: 2,
                    at_ms: 2_000,
                    text: "Source-provided reasoning".to_owned(),
                },
                PersistedEntry::ToolCall {
                    entry_key: format!("{session_id}_tool"),
                    ordinal: 3,
                    at_ms: 3_000,
                    name: "read_file".to_owned(),
                    status: IndexedToolStatus::Succeeded,
                    summary: "Read a synthetic file".to_owned(),
                    detail: PersistedToolDetail::Available {
                        arguments: Some("{\"path\":\"fixture.txt\"}".to_owned()),
                        result: None,
                    },
                },
                PersistedEntry::FileChange {
                    entry_key: format!("{session_id}_file"),
                    ordinal: 4,
                    at_ms: 3_500,
                    display_path: "src/example.ts".to_owned(),
                    summary: "Updated the synthetic fixture".to_owned(),
                },
                PersistedEntry::Unknown {
                    entry_key: format!("{session_id}_unknown"),
                    ordinal: 5,
                    at_ms: 4_000,
                    source_type: "future_record".to_owned(),
                },
            ],
        },
        source_location: PersistedSourceLocation {
            canonical_path_utf16le: utf16le(&format!(r"C:\Synthetic\{session_id}.jsonl")),
            source_identity_hash: [identity_byte.wrapping_add(32); 32],
            root_identity: vec![identity_byte],
            file_identity: vec![identity_byte.wrapping_add(64)],
            file_size_bytes: 512,
            modified_at_ms: indexed_at_ms,
            last_seen_generation: None,
        },
        observed_at_ms: indexed_at_ms,
    }
}

fn list_request(
    source: Option<IndexedSessionSourceV1>,
    query: &str,
    cursor: Option<String>,
    page_size: usize,
) -> IndexedSessionListRequestV1 {
    IndexedSessionListRequestV1 {
        schema_version: 1,
        source,
        query: query.to_owned(),
        sort_order: IndexedSessionSortOrderV1::Newest,
        cursor,
        page_size,
    }
}

#[test]
fn summaries_are_filtered_sorted_and_keyset_paginated() {
    tauri::async_runtime::block_on(async {
        let repository = migrated_repository(Path::new(":memory:")).await;
        for write in [
            sample_write(
                "session_old",
                "revision_old",
                IndexedSessionSourceV1::ClaudeCode,
                "Older design notes",
                1_000,
                1,
            ),
            sample_write(
                "session_api",
                "revision_api",
                IndexedSessionSourceV1::Codex,
                "API cleanup",
                3_000,
                2,
            ),
            sample_write(
                "session_replay",
                "revision_replay",
                IndexedSessionSourceV1::Codex,
                "Replay architecture",
                2_000,
                3,
            ),
        ] {
            repository
                .persist_revision(&write)
                .await
                .expect("fixture should persist");
        }

        let first = repository
            .list_indexed_sessions(&list_request(None, "", None, 2))
            .await
            .expect("first page should load");
        assert_eq!(
            first
                .items
                .iter()
                .map(|item| item.session_id.as_str())
                .collect::<Vec<_>>(),
            ["session_api", "session_replay"]
        );
        assert_eq!(first.next_cursor.as_deref(), Some("newest__session_replay"));
        let second = repository
            .list_indexed_sessions(&list_request(None, "", first.next_cursor, 2))
            .await
            .expect("second page should load");
        assert_eq!(second.items[0].session_id, "session_old");
        assert!(second.next_cursor.is_none());

        let oldest_first = repository
            .list_indexed_sessions(&IndexedSessionListRequestV1 {
                sort_order: IndexedSessionSortOrderV1::Oldest,
                ..list_request(None, "", None, 2)
            })
            .await
            .expect("oldest-first page should load");
        assert_eq!(
            oldest_first
                .items
                .iter()
                .map(|item| item.session_id.as_str())
                .collect::<Vec<_>>(),
            ["session_old", "session_replay"]
        );
        assert_eq!(
            oldest_first.next_cursor.as_deref(),
            Some("oldest__session_replay")
        );
        let oldest_second = repository
            .list_indexed_sessions(&IndexedSessionListRequestV1 {
                sort_order: IndexedSessionSortOrderV1::Oldest,
                cursor: oldest_first.next_cursor,
                ..list_request(None, "", None, 2)
            })
            .await
            .expect("second oldest-first page should load");
        assert_eq!(oldest_second.items[0].session_id, "session_api");
        assert!(oldest_second.next_cursor.is_none());

        assert_eq!(
            repository
                .list_indexed_sessions(&list_request(
                    None,
                    "",
                    Some("oldest__session_replay".to_owned()),
                    2,
                ))
                .await,
            Err(RepositoryError::InvalidInput)
        );

        let filtered = repository
            .list_indexed_sessions(&list_request(
                Some(IndexedSessionSourceV1::Codex),
                "REPLAY",
                None,
                20,
            ))
            .await
            .expect("filtered page should load");
        assert_eq!(filtered.items.len(), 1);
        assert_eq!(filtered.items[0].title, "Replay architecture");
        assert_eq!(filtered.items[0].entry_count, 6);
        assert_eq!(filtered.items[0].selected_entry_count, 6);

        let mut undated = sample_write(
            "session_undated",
            "revision_undated",
            IndexedSessionSourceV1::CopilotCli,
            "Timestamp fallback",
            4_000,
            4,
        );
        undated.session.created_at_ms = None;
        repository
            .persist_revision(&undated)
            .await
            .expect("undated fixture should persist");
        let newest_with_fallback = repository
            .list_indexed_sessions(&list_request(None, "", None, 1))
            .await
            .expect("newest fallback should load");
        assert_eq!(newest_with_fallback.items[0].session_id, "session_undated");

        assert_eq!(
            repository
                .list_indexed_sessions(&list_request(
                    Some(IndexedSessionSourceV1::ClaudeCode),
                    "",
                    Some("newest__session_api".to_owned()),
                    20,
                ))
                .await,
            Err(RepositoryError::InvalidInput)
        );
        assert_eq!(
            repository
                .list_indexed_sessions(&list_request(
                    None,
                    "older",
                    Some("newest__session_api".to_owned()),
                    20,
                ))
                .await,
            Err(RepositoryError::InvalidInput)
        );

        repository.close().await;
    });
}

#[test]
fn summary_search_matches_normalized_full_text_as_literal_terms() {
    tauri::async_runtime::block_on(async {
        let repository = migrated_repository(Path::new(":memory:")).await;
        let mut write = sample_write(
            "session_search",
            "revision_search",
            IndexedSessionSourceV1::Codex,
            "Replay architecture",
            1_000,
            1,
        );
        write.revision.entries[0] = PersistedEntry::User {
            entry_key: "session_search_user".to_owned(),
            ordinal: 0,
            at_ms: 0,
            text: "Résumé the authentication boundary".to_owned(),
        };
        repository
            .persist_revision(&write)
            .await
            .expect("search fixture should persist");

        for query in [
            "replay",
            "resume",
            "authentication",
            "response",
            "reasoning",
            "read file",
            "read_file",
            "fixture txt",
            "example updated",
            "future record",
            "codex",
        ] {
            let page = repository
                .list_indexed_sessions(&list_request(None, query, None, 20))
                .await
                .expect("full-text query should remain valid");
            assert_eq!(
                page.items
                    .iter()
                    .map(|item| item.session_id.as_str())
                    .collect::<Vec<_>>(),
                ["session_search"],
                "query {query:?} should match normalized session content",
            );
        }

        let cross_entry = repository
            .list_indexed_sessions(&list_request(
                None,
                "authentication response fixture",
                None,
                20,
            ))
            .await
            .expect("terms may match different normalized entries");
        assert_eq!(cross_entry.items.len(), 1);

        for query in ["response OR absent", "+++", "\" OR *"] {
            let page = repository
                .list_indexed_sessions(&list_request(None, query, None, 20))
                .await
                .expect("query syntax must be treated as inert text");
            assert!(
                page.items.is_empty(),
                "query {query:?} must not act as FTS syntax"
            );
        }

        repository.close().await;
    });
}

#[test]
fn summary_search_matches_session_dates_and_timeframes() {
    tauri::async_runtime::block_on(async {
        let repository = migrated_repository(Path::new(":memory:")).await;
        let mut september_first = sample_write(
            "session_september_first",
            "revision_september_first",
            IndexedSessionSourceV1::ClaudeCode,
            "Alpha session",
            1_788_264_000_000,
            21,
        );
        september_first.session.created_at_ms = Some(1_788_264_000_000);
        repository.persist_revision(&september_first).await.unwrap();

        let mut september_later = sample_write(
            "session_september_later",
            "revision_september_later",
            IndexedSessionSourceV1::Codex,
            "Beta session",
            1_789_473_600_000,
            22,
        );
        september_later.session.created_at_ms = None;
        repository.persist_revision(&september_later).await.unwrap();

        let mut october = sample_write(
            "session_october",
            "revision_october",
            IndexedSessionSourceV1::CopilotCli,
            "Gamma session",
            1_790_856_000_000,
            23,
        );
        october.session.created_at_ms = Some(1_790_856_000_000);
        repository.persist_revision(&october).await.unwrap();

        for (query, expected) in [
            (
                "September",
                vec!["session_september_later", "session_september_first"],
            ),
            ("Sep 1", vec!["session_september_first"]),
            ("1 September", vec!["session_september_first"]),
            (
                "September 2026",
                vec!["session_september_later", "session_september_first"],
            ),
            (
                "2026-09",
                vec!["session_september_later", "session_september_first"],
            ),
            ("2026-09-01", vec!["session_september_first"]),
            (
                "2026",
                vec![
                    "session_october",
                    "session_september_later",
                    "session_september_first",
                ],
            ),
        ] {
            let page = repository
                .list_indexed_sessions(&list_request(None, query, None, 20))
                .await
                .expect("date query should remain valid");
            assert_eq!(
                page.items
                    .iter()
                    .map(|item| item.session_id.as_str())
                    .collect::<Vec<_>>(),
                expected,
                "query {query:?} should match the displayed session date",
            );
        }

        let first_page = repository
            .list_indexed_sessions(&list_request(None, "September", None, 1))
            .await
            .expect("date search should paginate");
        assert_eq!(first_page.items[0].session_id, "session_september_later");
        let second_page = repository
            .list_indexed_sessions(&list_request(None, "September", first_page.next_cursor, 1))
            .await
            .expect("date-search cursor should retain its scope");
        assert_eq!(second_page.items[0].session_id, "session_september_first");

        let claude_only = repository
            .list_indexed_sessions(&list_request(
                Some(IndexedSessionSourceV1::ClaudeCode),
                "September",
                None,
                20,
            ))
            .await
            .expect("date search should compose with source filters");
        assert_eq!(claude_only.items[0].session_id, "session_september_first");

        assert!(
            repository
                .list_indexed_sessions(&list_request(None, "Sep 31", None, 20))
                .await
                .expect("an invalid date should remain an inert full-text query")
                .items
                .is_empty()
        );

        repository.close().await;
    });
}

#[test]
fn full_text_index_tracks_title_updates_and_current_revision_changes() {
    tauri::async_runtime::block_on(async {
        let repository = migrated_repository(Path::new(":memory:")).await;
        let mut original = sample_write(
            "session_search",
            "revision_original",
            IndexedSessionSourceV1::Codex,
            "Original title token",
            1_000,
            1,
        );
        original.revision.entries = vec![PersistedEntry::User {
            entry_key: "entry_original".to_owned(),
            ordinal: 0,
            at_ms: 0,
            text: "obsolete transcript token".to_owned(),
        }];
        repository.persist_revision(&original).await.unwrap();

        let mut rewritten = original.clone();
        rewritten.session.title = "Rewritten title token".to_owned();
        rewritten.revision.id = "revision_rewritten".to_owned();
        rewritten.revision.content_hash = [9; 32];
        rewritten.revision.indexed_at_ms = 2_000;
        rewritten.revision.entries = vec![PersistedEntry::Assistant {
            entry_key: "entry_rewritten".to_owned(),
            ordinal: 0,
            at_ms: 0,
            markdown: "current transcript constellation".to_owned(),
        }];
        rewritten.observed_at_ms = 2_000;
        repository.persist_revision(&rewritten).await.unwrap();

        for query in ["original", "obsolete"] {
            assert!(
                repository
                    .list_indexed_sessions(&list_request(None, query, None, 20))
                    .await
                    .unwrap()
                    .items
                    .is_empty(),
                "superseded query {query:?} should not match",
            );
        }
        for query in ["rewritten", "constellation"] {
            assert_eq!(
                repository
                    .list_indexed_sessions(&list_request(None, query, None, 20))
                    .await
                    .unwrap()
                    .items
                    .len(),
                1,
            );
        }

        let mut renamed = rewritten.clone();
        renamed.session.title = "Renamed metadata token".to_owned();
        renamed.observed_at_ms = 3_000;
        repository.persist_revision(&renamed).await.unwrap();
        assert!(
            repository
                .list_indexed_sessions(&list_request(None, "rewritten", None, 20))
                .await
                .unwrap()
                .items
                .is_empty(),
        );
        assert_eq!(
            repository
                .list_indexed_sessions(&list_request(None, "renamed", None, 20))
                .await
                .unwrap()
                .items
                .len(),
            1,
        );
        assert_eq!(
            repository
                .list_indexed_sessions(&list_request(None, "constellation", None, 20))
                .await
                .unwrap()
                .items
                .len(),
            1,
            "an idempotent observation should retain current transcript terms",
        );

        repository.close().await;
    });
}

#[test]
fn summary_selection_counts_default_selected_and_respect_overrides() {
    tauri::async_runtime::block_on(async {
        let repository = migrated_repository(Path::new(":memory:")).await;
        repository
            .persist_revision(&sample_write(
                "session_1",
                "revision_1",
                IndexedSessionSourceV1::Codex,
                "Selection test",
                1_000,
                1,
            ))
            .await
            .expect("fixture should persist");
        sqlx::query(
            "INSERT INTO entry_selection_overrides
                (session_id, stable_entry_key, selected, updated_at_ms)
             VALUES ('session_1', 'session_1_assistant', 0, 1001)",
        )
        .execute(repository.pool())
        .await
        .expect("selection override should persist");

        let page = repository
            .list_indexed_sessions(&list_request(None, "", None, 20))
            .await
            .expect("page should load");
        assert_eq!(page.items[0].selected_entry_count, 5);

        repository.close().await;
    });
}

#[test]
fn detail_reads_all_entry_shapes_preferences_and_missing_source_content() {
    tauri::async_runtime::block_on(async {
        let repository = migrated_repository(Path::new(":memory:")).await;
        repository
            .persist_revision(&sample_write(
                "session_1",
                "revision_1",
                IndexedSessionSourceV1::Codex,
                "Durable detail",
                1_000,
                1,
            ))
            .await
            .expect("fixture should persist");
        sqlx::query("UPDATE sessions SET source_present = 0 WHERE id = 'session_1'")
            .execute(repository.pool())
            .await
            .expect("source should be marked missing");
        sqlx::query(
            "UPDATE session_preferences
             SET show_reasoning = 0, entry_delay_ms = 2500, playback_speed = 1.5
             WHERE session_id = 'session_1'",
        )
        .execute(repository.pool())
        .await
        .expect("preferences should update");

        let detail = repository
            .get_indexed_session("session_1", None)
            .await
            .expect("indexed content should remain readable");

        assert!(!detail.summary.source_present);
        assert_eq!(detail.revision.revision_id, "revision_1");
        assert_eq!(detail.entry_page.entries.len(), 6);
        assert!(detail.entry_page.next_cursor.is_none());
        assert!(!detail.preferences.visibility.show_reasoning);
        assert_eq!(detail.preferences.timing.entry_delay_ms, 2_500);
        assert_eq!(detail.preferences.timing.playback_speed, 1.5);
        assert!(matches!(
            detail.entry_page.entries[0].entry,
            SessionEntryV1::User { ref text, .. } if text.contains("<script>")
        ));
        assert!(matches!(
            detail.entry_page.entries[3].entry,
            SessionEntryV1::ToolCall { ref name, .. } if name == "read_file"
        ));

        repository.close().await;
    });
}

#[test]
fn entry_pages_are_bounded_and_reject_unknown_cursors() {
    tauri::async_runtime::block_on(async {
        let repository = migrated_repository(Path::new(":memory:")).await;
        let mut write = sample_write(
            "session_many",
            "revision_many",
            IndexedSessionSourceV1::Codex,
            "Many entries",
            1_000,
            1,
        );
        write.revision.entries = (0..201)
            .map(|ordinal| PersistedEntry::User {
                entry_key: format!("entry_{ordinal}"),
                ordinal,
                at_ms: ordinal,
                text: format!("Entry {ordinal}"),
            })
            .collect();
        repository
            .persist_revision(&write)
            .await
            .expect("fixture should persist");

        let first = repository
            .get_indexed_session("session_many", None)
            .await
            .expect("first entry page should load");
        assert_eq!(first.entry_page.entries.len(), 200);
        assert_eq!(first.entry_page.total_entry_count, 201);
        let cursor = first
            .entry_page
            .next_cursor
            .expect("another page should exist");
        let second = repository
            .get_indexed_session("session_many", Some(&cursor))
            .await
            .expect("second entry page should load");
        assert_eq!(second.entry_page.entries.len(), 1);
        assert!(matches!(
            second.entry_page.entries[0].entry,
            SessionEntryV1::User { ordinal: 200, .. }
        ));
        assert!(second.entry_page.next_cursor.is_none());

        assert_eq!(
            repository
                .get_indexed_session("session_many", Some("unknown_entry"))
                .await,
            Err(RepositoryError::InvalidInput)
        );
        assert_eq!(
            repository
                .get_indexed_session("missing_session", None)
                .await,
            Err(RepositoryError::SessionNotFound)
        );

        repository.close().await;
    });
}

#[test]
fn indexed_detail_survives_database_restart_after_source_removal() {
    tauri::async_runtime::block_on(async {
        let database = TempDatabase::new();
        let source = database.root.join("synthetic-source.jsonl");
        fs::write(&source, "synthetic source").expect("source fixture should be written");
        let repository = migrated_repository(&database.path).await;
        let mut write = sample_write(
            "session_restart",
            "revision_restart",
            IndexedSessionSourceV1::ClaudeCode,
            "Restart-safe session",
            1_000,
            1,
        );
        write.source_location.canonical_path_utf16le = utf16le(&source.to_string_lossy());
        repository
            .persist_revision(&write)
            .await
            .expect("fixture should persist");
        repository.close().await;
        fs::remove_file(source).expect("source fixture should be removed");

        let reopened = migrated_repository(&database.path).await;
        sqlx::query("UPDATE sessions SET source_present = 0 WHERE id = 'session_restart'")
            .execute(reopened.pool())
            .await
            .expect("completed missing-source scan should update presence");
        let page = reopened
            .list_indexed_sessions(&list_request(None, "", None, 20))
            .await
            .expect("restarted library should load");
        let detail = reopened
            .get_indexed_session("session_restart", None)
            .await
            .expect("restarted detail should load without source");

        assert_eq!(page.items[0].session_id, "session_restart");
        assert!(!detail.summary.source_present);
        assert_eq!(detail.entry_page.entries.len(), 6);
        let serialized = serde_json::to_string(&detail).expect("detail should serialize");
        assert!(!serialized.contains("synthetic-source.jsonl"));
        assert!(!serialized.contains("canonicalPath"));

        reopened.close().await;
    });
}
