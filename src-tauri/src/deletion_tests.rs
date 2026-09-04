use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};
use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

use super::{
    DeletionClock, DeletionError, DeletionIdGenerator, DeletionManifest, PendingDeletion,
    SessionDeletionService, TargetIdentity, execute_deletion_plan, remember_pending_deletion,
    resolve_deletion_plan,
};
use crate::database::repository::{
    IndexedSessionRepository, PersistSessionRevision, PersistedEntry, PersistedRevision,
    PersistedSession, PersistedSourceLocation,
};
use crate::database::{connect, migrator};
use crate::discovery::catalog::DiscoverySource;
use crate::discovery::roots::{CandidateMatcher, FixtureRootProvider};
use crate::indexed_library::{
    ContentAvailability, ContentAvailabilityV1, IndexedSessionSourceV1, ReasoningAvailability,
    SuppressedSourceListRequestV1,
};
use crate::io::LocalRoot;

static DELETION_TEST_NONCE: AtomicU64 = AtomicU64::new(0);

struct TempTree(PathBuf);

impl TempTree {
    fn new(label: &str) -> Self {
        let nonce = DELETION_TEST_NONCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ai-session-replay-deletion-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary deletion tree should be created");
        Self(path)
    }

    fn write(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("fixture parent should be created");
        }
        fs::write(&path, bytes).expect("fixture should be written");
        path
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct FixedClock(AtomicU64);

impl FixedClock {
    fn set(&self, value: u64) {
        self.0.store(value, Ordering::Relaxed);
    }
}

impl DeletionClock for FixedClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

struct SequenceIds(AtomicU64);

impl DeletionIdGenerator for SequenceIds {
    fn generate(&self, prefix: &str) -> Result<String, DeletionError> {
        Ok(format!(
            "{prefix}_{}",
            self.0.fetch_add(1, Ordering::Relaxed)
        ))
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
    IndexedSessionRepository::new(pool)
}

fn source_identity(
    source: &IndexedSessionSourceV1,
    root: (u64, [u8; 16]),
    file: (u64, [u8; 16]),
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"ai-session-replay-source-identity-v1\0");
    hasher.update(match source {
        IndexedSessionSourceV1::ClaudeCode => b"claude-code".as_slice(),
        IndexedSessionSourceV1::Codex => b"codex".as_slice(),
        IndexedSessionSourceV1::CopilotCli => b"copilot-cli".as_slice(),
        IndexedSessionSourceV1::VscodeCopilot => b"vscode-copilot".as_slice(),
    });
    hasher.update(root.0.to_le_bytes());
    hasher.update(root.1);
    hasher.update(file.0.to_le_bytes());
    hasher.update(file.1);
    hasher.finalize().into()
}

fn encode_identity(identity: (u64, [u8; 16])) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(24);
    bytes.extend_from_slice(&identity.0.to_le_bytes());
    bytes.extend_from_slice(&identity.1);
    bytes
}

async fn persist_fixture(
    repository: &IndexedSessionRepository,
    source: IndexedSessionSourceV1,
    collection: &str,
    read_root: &Path,
    primary: &Path,
) {
    let root = LocalRoot::new(read_root).expect("fixture read root should open");
    let opened = root
        .open_file(primary)
        .expect("fixture primary should open");
    let root_identity = root.identity();
    let file_identity = (opened.stamp.volume_serial, opened.stamp.file_id);
    repository
        .persist_revision(&PersistSessionRevision {
            session: PersistedSession {
                id: "session_1".to_owned(),
                source: source.clone(),
                vendor_session_id: "vendor-session-1".to_owned(),
                title: "Synthetic deletion session".to_owned(),
                created_at_ms: Some(900),
                source_version: None,
                collection: Some(collection.to_owned()),
                display_filename: Some("events.jsonl".to_owned()),
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
                canonical_path_utf16le: opened
                    .canonical_path
                    .as_os_str()
                    .encode_wide()
                    .flat_map(u16::to_le_bytes)
                    .collect(),
                source_identity_hash: source_identity(&source, root_identity, file_identity),
                root_identity: encode_identity(root_identity),
                file_identity: encode_identity(file_identity),
                file_size_bytes: opened.stamp.length,
                modified_at_ms: 950,
                last_seen_generation: None,
            },
            observed_at_ms: 1_000,
        })
        .await
        .expect("fixture should persist");
}

fn service(
    repository: IndexedSessionRepository,
    provider: FixtureRootProvider,
    clock: Arc<FixedClock>,
) -> SessionDeletionService {
    SessionDeletionService::with_dependencies(
        repository,
        Arc::new(provider),
        clock,
        Arc::new(SequenceIds(AtomicU64::new(1))),
    )
}

fn pending(session_id: &str, expires_at_ms: u64) -> PendingDeletion {
    PendingDeletion {
        session_id: session_id.to_owned(),
        source_record: crate::database::repository::SourceDeletionRecord {
            session_id: session_id.to_owned(),
            source: IndexedSessionSourceV1::Codex,
            vendor_session_id: format!("vendor-{session_id}"),
            collection: "active".to_owned(),
            canonical_path_utf16le: vec![1, 0],
            source_identity_hash: [2; 32],
            root_identity: vec![3; 24],
            file_identity: vec![4; 24],
        },
        manifest: DeletionManifest {
            files: vec![TargetIdentity {
                canonical_path: PathBuf::from(format!(r"C:\synthetic\{session_id}.jsonl")),
                volume_serial: 1,
                file_id: [2; 16],
            }],
            directories: Vec::new(),
        },
        expires_at_ms,
    }
}

#[test]
fn pending_confirmations_replace_per_session_and_remain_globally_bounded() {
    let mut plans = HashMap::new();
    remember_pending_deletion(
        &mut plans,
        100,
        "old_same_session".to_owned(),
        pending("session_same", 1_000),
    );
    remember_pending_deletion(
        &mut plans,
        100,
        "new_same_session".to_owned(),
        pending("session_same", 2_000),
    );
    assert!(!plans.contains_key("old_same_session"));

    for index in 0..128 {
        remember_pending_deletion(
            &mut plans,
            100,
            format!("token_{index:03}"),
            pending(&format!("session_{index:03}"), 3_000 + index as u64),
        );
    }
    assert_eq!(plans.len(), 128);
    assert!(!plans.contains_key("new_same_session"));
    assert!(plans.contains_key("token_127"));
}

#[test]
fn library_only_deletion_never_mutates_source_and_can_be_restored() {
    tauri::async_runtime::block_on(async {
        let tree = TempTree::new("library-only");
        let source = tree.write(r"codex\session.jsonl", b"source");
        let repository = migrated_repository(&tree.0.join("library.sqlite3")).await;
        persist_fixture(
            &repository,
            IndexedSessionSourceV1::Codex,
            "active",
            &tree.0.join("codex"),
            &source,
        )
        .await;
        let mut provider = FixtureRootProvider::new(&tree.0).expect("provider should initialize");
        provider
            .add_root(
                DiscoverySource::Codex,
                "active",
                Path::new("codex"),
                CandidateMatcher::Codex,
            )
            .expect("root should register");
        let deletion = service(
            repository.clone(),
            provider,
            Arc::new(FixedClock(AtomicU64::new(2_000))),
        );

        let tombstone = deletion
            .delete_library_only("session_1")
            .await
            .expect("library deletion should succeed");
        assert!(source.exists());
        assert!(!tombstone.source_deleted);
        deletion
            .restore(&tombstone.suppression_id)
            .await
            .expect("suppression should restore");
        assert!(
            deletion
                .list_suppressed(&SuppressedSourceListRequestV1 {
                    schema_version: 1,
                    cursor: None,
                    page_size: 20,
                })
                .await
                .expect("suppression page should read")
                .items
                .is_empty()
        );
    });
}

#[test]
fn source_deletion_requires_a_fresh_one_use_token_and_preserves_siblings() {
    tauri::async_runtime::block_on(async {
        let tree = TempTree::new("single-source");
        let source = tree.write(r"codex\session.jsonl", b"source");
        let sibling = tree.write(r"codex\unrelated.jsonl", b"keep");
        let repository = migrated_repository(&tree.0.join("library.sqlite3")).await;
        persist_fixture(
            &repository,
            IndexedSessionSourceV1::Codex,
            "active",
            &tree.0.join("codex"),
            &source,
        )
        .await;
        let mut provider = FixtureRootProvider::new(&tree.0).expect("provider should initialize");
        provider
            .add_root(
                DiscoverySource::Codex,
                "active",
                Path::new("codex"),
                CandidateMatcher::Codex,
            )
            .expect("root should register");
        let deletion = service(
            repository.clone(),
            provider,
            Arc::new(FixedClock(AtomicU64::new(2_000))),
        );

        let confirmation = deletion
            .prepare_source_deletion("session_1")
            .await
            .expect("source deletion should prepare");
        assert_eq!(confirmation.artifact_count, 1);
        assert_eq!(confirmation.expires_at_ms, 302_000);
        assert_eq!(
            deletion
                .delete_with_source("session_1", "forged_token")
                .await,
            Err(DeletionError::ConfirmationInvalid)
        );
        assert!(source.exists());

        let tombstone = deletion
            .delete_with_source("session_1", &confirmation.confirmation_token)
            .await
            .expect("confirmed source deletion should succeed");
        assert!(tombstone.source_deleted);
        assert!(!source.exists());
        assert!(sibling.exists());
        assert_eq!(
            deletion
                .delete_with_source("session_1", &confirmation.confirmation_token)
                .await,
            Err(DeletionError::ConfirmationInvalid)
        );
    });
}

#[test]
fn vscode_source_deletion_removes_only_the_selected_chat_artifact() {
    tauri::async_runtime::block_on(async {
        let tree = TempTree::new("vscode-source");
        let source = tree.write(
            r"vscode\workspace-a\chatSessions\vscode-session-1.jsonl",
            b"synthetic source",
        );
        let legacy_sibling = tree.write(
            r"vscode\workspace-a\chatSessions\vscode-session-1.json",
            b"legacy synthetic source",
        );
        let sibling = tree.write(
            r"vscode\workspace-a\chatSessions\other-session.jsonl",
            b"other session",
        );
        let repository = migrated_repository(&tree.0.join("library.sqlite3")).await;
        persist_fixture(
            &repository,
            IndexedSessionSourceV1::VscodeCopilot,
            "stable-workspaces",
            &tree.0.join("vscode"),
            &source,
        )
        .await;
        let mut provider = FixtureRootProvider::new(&tree.0).expect("provider should initialize");
        provider
            .add_root(
                DiscoverySource::VscodeCopilot,
                "stable-workspaces",
                Path::new("vscode"),
                CandidateMatcher::VsCodeWorkspace,
            )
            .expect("VS Code root should register");
        let deletion = service(
            repository,
            provider,
            Arc::new(FixedClock(AtomicU64::new(2_000))),
        );

        let confirmation = deletion
            .prepare_source_deletion("session_1")
            .await
            .expect("VS Code source deletion should prepare");
        assert_eq!(confirmation.artifact_count, 2);
        deletion
            .delete_with_source("session_1", &confirmation.confirmation_token)
            .await
            .expect("VS Code source deletion should succeed");

        assert!(!source.exists());
        assert!(!legacy_sibling.exists());
        assert!(sibling.exists());
    });
}

#[test]
fn stale_or_expired_source_plans_never_delete_files_or_library_rows() {
    tauri::async_runtime::block_on(async {
        let tree = TempTree::new("stale");
        let source = tree.write(r"codex\session.jsonl", b"source");
        let repository = migrated_repository(&tree.0.join("library.sqlite3")).await;
        persist_fixture(
            &repository,
            IndexedSessionSourceV1::Codex,
            "active",
            &tree.0.join("codex"),
            &source,
        )
        .await;
        let mut provider = FixtureRootProvider::new(&tree.0).expect("provider should initialize");
        provider
            .add_root(
                DiscoverySource::Codex,
                "active",
                Path::new("codex"),
                CandidateMatcher::Codex,
            )
            .expect("root should register");
        let clock = Arc::new(FixedClock(AtomicU64::new(2_000)));
        let deletion = service(repository.clone(), provider, clock.clone());

        let expired = deletion
            .prepare_source_deletion("session_1")
            .await
            .expect("source deletion should prepare");
        clock.set(expired.expires_at_ms + 1);
        assert_eq!(
            deletion
                .delete_with_source("session_1", &expired.confirmation_token)
                .await,
            Err(DeletionError::ConfirmationExpired)
        );
        assert!(source.exists());

        clock.set(3_000);
        let stale = deletion
            .prepare_source_deletion("session_1")
            .await
            .expect("replacement deletion should prepare");
        fs::remove_file(&source).expect("old source should be removed");
        fs::write(&source, b"replacement").expect("replacement source should be written");
        assert_eq!(
            deletion
                .delete_with_source("session_1", &stale.confirmation_token)
                .await,
            Err(DeletionError::SourceStateChanged)
        );
        assert!(source.exists());
        repository
            .get_indexed_session("session_1", None)
            .await
            .expect("indexed session should remain");
    });
}

#[test]
fn copilot_deletion_removes_only_the_bounded_session_directory() {
    tauri::async_runtime::block_on(async {
        let tree = TempTree::new("copilot-bundle");
        let source = tree.write(r"copilot\session-state\session-1\events.jsonl", b"events");
        tree.write(r"copilot\session-state\session-1\plans\plan.md", b"plan");
        tree.write(
            r"copilot\session-state\session-1\checkpoints\one.json",
            b"checkpoint",
        );
        let sibling = tree.write(
            r"copilot\session-state\session-2\events.jsonl",
            b"other session",
        );
        let settings = tree.write(r"copilot\settings.json", b"settings");
        let repository = migrated_repository(&tree.0.join("library.sqlite3")).await;
        persist_fixture(
            &repository,
            IndexedSessionSourceV1::CopilotCli,
            "sessions",
            &tree.0.join("copilot"),
            &source,
        )
        .await;
        let mut provider = FixtureRootProvider::new(&tree.0).expect("provider should initialize");
        provider
            .add_root_with_read_scope(
                DiscoverySource::CopilotCli,
                "sessions",
                Path::new(r"copilot\session-state"),
                Path::new("copilot"),
                CandidateMatcher::CopilotCli,
            )
            .expect("root should register");
        let deletion = service(
            repository,
            provider,
            Arc::new(FixedClock(AtomicU64::new(2_000))),
        );

        let confirmation = deletion
            .prepare_source_deletion("session_1")
            .await
            .expect("bundle deletion should prepare");
        assert_eq!(confirmation.artifact_count, 3);
        deletion
            .delete_with_source("session_1", &confirmation.confirmation_token)
            .await
            .expect("bundle deletion should succeed");

        assert!(!source.parent().expect("session directory").exists());
        assert!(sibling.exists());
        assert!(settings.exists());
    });
}

#[test]
fn partial_bundle_failure_stops_before_library_mutation() {
    tauri::async_runtime::block_on(async {
        let tree = TempTree::new("partial-bundle");
        let source = tree.write(r"copilot\session-state\session-1\events.jsonl", b"events");
        tree.write(r"copilot\session-state\session-1\plans\plan.md", b"plan");
        let repository = migrated_repository(&tree.0.join("library.sqlite3")).await;
        persist_fixture(
            &repository,
            IndexedSessionSourceV1::CopilotCli,
            "sessions",
            &tree.0.join("copilot"),
            &source,
        )
        .await;
        let mut provider = FixtureRootProvider::new(&tree.0).expect("provider should initialize");
        provider
            .add_root_with_read_scope(
                DiscoverySource::CopilotCli,
                "sessions",
                Path::new(r"copilot\session-state"),
                Path::new("copilot"),
                CandidateMatcher::CopilotCli,
            )
            .expect("root should register");
        let record = repository
            .source_deletion_record("session_1")
            .await
            .expect("source record should read");
        let plan = resolve_deletion_plan(&provider, &record)
            .expect("initial bundle should resolve safely");
        let late_artifact = tree.write(
            r"copilot\session-state\session-1\late-checkpoint.json",
            b"created after validation",
        );

        assert_eq!(
            execute_deletion_plan(plan),
            Err(DeletionError::SourceDeleteFailed)
        );
        assert!(!source.exists());
        assert!(late_artifact.exists());
        repository
            .get_indexed_session("session_1", None)
            .await
            .expect("partial source failure must not remove indexed content");
    });
}

#[test]
fn source_delete_failure_preserves_source_and_indexed_content() {
    tauri::async_runtime::block_on(async {
        let tree = TempTree::new("source-failure");
        let source = tree.write(r"codex\session.jsonl", b"source");
        let repository = migrated_repository(&tree.0.join("library.sqlite3")).await;
        persist_fixture(
            &repository,
            IndexedSessionSourceV1::Codex,
            "active",
            &tree.0.join("codex"),
            &source,
        )
        .await;
        let mut provider = FixtureRootProvider::new(&tree.0).expect("provider should initialize");
        provider
            .add_root(
                DiscoverySource::Codex,
                "active",
                Path::new("codex"),
                CandidateMatcher::Codex,
            )
            .expect("root should register");
        let deletion = service(
            repository.clone(),
            provider,
            Arc::new(FixedClock(AtomicU64::new(2_000))),
        );
        let confirmation = deletion
            .prepare_source_deletion("session_1")
            .await
            .expect("source deletion should prepare");
        let _lock = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&source)
            .expect("source fixture should be locked against deletion");

        assert_eq!(
            deletion
                .delete_with_source("session_1", &confirmation.confirmation_token)
                .await,
            Err(DeletionError::SourceUnavailable)
        );
        assert!(source.exists());
        repository
            .get_indexed_session("session_1", None)
            .await
            .expect("indexed content should remain");
    });
}

#[test]
fn post_delete_database_failure_preserves_indexed_content_as_missing() {
    tauri::async_runtime::block_on(async {
        let tree = TempTree::new("failures");
        let source = tree.write(r"codex\session.jsonl", b"source");
        let repository = migrated_repository(&tree.0.join("library.sqlite3")).await;
        persist_fixture(
            &repository,
            IndexedSessionSourceV1::Codex,
            "active",
            &tree.0.join("codex"),
            &source,
        )
        .await;
        let mut provider = FixtureRootProvider::new(&tree.0).expect("provider should initialize");
        provider
            .add_root(
                DiscoverySource::Codex,
                "active",
                Path::new("codex"),
                CandidateMatcher::Codex,
            )
            .expect("root should register");
        let deletion = service(
            repository.clone(),
            provider,
            Arc::new(FixedClock(AtomicU64::new(2_000))),
        );
        let confirmation = deletion
            .prepare_source_deletion("session_1")
            .await
            .expect("source deletion should prepare");
        sqlx::query(
            "CREATE TRIGGER reject_session_delete BEFORE DELETE ON sessions
             BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END",
        )
        .execute(repository.pool())
        .await
        .expect("failure trigger should install");

        assert_eq!(
            deletion
                .delete_with_source("session_1", &confirmation.confirmation_token)
                .await,
            Err(DeletionError::Storage)
        );
        assert!(!source.exists());
        let detail = repository
            .get_indexed_session("session_1", None)
            .await
            .expect("indexed content should remain");
        assert!(!detail.summary.source_present);
    });
}
