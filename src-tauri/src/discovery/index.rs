use std::collections::BTreeMap;
use std::os::windows::ffi::OsStrExt;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::{Mutex as AsyncMutex, watch};

use crate::database::repository::{
    IndexedSessionRepository, PersistRevisionOutcome, PersistSessionRevision, PersistedEntry,
    PersistedRevision, PersistedSession, PersistedSourceLocation, PersistedToolDetail,
    RepositoryError, ScanCompletionOutcome, ScanCounts, ScanDiagnostic,
    ScannedCollection as RepositoryScannedCollection,
};
use crate::indexed_library::{
    ContentAvailability, ContentAvailabilityV1, IndexRefreshStateV1, IndexedSessionSourceV1,
    IndexedToolStatus, ReasoningAvailability, ResetLocalDatabaseResultV1,
};
use crate::model::{
    NormalizedContentAvailabilityV2, NormalizedContentAvailabilityValueV2, NormalizedEntryV2,
    NormalizedReasoningAvailabilityV2, NormalizedSessionV2, NormalizedToolDetailV2,
    NormalizedToolStatusV2, SessionSource,
};

use super::catalog::{
    AvailabilityDiagnostic, AvailabilityStatus, CatalogIdGenerator, CatalogSnapshot,
    DiscoveryCatalog, DiscoveryLimits, DiscoverySource, PrivateCandidateMetadata,
    WindowsCatalogIdGenerator,
};
use super::load::{SessionLoadError, load_catalog_session_v2};
use super::roots::{ProductionRootProvider, RootProvider};

const SCHEMA_VERSION: u8 = 1;
const PROGRESS_FLUSH_INTERVAL: u64 = 64;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

type RefreshResult = Result<IndexRefreshStateV1, IndexError>;
pub(crate) type RefreshObserver = Arc<dyn Fn(IndexRefreshStateV1) + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum IndexError {
    #[error("INDEX_STORAGE_UNAVAILABLE")]
    Storage,
    #[error("INDEX_RUNTIME_UNAVAILABLE")]
    Runtime,
    #[error("INDEX_IDENTIFIER_UNAVAILABLE")]
    Identifier,
    #[error("INDEX_STATE_CONFLICT")]
    StateConflict,
    #[error("INDEX_REFRESH_IN_PROGRESS")]
    RefreshInProgress,
}

pub(crate) trait IndexIdGenerator: Send + Sync {
    fn generate(&self) -> Result<String, IndexError>;
}

pub(crate) trait IndexClock: Send + Sync {
    fn now_ms(&self) -> u64;
}

struct SystemIndexClock;

impl IndexClock for SystemIndexClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| u64::try_from(duration.as_millis()).ok())
            .unwrap_or(0)
            .min(MAX_SAFE_INTEGER)
    }
}

struct WindowsIndexIdGenerator {
    generator: WindowsCatalogIdGenerator,
    next_sequence: AtomicU64,
}

impl WindowsIndexIdGenerator {
    fn new() -> Result<Self, IndexError> {
        Ok(Self {
            generator: WindowsCatalogIdGenerator::new().map_err(|_| IndexError::Identifier)?,
            next_sequence: AtomicU64::new(0),
        })
    }
}

impl IndexIdGenerator for WindowsIndexIdGenerator {
    fn generate(&self) -> Result<String, IndexError> {
        let sequence = self
            .next_sequence
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| IndexError::Identifier)?;
        Ok(self.generator.generate(sequence).as_str().to_owned())
    }
}

struct CoordinationState {
    active: Option<watch::Receiver<Option<RefreshResult>>>,
    current: IndexRefreshStateV1,
}

struct IndexCoordinatorInner {
    repository: IndexedSessionRepository,
    provider: Arc<dyn RootProvider>,
    catalog: Arc<Mutex<DiscoveryCatalog>>,
    ids: Arc<dyn IndexIdGenerator>,
    clock: Arc<dyn IndexClock>,
    observer: RefreshObserver,
    coordination: AsyncMutex<CoordinationState>,
    joined_refreshes: AtomicUsize,
}

#[derive(Clone)]
pub(crate) struct IndexCoordinator {
    inner: Arc<IndexCoordinatorInner>,
}

impl IndexCoordinator {
    pub(crate) fn new(
        repository: IndexedSessionRepository,
        observer: RefreshObserver,
    ) -> Result<Self, IndexError> {
        Ok(Self::with_components(
            repository,
            Arc::new(ProductionRootProvider::from_environment()),
            DiscoveryCatalog::new(
                WindowsCatalogIdGenerator::new().map_err(|_| IndexError::Identifier)?,
            ),
            Arc::new(WindowsIndexIdGenerator::new()?),
            Arc::new(SystemIndexClock),
            observer,
        ))
    }

    #[cfg(test)]
    pub(super) fn with_dependencies(
        repository: IndexedSessionRepository,
        provider: Arc<dyn RootProvider>,
        catalog: DiscoveryCatalog,
        ids: Arc<dyn IndexIdGenerator>,
        clock: Arc<dyn IndexClock>,
        observer: RefreshObserver,
    ) -> Self {
        Self::with_components(repository, provider, catalog, ids, clock, observer)
    }

    fn with_components(
        repository: IndexedSessionRepository,
        provider: Arc<dyn RootProvider>,
        catalog: DiscoveryCatalog,
        ids: Arc<dyn IndexIdGenerator>,
        clock: Arc<dyn IndexClock>,
        observer: RefreshObserver,
    ) -> Self {
        Self {
            inner: Arc::new(IndexCoordinatorInner {
                repository,
                provider,
                catalog: Arc::new(Mutex::new(catalog)),
                ids,
                clock,
                observer,
                coordination: AsyncMutex::new(CoordinationState {
                    active: None,
                    current: IndexRefreshStateV1::Idle {
                        schema_version: SCHEMA_VERSION,
                        last_completed_at_ms: None,
                    },
                }),
                joined_refreshes: AtomicUsize::new(0),
            }),
        }
    }

    /// Concurrent callers clone one watch receiver and therefore observe the
    /// same terminal result instead of queueing duplicate scans.
    /// Source: https://docs.rs/tokio/1.53.1/tokio/sync/watch/index.html
    pub(crate) async fn refresh(&self) -> RefreshResult {
        let receiver = {
            let mut coordination = self.inner.coordination.lock().await;
            if let Some(active) = &coordination.active {
                self.inner.joined_refreshes.fetch_add(1, Ordering::Relaxed);
                active.clone()
            } else {
                let (sender, receiver) = watch::channel(None);
                coordination.active = Some(receiver.clone());
                let coordinator = self.clone();
                tauri::async_runtime::spawn(async move {
                    let result = coordinator.run_scan().await;
                    coordinator.inner.coordination.lock().await.active = None;
                    sender.send_replace(Some(result));
                });
                receiver
            }
        };

        receive_refresh_result(receiver).await
    }

    pub(crate) async fn current_state(&self) -> IndexRefreshStateV1 {
        self.inner.coordination.lock().await.current.clone()
    }

    pub(crate) async fn reset_local_database(
        &self,
    ) -> Result<ResetLocalDatabaseResultV1, IndexError> {
        let mut coordination = self.inner.coordination.lock().await;
        if coordination.active.is_some() {
            return Err(IndexError::RefreshInProgress);
        }
        self.inner
            .repository
            .reset_local_database()
            .await
            .map_err(|_| IndexError::Storage)?;
        coordination.current = IndexRefreshStateV1::Idle {
            schema_version: SCHEMA_VERSION,
            last_completed_at_ms: None,
        };
        Ok(ResetLocalDatabaseResultV1 {
            schema_version: SCHEMA_VERSION,
            reset_at_ms: self.inner.clock.now_ms(),
        })
    }

    #[cfg(test)]
    pub(super) fn joined_refreshes_for_test(&self) -> usize {
        self.inner.joined_refreshes.load(Ordering::Relaxed)
    }

    async fn run_scan(&self) -> RefreshResult {
        let started_at_ms = self.inner.clock.now_ms();
        let generation = self
            .inner
            .repository
            .begin_scan(started_at_ms)
            .await
            .map_err(|_| IndexError::Storage)?;
        let mut counts = ScanCounts::default();
        self.publish(running_state(generation, started_at_ms, counts))
            .await;

        let snapshot = match self.discover().await {
            Ok(snapshot) => snapshot,
            Err(error_code) => {
                return Ok(self
                    .fail_started_scan(generation, started_at_ms, counts, error_code)
                    .await);
            }
        };
        counts.discovered = snapshot.entries.len() as u64;
        counts.warnings = snapshot.diagnostics.len() as u64;
        if self
            .inner
            .repository
            .update_scan_progress(generation, counts)
            .await
            != Ok(ScanCompletionOutcome::Applied)
        {
            return Ok(self
                .fail_started_scan(generation, started_at_ms, counts, "INDEX_STORAGE_FAILED")
                .await);
        }
        self.publish(running_state(generation, started_at_ms, counts))
            .await;

        let mut diagnostics = DiagnosticAccumulator::default();
        for diagnostic in snapshot.diagnostics {
            diagnostics.record(diagnostic.source, availability_diagnostic_code(&diagnostic));
        }
        let scanned_collections = snapshot
            .scanned_collections
            .into_iter()
            .filter_map(|scope| {
                indexed_source(scope.source)
                    .map(|source| RepositoryScannedCollection::new(source, scope.collection))
            })
            .collect::<Vec<_>>();

        for entry in snapshot.entries {
            let outcome = self
                .process_candidate(
                    generation,
                    entry.id.as_str(),
                    entry.source,
                    entry.display.file_name,
                )
                .await;
            counts.processed += 1;
            match outcome {
                CandidateOutcome::Indexed => counts.indexed += 1,
                CandidateOutcome::Unchanged => counts.unchanged += 1,
                CandidateOutcome::Ignored => counts.skipped += 1,
                CandidateOutcome::Failed { source, code } => {
                    counts.failed += 1;
                    diagnostics.record(source, code);
                }
                CandidateOutcome::Fatal(code) => {
                    return Ok(self
                        .fail_started_scan(generation, started_at_ms, counts, code)
                        .await);
                }
            }
            self.publish(running_state(generation, started_at_ms, counts))
                .await;
            if counts.processed.is_multiple_of(PROGRESS_FLUSH_INTERVAL)
                && self
                    .inner
                    .repository
                    .update_scan_progress(generation, counts)
                    .await
                    != Ok(ScanCompletionOutcome::Applied)
            {
                return Ok(self
                    .fail_started_scan(generation, started_at_ms, counts, "INDEX_STORAGE_FAILED")
                    .await);
            }
        }

        let completed_at_ms = self.inner.clock.now_ms().max(started_at_ms);
        let diagnostics = diagnostics.into_repository_diagnostics();
        match self
            .inner
            .repository
            .complete_scan(
                generation,
                completed_at_ms,
                counts,
                &scanned_collections,
                &diagnostics,
            )
            .await
        {
            Ok(ScanCompletionOutcome::Applied) => {
                let state = completed_state(generation, started_at_ms, completed_at_ms, counts);
                self.publish(state.clone()).await;
                Ok(state)
            }
            Ok(ScanCompletionOutcome::Stale) | Err(_) => Ok(self
                .fail_started_scan(generation, started_at_ms, counts, "INDEX_STORAGE_FAILED")
                .await),
        }
    }

    async fn discover(&self) -> Result<CatalogSnapshot, &'static str> {
        let catalog = Arc::clone(&self.inner.catalog);
        let provider = Arc::clone(&self.inner.provider);
        tauri::async_runtime::spawn_blocking(move || {
            let mut catalog = catalog.lock().map_err(|_| "INDEX_CATALOG_FAILED")?;
            Ok(catalog.refresh(provider.as_ref(), DiscoveryLimits::default()))
        })
        .await
        .map_err(|_| "INDEX_RUNTIME_FAILED")?
    }

    async fn process_candidate(
        &self,
        generation: u64,
        catalog_id: &str,
        expected_source: DiscoverySource,
        display_filename: String,
    ) -> CandidateOutcome {
        let metadata = match self.private_metadata(catalog_id) {
            Ok(metadata) => metadata,
            Err(code) => {
                return CandidateOutcome::Failed {
                    source: expected_source,
                    code,
                };
            }
        };
        let Some(source) = indexed_source(metadata.source) else {
            return CandidateOutcome::Failed {
                source: metadata.source,
                code: "UNSUPPORTED_SESSION_SOURCE",
            };
        };
        let location = persisted_location(&metadata, generation);
        match self
            .inner
            .repository
            .is_source_suppressed(&source, &location.source_identity_hash, None)
            .await
        {
            Ok(true) => return CandidateOutcome::Ignored,
            Ok(false) => {}
            Err(_) => return CandidateOutcome::Fatal("INDEX_STORAGE_FAILED"),
        }

        let normalized = match self.load(catalog_id.to_owned()).await {
            Ok(normalized) => normalized,
            Err(error) => {
                if self
                    .inner
                    .repository
                    .observe_existing_source(
                        generation,
                        &source,
                        &location,
                        self.inner.clock.now_ms(),
                    )
                    .await
                    .is_err()
                {
                    return CandidateOutcome::Fatal("INDEX_STORAGE_FAILED");
                }
                return CandidateOutcome::Failed {
                    source: metadata.source,
                    code: error.diagnostic_code(),
                };
            }
        };
        match self
            .inner
            .repository
            .is_source_suppressed(
                &source,
                &location.source_identity_hash,
                Some(&normalized.id),
            )
            .await
        {
            Ok(true) => return CandidateOutcome::Ignored,
            Ok(false) => {}
            Err(_) => return CandidateOutcome::Fatal("INDEX_STORAGE_FAILED"),
        }
        let session_id = match self
            .inner
            .repository
            .find_session_id(&source, &normalized.id)
            .await
        {
            Ok(Some(id)) => id,
            Ok(None) => match self.inner.ids.generate() {
                Ok(id) => id,
                Err(_) => return CandidateOutcome::Fatal("INDEX_IDENTIFIER_FAILED"),
            },
            Err(_) => return CandidateOutcome::Fatal("INDEX_STORAGE_FAILED"),
        };
        let revision_id = match self.inner.ids.generate() {
            Ok(id) => id,
            Err(_) => return CandidateOutcome::Fatal("INDEX_IDENTIFIER_FAILED"),
        };
        let observed_at_ms = self.inner.clock.now_ms();
        let input = match persistence_input(
            normalized,
            &metadata,
            location,
            session_id,
            revision_id,
            display_filename,
            observed_at_ms,
        ) {
            Ok(input) => input,
            Err(_) => {
                return CandidateOutcome::Failed {
                    source: metadata.source,
                    code: "INDEX_CONTRACT_INVALID",
                };
            }
        };
        match self.inner.repository.persist_revision(&input).await {
            Ok(PersistRevisionOutcome::Inserted { .. }) => CandidateOutcome::Indexed,
            Ok(PersistRevisionOutcome::Unchanged { .. }) => CandidateOutcome::Unchanged,
            Ok(PersistRevisionOutcome::Suppressed) => CandidateOutcome::Ignored,
            Err(RepositoryError::WriteFailed | RepositoryError::ScanStateConflict) => {
                CandidateOutcome::Fatal("INDEX_STORAGE_FAILED")
            }
            Err(_) => CandidateOutcome::Failed {
                source: metadata.source,
                code: "INDEX_IDENTITY_CONFLICT",
            },
        }
    }

    fn private_metadata(&self, catalog_id: &str) -> Result<PrivateCandidateMetadata, &'static str> {
        self.inner
            .catalog
            .lock()
            .map_err(|_| "INDEX_CATALOG_FAILED")?
            .private_metadata(catalog_id)
            .ok_or("CATALOG_ENTRY_UNAVAILABLE")
    }

    async fn load(&self, catalog_id: String) -> Result<NormalizedSessionV2, SessionLoadError> {
        let catalog = Arc::clone(&self.inner.catalog);
        tauri::async_runtime::spawn_blocking(move || load_catalog_session_v2(&catalog_id, &catalog))
            .await
            .map_err(|_| SessionLoadError::CatalogStateUnavailable)?
    }

    async fn fail_started_scan(
        &self,
        generation: u64,
        started_at_ms: u64,
        counts: ScanCounts,
        error_code: &'static str,
    ) -> IndexRefreshStateV1 {
        let completed_at_ms = self.inner.clock.now_ms().max(started_at_ms);
        let _ = self
            .inner
            .repository
            .fail_scan(generation, completed_at_ms, error_code, counts)
            .await;
        let state = failed_state(
            generation,
            started_at_ms,
            completed_at_ms,
            counts,
            error_code,
        );
        self.publish(state.clone()).await;
        state
    }

    async fn publish(&self, state: IndexRefreshStateV1) {
        self.inner.coordination.lock().await.current = state.clone();
        (self.inner.observer)(state);
    }
}

pub(super) async fn receive_refresh_result(
    mut receiver: watch::Receiver<Option<RefreshResult>>,
) -> RefreshResult {
    loop {
        if let Some(result) = receiver.borrow_and_update().clone() {
            return result;
        }
        if receiver.changed().await.is_err() {
            return Err(IndexError::Runtime);
        }
    }
}

enum CandidateOutcome {
    Indexed,
    Unchanged,
    Ignored,
    Failed {
        source: DiscoverySource,
        code: &'static str,
    },
    Fatal(&'static str),
}

#[derive(Default)]
struct DiagnosticAccumulator(BTreeMap<(DiscoverySource, &'static str), u64>);

impl DiagnosticAccumulator {
    fn record(&mut self, source: DiscoverySource, code: &'static str) {
        let count = self.0.entry((source, code)).or_default();
        *count = count.saturating_add(1).min(1_000);
    }

    fn into_repository_diagnostics(self) -> Vec<ScanDiagnostic> {
        self.0
            .into_iter()
            .filter_map(|((source, code), count)| {
                indexed_source(source).map(|source| ScanDiagnostic::new(None, source, code, count))
            })
            .collect()
    }
}

fn persistence_input(
    normalized: NormalizedSessionV2,
    metadata: &PrivateCandidateMetadata,
    location: PersistedSourceLocation,
    session_id: String,
    revision_id: String,
    display_filename: String,
    observed_at_ms: u64,
) -> Result<PersistSessionRevision, IndexError> {
    let source = indexed_model_source(&normalized.source).ok_or(IndexError::Runtime)?;
    let content_hash = canonical_content_hash(&normalized)?;
    let source_created_at_ms = normalized
        .created_at
        .as_deref()
        .and_then(parse_timestamp_ms);
    let entries = normalized
        .entries
        .into_iter()
        .enumerate()
        .map(|(ordinal, event)| persisted_entry(ordinal as u64, event))
        .collect();
    Ok(PersistSessionRevision {
        session: PersistedSession {
            id: session_id,
            source,
            vendor_session_id: normalized.id,
            title: normalized.title,
            created_at_ms: source_created_at_ms,
            source_version: normalized.source_version,
            collection: Some(metadata.collection.clone()),
            display_filename: Some(display_filename),
        },
        revision: PersistedRevision {
            id: revision_id,
            content_hash,
            indexed_at_ms: observed_at_ms,
            source_created_at_ms,
            source_updated_at_ms: metadata.modified_unix_ms,
            duration_ms: normalized.duration_ms,
            diagnostic_count: normalized.diagnostics.len() as u64,
            content_availability: persisted_content_availability(&normalized.content_availability),
            entries,
        },
        source_location: location,
        observed_at_ms,
    })
}

pub(super) fn persisted_entry(ordinal: u64, entry: NormalizedEntryV2) -> PersistedEntry {
    match entry {
        NormalizedEntryV2::User {
            entry_key,
            at_ms,
            text,
        } => PersistedEntry::User {
            entry_key,
            ordinal,
            at_ms,
            text,
        },
        NormalizedEntryV2::Assistant {
            entry_key,
            at_ms,
            markdown,
        } => PersistedEntry::Assistant {
            entry_key,
            ordinal,
            at_ms,
            markdown,
        },
        NormalizedEntryV2::Reasoning {
            entry_key,
            at_ms,
            text,
        } => PersistedEntry::Reasoning {
            entry_key,
            ordinal,
            at_ms,
            text,
        },
        NormalizedEntryV2::ToolCall {
            entry_key,
            at_ms,
            name,
            status,
            summary,
            detail,
        } => PersistedEntry::ToolCall {
            entry_key,
            ordinal,
            at_ms,
            name,
            status: match status {
                NormalizedToolStatusV2::Pending => IndexedToolStatus::Pending,
                NormalizedToolStatusV2::Running => IndexedToolStatus::Running,
                NormalizedToolStatusV2::Succeeded => IndexedToolStatus::Succeeded,
                NormalizedToolStatusV2::Failed => IndexedToolStatus::Failed,
            },
            summary,
            detail: match detail {
                NormalizedToolDetailV2::Unavailable => PersistedToolDetail::Unavailable,
                NormalizedToolDetailV2::Available { arguments, result } => {
                    PersistedToolDetail::Available { arguments, result }
                }
            },
        },
        NormalizedEntryV2::FileChange {
            entry_key,
            at_ms,
            display_path,
            summary,
        } => PersistedEntry::FileChange {
            entry_key,
            ordinal,
            at_ms,
            display_path,
            summary,
        },
        NormalizedEntryV2::Unknown {
            entry_key,
            at_ms,
            source_type,
        } => PersistedEntry::Unknown {
            entry_key,
            ordinal,
            at_ms,
            source_type,
        },
    }
}

pub(super) fn persisted_content_availability(
    availability: &NormalizedContentAvailabilityV2,
) -> ContentAvailabilityV1 {
    ContentAvailabilityV1 {
        reasoning: match availability.reasoning {
            NormalizedReasoningAvailabilityV2::Available => ReasoningAvailability::Available,
            NormalizedReasoningAvailabilityV2::Unavailable => ReasoningAvailability::Unavailable,
        },
        tool_details: match availability.tool_details {
            NormalizedContentAvailabilityValueV2::Available => ContentAvailability::Available,
            NormalizedContentAvailabilityValueV2::Partial => ContentAvailability::Partial,
            NormalizedContentAvailabilityValueV2::Unavailable => ContentAvailability::Unavailable,
        },
    }
}

fn persisted_location(
    metadata: &PrivateCandidateMetadata,
    generation: u64,
) -> PersistedSourceLocation {
    PersistedSourceLocation {
        canonical_path_utf16le: metadata
            .canonical_path
            .as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect(),
        source_identity_hash: source_identity_hash(metadata),
        root_identity: encode_identity(metadata.root_identity),
        file_identity: encode_identity(metadata.file_identity),
        file_size_bytes: metadata.file_size_bytes,
        modified_at_ms: metadata.modified_unix_ms.unwrap_or(0),
        last_seen_generation: Some(generation),
    }
}

fn canonical_content_hash(session: &NormalizedSessionV2) -> Result<[u8; 32], IndexError> {
    let encoded = serde_json::to_vec(session).map_err(|_| IndexError::Runtime)?;
    Ok(Sha256::digest(encoded).into())
}

fn source_identity_hash(metadata: &PrivateCandidateMetadata) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"ai-session-replay-source-identity-v1\0");
    hasher.update(discovery_source_name(metadata.source).as_bytes());
    hasher.update(metadata.root_identity.0.to_le_bytes());
    hasher.update(metadata.root_identity.1);
    hasher.update(metadata.file_identity.0.to_le_bytes());
    hasher.update(metadata.file_identity.1);
    hasher.finalize().into()
}

fn encode_identity(identity: (u64, [u8; 16])) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(24);
    encoded.extend_from_slice(&identity.0.to_le_bytes());
    encoded.extend_from_slice(&identity.1);
    encoded
}

fn parse_timestamp_ms(value: &str) -> Option<u64> {
    let milliseconds = OffsetDateTime::parse(value, &Rfc3339)
        .ok()?
        .unix_timestamp_nanos()
        .checked_div(1_000_000)?;
    u64::try_from(milliseconds).ok()
}

pub(super) fn availability_diagnostic_code(diagnostic: &AvailabilityDiagnostic) -> &'static str {
    match diagnostic.status {
        AvailabilityStatus::Missing => "SOURCE_ROOT_MISSING",
        AvailabilityStatus::Disabled => "SOURCE_ROOT_DISABLED",
        AvailabilityStatus::Denied => "SOURCE_ROOT_DENIED",
        AvailabilityStatus::Locked => "SOURCE_ROOT_LOCKED",
        AvailabilityStatus::Unsupported => "SOURCE_ROOT_UNSUPPORTED",
        AvailabilityStatus::RemoteOnly => "SOURCE_ROOT_REMOTE_ONLY",
    }
}

pub(super) fn indexed_source(source: DiscoverySource) -> Option<IndexedSessionSourceV1> {
    match source {
        DiscoverySource::ClaudeCode => Some(IndexedSessionSourceV1::ClaudeCode),
        DiscoverySource::Codex => Some(IndexedSessionSourceV1::Codex),
        DiscoverySource::CopilotCli => Some(IndexedSessionSourceV1::CopilotCli),
        DiscoverySource::VscodeCopilot => Some(IndexedSessionSourceV1::VscodeCopilot),
        DiscoverySource::JetBrains => None,
    }
}

pub(super) fn indexed_model_source(source: &SessionSource) -> Option<IndexedSessionSourceV1> {
    match source {
        SessionSource::ClaudeCode => Some(IndexedSessionSourceV1::ClaudeCode),
        SessionSource::Codex => Some(IndexedSessionSourceV1::Codex),
        SessionSource::CopilotCli => Some(IndexedSessionSourceV1::CopilotCli),
        SessionSource::VscodeCopilot => Some(IndexedSessionSourceV1::VscodeCopilot),
    }
}

fn discovery_source_name(source: DiscoverySource) -> &'static str {
    match source {
        DiscoverySource::ClaudeCode => "claude-code",
        DiscoverySource::Codex => "codex",
        DiscoverySource::CopilotCli => "copilot-cli",
        DiscoverySource::VscodeCopilot => "vscode-copilot",
        DiscoverySource::JetBrains => "jetbrains",
    }
}

fn running_state(generation: u64, started_at_ms: u64, counts: ScanCounts) -> IndexRefreshStateV1 {
    IndexRefreshStateV1::Running {
        schema_version: SCHEMA_VERSION,
        generation,
        started_at_ms,
        discovered_count: counts.discovered,
        processed_count: counts.processed,
        indexed_count: counts.indexed,
        unchanged_count: counts.unchanged,
        failed_count: counts.failed,
        skipped_count: counts.skipped,
        warning_count: counts.warnings,
    }
}

fn completed_state(
    generation: u64,
    started_at_ms: u64,
    completed_at_ms: u64,
    counts: ScanCounts,
) -> IndexRefreshStateV1 {
    IndexRefreshStateV1::Completed {
        schema_version: SCHEMA_VERSION,
        generation,
        started_at_ms,
        completed_at_ms,
        discovered_count: counts.discovered,
        processed_count: counts.processed,
        indexed_count: counts.indexed,
        unchanged_count: counts.unchanged,
        failed_count: counts.failed,
        skipped_count: counts.skipped,
        warning_count: counts.warnings,
    }
}

fn failed_state(
    generation: u64,
    started_at_ms: u64,
    completed_at_ms: u64,
    counts: ScanCounts,
    error_code: &str,
) -> IndexRefreshStateV1 {
    IndexRefreshStateV1::Failed {
        schema_version: SCHEMA_VERSION,
        generation,
        started_at_ms,
        completed_at_ms,
        error_code: error_code.to_owned(),
        discovered_count: counts.discovered,
        processed_count: counts.processed,
        indexed_count: counts.indexed,
        unchanged_count: counts.unchanged,
        failed_count: counts.failed,
        skipped_count: counts.skipped,
        warning_count: counts.warnings,
    }
}
