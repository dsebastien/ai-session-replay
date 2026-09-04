use std::path::Path;

use sqlx::Row;

use super::repository::{
    IndexedSessionRepository, PersistSessionRevision, PersistedEntry, PersistedRevision,
    PersistedSession, PersistedSourceLocation, RepositoryError, ScanCompletionOutcome, ScanCounts,
    ScanDiagnostic, ScannedCollection,
};
use super::{DatabaseState, connect, migrator};
use crate::indexed_library::{
    ContentAvailability, ContentAvailabilityV1, IndexedSessionSourceV1, ReasoningAvailability,
};

async fn repository() -> IndexedSessionRepository {
    let pool = connect(Path::new(":memory:"))
        .await
        .expect("in-memory database should connect");
    migrator()
        .run(&pool)
        .await
        .expect("initial migration should succeed");
    DatabaseState { pool }.repository()
}

fn utf16le(value: &str) -> Vec<u8> {
    value.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn sample_write(
    session_id: &str,
    vendor_session_id: &str,
    collection: &str,
    marker: u8,
    generation: u64,
) -> PersistSessionRevision {
    PersistSessionRevision {
        session: PersistedSession {
            id: session_id.to_owned(),
            source: IndexedSessionSourceV1::Codex,
            vendor_session_id: vendor_session_id.to_owned(),
            title: format!("Synthetic {session_id}"),
            created_at_ms: Some(900),
            source_version: None,
            collection: Some(collection.to_owned()),
            display_filename: Some(format!("{session_id}.jsonl")),
        },
        revision: PersistedRevision {
            id: format!("revision_{session_id}_{generation}"),
            content_hash: [marker; 32],
            indexed_at_ms: 1_000 + generation,
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
            canonical_path_utf16le: utf16le(&format!(r"C:\Synthetic\{session_id}.jsonl")),
            source_identity_hash: [marker.wrapping_add(40); 32],
            root_identity: vec![marker.wrapping_add(50)],
            file_identity: vec![marker.wrapping_add(60)],
            file_size_bytes: 128,
            modified_at_ms: 950,
            last_seen_generation: Some(generation),
        },
        observed_at_ms: 1_000 + generation,
    }
}

fn counts(
    discovered: u64,
    processed: u64,
    indexed: u64,
    unchanged: u64,
    failed: u64,
) -> ScanCounts {
    ScanCounts {
        discovered,
        processed,
        indexed,
        unchanged,
        failed,
        skipped: 0,
        warnings: 0,
    }
}

#[test]
fn scan_generations_are_monotonic_interrupt_stale_runs_and_bound_history() {
    tauri::async_runtime::block_on(async {
        let repository = repository().await;

        let first = repository.begin_scan(100).await.expect("scan should start");
        assert_eq!(first, 1);
        repository
            .update_scan_progress(first, counts(3, 1, 1, 0, 0))
            .await
            .expect("current scan progress should update");
        let second = repository.begin_scan(200).await.expect("scan should start");
        assert_eq!(second, 2);

        let interrupted = sqlx::query(
            "SELECT status, completed_at_ms, error_code FROM scan_runs WHERE generation = 1",
        )
        .fetch_one(repository.pool())
        .await
        .expect("interrupted scan should remain inspectable");
        assert_eq!(interrupted.get::<String, _>("status"), "failed");
        assert_eq!(interrupted.get::<i64, _>("completed_at_ms"), 200);
        assert_eq!(
            interrupted.get::<String, _>("error_code"),
            "INDEX_SCAN_INTERRUPTED"
        );

        repository
            .complete_scan(second, 201, counts(0, 0, 0, 0, 0), &[], &[])
            .await
            .expect("latest scan should complete");
        for timestamp in 202..=301 {
            let generation = repository
                .begin_scan(timestamp)
                .await
                .expect("bounded history scan should start");
            repository
                .complete_scan(generation, timestamp, counts(0, 0, 0, 0, 0), &[], &[])
                .await
                .expect("bounded history scan should complete");
        }

        let retained: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM scan_runs")
            .fetch_one(repository.pool())
            .await
            .expect("scan history should be countable");
        let oldest: i64 = sqlx::query_scalar("SELECT MIN(generation) FROM scan_runs")
            .fetch_one(repository.pool())
            .await
            .expect("oldest generation should be readable");
        assert_eq!(retained, 100);
        assert_eq!(oldest, 3);
    });
}

#[test]
fn completed_scan_marks_only_absent_sessions_in_authoritative_collections() {
    tauri::async_runtime::block_on(async {
        let repository = repository().await;
        let first = repository
            .begin_scan(1_000)
            .await
            .expect("scan should start");
        let active = sample_write("session_active", "vendor-active", "active", 1, first);
        let archived = sample_write("session_archived", "vendor-archived", "archived", 2, first);
        repository
            .persist_revision(&active)
            .await
            .expect("active session should persist");
        repository
            .persist_revision(&archived)
            .await
            .expect("archived session should persist");
        repository
            .complete_scan(
                first,
                1_001,
                counts(2, 2, 2, 0, 0),
                &[
                    ScannedCollection::new(IndexedSessionSourceV1::Codex, "active"),
                    ScannedCollection::new(IndexedSessionSourceV1::Codex, "archived"),
                ],
                &[],
            )
            .await
            .expect("initial scan should complete");

        let second = repository
            .begin_scan(2_000)
            .await
            .expect("scan should start");
        repository
            .observe_existing_source(
                second,
                &IndexedSessionSourceV1::Codex,
                &active.source_location,
                2_000,
            )
            .await
            .expect("present source should be observed");
        repository
            .complete_scan(
                second,
                2_001,
                counts(1, 1, 0, 0, 1),
                &[ScannedCollection::new(
                    IndexedSessionSourceV1::Codex,
                    "active",
                )],
                &[ScanDiagnostic::new(
                    None,
                    IndexedSessionSourceV1::Codex,
                    "SOURCE_NORMALIZATION_FAILED",
                    1,
                )],
            )
            .await
            .expect("partial failure should still complete the scan");

        let rows = sqlx::query("SELECT id, source_present FROM sessions ORDER BY id")
            .fetch_all(repository.pool())
            .await
            .expect("session presence should be readable");
        assert_eq!(rows[0].get::<String, _>("id"), "session_active");
        assert_eq!(rows[0].get::<i64, _>("source_present"), 1);
        assert_eq!(rows[1].get::<String, _>("id"), "session_archived");
        assert_eq!(rows[1].get::<i64, _>("source_present"), 1);
        let revisions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM session_revisions")
            .fetch_one(repository.pool())
            .await
            .expect("last-known-good revisions should remain");
        assert_eq!(revisions, 2);

        let third = repository
            .begin_scan(3_000)
            .await
            .expect("scan should start");
        repository
            .complete_scan(
                third,
                3_001,
                counts(0, 0, 0, 0, 0),
                &[ScannedCollection::new(
                    IndexedSessionSourceV1::Codex,
                    "active",
                )],
                &[],
            )
            .await
            .expect("empty authoritative scan should complete");
        let presence: Vec<(String, i64)> =
            sqlx::query_as("SELECT id, source_present FROM sessions ORDER BY id")
                .fetch_all(repository.pool())
                .await
                .expect("presence state should be readable");
        assert_eq!(
            presence,
            [
                ("session_active".to_owned(), 0),
                ("session_archived".to_owned(), 1)
            ]
        );
    });
}

#[test]
fn suppression_matches_private_identity_or_vendor_identity_without_crossing_sources() {
    tauri::async_runtime::block_on(async {
        let repository = repository().await;
        sqlx::query(
            "INSERT INTO suppressed_sources (
                id, source, source_identity_hash, vendor_session_id,
                source_deleted, suppressed_at_ms
             ) VALUES ('suppression_1', 'codex', ?, 'vendor-suppressed', 0, 1000)",
        )
        .bind([9_u8; 32].as_slice())
        .execute(repository.pool())
        .await
        .expect("synthetic suppression should insert");

        assert!(
            repository
                .is_source_suppressed(&IndexedSessionSourceV1::Codex, &[9; 32], None)
                .await
                .expect("identity lookup should succeed")
        );
        assert!(
            repository
                .is_source_suppressed(
                    &IndexedSessionSourceV1::Codex,
                    &[8; 32],
                    Some("vendor-suppressed"),
                )
                .await
                .expect("vendor lookup should succeed")
        );
        assert!(
            !repository
                .is_source_suppressed(
                    &IndexedSessionSourceV1::ClaudeCode,
                    &[9; 32],
                    Some("vendor-suppressed"),
                )
                .await
                .expect("source namespaces must stay isolated")
        );
    });
}

#[test]
fn stale_generations_cannot_write_progress_sessions_or_presence() {
    tauri::async_runtime::block_on(async {
        let repository = repository().await;
        let stale = repository
            .begin_scan(1_000)
            .await
            .expect("scan should start");
        let current = repository
            .begin_scan(2_000)
            .await
            .expect("scan should start");

        assert_eq!(
            repository
                .update_scan_progress(stale, counts(1, 1, 1, 0, 0))
                .await,
            Ok(ScanCompletionOutcome::Stale)
        );
        assert_eq!(
            repository
                .complete_scan(
                    stale,
                    2_001,
                    counts(1, 1, 1, 0, 0),
                    &[ScannedCollection::new(
                        IndexedSessionSourceV1::Codex,
                        "active"
                    )],
                    &[],
                )
                .await,
            Ok(ScanCompletionOutcome::Stale)
        );
        let stale_write = sample_write("session_stale", "vendor-stale", "active", 3, stale);
        assert_eq!(
            repository.persist_revision(&stale_write).await,
            Err(RepositoryError::ScanStateConflict)
        );

        repository
            .complete_scan(current, 2_002, counts(0, 0, 0, 0, 0), &[], &[])
            .await
            .expect("current scan should remain completable");
    });
}

#[test]
fn scan_inputs_and_diagnostics_are_bounded_and_path_free() {
    tauri::async_runtime::block_on(async {
        let repository = repository().await;
        assert_eq!(
            repository.begin_scan(9_007_199_254_740_992).await,
            Err(RepositoryError::InvalidInput)
        );

        let generation = repository
            .begin_scan(1_000)
            .await
            .expect("scan should start");
        assert_eq!(
            repository
                .update_scan_progress(generation, counts(1, 2, 0, 0, 0))
                .await,
            Err(RepositoryError::InvalidInput)
        );
        assert_eq!(
            repository
                .complete_scan(
                    generation,
                    1_001,
                    counts(1, 1, 0, 0, 1),
                    &[ScannedCollection::new(
                        IndexedSessionSourceV1::Codex,
                        "active"
                    )],
                    &[ScanDiagnostic::new(
                        None,
                        IndexedSessionSourceV1::Codex,
                        r"C:\private\session.jsonl",
                        1,
                    )],
                )
                .await,
            Err(RepositoryError::InvalidInput)
        );
        for unsafe_code in [
            "1_STARTS_WITH_DIGIT",
            "_STARTS_WITH_UNDERSCORE",
            "lowercase",
            "A-B",
        ] {
            assert_eq!(
                repository
                    .complete_scan(
                        generation,
                        1_001,
                        counts(1, 1, 0, 0, 1),
                        &[],
                        &[ScanDiagnostic::new(
                            None,
                            IndexedSessionSourceV1::Codex,
                            unsafe_code,
                            1,
                        )],
                    )
                    .await,
                Err(RepositoryError::InvalidInput)
            );
        }

        repository
            .fail_scan(
                generation,
                1_001,
                "INDEX_STORAGE_FAILED",
                counts(1, 1, 0, 0, 1),
            )
            .await
            .expect("safe terminal failure should persist");
        let row = sqlx::query("SELECT status, error_code FROM scan_runs WHERE generation = ?")
            .bind(generation as i64)
            .fetch_one(repository.pool())
            .await
            .expect("failed scan should be readable");
        assert_eq!(row.get::<String, _>("status"), "failed");
        assert_eq!(row.get::<String, _>("error_code"), "INDEX_STORAGE_FAILED");
    });
}
