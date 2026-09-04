use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::Manager;
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

use crate::adapters::deletion::{DeletionArtifactDeclaration, artifact_declaration};
use crate::database::repository::{
    IndexedSessionRepository, RepositoryError, SourceDeletionRecord,
};
use crate::discovery::catalog::DiscoverySource;
use crate::discovery::roots::{ProductionRootProvider, RootProvider, RootSpec};
use crate::indexed_library::{
    ContractValidate, RestoreSuppressedSourceResultV1, SourceDeletionConfirmationV1,
    SuppressedSourceListRequestV1, SuppressedSourcePageV1, SuppressedSourceV1,
};
use crate::io::{LocalRoot, OpenedDeletionTarget, SnapshotError};

const CONFIRMATION_LIFETIME_MS: u64 = 5 * 60 * 1_000;
const MAX_PENDING_CONFIRMATIONS: usize = 128;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum DeletionError {
    #[error("INVALID_DELETION_REQUEST")]
    InvalidRequest,
    #[error("SESSION_NOT_FOUND")]
    SessionNotFound,
    #[error("SUPPRESSION_NOT_FOUND")]
    SuppressionNotFound,
    #[error("SOURCE_UNAVAILABLE")]
    SourceUnavailable,
    #[error("UNSAFE_SOURCE_ARTIFACTS")]
    UnsafeSource,
    #[error("SOURCE_STATE_CHANGED")]
    SourceStateChanged,
    #[error("CONFIRMATION_INVALID")]
    ConfirmationInvalid,
    #[error("CONFIRMATION_EXPIRED")]
    ConfirmationExpired,
    #[error("SOURCE_DELETE_FAILED")]
    SourceDeleteFailed,
    #[error("DELETION_STORAGE_FAILED")]
    Storage,
    #[error("DELETION_IDENTIFIER_FAILED")]
    Identifier,
    #[error("DELETION_STATE_CONFLICT")]
    StateConflict,
}

pub(crate) trait DeletionClock: Send + Sync {
    fn now_ms(&self) -> u64;
}

pub(crate) trait DeletionIdGenerator: Send + Sync {
    fn generate(&self, prefix: &str) -> Result<String, DeletionError>;
}

struct SystemDeletionClock;

impl DeletionClock for SystemDeletionClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| u64::try_from(duration.as_millis()).ok())
            .unwrap_or(0)
            .min(MAX_SAFE_INTEGER)
    }
}

struct WindowsDeletionIdGenerator;

impl DeletionIdGenerator for WindowsDeletionIdGenerator {
    fn generate(&self, prefix: &str) -> Result<String, DeletionError> {
        let mut random = [0_u8; 24];
        // SAFETY: the buffer is writable for exactly the length passed to the OS.
        if unsafe { rtl_gen_random(random.as_mut_ptr(), random.len() as u32) } == 0 {
            return Err(DeletionError::Identifier);
        }
        let mut value = String::with_capacity(prefix.len() + 1 + random.len() * 2);
        value.push_str(prefix);
        value.push('_');
        use std::fmt::Write as _;
        for byte in random {
            write!(&mut value, "{byte:02x}").map_err(|_| DeletionError::Identifier)?;
        }
        Ok(value)
    }
}

#[link(name = "advapi32")]
unsafe extern "system" {
    #[link_name = "SystemFunction036"]
    fn rtl_gen_random(buffer: *mut u8, length: u32) -> u8;
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetIdentity {
    canonical_path: PathBuf,
    volume_serial: u64,
    file_id: [u8; 16],
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeletionManifest {
    files: Vec<TargetIdentity>,
    directories: Vec<TargetIdentity>,
}

struct ResolvedDeletionPlan {
    manifest: DeletionManifest,
    root: LocalRoot,
    collection: LocalRoot,
    files: Vec<OpenedDeletionTarget>,
    directories: Vec<OpenedDeletionTarget>,
}

#[derive(Debug, Clone)]
struct PendingDeletion {
    session_id: String,
    source_record: SourceDeletionRecord,
    manifest: DeletionManifest,
    expires_at_ms: u64,
}

struct SessionDeletionInner {
    repository: IndexedSessionRepository,
    roots: Arc<dyn RootProvider>,
    clock: Arc<dyn DeletionClock>,
    ids: Arc<dyn DeletionIdGenerator>,
    pending: Mutex<HashMap<String, PendingDeletion>>,
}

#[derive(Clone)]
pub(crate) struct SessionDeletionService {
    inner: Arc<SessionDeletionInner>,
}

impl SessionDeletionService {
    pub(crate) fn new(repository: IndexedSessionRepository) -> Self {
        Self::with_dependencies(
            repository,
            Arc::new(ProductionRootProvider::default()),
            Arc::new(SystemDeletionClock),
            Arc::new(WindowsDeletionIdGenerator),
        )
    }

    pub(crate) fn with_dependencies(
        repository: IndexedSessionRepository,
        roots: Arc<dyn RootProvider>,
        clock: Arc<dyn DeletionClock>,
        ids: Arc<dyn DeletionIdGenerator>,
    ) -> Self {
        Self {
            inner: Arc::new(SessionDeletionInner {
                repository,
                roots,
                clock,
                ids,
                pending: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub(crate) async fn delete_library_only(
        &self,
        session_id: &str,
    ) -> Result<SuppressedSourceV1, DeletionError> {
        let suppression_id = self.inner.ids.generate("suppression")?;
        self.inner
            .repository
            .delete_indexed_session(
                session_id,
                &suppression_id,
                false,
                self.inner.clock.now_ms(),
                None,
            )
            .await
            .map_err(map_repository_error)
    }

    pub(crate) async fn list_suppressed(
        &self,
        request: &SuppressedSourceListRequestV1,
    ) -> Result<SuppressedSourcePageV1, DeletionError> {
        self.inner
            .repository
            .list_suppressed_sources(request)
            .await
            .map_err(map_repository_error)
    }

    pub(crate) async fn restore(
        &self,
        suppression_id: &str,
    ) -> Result<RestoreSuppressedSourceResultV1, DeletionError> {
        self.inner
            .repository
            .restore_suppressed_source(suppression_id)
            .await
            .map_err(map_repository_error)
    }

    pub(crate) async fn prepare_source_deletion(
        &self,
        session_id: &str,
    ) -> Result<SourceDeletionConfirmationV1, DeletionError> {
        let source_record = self
            .inner
            .repository
            .source_deletion_record(session_id)
            .await
            .map_err(map_repository_error)?;
        let roots = self.inner.roots.clone();
        let record_for_resolution = source_record.clone();
        let resolved = tokio::task::spawn_blocking(move || {
            resolve_deletion_plan(roots.as_ref(), &record_for_resolution)
        })
        .await
        .map_err(|_| DeletionError::SourceUnavailable)??;
        let artifact_count = u64::try_from(resolved.manifest.files.len())
            .map_err(|_| DeletionError::UnsafeSource)?;
        let now_ms = self.inner.clock.now_ms();
        let expires_at_ms = now_ms
            .checked_add(CONFIRMATION_LIFETIME_MS)
            .filter(|value| *value <= MAX_SAFE_INTEGER)
            .ok_or(DeletionError::InvalidRequest)?;
        let confirmation_token = self.inner.ids.generate("delete")?;
        let pending = PendingDeletion {
            session_id: session_id.to_owned(),
            source_record,
            manifest: resolved.manifest,
            expires_at_ms,
        };
        let mut plans = self
            .inner
            .pending
            .lock()
            .map_err(|_| DeletionError::Storage)?;
        remember_pending_deletion(&mut plans, now_ms, confirmation_token.clone(), pending);
        drop(plans);

        let confirmation = SourceDeletionConfirmationV1 {
            schema_version: 1,
            session_id: session_id.to_owned(),
            confirmation_token,
            artifact_count,
            expires_at_ms,
        };
        confirmation
            .validate()
            .map_err(|_| DeletionError::InvalidRequest)?;
        Ok(confirmation)
    }

    pub(crate) async fn delete_with_source(
        &self,
        session_id: &str,
        confirmation_token: &str,
    ) -> Result<SuppressedSourceV1, DeletionError> {
        let now_ms = self.inner.clock.now_ms();
        let pending = {
            let mut plans = self
                .inner
                .pending
                .lock()
                .map_err(|_| DeletionError::Storage)?;
            let Some(plan) = plans.get(confirmation_token) else {
                return Err(DeletionError::ConfirmationInvalid);
            };
            if plan.session_id != session_id {
                return Err(DeletionError::ConfirmationInvalid);
            }
            plans
                .remove(confirmation_token)
                .ok_or(DeletionError::ConfirmationInvalid)?
        };
        if pending.expires_at_ms < now_ms {
            return Err(DeletionError::ConfirmationExpired);
        }

        let current = self
            .inner
            .repository
            .source_deletion_record(session_id)
            .await
            .map_err(map_repository_error)?;
        if current != pending.source_record {
            return Err(DeletionError::SourceStateChanged);
        }
        let suppression_id = self.inner.ids.generate("suppression")?;
        let roots = self.inner.roots.clone();
        let current_for_resolution = current.clone();
        let resolved = tokio::task::spawn_blocking(move || {
            resolve_deletion_plan(roots.as_ref(), &current_for_resolution)
        })
        .await
        .map_err(|_| DeletionError::SourceUnavailable)??;
        if resolved.manifest != pending.manifest {
            return Err(DeletionError::SourceStateChanged);
        }
        tokio::task::spawn_blocking(move || execute_deletion_plan(resolved))
            .await
            .map_err(|_| DeletionError::SourceDeleteFailed)??;

        self.inner
            .repository
            .mark_source_missing(session_id, &current.source_identity_hash)
            .await
            .map_err(map_repository_error)?;
        self.inner
            .repository
            .delete_indexed_session(
                session_id,
                &suppression_id,
                true,
                now_ms,
                Some(&current.source_identity_hash),
            )
            .await
            .map_err(map_repository_error)
    }
}

fn remember_pending_deletion(
    plans: &mut HashMap<String, PendingDeletion>,
    now_ms: u64,
    confirmation_token: String,
    pending: PendingDeletion,
) {
    plans.retain(|_, plan| plan.expires_at_ms >= now_ms && plan.session_id != pending.session_id);
    if plans.len() >= MAX_PENDING_CONFIRMATIONS {
        let oldest = plans
            .iter()
            .min_by(|(left_token, left), (right_token, right)| {
                left.expires_at_ms
                    .cmp(&right.expires_at_ms)
                    .then_with(|| left_token.cmp(right_token))
            })
            .map(|(token, _)| token.clone());
        if let Some(oldest) = oldest {
            plans.remove(&oldest);
        }
    }
    plans.insert(confirmation_token, pending);
}

pub(crate) fn initialize<R: tauri::Runtime>(app: &mut tauri::App<R>) -> Result<(), DeletionError> {
    let repository = app
        .try_state::<crate::database::DatabaseState>()
        .ok_or(DeletionError::Storage)?
        .repository();
    if app.manage(SessionDeletionService::new(repository)) {
        Ok(())
    } else {
        Err(DeletionError::StateConflict)
    }
}

fn resolve_deletion_plan(
    provider: &dyn RootProvider,
    record: &SourceDeletionRecord,
) -> Result<ResolvedDeletionPlan, DeletionError> {
    let expected_source = discovery_source(&record.source);
    let mut matching = provider.roots().into_iter().filter_map(|root| match root {
        RootSpec::Local {
            source,
            collection,
            path,
            read_root_path,
            ..
        } if source == expected_source && collection == record.collection => {
            Some((path, read_root_path))
        }
        _ => None,
    });
    let (collection_path, read_root_path) =
        matching.next().ok_or(DeletionError::SourceUnavailable)?;
    if matching.next().is_some() {
        return Err(DeletionError::UnsafeSource);
    }

    let root = LocalRoot::new(read_root_path).map_err(map_prepare_snapshot_error)?;
    if root.identity() != decode_identity(&record.root_identity)? {
        return Err(DeletionError::SourceStateChanged);
    }
    let collection = LocalRoot::new(collection_path).map_err(map_prepare_snapshot_error)?;
    if !root.contains_canonical_path(collection.canonical_path()) {
        return Err(DeletionError::UnsafeSource);
    }
    let primary_path = decode_path(&record.canonical_path_utf16le)?;
    let primary = collection
        .open_file_for_deletion(&primary_path)
        .map_err(map_prepare_snapshot_error)?;
    if primary.canonical_path() != primary_path
        || (primary.stamp().volume_serial, primary.stamp().file_id)
            != decode_identity(&record.file_identity)?
    {
        return Err(DeletionError::SourceStateChanged);
    }

    match artifact_declaration(&record.source) {
        DeletionArtifactDeclaration::PrimaryFile => {
            let manifest = DeletionManifest {
                files: vec![identity_of(&primary)],
                directories: Vec::new(),
            };
            Ok(ResolvedDeletionPlan {
                manifest,
                root,
                collection,
                files: vec![primary],
                directories: Vec::new(),
            })
        }
        DeletionArtifactDeclaration::SiblingFiles { extensions } => {
            if extensions.is_empty() || extensions.len() > 8 {
                return Err(DeletionError::UnsafeSource);
            }
            let mut files = vec![primary];
            for extension in extensions {
                if primary_path.extension().is_some_and(|current| {
                    current.to_string_lossy().eq_ignore_ascii_case(extension)
                }) {
                    continue;
                }
                let mut sibling_path = primary_path.clone();
                sibling_path.set_extension(extension);
                match collection.open_file_for_deletion(&sibling_path) {
                    Ok(sibling) => files.push(sibling),
                    Err(SnapshotError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(map_prepare_snapshot_error(error)),
                }
            }
            sort_targets(&mut files);
            let manifest = DeletionManifest {
                files: files.iter().map(identity_of).collect(),
                directories: Vec::new(),
            };
            Ok(ResolvedDeletionPlan {
                manifest,
                root,
                collection,
                files,
                directories: Vec::new(),
            })
        }
        DeletionArtifactDeclaration::SessionDirectory {
            primary_file_name,
            max_artifacts,
            max_depth,
        } => {
            if !primary_path.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .eq_ignore_ascii_case(primary_file_name)
            }) {
                return Err(DeletionError::UnsafeSource);
            }
            drop(primary);
            let session_directory = primary_path.parent().ok_or(DeletionError::UnsafeSource)?;
            let mut files = Vec::new();
            let mut directories = Vec::new();
            collect_directory_targets(
                &collection,
                session_directory,
                0,
                max_depth,
                max_artifacts,
                &mut files,
                &mut directories,
            )?;
            let expected_primary = decode_identity(&record.file_identity)?;
            if !files.iter().any(|file| {
                file.canonical_path() == primary_path
                    && (file.stamp().volume_serial, file.stamp().file_id) == expected_primary
            }) {
                return Err(DeletionError::SourceStateChanged);
            }
            sort_targets(&mut files);
            sort_targets(&mut directories);
            let manifest = DeletionManifest {
                files: files.iter().map(identity_of).collect(),
                directories: directories.iter().map(identity_of).collect(),
            };
            Ok(ResolvedDeletionPlan {
                manifest,
                root,
                collection,
                files,
                directories,
            })
        }
    }
}

fn collect_directory_targets(
    root: &LocalRoot,
    path: &Path,
    depth: usize,
    max_depth: usize,
    max_artifacts: usize,
    files: &mut Vec<OpenedDeletionTarget>,
    directories: &mut Vec<OpenedDeletionTarget>,
) -> Result<(), DeletionError> {
    if depth > max_depth || directories.len() >= max_artifacts {
        return Err(DeletionError::UnsafeSource);
    }
    let directory = root
        .open_directory_for_deletion(path)
        .map_err(map_prepare_snapshot_error)?;
    let mut entries = fs::read_dir(directory.canonical_path())
        .map_err(|_| DeletionError::SourceUnavailable)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| DeletionError::SourceUnavailable)?;
    entries.sort_by_key(|entry| entry.file_name().encode_wide().collect::<Vec<_>>());
    for entry in entries {
        let candidate = entry.path();
        let metadata =
            fs::symlink_metadata(&candidate).map_err(|_| DeletionError::SourceUnavailable)?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(DeletionError::UnsafeSource);
        }
        if metadata.is_file() {
            if files.len() >= max_artifacts {
                return Err(DeletionError::UnsafeSource);
            }
            files.push(
                root.open_file_for_deletion(candidate)
                    .map_err(map_prepare_snapshot_error)?,
            );
        } else if metadata.is_dir() {
            collect_directory_targets(
                root,
                &candidate,
                depth + 1,
                max_depth,
                max_artifacts,
                files,
                directories,
            )?;
        } else {
            return Err(DeletionError::UnsafeSource);
        }
    }
    directories.push(directory);
    Ok(())
}

fn execute_deletion_plan(mut plan: ResolvedDeletionPlan) -> Result<(), DeletionError> {
    plan.root
        .revalidate()
        .map_err(|_| DeletionError::SourceStateChanged)?;
    plan.collection
        .revalidate()
        .map_err(|_| DeletionError::SourceStateChanged)?;
    for file in plan.files.drain(..) {
        file.delete()
            .map_err(|_| DeletionError::SourceDeleteFailed)?;
    }
    plan.directories.sort_by_key(|directory| {
        std::cmp::Reverse(directory.canonical_path().components().count())
    });
    for directory in plan.directories.drain(..) {
        directory
            .delete()
            .map_err(|_| DeletionError::SourceDeleteFailed)?;
    }
    Ok(())
}

fn sort_targets(targets: &mut [OpenedDeletionTarget]) {
    targets.sort_by_key(|target| {
        target
            .canonical_path()
            .as_os_str()
            .encode_wide()
            .collect::<Vec<_>>()
    });
}

fn identity_of(target: &OpenedDeletionTarget) -> TargetIdentity {
    TargetIdentity {
        canonical_path: target.canonical_path().to_path_buf(),
        volume_serial: target.stamp().volume_serial,
        file_id: target.stamp().file_id,
    }
}

fn decode_identity(value: &[u8]) -> Result<(u64, [u8; 16]), DeletionError> {
    if value.len() != 24 {
        return Err(DeletionError::UnsafeSource);
    }
    let volume_serial = u64::from_le_bytes(
        value[..8]
            .try_into()
            .map_err(|_| DeletionError::UnsafeSource)?,
    );
    let file_id = value[8..]
        .try_into()
        .map_err(|_| DeletionError::UnsafeSource)?;
    Ok((volume_serial, file_id))
}

fn decode_path(value: &[u8]) -> Result<PathBuf, DeletionError> {
    let mut chunks = value.chunks_exact(2);
    let units = chunks
        .by_ref()
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();
    if !chunks.remainder().is_empty() || units.is_empty() || units.contains(&0) {
        return Err(DeletionError::UnsafeSource);
    }
    Ok(PathBuf::from(OsString::from_wide(&units)))
}

fn discovery_source(source: &crate::indexed_library::IndexedSessionSourceV1) -> DiscoverySource {
    match source {
        crate::indexed_library::IndexedSessionSourceV1::ClaudeCode => DiscoverySource::ClaudeCode,
        crate::indexed_library::IndexedSessionSourceV1::Codex => DiscoverySource::Codex,
        crate::indexed_library::IndexedSessionSourceV1::CopilotCli => DiscoverySource::CopilotCli,
        crate::indexed_library::IndexedSessionSourceV1::VscodeCopilot => {
            DiscoverySource::VscodeCopilot
        }
    }
}

fn map_prepare_snapshot_error(error: SnapshotError) -> DeletionError {
    match error {
        SnapshotError::Io(error) if error.kind() == io::ErrorKind::NotFound => {
            DeletionError::SourceUnavailable
        }
        SnapshotError::Io(_) => DeletionError::SourceUnavailable,
        _ => DeletionError::UnsafeSource,
    }
}

fn map_repository_error(error: RepositoryError) -> DeletionError {
    match error {
        RepositoryError::InvalidInput => DeletionError::InvalidRequest,
        RepositoryError::SessionNotFound => DeletionError::SessionNotFound,
        RepositoryError::SourceStateChanged => DeletionError::SourceStateChanged,
        RepositoryError::SuppressionNotFound => DeletionError::SuppressionNotFound,
        _ => DeletionError::Storage,
    }
}

#[cfg(test)]
#[path = "deletion_tests.rs"]
mod tests;
