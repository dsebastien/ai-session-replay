use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::catalog::{
    AvailabilityDiagnostic, AvailabilityStatus, CatalogId, CatalogIdGenerator, DiscoveryCatalog,
    DiscoverySource,
};
use super::index::{
    IndexClock, IndexCoordinator, IndexError, IndexIdGenerator, availability_diagnostic_code,
    indexed_model_source, indexed_source, persisted_content_availability, persisted_entry,
    receive_refresh_result,
};
use super::index_runtime::register_and_start;
use super::roots::{CandidateMatcher, FixtureRootProvider, RootProvider, RootSpec};
use crate::database::repository::{IndexedSessionRepository, PersistedEntry, PersistedToolDetail};
use crate::database::{connect, migrator};
use crate::indexed_library::{
    EntrySelectionChangeV1, IndexRefreshStateV1, IndexedSessionListRequestV1,
    IndexedSessionSourceV1, SetEntrySelectionsRequestV1, SetSessionPreferencesRequestV1,
};
use crate::model::{NormalizedSessionV2, SessionSource};

struct TempTree(PathBuf);

impl TempTree {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should follow the epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "ai-session-index-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary fixture root should be created");
        Self(path)
    }

    fn copy_fixture(&self, fixture_name: &str, relative: &str) {
        let destination = self.0.join(relative);
        fs::create_dir_all(
            destination
                .parent()
                .expect("fixture destination should have a parent"),
        )
        .expect("fixture directory should be created");
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("tests/fixtures")
                .join(fixture_name),
            destination,
        )
        .expect("synthetic fixture should be copied");
    }

    fn three_source_provider(&self) -> FixtureRootProvider {
        let mut provider = FixtureRootProvider::new(&self.0).expect("fixture root should be valid");
        provider
            .add_root(
                DiscoverySource::ClaudeCode,
                "projects",
                Path::new("claude"),
                CandidateMatcher::ClaudeProject,
            )
            .expect("Claude fixture root should be accepted");
        provider
            .add_root(
                DiscoverySource::Codex,
                "active",
                Path::new("codex"),
                CandidateMatcher::Codex,
            )
            .expect("Codex fixture root should be accepted");
        provider
            .add_root_with_read_scope(
                DiscoverySource::CopilotCli,
                "sessions",
                Path::new("copilot/session-state"),
                Path::new("copilot"),
                CandidateMatcher::CopilotCli,
            )
            .expect("Copilot fixture root should be accepted");
        provider
    }

    fn copy_codex_fixtures(&self, count: usize) {
        for index in 0..count {
            self.copy_fixture(
                "codex-current.jsonl",
                &format!(r"codex\rollout-{index:03}.jsonl"),
            );
        }
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug)]
struct DeterministicCatalogIds(&'static str);

impl CatalogIdGenerator for DeterministicCatalogIds {
    fn generate(&self, sequence: u64) -> CatalogId {
        CatalogId::new(format!("{}_{sequence}", self.0))
    }
}

#[derive(Debug)]
struct DeterministicIndexIds(AtomicU64);

impl IndexIdGenerator for DeterministicIndexIds {
    fn generate(&self) -> Result<String, super::index::IndexError> {
        Ok(format!(
            "indexed_{:04}",
            self.0.fetch_add(1, Ordering::Relaxed)
        ))
    }
}

#[derive(Debug)]
struct IncrementingClock(AtomicU64);

impl IndexClock for IncrementingClock {
    fn now_ms(&self) -> u64 {
        self.0.fetch_add(10, Ordering::Relaxed)
    }
}

async fn repository() -> IndexedSessionRepository {
    let pool = connect(Path::new(":memory:"))
        .await
        .expect("in-memory database should connect");
    migrator()
        .run(&pool)
        .await
        .expect("initial migration should succeed");
    IndexedSessionRepository::new(pool)
}

fn completed_counts(state: &IndexRefreshStateV1) -> (u64, u64, u64, u64, u64, u64, u64, u64) {
    match state {
        IndexRefreshStateV1::Completed {
            generation,
            discovered_count,
            processed_count,
            indexed_count,
            unchanged_count,
            failed_count,
            skipped_count,
            warning_count,
            ..
        } => (
            *generation,
            *discovered_count,
            *processed_count,
            *indexed_count,
            *unchanged_count,
            *failed_count,
            *skipped_count,
            *warning_count,
        ),
        other => panic!("expected completed refresh, got {other:?}"),
    }
}

fn coordinator(
    repository: IndexedSessionRepository,
    provider: Arc<dyn RootProvider>,
    observed: Arc<Mutex<Vec<IndexRefreshStateV1>>>,
) -> IndexCoordinator {
    IndexCoordinator::with_dependencies(
        repository,
        provider,
        DiscoveryCatalog::new(DeterministicCatalogIds("catalog")),
        Arc::new(DeterministicIndexIds(AtomicU64::new(1))),
        Arc::new(IncrementingClock(AtomicU64::new(1_000))),
        Arc::new(move |state| {
            observed
                .lock()
                .expect("observer lock should remain available")
                .push(state);
        }),
    )
}

#[test]
fn milestone_one_workflow_survives_restart_source_removal_curation_presentation_and_deletion() {
    tauri::async_runtime::block_on(async {
        let tree = TempTree::new("milestone-workflow");
        let source_path = tree.0.join(r"codex\rollout.jsonl");
        tree.copy_fixture("codex-current.jsonl", r"codex\rollout.jsonl");
        let database_path = tree.0.join("library.sqlite3");
        let pool = connect(&database_path)
            .await
            .expect("database should connect");
        migrator().run(&pool).await.expect("migration should run");
        let repository = IndexedSessionRepository::new(pool);
        let first = coordinator(
            repository.clone(),
            Arc::new(tree.three_source_provider()),
            Arc::new(Mutex::new(Vec::new())),
        );
        first.refresh().await.expect("source should index");
        let list_request = IndexedSessionListRequestV1 {
            schema_version: 1,
            source: None,
            query: String::new(),
            sort_order: crate::indexed_library::IndexedSessionSortOrderV1::Newest,
            cursor: None,
            page_size: 10,
        };
        let page = repository
            .list_indexed_sessions(&list_request)
            .await
            .expect("indexed session should list");
        assert_eq!(page.items.len(), 1);
        let session_id = page.items[0].session_id.clone();
        let detail = repository
            .get_indexed_session(&session_id, None)
            .await
            .expect("indexed session should open");
        let revision_id = detail.revision.revision_id.clone();
        let first_entry_key = match &detail.entry_page.entries[0].entry {
            crate::indexed_library::SessionEntryV1::User { entry_key, .. }
            | crate::indexed_library::SessionEntryV1::Assistant { entry_key, .. }
            | crate::indexed_library::SessionEntryV1::Reasoning { entry_key, .. }
            | crate::indexed_library::SessionEntryV1::ToolCall { entry_key, .. }
            | crate::indexed_library::SessionEntryV1::FileChange { entry_key, .. }
            | crate::indexed_library::SessionEntryV1::Unknown { entry_key, .. } => {
                entry_key.clone()
            }
        };
        repository
            .set_entry_selections(
                &SetEntrySelectionsRequestV1 {
                    schema_version: 1,
                    session_id: session_id.clone(),
                    revision_id: revision_id.clone(),
                    changes: vec![EntrySelectionChangeV1 {
                        entry_key: first_entry_key,
                        selected: false,
                    }],
                },
                2_000,
            )
            .await
            .expect("selection should persist");
        let mut preferences = detail.preferences;
        preferences.timing.playback_speed = 2.0;
        repository
            .set_session_preferences(
                &SetSessionPreferencesRequestV1 {
                    schema_version: 1,
                    session_id: session_id.clone(),
                    revision_id: revision_id.clone(),
                    preferences,
                },
                2_010,
            )
            .await
            .expect("presentation preferences should persist");
        drop(first);
        repository.pool().close().await;
        drop(repository);
        fs::remove_file(source_path).expect("synthetic source should be removable");

        let reopened_pool = connect(&database_path)
            .await
            .expect("database should reopen");
        migrator()
            .run(&reopened_pool)
            .await
            .expect("migration should remain idempotent");
        let reopened = IndexedSessionRepository::new(reopened_pool);
        let second = coordinator(
            reopened.clone(),
            Arc::new(tree.three_source_provider()),
            Arc::new(Mutex::new(Vec::new())),
        );
        second
            .refresh()
            .await
            .expect("missing source scan should complete");
        let retained = reopened
            .get_indexed_session(&session_id, None)
            .await
            .expect("indexed content should survive source removal and restart");
        assert!(!retained.summary.source_present);
        assert_eq!(
            retained.summary.selected_entry_count + 1,
            retained.summary.entry_count
        );
        assert_eq!(retained.preferences.timing.playback_speed, 2.0);

        let plan = crate::commands::runtime::build_presentation_plan(
            &reopened,
            &session_id,
            &revision_id,
            "workflow_plan".to_owned(),
            3_000,
        )
        .await
        .expect("the retained curated session should produce a frozen plan");
        assert_eq!(
            plan.entries.len() as u64,
            retained.summary.selected_entry_count
        );
        assert_eq!(plan.preferences.timing.playback_speed, 2.0);

        reopened
            .delete_indexed_session(&session_id, "workflow_suppression", false, 4_000, None)
            .await
            .expect("library deletion should succeed");
        assert!(
            reopened
                .list_indexed_sessions(&list_request)
                .await
                .expect("empty library should list")
                .items
                .is_empty()
        );
        drop(second);
        reopened.pool().close().await;
    });
}

#[test]
fn indexes_three_sources_and_retains_last_known_good_data_across_refresh_shapes() {
    tauri::async_runtime::block_on(async {
        let tree = TempTree::new("lifecycle");
        tree.copy_fixture(
            "claude-current.jsonl",
            r"claude\project-a\claude-session-1.jsonl",
        );
        tree.copy_fixture("codex-current.jsonl", r"codex\rollout.jsonl");
        tree.copy_fixture(
            "copilot-cli-events.jsonl",
            r"copilot\session-state\copilot-session-1\events.jsonl",
        );
        tree.copy_fixture("copilot-cli-settings.json", r"copilot\settings.json");
        let repository = repository().await;
        let observed = Arc::new(Mutex::new(Vec::new()));
        let coordinator = coordinator(
            repository.clone(),
            Arc::new(tree.three_source_provider()),
            Arc::clone(&observed),
        );

        let initial = coordinator
            .refresh()
            .await
            .expect("initial scan should complete");
        assert_eq!(completed_counts(&initial), (1, 3, 3, 3, 0, 0, 0, 0));
        let unchanged = coordinator
            .refresh()
            .await
            .expect("unchanged scan should complete");
        assert_eq!(completed_counts(&unchanged), (2, 3, 3, 0, 3, 0, 0, 0));

        let mut codex = OpenOptions::new()
            .append(true)
            .open(tree.0.join(r"codex\rollout.jsonl"))
            .expect("Codex fixture should open for append");
        writeln!(
            codex,
            r#"{{"timestamp":"2026-08-24T08:00:08.000Z","ordinal":9,"type":"response_item","payload":{{"id":"ri-new","type":"message","role":"user","content":[{{"type":"input_text","text":"Continue"}}]}}}}"#
        )
        .expect("synthetic append should succeed");
        drop(codex);
        let appended = coordinator
            .refresh()
            .await
            .expect("append scan should complete");
        assert_eq!(completed_counts(&appended), (3, 3, 3, 1, 2, 0, 0, 0));

        let original_codex = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/codex-current.jsonl"),
        )
        .expect("Codex fixture should be readable");
        let truncated = original_codex
            .lines()
            .take(3)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(tree.0.join(r"codex\rollout.jsonl"), truncated)
            .expect("Codex fixture should be rewritten");
        let rewritten = coordinator
            .refresh()
            .await
            .expect("rewrite scan should complete");
        assert_eq!(completed_counts(&rewritten), (4, 3, 3, 1, 2, 0, 0, 0));

        fs::remove_file(tree.0.join(r"claude\project-a\claude-session-1.jsonl"))
            .expect("Claude fixture should be removed");
        fs::write(
            tree.0
                .join(r"copilot\session-state\copilot-session-1\events.jsonl"),
            b"{not-json}\n",
        )
        .expect("Copilot fixture should be corrupted synthetically");
        let partial = coordinator
            .refresh()
            .await
            .expect("partial scan should complete");
        assert_eq!(completed_counts(&partial), (5, 2, 2, 0, 1, 1, 0, 0));

        let presence: Vec<(String, i64)> =
            sqlx::query_as("SELECT source, source_present FROM sessions ORDER BY source")
                .fetch_all(repository.pool())
                .await
                .expect("source presence should be readable");
        assert_eq!(
            presence,
            [
                ("claude-code".to_owned(), 0),
                ("codex".to_owned(), 1),
                ("copilot-cli".to_owned(), 1),
            ]
        );
        let revisions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM session_revisions")
            .fetch_one(repository.pool())
            .await
            .expect("revision count should be readable");
        assert_eq!(revisions, 5);
        let diagnostic: String =
            sqlx::query_scalar("SELECT code FROM source_diagnostics WHERE scan_generation = 5")
                .fetch_one(repository.pool())
                .await
                .expect("safe candidate failure should be recorded");
        assert_eq!(diagnostic, "SOURCE_NORMALIZATION_FAILED");

        tree.copy_fixture(
            "claude-current.jsonl",
            r"claude\project-a\claude-session-1.jsonl",
        );
        tree.copy_fixture(
            "copilot-cli-events.jsonl",
            r"copilot\session-state\copilot-session-1\events.jsonl",
        );
        let reappeared = coordinator
            .refresh()
            .await
            .expect("reappearance scan should complete");
        assert_eq!(completed_counts(&reappeared), (6, 3, 3, 0, 3, 0, 0, 0));
        let all_present: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE source_present = 1")
                .fetch_one(repository.pool())
                .await
                .expect("presence count should be readable");
        assert_eq!(all_present, 3);

        let states = observed
            .lock()
            .expect("observer lock should remain available");
        assert!(
            states
                .iter()
                .any(|state| matches!(state, IndexRefreshStateV1::Running { .. }))
        );
        let serialized = serde_json::to_string(&*states).expect("progress should serialize");
        assert!(!serialized.contains(&tree.0.to_string_lossy().to_string()));
        assert!(!serialized.contains("Continue"));
    });
}

#[test]
fn distinct_codex_rollout_files_with_an_inherited_metadata_id_remain_distinct_sessions() {
    tauri::async_runtime::block_on(async {
        let tree = TempTree::new("codex-inherited-session-id");
        let transcript = concat!(
            r#"{"timestamp":"2026-09-02T08:00:00.000Z","ordinal":0,"type":"session_meta","payload":{"session_id":"11111111-1111-4111-8111-111111111111","timestamp":"2026-09-02T08:00:00.000Z"}}"#,
            "\n",
            r#"{"timestamp":"2026-09-02T08:00:01.000Z","ordinal":1,"type":"response_item","payload":{"id":"message-1","type":"message","role":"user","content":[{"type":"input_text","text":"Synthetic question"}]}}"#,
            "\n",
            r#"{"timestamp":"2026-09-02T08:00:02.000Z","ordinal":2,"type":"response_item","payload":{"id":"message-2","type":"message","role":"assistant","content":[{"type":"output_text","text":"Synthetic answer"}]}}"#,
            "\n"
        );
        for session_id in [
            "11111111-1111-4111-8111-111111111111",
            "22222222-2222-4222-8222-222222222222",
        ] {
            let path = tree.0.join(format!(
                r"codex\2026\09\02\rollout-2026-09-02T08-00-00-{session_id}.jsonl"
            ));
            fs::create_dir_all(path.parent().expect("fixture should have a parent"))
                .expect("fixture directory should be created");
            fs::write(path, transcript).expect("synthetic Codex session should be written");
        }

        let repository = repository().await;
        let coordinator = coordinator(
            repository.clone(),
            Arc::new(tree.three_source_provider()),
            Arc::new(Mutex::new(Vec::new())),
        );

        let state = coordinator
            .refresh()
            .await
            .expect("both Codex rollout files should index");
        assert_eq!(completed_counts(&state), (1, 2, 2, 2, 0, 0, 0, 2));

        let page = repository
            .list_indexed_sessions(&IndexedSessionListRequestV1 {
                schema_version: 1,
                source: Some(IndexedSessionSourceV1::Codex),
                query: String::new(),
                sort_order: crate::indexed_library::IndexedSessionSortOrderV1::Newest,
                cursor: None,
                page_size: 10,
            })
            .await
            .expect("indexed Codex sessions should list");
        assert_eq!(page.items.len(), 2);
    });
}

#[derive(Debug, Default)]
struct BlockingProvider {
    calls: AtomicUsize,
    state: Mutex<(bool, bool)>,
    changed: Condvar,
}

impl BlockingProvider {
    fn wait_until_entered(&self) {
        let (lock, changed) = (&self.state, &self.changed);
        let state = lock.lock().expect("provider lock should remain available");
        let (state, timeout) = changed
            .wait_timeout_while(state, Duration::from_secs(5), |state| !state.0)
            .expect("provider wait should succeed");
        assert!(
            state.0 && !timeout.timed_out(),
            "scan did not enter provider"
        );
    }

    fn release(&self) {
        let mut state = self
            .state
            .lock()
            .expect("provider lock should remain available");
        state.1 = true;
        self.changed.notify_all();
    }
}

impl RootProvider for BlockingProvider {
    fn roots(&self) -> Vec<RootSpec> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let mut state = self
            .state
            .lock()
            .expect("provider lock should remain available");
        state.0 = true;
        self.changed.notify_all();
        while !state.1 {
            state = self
                .changed
                .wait(state)
                .expect("provider wait should succeed");
        }
        Vec::new()
    }
}

#[test]
fn concurrent_refresh_callers_join_one_scan_generation() {
    tauri::async_runtime::block_on(async {
        let repository = repository().await;
        let provider = Arc::new(BlockingProvider::default());
        let coordinator = coordinator(
            repository,
            provider.clone(),
            Arc::new(Mutex::new(Vec::new())),
        );
        let release_provider = provider.clone();
        let wait_for_join = coordinator.clone();
        let releaser = std::thread::spawn(move || {
            release_provider.wait_until_entered();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while wait_for_join.joined_refreshes_for_test() == 0 {
                assert!(std::time::Instant::now() < deadline, "refresh did not join");
                std::thread::yield_now();
            }
            release_provider.release();
        });

        let first = coordinator.refresh();
        let second = coordinator.refresh();
        let (first, second) = tokio::join!(first, second);
        releaser.join().expect("releaser should finish");

        assert_eq!(first, second);
        let terminal = first.expect("joined scan should complete");
        assert_eq!(completed_counts(&terminal).0, 1);
        assert_eq!(coordinator.current_state().await, terminal);
        assert_eq!(provider.calls.load(Ordering::Relaxed), 1);
    });
}

#[test]
fn reset_is_rejected_while_a_refresh_is_active() {
    tauri::async_runtime::block_on(async {
        let repository = repository().await;
        let provider = Arc::new(BlockingProvider::default());
        let coordinator = coordinator(
            repository,
            provider.clone(),
            Arc::new(Mutex::new(Vec::new())),
        );
        let refreshing = coordinator.clone();
        let refresh = tauri::async_runtime::spawn(async move { refreshing.refresh().await });
        provider.wait_until_entered();

        assert_eq!(
            coordinator.reset_local_database().await,
            Err(IndexError::RefreshInProgress)
        );

        provider.release();
        refresh
            .await
            .expect("refresh task should complete")
            .expect("refresh should succeed");
    });
}

#[test]
fn reset_clears_index_state_without_mutating_sources_and_allows_reindexing() {
    tauri::async_runtime::block_on(async {
        let tree = TempTree::new("reset");
        tree.copy_fixture("codex-current.jsonl", r"codex\rollout.jsonl");
        let source_path = tree.0.join(r"codex\rollout.jsonl");
        let source_before = fs::read(&source_path).expect("source fixture should be readable");
        let repository = repository().await;
        let coordinator = coordinator(
            repository.clone(),
            Arc::new(tree.three_source_provider()),
            Arc::new(Mutex::new(Vec::new())),
        );
        coordinator
            .refresh()
            .await
            .expect("initial indexing should succeed");

        let result = coordinator
            .reset_local_database()
            .await
            .expect("reset should succeed while idle");
        assert_eq!(result.schema_version, 1);
        assert_eq!(result.reset_at_ms, 1_030);
        assert_eq!(
            coordinator.current_state().await,
            IndexRefreshStateV1::Idle {
                schema_version: 1,
                last_completed_at_ms: None,
            }
        );
        let session_count: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
            .fetch_one(repository.pool())
            .await
            .expect("session count should be readable");
        assert_eq!(session_count, 0);
        assert_eq!(
            fs::read(&source_path).expect("source fixture should remain readable"),
            source_before
        );

        let refreshed = coordinator
            .refresh()
            .await
            .expect("manual refresh should rebuild the library");
        assert_eq!(completed_counts(&refreshed), (1, 1, 1, 1, 0, 0, 0, 2));
    });
}

#[test]
fn indexed_data_stays_readable_and_a_cancelled_caller_does_not_cancel_the_scan() {
    tauri::async_runtime::block_on(async {
        let tree = TempTree::new("read-during-refresh");
        tree.copy_fixture("codex-current.jsonl", r"codex\rollout.jsonl");
        let repository = repository().await;
        coordinator(
            repository.clone(),
            Arc::new(tree.three_source_provider()),
            Arc::new(Mutex::new(Vec::new())),
        )
        .refresh()
        .await
        .expect("initial indexing should complete");

        let provider = Arc::new(BlockingProvider::default());
        let coordinator = coordinator(
            repository.clone(),
            provider.clone(),
            Arc::new(Mutex::new(Vec::new())),
        );
        let first_coordinator = coordinator.clone();
        let first = tauri::async_runtime::spawn(async move { first_coordinator.refresh().await });
        provider.wait_until_entered();
        let joined_coordinator = coordinator.clone();
        let joined = tauri::async_runtime::spawn(async move { joined_coordinator.refresh().await });
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while coordinator.joined_refreshes_for_test() == 0 {
            assert!(std::time::Instant::now() < deadline, "refresh did not join");
            std::thread::yield_now();
        }
        first.abort();

        let retained_title: String =
            sqlx::query_scalar("SELECT title FROM sessions WHERE source = 'codex'")
                .fetch_one(repository.pool())
                .await
                .expect("old indexed data should remain readable during refresh");
        assert!(!retained_title.is_empty());
        provider.release();

        let completed = joined
            .await
            .expect("joined caller task should remain healthy")
            .expect("shared scan should survive caller cancellation");
        assert_eq!(completed_counts(&completed).0, 2);
        assert_eq!(provider.calls.load(Ordering::Relaxed), 1);
    });
}

#[test]
fn suppressed_vendor_identity_is_not_reimported() {
    tauri::async_runtime::block_on(async {
        let tree = TempTree::new("suppression");
        tree.copy_fixture("codex-current.jsonl", r"codex\rollout.jsonl");
        let repository = repository().await;
        let coordinator = coordinator(
            repository.clone(),
            Arc::new(tree.three_source_provider()),
            Arc::new(Mutex::new(Vec::new())),
        );

        let initial = coordinator.refresh().await.expect("scan should complete");
        assert_eq!(completed_counts(&initial), (1, 1, 1, 1, 0, 0, 0, 2));
        let (session_id, vendor_session_id): (String, String) =
            sqlx::query_as("SELECT id, vendor_session_id FROM sessions WHERE source = 'codex'")
                .fetch_one(repository.pool())
                .await
                .expect("indexed session identity should be readable");
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(session_id)
            .execute(repository.pool())
            .await
            .expect("synthetic library deletion should succeed");
        sqlx::query(
            "INSERT INTO suppressed_sources (
                id, source, source_identity_hash, vendor_session_id,
                source_deleted, suppressed_at_ms
             ) VALUES ('suppression_1', 'codex', ?, ?, 0, 1100)",
        )
        .bind([0xA5_u8; 32].as_slice())
        .bind(vendor_session_id)
        .execute(repository.pool())
        .await
        .expect("synthetic tombstone should insert");

        let suppressed = coordinator
            .refresh()
            .await
            .expect("suppressed scan should complete");
        assert_eq!(completed_counts(&suppressed), (2, 1, 1, 0, 0, 0, 1, 2));
        let session_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
            .fetch_one(repository.pool())
            .await
            .expect("session count should be readable");
        assert_eq!(session_count, 0);
    });
}

struct AvailableThenDeniedProvider {
    available: Vec<RootSpec>,
    calls: AtomicUsize,
}

impl RootProvider for AvailableThenDeniedProvider {
    fn roots(&self) -> Vec<RootSpec> {
        if self.calls.fetch_add(1, Ordering::Relaxed) == 0 {
            self.available.clone()
        } else {
            vec![RootSpec::Unavailable {
                source: DiscoverySource::Codex,
                collection: "active",
                status: AvailabilityStatus::Denied,
            }]
        }
    }
}

#[test]
fn denied_collection_retains_source_presence_and_records_only_a_safe_code() {
    tauri::async_runtime::block_on(async {
        let tree = TempTree::new("denied");
        tree.copy_fixture("codex-current.jsonl", r"codex\rollout.jsonl");
        let available = tree.three_source_provider().roots();
        let provider = Arc::new(AvailableThenDeniedProvider {
            available,
            calls: AtomicUsize::new(0),
        });
        let repository = repository().await;
        let coordinator = coordinator(
            repository.clone(),
            provider,
            Arc::new(Mutex::new(Vec::new())),
        );

        coordinator.refresh().await.expect("scan should complete");
        let denied = coordinator
            .refresh()
            .await
            .expect("denied scan should complete safely");
        assert_eq!(completed_counts(&denied), (2, 0, 0, 0, 0, 0, 0, 1));
        let source_present: i64 =
            sqlx::query_scalar("SELECT source_present FROM sessions WHERE source = 'codex'")
                .fetch_one(repository.pool())
                .await
                .expect("source presence should remain readable");
        assert_eq!(source_present, 1);
        let code: String = sqlx::query_scalar(
            "SELECT code FROM source_diagnostics WHERE scan_generation = 2 AND source = 'codex'",
        )
        .fetch_one(repository.pool())
        .await
        .expect("safe root diagnostic should be recorded");
        assert_eq!(code, "SOURCE_ROOT_DENIED");
    });
}

#[test]
fn closed_source_and_availability_mappings_do_not_invent_support() {
    let statuses = [
        (AvailabilityStatus::Missing, "SOURCE_ROOT_MISSING"),
        (AvailabilityStatus::Disabled, "SOURCE_ROOT_DISABLED"),
        (AvailabilityStatus::Denied, "SOURCE_ROOT_DENIED"),
        (AvailabilityStatus::Locked, "SOURCE_ROOT_LOCKED"),
        (AvailabilityStatus::Unsupported, "SOURCE_ROOT_UNSUPPORTED"),
        (AvailabilityStatus::RemoteOnly, "SOURCE_ROOT_REMOTE_ONLY"),
    ];
    for (status, expected) in statuses {
        assert_eq!(
            availability_diagnostic_code(&AvailabilityDiagnostic {
                source: DiscoverySource::Codex,
                collection: "active".to_owned(),
                status,
            }),
            expected
        );
    }

    assert_eq!(
        indexed_source(DiscoverySource::ClaudeCode),
        Some(IndexedSessionSourceV1::ClaudeCode)
    );
    assert_eq!(
        indexed_source(DiscoverySource::Codex),
        Some(IndexedSessionSourceV1::Codex)
    );
    assert_eq!(
        indexed_source(DiscoverySource::CopilotCli),
        Some(IndexedSessionSourceV1::CopilotCli)
    );
    assert_eq!(
        indexed_source(DiscoverySource::VscodeCopilot),
        Some(IndexedSessionSourceV1::VscodeCopilot)
    );
    assert_eq!(indexed_source(DiscoverySource::JetBrains), None);
    assert_eq!(
        indexed_model_source(&SessionSource::ClaudeCode),
        Some(IndexedSessionSourceV1::ClaudeCode)
    );
    assert_eq!(
        indexed_model_source(&SessionSource::Codex),
        Some(IndexedSessionSourceV1::Codex)
    );
    assert_eq!(
        indexed_model_source(&SessionSource::CopilotCli),
        Some(IndexedSessionSourceV1::CopilotCli)
    );
    assert_eq!(
        indexed_model_source(&SessionSource::VscodeCopilot),
        Some(IndexedSessionSourceV1::VscodeCopilot)
    );
}

#[test]
fn rich_normalized_entries_map_to_persistence_without_losing_detail() {
    let normalized: NormalizedSessionV2 = serde_json::from_str(include_str!(
        "../../../tests/fixtures/normalized-session-v2.json"
    ))
    .expect("rich normalized fixture should be valid");
    let availability = persisted_content_availability(&normalized.content_availability);
    let persisted: Vec<_> = normalized
        .entries
        .into_iter()
        .enumerate()
        .map(|(ordinal, entry)| persisted_entry(ordinal as u64, entry))
        .collect();

    assert_eq!(
        availability.reasoning,
        crate::indexed_library::ReasoningAvailability::Available
    );
    assert_eq!(
        availability.tool_details,
        crate::indexed_library::ContentAvailability::Partial
    );
    assert!(matches!(persisted[2], PersistedEntry::Reasoning { .. }));
    assert!(matches!(
        &persisted[3],
        PersistedEntry::ToolCall {
            detail: PersistedToolDetail::Available {
                arguments: Some(arguments),
                result: Some(result),
            },
            ..
        } if arguments.contains("fixture.txt") && result == "synthetic result"
    ));
    assert!(matches!(
        persisted[4],
        PersistedEntry::ToolCall {
            detail: PersistedToolDetail::Unavailable,
            ..
        }
    ));
}

#[test]
fn coordinator_registration_starts_once_and_rejects_state_conflicts() {
    tauri::async_runtime::block_on(async {
        let successful_repository = repository().await;
        let successful = coordinator(
            successful_repository.clone(),
            Arc::new(BlockingProvider {
                calls: AtomicUsize::new(0),
                state: Mutex::new((false, true)),
                changed: Condvar::new(),
            }),
            Arc::new(Mutex::new(Vec::new())),
        );
        let retained = successful.clone();
        let registrations = AtomicUsize::new(0);
        register_and_start(successful, |_| {
            registrations.fetch_add(1, Ordering::Relaxed);
            true
        })
        .expect("coordinator registration should succeed");
        retained
            .refresh()
            .await
            .expect("startup refresh should complete");
        assert_eq!(registrations.load(Ordering::Relaxed), 1);
        let scans: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM scan_runs")
            .fetch_one(successful_repository.pool())
            .await
            .expect("startup scan should be persisted");
        assert!(scans >= 1);

        let rejected_repository = repository().await;
        let rejected = coordinator(
            rejected_repository.clone(),
            Arc::new(BlockingProvider::default()),
            Arc::new(Mutex::new(Vec::new())),
        );
        assert_eq!(
            register_and_start(rejected, |_| false),
            Err(IndexError::StateConflict)
        );
        let scans: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM scan_runs")
            .fetch_one(rejected_repository.pool())
            .await
            .expect("rejected registration should remain readable");
        assert_eq!(scans, 0);
    });
}

#[test]
fn closed_refresh_channel_returns_a_safe_runtime_error() {
    tauri::async_runtime::block_on(async {
        let (sender, receiver) = tokio::sync::watch::channel(None);
        drop(sender);

        assert_eq!(
            receive_refresh_result(receiver).await,
            Err(IndexError::Runtime)
        );
    });
}

#[test]
fn vscode_copilot_sessions_flow_through_the_durable_library() {
    tauri::async_runtime::block_on(async {
        let tree = TempTree::new("vscode-integration");
        let vscode_path = tree
            .0
            .join(r"vscode\workspace-a\chatSessions\vscode-session-1.jsonl");
        tree.copy_fixture(
            "vscode-copilot-current.jsonl",
            r"vscode\workspace-a\chatSessions\vscode-session-1.jsonl",
        );
        tree.copy_fixture(
            "vscode-copilot-legacy.json",
            r"vscode\workspace-b\chatSessions\vscode-legacy-1.json",
        );
        let mut provider = FixtureRootProvider::new(&tree.0).expect("fixture root should be valid");
        provider
            .add_root(
                DiscoverySource::VscodeCopilot,
                "workspaces",
                Path::new("vscode"),
                CandidateMatcher::VsCodeWorkspace,
            )
            .expect("VS Code source fixture should be accepted");
        let repository = repository().await;
        let coordinator = coordinator(
            repository.clone(),
            Arc::new(provider),
            Arc::new(Mutex::new(Vec::new())),
        );

        let state = coordinator
            .refresh()
            .await
            .expect("VS Code source should index");
        assert_eq!(completed_counts(&state), (1, 2, 2, 2, 0, 0, 0, 0));
        let request = IndexedSessionListRequestV1 {
            schema_version: 1,
            source: Some(IndexedSessionSourceV1::VscodeCopilot),
            query: "synthetic workspace".to_owned(),
            sort_order: crate::indexed_library::IndexedSessionSortOrderV1::Newest,
            cursor: None,
            page_size: 10,
        };
        let page = repository
            .list_indexed_sessions(&request)
            .await
            .expect("VS Code session should be searchable");
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].source, IndexedSessionSourceV1::VscodeCopilot);
        assert_eq!(page.items[0].title, "Synthetic VS Code chat");
        let detail = repository
            .get_indexed_session(&page.items[0].session_id, None)
            .await
            .expect("VS Code session should open from the durable library");
        assert_eq!(detail.entry_page.entries.len(), 8);
        let first_entry_key = match &detail.entry_page.entries[0].entry {
            crate::indexed_library::SessionEntryV1::User { entry_key, .. }
            | crate::indexed_library::SessionEntryV1::Assistant { entry_key, .. }
            | crate::indexed_library::SessionEntryV1::Reasoning { entry_key, .. }
            | crate::indexed_library::SessionEntryV1::ToolCall { entry_key, .. }
            | crate::indexed_library::SessionEntryV1::FileChange { entry_key, .. }
            | crate::indexed_library::SessionEntryV1::Unknown { entry_key, .. } => {
                entry_key.clone()
            }
        };
        repository
            .set_entry_selections(
                &SetEntrySelectionsRequestV1 {
                    schema_version: 1,
                    session_id: page.items[0].session_id.clone(),
                    revision_id: detail.revision.revision_id.clone(),
                    changes: vec![EntrySelectionChangeV1 {
                        entry_key: first_entry_key,
                        selected: false,
                    }],
                },
                2_000,
            )
            .await
            .expect("VS Code curation should persist");
        let mut source = OpenOptions::new()
            .append(true)
            .open(vscode_path)
            .expect("VS Code fixture should open for append");
        writeln!(
            source,
            r#"{{"kind":2,"k":["requests"],"v":[{{"requestId":"request-3","timestamp":1788264005000,"message":"Continue","variableData":{{"variables":[]}},"response":[{{"value":"Appended answer."}}],"responseTimestamp":1788264006000}}]}}"#
        )
        .expect("synthetic VS Code mutation should append");
        drop(source);
        let refreshed = coordinator
            .refresh()
            .await
            .expect("appended VS Code source should refresh");
        assert_eq!(completed_counts(&refreshed), (2, 2, 2, 1, 1, 0, 0, 0));
        let updated = repository
            .get_indexed_session(&page.items[0].session_id, None)
            .await
            .expect("updated VS Code session should open");
        assert_eq!(updated.entry_page.entries.len(), 10);
        assert!(!updated.entry_page.entries[0].selected);
        assert!(updated.entry_page.entries[9].selected);
        let plan = crate::commands::runtime::build_presentation_plan(
            &repository,
            &page.items[0].session_id,
            &updated.revision.revision_id,
            "vscode_plan".to_owned(),
            2_000,
        )
        .await
        .expect("VS Code session should project through presentation and export");
        assert_eq!(plan.entries.len(), 9);

        let legacy_page = repository
            .list_indexed_sessions(&IndexedSessionListRequestV1 {
                query: "legacy question".to_owned(),
                ..request
            })
            .await
            .expect("legacy VS Code text should be searchable");
        assert_eq!(legacy_page.items.len(), 1);
        assert_eq!(legacy_page.items[0].title, "Legacy synthetic chat");
    });
}

#[test]
fn progress_flushes_at_the_bounded_batch_boundary() {
    tauri::async_runtime::block_on(async {
        let tree = TempTree::new("progress-batch");
        tree.copy_codex_fixtures(64);
        let repository = repository().await;
        let coordinator = coordinator(
            repository,
            Arc::new(tree.three_source_provider()),
            Arc::new(Mutex::new(Vec::new())),
        );

        let state = coordinator
            .refresh()
            .await
            .expect("bounded progress scan should complete");
        assert_eq!(completed_counts(&state), (1, 64, 64, 1, 63, 0, 0, 2));
    });
}

#[test]
fn progress_storage_failures_produce_a_safe_terminal_state() {
    tauri::async_runtime::block_on(async {
        let tree = TempTree::new("progress-failure");
        tree.copy_codex_fixtures(64);
        let repository = repository().await;
        sqlx::query(
            "CREATE TEMP TRIGGER reject_batched_progress
             BEFORE UPDATE OF processed_count ON scan_runs
             WHEN NEW.processed_count >= 64
             BEGIN SELECT RAISE(ABORT, 'synthetic'); END",
        )
        .execute(repository.pool())
        .await
        .expect("synthetic progress fault should install");
        let coordinator = coordinator(
            repository,
            Arc::new(tree.three_source_provider()),
            Arc::new(Mutex::new(Vec::new())),
        );

        let state = coordinator
            .refresh()
            .await
            .expect("storage fault should remain a typed refresh state");
        assert!(matches!(
            state,
            IndexRefreshStateV1::Failed { error_code, .. }
                if error_code == "INDEX_STORAGE_FAILED"
        ));
    });
}

#[test]
fn initial_progress_and_failed_observation_faults_retain_safe_failure_semantics() {
    tauri::async_runtime::block_on(async {
        let tree = TempTree::new("storage-failures");
        tree.copy_fixture("codex-current.jsonl", r"codex\rollout.jsonl");

        let progress_repository = repository().await;
        sqlx::query(
            "CREATE TEMP TRIGGER reject_initial_progress
             BEFORE UPDATE OF discovered_count ON scan_runs
             WHEN NEW.discovered_count > 0
             BEGIN SELECT RAISE(ABORT, 'synthetic'); END",
        )
        .execute(progress_repository.pool())
        .await
        .expect("synthetic initial-progress fault should install");
        let progress_coordinator = coordinator(
            progress_repository,
            Arc::new(tree.three_source_provider()),
            Arc::new(Mutex::new(Vec::new())),
        );
        let progress_state = progress_coordinator
            .refresh()
            .await
            .expect("initial progress fault should remain typed");
        assert!(matches!(
            progress_state,
            IndexRefreshStateV1::Failed { error_code, .. }
                if error_code == "INDEX_STORAGE_FAILED"
        ));

        let observation_repository = repository().await;
        let observation_coordinator = coordinator(
            observation_repository.clone(),
            Arc::new(tree.three_source_provider()),
            Arc::new(Mutex::new(Vec::new())),
        );
        observation_coordinator
            .refresh()
            .await
            .expect("initial indexing should complete");
        fs::write(tree.0.join(r"codex\rollout.jsonl"), b"{not-json}\n")
            .expect("synthetic source corruption should succeed");
        sqlx::query(
            "CREATE TEMP TRIGGER reject_failed_observation
             BEFORE UPDATE OF last_seen_generation ON source_locations
             BEGIN SELECT RAISE(ABORT, 'synthetic'); END",
        )
        .execute(observation_repository.pool())
        .await
        .expect("synthetic observation fault should install");
        let observation_state = observation_coordinator
            .refresh()
            .await
            .expect("observation fault should remain typed");
        assert!(matches!(
            observation_state,
            IndexRefreshStateV1::Failed { error_code, .. }
                if error_code == "INDEX_STORAGE_FAILED"
        ));
        let retained: (i64, i64) = sqlx::query_as(
            "SELECT source_present,
                    (SELECT COUNT(*) FROM session_revisions)
             FROM sessions WHERE source = 'codex'",
        )
        .fetch_one(observation_repository.pool())
        .await
        .expect("last-known-good state should remain readable");
        assert_eq!(retained, (1, 1));
    });
}
