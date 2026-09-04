use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sqlx::Row;

use super::repository::{
    ENTRY_INSERT_BATCH_SIZE, IndexedSessionRepository, PersistRevisionOutcome,
    PersistSessionRevision, PersistedEntry, PersistedRevision, PersistedSession,
    PersistedSourceLocation, RepositoryError,
};
use super::{DatabaseState, connect, migrator};
use crate::commands::runtime::build_presentation_plan;
use crate::indexed_library::{
    ContentAvailability, ContentAvailabilityV1, EntrySelectionChangeV1, IndexedSessionSourceV1,
    IndexedToolStatus, ReasoningAvailability, RenameIndexedSessionRequestV1,
    SetEntrySelectionsRequestV1, SetSessionPreferencesRequestV1,
};

static REPOSITORY_TEMP_NONCE: AtomicU64 = AtomicU64::new(0);

struct TempRepositoryDatabase {
    root: PathBuf,
    path: PathBuf,
}

impl TempRepositoryDatabase {
    fn new() -> Self {
        let nonce = REPOSITORY_TEMP_NONCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "ai-session-replay-repository-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("temporary repository directory should be created");
        let path = root.join("library.sqlite3");
        Self { root, path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempRepositoryDatabase {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

async fn migrated_memory_repository() -> IndexedSessionRepository {
    let pool = connect(Path::new(":memory:"))
        .await
        .expect("in-memory database should connect");
    migrator()
        .run(&pool)
        .await
        .expect("initial migration should succeed");
    DatabaseState { pool }.repository()
}

async fn migrated_file_repository(database: &TempRepositoryDatabase) -> IndexedSessionRepository {
    let pool = connect(database.path())
        .await
        .expect("temporary database should connect");
    migrator()
        .run(&pool)
        .await
        .expect("temporary database should migrate");
    DatabaseState { pool }.repository()
}

fn utf16le(value: &str) -> Vec<u8> {
    value.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn sample_write() -> PersistSessionRevision {
    PersistSessionRevision {
        session: PersistedSession {
            id: "session_1".to_owned(),
            source: IndexedSessionSourceV1::Codex,
            vendor_session_id: "vendor-session-1".to_owned(),
            title: "Synthetic session".to_owned(),
            created_at_ms: Some(900),
            source_version: Some("1.0".to_owned()),
            collection: Some("Synthetic collection".to_owned()),
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
            canonical_path_utf16le: utf16le(r"C:\Synthetic\session.jsonl"),
            source_identity_hash: [2; 32],
            root_identity: vec![3],
            file_identity: vec![4],
            file_size_bytes: 128,
            modified_at_ms: 950,
            last_seen_generation: None,
        },
        observed_at_ms: 1_000,
    }
}

#[test]
fn new_session_revision_entries_source_and_defaults_are_atomic() {
    tauri::async_runtime::block_on(async {
        let repository = migrated_memory_repository().await;

        let outcome = repository
            .persist_revision(&sample_write())
            .await
            .expect("valid revision should persist");

        assert_eq!(
            outcome,
            PersistRevisionOutcome::Inserted {
                revision_id: "revision_1".to_owned(),
            }
        );
        let row = sqlx::query(
            "SELECT
                sessions.current_revision_id,
                session_revisions.entry_count,
                revision_entries.body_text,
                length(source_locations.canonical_path_utf16le) AS path_bytes,
                session_preferences.show_tool_calls,
                session_preferences.show_tool_details,
                session_preferences.show_reasoning,
                session_preferences.entry_delay_ms,
                session_preferences.playback_speed,
                session_preferences.background_color,
                session_preferences.surface_color,
                session_preferences.text_color,
                session_preferences.muted_color,
                session_preferences.accent_color,
                session_preferences.success_color,
                session_preferences.error_color,
                session_preferences.font_family,
                session_preferences.font_size_px,
                session_preferences.line_height
             FROM sessions
             JOIN session_revisions ON session_revisions.id = sessions.current_revision_id
             JOIN revision_entries ON revision_entries.revision_id = session_revisions.id
             JOIN source_locations ON source_locations.session_id = sessions.id
             JOIN session_preferences ON session_preferences.session_id = sessions.id
             WHERE sessions.id = ?",
        )
        .bind("session_1")
        .fetch_one(repository.pool())
        .await
        .expect("persisted aggregate should be readable");

        assert_eq!(row.get::<String, _>("current_revision_id"), "revision_1");
        assert_eq!(row.get::<i64, _>("entry_count"), 1);
        assert_eq!(row.get::<String, _>("body_text"), "Synthetic prompt");
        assert!(row.get::<i64, _>("path_bytes") > 0);
        assert_eq!(row.get::<i64, _>("show_tool_calls"), 1);
        assert_eq!(row.get::<i64, _>("show_tool_details"), 1);
        assert_eq!(row.get::<i64, _>("show_reasoning"), 1);
        assert_eq!(row.get::<i64, _>("entry_delay_ms"), 5_000);
        assert_eq!(row.get::<f64, _>("playback_speed"), 1.0);
        assert_eq!(row.get::<String, _>("background_color"), "#101211");
        assert_eq!(row.get::<String, _>("surface_color"), "#171A18");
        assert_eq!(row.get::<String, _>("text_color"), "#F5F5F4");
        assert_eq!(row.get::<String, _>("muted_color"), "#9B9E9C");
        assert_eq!(row.get::<String, _>("accent_color"), "#D6AA68");
        assert_eq!(row.get::<String, _>("success_color"), "#A8D5BD");
        assert_eq!(row.get::<String, _>("error_color"), "#E59A91");
        assert_eq!(row.get::<String, _>("font_family"), "JetBrains Mono");
        assert_eq!(row.get::<i64, _>("font_size_px"), 34);
        assert_eq!(row.get::<f64, _>("line_height"), 1.5);

        repository.close().await;
    });
}

#[test]
fn backend_builds_a_frozen_plan_from_the_requested_current_revision() {
    tauri::async_runtime::block_on(async {
        let repository = migrated_memory_repository().await;
        let mut write = sample_write();
        write.revision.entries = (0..201)
            .map(|ordinal| PersistedEntry::User {
                entry_key: format!("entry_{ordinal}"),
                ordinal,
                at_ms: ordinal * 10,
                text: format!("Synthetic prompt {ordinal}"),
            })
            .collect();
        write.revision.duration_ms = 3_000;
        repository.persist_revision(&write).await.unwrap();
        repository
            .set_entry_selections(
                &SetEntrySelectionsRequestV1 {
                    schema_version: 1,
                    session_id: "session_1".to_owned(),
                    revision_id: "revision_1".to_owned(),
                    changes: vec![EntrySelectionChangeV1 {
                        entry_key: "entry_100".to_owned(),
                        selected: false,
                    }],
                },
                1_500,
            )
            .await
            .unwrap();

        let plan = build_presentation_plan(
            &repository,
            "session_1",
            "revision_1",
            "plan_test".to_owned(),
            2_000,
        )
        .await
        .unwrap();

        assert_eq!(plan.session_id, "session_1");
        assert_eq!(plan.revision_id, "revision_1");
        assert_eq!(plan.entries.len(), 200);
        assert_eq!(plan.entries[0].reveal_at_ms, 0);
        assert_eq!(
            serde_json::to_value(&plan.entries[100].entry).unwrap()["ordinal"],
            101
        );
        assert_eq!(plan.duration_ms, 1_000_000);
        assert_eq!(
            build_presentation_plan(
                &repository,
                "session_1",
                "stale_revision",
                "plan_stale".to_owned(),
                2_000,
            )
            .await
            .unwrap_err()
            .code,
            "STALE_REVISION"
        );
        repository.close().await;
    });
}

#[test]
fn unchanged_content_updates_observation_without_duplicate_revision_or_reset_preferences() {
    tauri::async_runtime::block_on(async {
        let repository = migrated_memory_repository().await;
        repository
            .persist_revision(&sample_write())
            .await
            .expect("initial revision should persist");
        sqlx::query(
            "UPDATE session_preferences
             SET entry_delay_ms = 2500, updated_at_ms = 1500
             WHERE session_id = 'session_1'",
        )
        .execute(repository.pool())
        .await
        .expect("synthetic preference should update");

        let mut unchanged = sample_write();
        unchanged.revision.id = "unused_revision_id".to_owned();
        unchanged.session.title = "Updated synthetic title".to_owned();
        unchanged.source_location.file_size_bytes = 256;
        unchanged.observed_at_ms = 2_000;
        let outcome = repository
            .persist_revision(&unchanged)
            .await
            .expect("unchanged content should persist observation metadata");

        assert_eq!(
            outcome,
            PersistRevisionOutcome::Unchanged {
                revision_id: "revision_1".to_owned(),
            }
        );
        let revision_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM session_revisions WHERE session_id = 'session_1'",
        )
        .fetch_one(repository.pool())
        .await
        .expect("revision count should be readable");
        let row = sqlx::query(
            "SELECT
                sessions.title, sessions.first_indexed_at_ms, sessions.last_seen_at_ms,
                sessions.current_revision_id, source_locations.file_size_bytes,
                session_preferences.entry_delay_ms, session_revisions.indexed_at_ms
             FROM sessions
             JOIN source_locations ON source_locations.session_id = sessions.id
             JOIN session_preferences ON session_preferences.session_id = sessions.id
             JOIN session_revisions ON session_revisions.id = sessions.current_revision_id
             WHERE sessions.id = 'session_1'",
        )
        .fetch_one(repository.pool())
        .await
        .expect("updated aggregate should be readable");

        assert_eq!(revision_count, 1);
        assert_eq!(row.get::<String, _>("title"), "Updated synthetic title");
        assert_eq!(row.get::<i64, _>("first_indexed_at_ms"), 1_000);
        assert_eq!(row.get::<i64, _>("last_seen_at_ms"), 2_000);
        assert_eq!(row.get::<String, _>("current_revision_id"), "revision_1");
        assert_eq!(row.get::<i64, _>("file_size_bytes"), 256);
        assert_eq!(row.get::<i64, _>("entry_delay_ms"), 2_500);
        assert_eq!(row.get::<i64, _>("indexed_at_ms"), 1_000);

        repository.close().await;
    });
}

#[test]
fn appended_and_rewritten_content_add_immutable_revisions_and_advance_current() {
    tauri::async_runtime::block_on(async {
        let repository = migrated_memory_repository().await;
        repository
            .persist_revision(&sample_write())
            .await
            .expect("initial revision should persist");

        sqlx::query(
            "INSERT INTO entry_selection_overrides
                (session_id, stable_entry_key, selected, updated_at_ms)
             VALUES ('session_1', 'entry_1', 0, 1500)",
        )
        .execute(repository.pool())
        .await
        .expect("selection override should be created for the original revision");

        let mut appended = sample_write();
        appended.revision.id = "revision_2".to_owned();
        appended.revision.content_hash = [5; 32];
        appended.revision.indexed_at_ms = 2_000;
        appended.revision.entries.push(PersistedEntry::Assistant {
            entry_key: "entry_2".to_owned(),
            ordinal: 1,
            at_ms: 500,
            markdown: "Synthetic response".to_owned(),
        });
        appended.revision.duration_ms = 2_000;
        appended.observed_at_ms = 2_000;
        assert!(matches!(
            repository.persist_revision(&appended).await,
            Ok(PersistRevisionOutcome::Inserted { revision_id }) if revision_id == "revision_2"
        ));

        let mut rewritten = sample_write();
        rewritten.revision.id = "revision_3".to_owned();
        rewritten.revision.content_hash = [6; 32];
        rewritten.revision.indexed_at_ms = 3_000;
        rewritten.revision.entries = vec![PersistedEntry::User {
            entry_key: "entry_rewritten".to_owned(),
            ordinal: 0,
            at_ms: 0,
            text: "Rewritten prompt".to_owned(),
        }];
        rewritten.observed_at_ms = 3_000;
        assert!(matches!(
            repository.persist_revision(&rewritten).await,
            Ok(PersistRevisionOutcome::Inserted { revision_id }) if revision_id == "revision_3"
        ));

        let revision_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM session_revisions WHERE session_id = ?")
                .bind("session_1")
                .fetch_one(repository.pool())
                .await
                .expect("revision count should be readable");
        let original_body: String = sqlx::query_scalar(
            "SELECT body_text FROM revision_entries
             WHERE revision_id = 'revision_1' AND entry_key = 'entry_1'",
        )
        .fetch_one(repository.pool())
        .await
        .expect("original immutable entry should remain readable");
        let current_revision: String =
            sqlx::query_scalar("SELECT current_revision_id FROM sessions WHERE id = ?")
                .bind("session_1")
                .fetch_one(repository.pool())
                .await
                .expect("current revision should be readable");
        let retained_selection: i64 = sqlx::query_scalar(
            "SELECT selected FROM entry_selection_overrides
             WHERE session_id = 'session_1' AND stable_entry_key = 'entry_1'",
        )
        .fetch_one(repository.pool())
        .await
        .expect("selection override should survive revision changes");

        assert_eq!(revision_count, 3);
        assert_eq!(original_body, "Synthetic prompt");
        assert_eq!(current_revision, "revision_3");
        assert_eq!(retained_selection, 0);
        repository.close().await;
    });
}

#[test]
fn every_typed_entry_payload_is_persisted_without_raw_record_fallbacks() {
    tauri::async_runtime::block_on(async {
        let repository = migrated_memory_repository().await;
        let mut write = sample_write();
        write.revision.content_availability = ContentAvailabilityV1 {
            reasoning: ReasoningAvailability::Available,
            tool_details: ContentAvailability::Partial,
        };
        write.revision.entries = vec![
            PersistedEntry::User {
                entry_key: "entry_user".to_owned(),
                ordinal: 0,
                at_ms: 0,
                text: "Prompt".to_owned(),
            },
            PersistedEntry::Assistant {
                entry_key: "entry_assistant".to_owned(),
                ordinal: 1,
                at_ms: 100,
                markdown: "Response".to_owned(),
            },
            PersistedEntry::Reasoning {
                entry_key: "entry_reasoning".to_owned(),
                ordinal: 2,
                at_ms: 200,
                text: "Source-provided reasoning".to_owned(),
            },
            PersistedEntry::ToolCall {
                entry_key: "entry_tool_available".to_owned(),
                ordinal: 3,
                at_ms: 300,
                name: "synthetic-tool".to_owned(),
                status: IndexedToolStatus::Succeeded,
                summary: "Tool completed".to_owned(),
                detail: super::repository::PersistedToolDetail::Available {
                    arguments: Some("{\"safe\":true}".to_owned()),
                    result: None,
                },
            },
            PersistedEntry::ToolCall {
                entry_key: "entry_tool_unavailable".to_owned(),
                ordinal: 4,
                at_ms: 400,
                name: "synthetic-tool".to_owned(),
                status: IndexedToolStatus::Pending,
                summary: "Tool pending".to_owned(),
                detail: super::repository::PersistedToolDetail::Unavailable,
            },
            PersistedEntry::FileChange {
                entry_key: "entry_file".to_owned(),
                ordinal: 5,
                at_ms: 500,
                display_path: "src/example.rs".to_owned(),
                summary: "Changed synthetic file".to_owned(),
            },
            PersistedEntry::Unknown {
                entry_key: "entry_unknown".to_owned(),
                ordinal: 6,
                at_ms: 600,
                source_type: "future-record".to_owned(),
            },
        ];

        repository
            .persist_revision(&write)
            .await
            .expect("all typed entry variants should persist");

        let rows = sqlx::query(
            "SELECT kind, body_text, tool_name, tool_status, summary_text,
                    tool_detail_availability, tool_arguments, tool_result,
                    display_path, source_type
             FROM revision_entries WHERE revision_id = ? ORDER BY ordinal",
        )
        .bind("revision_1")
        .fetch_all(repository.pool())
        .await
        .expect("typed entries should be readable");

        assert_eq!(rows.len(), 7);
        assert_eq!(rows[0].get::<String, _>("kind"), "user");
        assert_eq!(rows[1].get::<String, _>("kind"), "assistant");
        assert_eq!(rows[2].get::<String, _>("kind"), "reasoning");
        assert_eq!(rows[3].get::<String, _>("tool_status"), "succeeded");
        assert_eq!(
            rows[3].get::<String, _>("tool_arguments"),
            "{\"safe\":true}"
        );
        assert_eq!(
            rows[4].get::<String, _>("tool_detail_availability"),
            "unavailable"
        );
        assert_eq!(rows[5].get::<String, _>("display_path"), "src/example.rs");
        assert_eq!(rows[6].get::<String, _>("source_type"), "future-record");

        repository.close().await;
    });
}

#[test]
fn entry_batches_preserve_order_across_the_batch_boundary() {
    tauri::async_runtime::block_on(async {
        let repository = migrated_memory_repository().await;
        let mut write = sample_write();
        write.revision.entries = (0..=ENTRY_INSERT_BATCH_SIZE)
            .map(|ordinal| PersistedEntry::User {
                entry_key: format!("entry_{ordinal}"),
                ordinal: ordinal as u64,
                at_ms: ordinal as u64,
                text: format!("Entry {ordinal}"),
            })
            .collect();

        repository
            .persist_revision(&write)
            .await
            .expect("entries spanning two batches should persist");

        let stored: Vec<(i64, String)> = sqlx::query_as(
            "SELECT ordinal, body_text
             FROM revision_entries WHERE revision_id = ? ORDER BY ordinal",
        )
        .bind("revision_1")
        .fetch_all(repository.pool())
        .await
        .expect("batched entries should be readable in order");
        assert_eq!(stored.len(), ENTRY_INSERT_BATCH_SIZE + 1);
        assert_eq!(stored.first(), Some(&(0, "Entry 0".to_owned())));
        assert_eq!(
            stored.last(),
            Some(&(
                ENTRY_INSERT_BATCH_SIZE as i64,
                format!("Entry {ENTRY_INSERT_BATCH_SIZE}"),
            ))
        );
        repository.close().await;
    });
}

#[test]
fn invalid_repository_inputs_are_rejected_before_any_rows_are_written() {
    tauri::async_runtime::block_on(async {
        let repository = migrated_memory_repository().await;
        let mut invalid_inputs = Vec::new();

        let mut invalid_id = sample_write();
        invalid_id.session.id = "invalid id".to_owned();
        invalid_inputs.push(invalid_id);

        let mut empty_id = sample_write();
        empty_id.session.id.clear();
        invalid_inputs.push(empty_id);

        let mut oversized_id = sample_write();
        oversized_id.session.id = "s".repeat(257);
        invalid_inputs.push(oversized_id);

        let mut empty_title = sample_write();
        empty_title.session.title.clear();
        invalid_inputs.push(empty_title);

        let mut empty_vendor_id = sample_write();
        empty_vendor_id.session.vendor_session_id.clear();
        invalid_inputs.push(empty_vendor_id);

        let mut oversized_source_version = sample_write();
        oversized_source_version.session.source_version = Some("s".repeat(257));
        invalid_inputs.push(oversized_source_version);

        let mut oversized_collection = sample_write();
        oversized_collection.session.collection = Some("c".repeat(513));
        invalid_inputs.push(oversized_collection);

        let mut oversized_display_filename = sample_write();
        oversized_display_filename.session.display_filename = Some("f".repeat(513));
        invalid_inputs.push(oversized_display_filename);

        let mut excessive_created_at = sample_write();
        excessive_created_at.session.created_at_ms = Some(9_007_199_254_740_992);
        invalid_inputs.push(excessive_created_at);

        let mut invalid_revision_id = sample_write();
        invalid_revision_id.revision.id = "invalid revision".to_owned();
        invalid_inputs.push(invalid_revision_id);

        let mut excessive_indexed_at = sample_write();
        excessive_indexed_at.revision.indexed_at_ms = 9_007_199_254_740_992;
        invalid_inputs.push(excessive_indexed_at);

        let mut excessive_source_created_at = sample_write();
        excessive_source_created_at.revision.source_created_at_ms = Some(9_007_199_254_740_992);
        invalid_inputs.push(excessive_source_created_at);

        let mut excessive_source_updated_at = sample_write();
        excessive_source_updated_at.revision.source_updated_at_ms = Some(9_007_199_254_740_992);
        invalid_inputs.push(excessive_source_updated_at);

        let mut excessive_timestamp = sample_write();
        excessive_timestamp.observed_at_ms = 9_007_199_254_740_992;
        invalid_inputs.push(excessive_timestamp);

        let mut no_entries = sample_write();
        no_entries.revision.entries.clear();
        invalid_inputs.push(no_entries);

        let mut excessive_entries = sample_write();
        excessive_entries.revision.entries = vec![
            PersistedEntry::User {
                entry_key: "unused".to_owned(),
                ordinal: 0,
                at_ms: 0,
                text: String::new(),
            };
            100_001
        ];
        invalid_inputs.push(excessive_entries);

        let mut excessive_duration = sample_write();
        excessive_duration.revision.duration_ms = 604_800_001;
        invalid_inputs.push(excessive_duration);

        let mut excessive_diagnostics = sample_write();
        excessive_diagnostics.revision.diagnostic_count = 1_001;
        invalid_inputs.push(excessive_diagnostics);

        let mut duplicate_ordinal = sample_write();
        duplicate_ordinal
            .revision
            .entries
            .push(PersistedEntry::User {
                entry_key: "entry_2".to_owned(),
                ordinal: 0,
                at_ms: 1,
                text: "Duplicate ordinal".to_owned(),
            });
        invalid_inputs.push(duplicate_ordinal);

        let mut decreasing_time = sample_write();
        decreasing_time.revision.entries = vec![
            PersistedEntry::User {
                entry_key: "entry_1".to_owned(),
                ordinal: 0,
                at_ms: 100,
                text: "First".to_owned(),
            },
            PersistedEntry::User {
                entry_key: "entry_2".to_owned(),
                ordinal: 1,
                at_ms: 99,
                text: "Second".to_owned(),
            },
        ];
        invalid_inputs.push(decreasing_time);

        let mut odd_utf16_path = sample_write();
        odd_utf16_path.source_location.canonical_path_utf16le = vec![0x43, 0, 0x3a];
        invalid_inputs.push(odd_utf16_path);

        let mut empty_utf16_path = sample_write();
        empty_utf16_path
            .source_location
            .canonical_path_utf16le
            .clear();
        invalid_inputs.push(empty_utf16_path);

        let mut nul_utf16_path = sample_write();
        nul_utf16_path.source_location.canonical_path_utf16le = vec![0, 0];
        invalid_inputs.push(nul_utf16_path);

        let mut empty_root_identity = sample_write();
        empty_root_identity.source_location.root_identity.clear();
        invalid_inputs.push(empty_root_identity);

        let mut empty_file_identity = sample_write();
        empty_file_identity.source_location.file_identity.clear();
        invalid_inputs.push(empty_file_identity);

        let mut excessive_file_size = sample_write();
        excessive_file_size.source_location.file_size_bytes = 9_007_199_254_740_992;
        invalid_inputs.push(excessive_file_size);

        let mut excessive_modified_at = sample_write();
        excessive_modified_at.source_location.modified_at_ms = 9_007_199_254_740_992;
        invalid_inputs.push(excessive_modified_at);

        let mut zero_scan_generation = sample_write();
        zero_scan_generation.source_location.last_seen_generation = Some(0);
        invalid_inputs.push(zero_scan_generation);

        let mut excessive_scan_generation = sample_write();
        excessive_scan_generation
            .source_location
            .last_seen_generation = Some(9_007_199_254_740_992);
        invalid_inputs.push(excessive_scan_generation);

        let mut unavailable_detail_claim = sample_write();
        unavailable_detail_claim.revision.entries = vec![PersistedEntry::ToolCall {
            entry_key: "entry_tool".to_owned(),
            ordinal: 0,
            at_ms: 0,
            name: "tool".to_owned(),
            status: IndexedToolStatus::Succeeded,
            summary: "No detail".to_owned(),
            detail: super::repository::PersistedToolDetail::Available {
                arguments: None,
                result: None,
            },
        }];
        invalid_inputs.push(unavailable_detail_claim);

        let mut invalid_entry_key = sample_write();
        invalid_entry_key.revision.entries = vec![PersistedEntry::User {
            entry_key: "bad key".to_owned(),
            ordinal: 0,
            at_ms: 0,
            text: String::new(),
        }];
        invalid_inputs.push(invalid_entry_key);

        let mut excessive_entry_ordinal = sample_write();
        excessive_entry_ordinal.revision.entries = vec![PersistedEntry::User {
            entry_key: "entry_1".to_owned(),
            ordinal: 100_000,
            at_ms: 0,
            text: String::new(),
        }];
        invalid_inputs.push(excessive_entry_ordinal);

        let mut excessive_entry_time = sample_write();
        excessive_entry_time.revision.duration_ms = 604_800_000;
        excessive_entry_time.revision.entries = vec![PersistedEntry::User {
            entry_key: "entry_1".to_owned(),
            ordinal: 0,
            at_ms: 604_800_001,
            text: String::new(),
        }];
        invalid_inputs.push(excessive_entry_time);

        let mut entry_after_duration = sample_write();
        entry_after_duration.revision.entries = vec![PersistedEntry::User {
            entry_key: "entry_1".to_owned(),
            ordinal: 0,
            at_ms: 1_501,
            text: String::new(),
        }];
        invalid_inputs.push(entry_after_duration);

        let mut duplicate_entry_key = sample_write();
        duplicate_entry_key
            .revision
            .entries
            .push(PersistedEntry::User {
                entry_key: "entry_1".to_owned(),
                ordinal: 1,
                at_ms: 1,
                text: String::new(),
            });
        invalid_inputs.push(duplicate_entry_key);

        let mut invalid_user_text = sample_write();
        invalid_user_text.revision.entries = vec![PersistedEntry::User {
            entry_key: "entry_1".to_owned(),
            ordinal: 0,
            at_ms: 0,
            text: "contains\0nul".to_owned(),
        }];
        invalid_inputs.push(invalid_user_text);

        let mut invalid_assistant_text = sample_write();
        invalid_assistant_text.revision.entries = vec![PersistedEntry::Assistant {
            entry_key: "entry_1".to_owned(),
            ordinal: 0,
            at_ms: 0,
            markdown: "contains\0nul".to_owned(),
        }];
        invalid_inputs.push(invalid_assistant_text);

        let mut invalid_tool_name = sample_write();
        invalid_tool_name.revision.entries = vec![PersistedEntry::ToolCall {
            entry_key: "entry_1".to_owned(),
            ordinal: 0,
            at_ms: 0,
            name: String::new(),
            status: IndexedToolStatus::Failed,
            summary: String::new(),
            detail: super::repository::PersistedToolDetail::Unavailable,
        }];
        invalid_inputs.push(invalid_tool_name);

        let mut invalid_tool_summary = sample_write();
        invalid_tool_summary.revision.entries = vec![PersistedEntry::ToolCall {
            entry_key: "entry_1".to_owned(),
            ordinal: 0,
            at_ms: 0,
            name: "tool".to_owned(),
            status: IndexedToolStatus::Failed,
            summary: "contains\0nul".to_owned(),
            detail: super::repository::PersistedToolDetail::Unavailable,
        }];
        invalid_inputs.push(invalid_tool_summary);

        let mut invalid_tool_arguments = sample_write();
        invalid_tool_arguments.revision.entries = vec![PersistedEntry::ToolCall {
            entry_key: "entry_1".to_owned(),
            ordinal: 0,
            at_ms: 0,
            name: "tool".to_owned(),
            status: IndexedToolStatus::Failed,
            summary: String::new(),
            detail: super::repository::PersistedToolDetail::Available {
                arguments: Some("contains\0nul".to_owned()),
                result: None,
            },
        }];
        invalid_inputs.push(invalid_tool_arguments);

        let mut invalid_tool_result = sample_write();
        invalid_tool_result.revision.entries = vec![PersistedEntry::ToolCall {
            entry_key: "entry_1".to_owned(),
            ordinal: 0,
            at_ms: 0,
            name: "tool".to_owned(),
            status: IndexedToolStatus::Failed,
            summary: String::new(),
            detail: super::repository::PersistedToolDetail::Available {
                arguments: None,
                result: Some("contains\0nul".to_owned()),
            },
        }];
        invalid_inputs.push(invalid_tool_result);

        let mut invalid_file_path = sample_write();
        invalid_file_path.revision.entries = vec![PersistedEntry::FileChange {
            entry_key: "entry_1".to_owned(),
            ordinal: 0,
            at_ms: 0,
            display_path: String::new(),
            summary: String::new(),
        }];
        invalid_inputs.push(invalid_file_path);

        let mut invalid_file_summary = sample_write();
        invalid_file_summary.revision.entries = vec![PersistedEntry::FileChange {
            entry_key: "entry_1".to_owned(),
            ordinal: 0,
            at_ms: 0,
            display_path: "src/example.rs".to_owned(),
            summary: "contains\0nul".to_owned(),
        }];
        invalid_inputs.push(invalid_file_summary);

        let mut invalid_unknown_type = sample_write();
        invalid_unknown_type.revision.entries = vec![PersistedEntry::Unknown {
            entry_key: "entry_1".to_owned(),
            ordinal: 0,
            at_ms: 0,
            source_type: String::new(),
        }];
        invalid_inputs.push(invalid_unknown_type);

        for input in invalid_inputs {
            assert_eq!(
                repository.persist_revision(&input).await,
                Err(RepositoryError::InvalidInput)
            );
        }

        let stored_sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
            .fetch_one(repository.pool())
            .await
            .expect("session count should be readable");
        assert_eq!(stored_sessions, 0);
        repository.close().await;
    });
}

#[test]
fn supported_sources_statuses_and_nullable_metadata_round_trip() {
    tauri::async_runtime::block_on(async {
        let repository = migrated_memory_repository().await;
        for (index, source) in [
            IndexedSessionSourceV1::ClaudeCode,
            IndexedSessionSourceV1::Codex,
            IndexedSessionSourceV1::CopilotCli,
            IndexedSessionSourceV1::VscodeCopilot,
        ]
        .into_iter()
        .enumerate()
        {
            let mut write = sample_write();
            write.session.id = format!("session_{index}");
            write.session.source = source;
            write.session.vendor_session_id = format!("vendor_{index}");
            write.session.created_at_ms = None;
            write.session.source_version = None;
            write.session.collection = None;
            write.session.display_filename = None;
            write.revision.id = format!("revision_{index}");
            write.revision.content_hash = [index as u8 + 20; 32];
            write.revision.source_created_at_ms = None;
            write.revision.source_updated_at_ms = None;
            write.revision.content_availability = ContentAvailabilityV1 {
                reasoning: ReasoningAvailability::Available,
                tool_details: ContentAvailability::Available,
            };
            write.revision.entries = vec![PersistedEntry::ToolCall {
                entry_key: format!("entry_{index}"),
                ordinal: 0,
                at_ms: 0,
                name: "tool".to_owned(),
                status: if index == 0 {
                    IndexedToolStatus::Running
                } else {
                    IndexedToolStatus::Failed
                },
                summary: String::new(),
                detail: super::repository::PersistedToolDetail::Available {
                    arguments: None,
                    result: Some("result".to_owned()),
                },
            }];
            write.source_location.source_identity_hash = [index as u8 + 30; 32];
            write.source_location.last_seen_generation = None;
            repository
                .persist_revision(&write)
                .await
                .expect("supported source and nullable metadata should persist");
        }

        let sources: Vec<String> = sqlx::query_scalar("SELECT source FROM sessions ORDER BY id")
            .fetch_all(repository.pool())
            .await
            .expect("stored sources should be readable");
        assert_eq!(
            sources,
            ["claude-code", "codex", "copilot-cli", "vscode-copilot"]
        );
        repository.close().await;
    });
}

#[test]
fn identity_conflicts_are_rejected_without_mutating_the_existing_session() {
    tauri::async_runtime::block_on(async {
        let repository = migrated_memory_repository().await;
        repository
            .persist_revision(&sample_write())
            .await
            .expect("initial revision should persist");

        let mut changed_identity = sample_write();
        changed_identity.session.vendor_session_id = "different-vendor-session".to_owned();
        changed_identity.session.title = "Must not replace title".to_owned();
        assert_eq!(
            repository.persist_revision(&changed_identity).await,
            Err(RepositoryError::SessionIdentityConflict)
        );

        let mut changed_source = sample_write();
        changed_source.session.source = IndexedSessionSourceV1::ClaudeCode;
        assert_eq!(
            repository.persist_revision(&changed_source).await,
            Err(RepositoryError::SessionIdentityConflict)
        );

        let mut duplicate_vendor_identity = sample_write();
        duplicate_vendor_identity.session.id = "session_2".to_owned();
        duplicate_vendor_identity.revision.id = "revision_2".to_owned();
        duplicate_vendor_identity.revision.content_hash = [8; 32];
        duplicate_vendor_identity
            .source_location
            .source_identity_hash = [8; 32];
        assert_eq!(
            repository
                .persist_revision(&duplicate_vendor_identity)
                .await,
            Err(RepositoryError::SessionIdentityConflict)
        );

        let mut duplicate_revision_id = sample_write();
        duplicate_revision_id.revision.content_hash = [9; 32];
        assert_eq!(
            repository.persist_revision(&duplicate_revision_id).await,
            Err(RepositoryError::RevisionIdentityConflict)
        );

        let mut duplicate_source_identity = sample_write();
        duplicate_source_identity.session.id = "session_3".to_owned();
        duplicate_source_identity.session.vendor_session_id = "vendor-session-3".to_owned();
        duplicate_source_identity.revision.id = "revision_3".to_owned();
        duplicate_source_identity.revision.content_hash = [11; 32];
        assert_eq!(
            repository
                .persist_revision(&duplicate_source_identity)
                .await,
            Err(RepositoryError::SourceIdentityConflict)
        );

        let title: String = sqlx::query_scalar("SELECT title FROM sessions WHERE id = 'session_1'")
            .fetch_one(repository.pool())
            .await
            .expect("existing title should be readable");
        let revision_count: i64 = sqlx::query_scalar("SELECT count(*) FROM session_revisions")
            .fetch_one(repository.pool())
            .await
            .expect("revision count should be readable");
        assert_eq!(title, "Synthetic session");
        assert_eq!(revision_count, 1);
        repository.close().await;
    });
}

#[test]
fn database_failure_rolls_back_the_whole_aggregate_and_returns_a_path_free_error() {
    tauri::async_runtime::block_on(async {
        let repository = migrated_memory_repository().await;
        repository
            .persist_revision(&sample_write())
            .await
            .expect("last-known-good revision should persist");
        sqlx::query(
            "CREATE TRIGGER reject_synthetic_revision
             BEFORE INSERT ON session_revisions
             BEGIN
                 SELECT RAISE(ABORT, 'synthetic write failure');
             END",
        )
        .execute(repository.pool())
        .await
        .expect("synthetic failure trigger should install");
        let mut write = sample_write();
        write.session.title = "Must roll back".to_owned();
        write.revision.id = "revision_2".to_owned();
        write.revision.content_hash = [10; 32];
        write.source_location.canonical_path_utf16le = utf16le(r"C:\Sensitive\must-not-leak.jsonl");

        let error = repository
            .persist_revision(&write)
            .await
            .expect_err("database failure should roll back the transaction");
        assert_eq!(error, RepositoryError::WriteFailed);
        assert_eq!(error.to_string(), "REPOSITORY_WRITE_FAILED");
        assert!(!error.to_string().contains("Sensitive"));

        let retained: (String, String, i64, i64, i64) = sqlx::query_as(
            "SELECT sessions.title, sessions.current_revision_id,
                    (SELECT count(*) FROM session_revisions),
                    (SELECT count(*) FROM revision_entries),
                    (SELECT count(*) FROM session_preferences)
             FROM sessions WHERE id = 'session_1'",
        )
        .fetch_one(repository.pool())
        .await
        .expect("last-known-good aggregate should remain readable");
        assert_eq!(
            retained,
            (
                "Synthetic session".to_owned(),
                "revision_1".to_owned(),
                1,
                1,
                1,
            )
        );
        repository.close().await;
    });
}

#[test]
fn transcript_and_metadata_values_are_bound_as_data() {
    tauri::async_runtime::block_on(async {
        let repository = migrated_memory_repository().await;
        let hostile = "value'); DROP TABLE sessions; --";
        let mut write = sample_write();
        write.session.title = hostile.to_owned();
        write.session.vendor_session_id = hostile.to_owned();
        write.revision.entries = vec![PersistedEntry::User {
            entry_key: "entry_bound".to_owned(),
            ordinal: 0,
            at_ms: 0,
            text: hostile.to_owned(),
        }];

        repository
            .persist_revision(&write)
            .await
            .expect("SQL-looking content should persist as inert data");
        let stored: (String, String, String) = sqlx::query_as(
            "SELECT sessions.title, sessions.vendor_session_id, revision_entries.body_text
             FROM sessions
             JOIN revision_entries ON revision_entries.revision_id = sessions.current_revision_id",
        )
        .fetch_one(repository.pool())
        .await
        .expect("bound values should remain readable");
        assert_eq!(
            stored,
            (hostile.to_owned(), hostile.to_owned(), hostile.to_owned())
        );

        let sessions_table: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name = 'sessions'",
        )
        .fetch_one(repository.pool())
        .await
        .expect("schema should remain readable");
        assert_eq!(sessions_table, 1);
        repository.close().await;
    });
}

#[test]
fn persisted_revisions_survive_repository_and_database_restart() {
    tauri::async_runtime::block_on(async {
        let database = TempRepositoryDatabase::new();
        let repository = migrated_file_repository(&database).await;
        repository
            .persist_revision(&sample_write())
            .await
            .expect("revision should persist before restart");
        repository.close().await;

        let reopened = migrated_file_repository(&database).await;
        let stored: (String, String) = sqlx::query_as(
            "SELECT sessions.current_revision_id, revision_entries.body_text
             FROM sessions
             JOIN revision_entries ON revision_entries.revision_id = sessions.current_revision_id
             WHERE sessions.id = ?",
        )
        .bind("session_1")
        .fetch_one(reopened.pool())
        .await
        .expect("persisted revision should survive restart");
        assert_eq!(
            stored,
            ("revision_1".to_owned(), "Synthetic prompt".to_owned())
        );
        reopened.close().await;
    });
}

#[test]
fn concurrent_reader_keeps_a_consistent_snapshot_while_current_revision_advances() {
    tauri::async_runtime::block_on(async {
        let database = TempRepositoryDatabase::new();
        let repository = migrated_file_repository(&database).await;
        repository
            .persist_revision(&sample_write())
            .await
            .expect("initial revision should persist");

        let mut reader = repository
            .pool()
            .begin()
            .await
            .expect("reader transaction should begin");
        let before: String =
            sqlx::query_scalar("SELECT current_revision_id FROM sessions WHERE id = ?")
                .bind("session_1")
                .fetch_one(&mut *reader)
                .await
                .expect("reader should establish the original snapshot");
        assert_eq!(before, "revision_1");

        let mut changed = sample_write();
        changed.revision.id = "revision_2".to_owned();
        changed.revision.content_hash = [12; 32];
        changed.revision.indexed_at_ms = 2_000;
        changed.observed_at_ms = 2_000;
        repository
            .persist_revision(&changed)
            .await
            .expect("writer should commit while the WAL reader remains active");

        let during: String =
            sqlx::query_scalar("SELECT current_revision_id FROM sessions WHERE id = ?")
                .bind("session_1")
                .fetch_one(&mut *reader)
                .await
                .expect("existing reader snapshot should remain consistent");
        assert_eq!(during, "revision_1");
        reader.commit().await.expect("reader should commit");

        let after: String =
            sqlx::query_scalar("SELECT current_revision_id FROM sessions WHERE id = ?")
                .bind("session_1")
                .fetch_one(repository.pool())
                .await
                .expect("new reader should observe the committed revision");
        assert_eq!(after, "revision_2");
        repository.close().await;
    });
}

#[test]
fn local_database_reset_clears_all_app_data_and_keeps_the_schema_usable() {
    tauri::async_runtime::block_on(async {
        let repository = migrated_memory_repository().await;
        repository
            .persist_revision(&sample_write())
            .await
            .expect("session should persist before reset");
        sqlx::query(
            "INSERT INTO suppressed_sources (
                id, source, source_identity_hash, vendor_session_id, source_deleted, suppressed_at_ms
             ) VALUES ('suppression_1', 'codex', ?, 'suppressed-vendor', 0, 1000)",
        )
        .bind([9_u8; 32].as_slice())
        .execute(repository.pool())
        .await
        .expect("suppression should exist before reset");
        sqlx::query(
            "INSERT INTO scan_runs (
                generation, status, started_at_ms, completed_at_ms,
                discovered_count, processed_count, indexed_count, unchanged_count, failed_count
             ) VALUES (1, 'completed', 1000, 1001, 0, 0, 0, 0, 0)",
        )
        .execute(repository.pool())
        .await
        .expect("scan should exist before reset");
        sqlx::query(
            "INSERT INTO source_diagnostics (
                scan_generation, session_id, source, code, occurrence_count, recorded_at_ms
             ) VALUES (1, NULL, 'codex', 'SYNTHETIC_FAILURE', 1, 1001)",
        )
        .execute(repository.pool())
        .await
        .expect("diagnostic should exist before reset");

        repository
            .reset_local_database()
            .await
            .expect("local database reset should succeed");

        for table in [
            "sessions",
            "source_locations",
            "session_revisions",
            "revision_entries",
            "entry_selection_overrides",
            "session_preferences",
            "scan_runs",
            "source_diagnostics",
            "suppressed_sources",
            "session_search_documents",
            "session_search",
        ] {
            let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(repository.pool())
                .await
                .expect("application table should remain readable");
            assert_eq!(count, 0, "{table} should be empty");
        }
        let migration_count: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
            .fetch_one(repository.pool())
            .await
            .expect("migration history should remain readable");
        assert_eq!(migration_count, 6);

        repository
            .persist_revision(&sample_write())
            .await
            .expect("schema should remain writable after reset");
        repository.close().await;
    });
}

#[test]
fn local_database_reset_rolls_back_every_delete_on_failure() {
    tauri::async_runtime::block_on(async {
        let repository = migrated_memory_repository().await;
        repository
            .persist_revision(&sample_write())
            .await
            .expect("session should persist before reset");
        sqlx::query(
            "INSERT INTO suppressed_sources (
                id, source, source_identity_hash, vendor_session_id, source_deleted, suppressed_at_ms
             ) VALUES ('suppression_1', 'codex', ?, 'suppressed-vendor', 0, 1000)",
        )
        .bind([9_u8; 32].as_slice())
        .execute(repository.pool())
        .await
        .expect("suppression should exist before reset");
        sqlx::query(
            "CREATE TRIGGER reject_reset
             BEFORE DELETE ON suppressed_sources
             BEGIN
                 SELECT RAISE(ABORT, 'synthetic reset failure');
             END",
        )
        .execute(repository.pool())
        .await
        .expect("synthetic failure trigger should install");

        assert_eq!(
            repository.reset_local_database().await,
            Err(RepositoryError::WriteFailed)
        );
        let retained: (i64, i64, i64) = sqlx::query_as(
            "SELECT
                (SELECT count(*) FROM sessions),
                (SELECT count(*) FROM session_revisions),
                (SELECT count(*) FROM suppressed_sources)",
        )
        .fetch_one(repository.pool())
        .await
        .expect("pre-reset data should remain after rollback");
        assert_eq!(retained, (1, 1, 1));
        repository.close().await;
    });
}

#[test]
fn reset_database_reopens_empty_with_migrations_intact() {
    tauri::async_runtime::block_on(async {
        let database = TempRepositoryDatabase::new();
        let repository = migrated_file_repository(&database).await;
        repository
            .persist_revision(&sample_write())
            .await
            .expect("session should persist before reset");
        repository
            .reset_local_database()
            .await
            .expect("reset should succeed");
        repository.close().await;

        let reopened = migrated_file_repository(&database).await;
        let counts: (i64, i64) = sqlx::query_as(
            "SELECT
                (SELECT count(*) FROM sessions),
                (SELECT count(*) FROM _sqlx_migrations)",
        )
        .fetch_one(reopened.pool())
        .await
        .expect("reset database should reopen");
        assert_eq!(counts, (0, 6));
        reopened.close().await;
    });
}

#[test]
fn reset_best_effort_overwrites_deleted_content_and_truncates_the_wal() {
    tauri::async_runtime::block_on(async {
        let database = TempRepositoryDatabase::new();
        let repository = migrated_file_repository(&database).await;
        let mut write = sample_write();
        const SENTINEL: &str = "SYNTHETIC_PRIVATE_RESET_SENTINEL_4F9A";
        write.revision.entries = vec![PersistedEntry::User {
            entry_key: "entry_1".to_owned(),
            ordinal: 0,
            at_ms: 0,
            text: SENTINEL.to_owned(),
        }];
        repository.persist_revision(&write).await.unwrap();
        repository.reset_local_database().await.unwrap();
        repository.close().await;

        for path in [
            database.path().to_path_buf(),
            PathBuf::from(format!("{}-wal", database.path().display())),
        ] {
            let bytes = match fs::read(path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => panic!("reset artifact should be readable: {error}"),
            };
            assert!(
                !bytes
                    .windows(SENTINEL.len())
                    .any(|window| window == SENTINEL.as_bytes())
            );
        }
    });
}

#[test]
fn typed_curation_writes_defaults_overrides_preferences_and_rejects_stale_revisions() {
    tauri::async_runtime::block_on(async {
        let repository = migrated_memory_repository().await;
        repository.persist_revision(&sample_write()).await.unwrap();
        let exclude = SetEntrySelectionsRequestV1 {
            schema_version: 1,
            session_id: "session_1".to_owned(),
            revision_id: "revision_1".to_owned(),
            changes: vec![EntrySelectionChangeV1 {
                entry_key: "entry_1".to_owned(),
                selected: false,
            }],
        };
        repository
            .set_entry_selections(&exclude, 2_000)
            .await
            .expect("selection override should persist");
        let selected: i64 = sqlx::query_scalar(
            "SELECT selected FROM entry_selection_overrides
             WHERE session_id = 'session_1' AND stable_entry_key = 'entry_1'",
        )
        .fetch_one(repository.pool())
        .await
        .unwrap();
        assert_eq!(selected, 0);

        let include = SetEntrySelectionsRequestV1 {
            changes: vec![EntrySelectionChangeV1 {
                entry_key: "entry_1".to_owned(),
                selected: true,
            }],
            ..exclude.clone()
        };
        repository
            .set_entry_selections(&include, 2_100)
            .await
            .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM entry_selection_overrides")
            .fetch_one(repository.pool())
            .await
            .unwrap();
        assert_eq!(count, 0);

        let mut preferences = repository
            .get_indexed_session("session_1", None)
            .await
            .unwrap()
            .preferences;
        preferences.visibility.show_reasoning = false;
        preferences.timing.entry_delay_ms = 2_000;
        let preference_request = SetSessionPreferencesRequestV1 {
            schema_version: 1,
            session_id: "session_1".to_owned(),
            revision_id: "revision_1".to_owned(),
            preferences,
        };
        repository
            .set_session_preferences(&preference_request, 2_200)
            .await
            .expect("preferences should persist");
        let stored: (i64, i64) = sqlx::query_as(
            "SELECT show_reasoning, entry_delay_ms FROM session_preferences
             WHERE session_id = 'session_1'",
        )
        .fetch_one(repository.pool())
        .await
        .unwrap();
        assert_eq!(stored, (0, 2_000));

        let stale = SetEntrySelectionsRequestV1 {
            revision_id: "revision_stale".to_owned(),
            ..exclude
        };
        assert_eq!(
            repository.set_entry_selections(&stale, 2_300).await,
            Err(RepositoryError::StaleRevision)
        );
        repository.close().await;
    });
}

#[test]
fn local_session_title_survives_refresh_restart_search_and_presentation() {
    tauri::async_runtime::block_on(async {
        let database = TempRepositoryDatabase::new();
        let repository = migrated_file_repository(&database).await;
        repository.persist_revision(&sample_write()).await.unwrap();
        repository
            .rename_session(&RenameIndexedSessionRequestV1 {
                schema_version: 1,
                session_id: "session_1".to_owned(),
                title: "My local replay".to_owned(),
            })
            .await
            .expect("local title should persist");

        let mut refreshed = sample_write();
        refreshed.session.title = "Source title changed".to_owned();
        refreshed.revision.id = "revision_2".to_owned();
        refreshed.revision.content_hash = [2; 32];
        repository.persist_revision(&refreshed).await.unwrap();
        let renamed = repository
            .get_indexed_session("session_1", None)
            .await
            .expect("renamed session should remain readable after refresh");
        assert_eq!(renamed.summary.title, "My local replay");

        let search = repository
            .list_indexed_sessions(&crate::indexed_library::IndexedSessionListRequestV1 {
                schema_version: 1,
                source: None,
                query: "local replay".to_owned(),
                sort_order: crate::indexed_library::IndexedSessionSortOrderV1::Newest,
                cursor: None,
                page_size: 20,
            })
            .await
            .expect("renamed title should be searchable");
        assert_eq!(search.items.len(), 1);
        let plan = build_presentation_plan(
            &repository,
            "session_1",
            "revision_2",
            "rename_plan".to_owned(),
            3_000,
        )
        .await
        .expect("presentation should use the local title");
        assert_eq!(plan.session_title, "My local replay");
        repository.close().await;

        let reopened = migrated_file_repository(&database).await;
        assert_eq!(
            reopened
                .get_indexed_session("session_1", None)
                .await
                .expect("renamed session should survive restart")
                .summary
                .title,
            "My local replay"
        );
        reopened.close().await;
    });
}

#[test]
fn local_session_rename_rejects_invalid_or_missing_sessions() {
    tauri::async_runtime::block_on(async {
        let repository = migrated_memory_repository().await;
        repository.persist_revision(&sample_write()).await.unwrap();
        assert_eq!(
            repository
                .rename_session(&RenameIndexedSessionRequestV1 {
                    schema_version: 1,
                    session_id: "session_1".to_owned(),
                    title: "  ".to_owned(),
                })
                .await,
            Err(RepositoryError::InvalidInput)
        );
        assert_eq!(
            repository
                .rename_session(&RenameIndexedSessionRequestV1 {
                    schema_version: 1,
                    session_id: "session_missing".to_owned(),
                    title: "Missing".to_owned(),
                })
                .await,
            Err(RepositoryError::SessionNotFound)
        );
        repository.close().await;
    });
}
