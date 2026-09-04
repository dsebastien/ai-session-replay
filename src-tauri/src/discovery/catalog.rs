use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf, Prefix};
use std::time::UNIX_EPOCH;

use serde::Serialize;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_SHARE_READ, FILE_SHARE_WRITE, SECURITY_IDENTIFICATION,
};

use crate::io::{LocalRoot, SnapshotError};

use super::roots::{CandidateMatcher, RootProvider, RootSpec};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DiscoverySource {
    ClaudeCode,
    Codex,
    CopilotCli,
    VscodeCopilot,
    #[allow(dead_code, reason = "reserved for milestone-two JetBrains detection")]
    JetBrains,
}

impl From<SnapshotError> for ScanError {
    fn from(error: SnapshotError) -> Self {
        Self::Snapshot(error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum AvailabilityStatus {
    Missing,
    Disabled,
    Denied,
    Locked,
    Unsupported,
    RemoteOnly,
}

fn directory_is_empty(path: &Path) -> Result<bool, ScanError> {
    let _guard = open_guarded_directory(path)?;
    match fs::read_dir(path)?.next() {
        None => Ok(true),
        Some(Ok(_)) => Ok(false),
        Some(Err(error)) if is_disappearance(&error) => Ok(true),
        Some(Err(error)) => Err(ScanError::Io(error)),
    }
}

fn is_disappearance(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AvailabilityDiagnostic {
    pub(crate) source: DiscoverySource,
    pub(crate) collection: String,
    pub(crate) status: AvailabilityStatus,
}

fn availability_from_scan_error(error: &ScanError) -> AvailabilityStatus {
    match error {
        ScanError::Io(error) => availability_from_io_error(error),
        ScanError::Snapshot(SnapshotError::Io(error)) => availability_from_io_error(error),
        ScanError::Snapshot(SnapshotError::NonLocalVolume) => AvailabilityStatus::RemoteOnly,
        ScanError::Snapshot(
            SnapshotError::InvalidPath | SnapshotError::OutsideRoot | SnapshotError::NotFile,
        ) => AvailabilityStatus::Unsupported,
        ScanError::Snapshot(_) => AvailabilityStatus::Denied,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub(crate) struct CatalogId(String);

impl CatalogId {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CandidateDisplay {
    pub(crate) collection: String,
    pub(crate) title: String,
    pub(crate) file_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) modified_unix_ms: Option<u64>,
    pub(crate) size_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogEntry {
    pub(crate) id: CatalogId,
    pub(crate) source: DiscoverySource,
    pub(crate) display: CandidateDisplay,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogSnapshot {
    pub(crate) entries: Vec<CatalogEntry>,
    pub(crate) diagnostics: Vec<AvailabilityDiagnostic>,
    #[serde(skip_serializing)]
    pub(crate) scanned_collections: Vec<ScannedCollection>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScannedCollection {
    pub(crate) source: DiscoverySource,
    pub(crate) collection: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DiscoveryLimits {
    pub(crate) max_depth: usize,
    pub(crate) max_entries_per_root: usize,
    pub(crate) max_candidates_per_root: usize,
    pub(crate) max_total_candidates: usize,
}

impl DiscoveryLimits {
    fn bounded(self) -> Self {
        let ceiling = Self::default();
        Self {
            max_depth: self.max_depth.min(ceiling.max_depth),
            max_entries_per_root: self.max_entries_per_root.min(ceiling.max_entries_per_root),
            max_candidates_per_root: self
                .max_candidates_per_root
                .min(ceiling.max_candidates_per_root),
            max_total_candidates: self.max_total_candidates.min(ceiling.max_total_candidates),
        }
    }
}

impl Default for DiscoveryLimits {
    fn default() -> Self {
        Self {
            max_depth: 8,
            max_entries_per_root: 50_000,
            max_candidates_per_root: 10_000,
            max_total_candidates: 25_000,
        }
    }
}

pub(crate) trait CatalogIdGenerator: Send + Sync {
    fn generate(&self, sequence: u64) -> CatalogId;
}

#[derive(Debug)]
pub(crate) struct WindowsCatalogIdGenerator {
    nonce: [u8; 16],
}

impl WindowsCatalogIdGenerator {
    pub(crate) fn new() -> io::Result<Self> {
        let mut nonce = [0_u8; 16];
        // SAFETY: the buffer is writable for exactly the length passed to the OS.
        if unsafe { rtl_gen_random(nonce.as_mut_ptr(), nonce.len() as u32) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { nonce })
    }
}

impl CatalogIdGenerator for WindowsCatalogIdGenerator {
    fn generate(&self, sequence: u64) -> CatalogId {
        let mut value = String::with_capacity(49);
        for byte in self.nonce {
            write!(&mut value, "{byte:02x}").expect("writing to a String cannot fail");
        }
        write!(&mut value, "-{sequence:016x}").expect("writing to a String cannot fail");
        CatalogId::new(value)
    }
}

#[link(name = "advapi32")]
unsafe extern "system" {
    #[link_name = "SystemFunction036"]
    fn rtl_gen_random(buffer: *mut u8, length: u32) -> u8;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CandidateFormat {
    Json,
    JsonLines,
    ZstdJsonLines,
}

#[derive(Debug)]
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "retained roots are consumed by the discovery-to-load command in task 9"
    )
)]
struct CatalogRoot {
    root: LocalRoot,
}

#[derive(Debug)]
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "private records are consumed by the discovery-to-load command in task 9"
    )
)]
struct PrivateCatalogRecord {
    root_index: usize,
    candidate_path: PathBuf,
    expected_identity: (u64, [u8; 16]),
    source: DiscoverySource,
    collection: &'static str,
    format: CandidateFormat,
    modified_unix_ms: Option<u64>,
    size_bytes: u64,
}

#[derive(Debug)]
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "resolved records are consumed by the discovery-to-load command in task 9"
    )
)]
pub(crate) struct ResolvedCatalogRecord<'a> {
    pub(crate) root: &'a LocalRoot,
    pub(crate) candidate_path: &'a Path,
    pub(crate) expected_identity: (u64, [u8; 16]),
    pub(crate) source: DiscoverySource,
    pub(crate) format: CandidateFormat,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PrivateCandidateMetadata {
    pub(crate) canonical_path: PathBuf,
    pub(crate) root_identity: (u64, [u8; 16]),
    pub(crate) file_identity: (u64, [u8; 16]),
    pub(crate) source: DiscoverySource,
    pub(crate) collection: String,
    pub(crate) modified_unix_ms: Option<u64>,
    pub(crate) file_size_bytes: u64,
}

pub(crate) struct DiscoveryCatalog {
    id_generator: Box<dyn CatalogIdGenerator>,
    next_sequence: u64,
    roots: Vec<CatalogRoot>,
    records: HashMap<String, PrivateCatalogRecord>,
}

impl DiscoveryCatalog {
    pub(crate) fn new(generator: impl CatalogIdGenerator + 'static) -> Self {
        Self {
            id_generator: Box::new(generator),
            next_sequence: 0,
            roots: Vec::new(),
            records: HashMap::new(),
        }
    }

    pub(crate) fn refresh(
        &mut self,
        provider: &dyn RootProvider,
        limits: DiscoveryLimits,
    ) -> CatalogSnapshot {
        let limits = limits.bounded();
        let mut roots = Vec::new();
        let mut discovered = Vec::new();
        let mut diagnostics = Vec::new();
        let mut scanned_collections = Vec::new();

        for spec in provider.roots() {
            match spec {
                RootSpec::Unavailable {
                    source,
                    collection,
                    status,
                } => {
                    if status == AvailabilityStatus::Missing {
                        scanned_collections.push(ScannedCollection {
                            source,
                            collection: collection.to_owned(),
                        });
                    }
                    diagnostics.push(AvailabilityDiagnostic {
                        source,
                        collection: collection.to_owned(),
                        status,
                    });
                }
                RootSpec::Local {
                    source,
                    collection,
                    path,
                    read_root_path,
                    matcher,
                } => {
                    let root = match LocalRoot::new(&read_root_path) {
                        Ok(root) => root,
                        Err(error) => {
                            let status = availability_from_root_error(&read_root_path, &error);
                            if status == AvailabilityStatus::Missing {
                                scanned_collections.push(ScannedCollection {
                                    source,
                                    collection: collection.to_owned(),
                                });
                            }
                            diagnostics.push(AvailabilityDiagnostic {
                                source,
                                collection: collection.to_owned(),
                                status,
                            });
                            continue;
                        }
                    };
                    let remaining = limits.max_total_candidates.saturating_sub(discovered.len());
                    let root_index = roots.len();
                    match scan_root(
                        &path,
                        &root,
                        matcher,
                        limits,
                        remaining.min(limits.max_candidates_per_root),
                    ) {
                        Ok(result) => {
                            if result.complete {
                                scanned_collections.push(ScannedCollection {
                                    source,
                                    collection: collection.to_owned(),
                                });
                            }
                            if let Some(status) = result.incomplete_status {
                                diagnostics.push(AvailabilityDiagnostic {
                                    source,
                                    collection: collection.to_owned(),
                                    status,
                                });
                            }
                            roots.push(CatalogRoot { root });
                            discovered.extend(result.candidates.into_iter().map(|candidate| {
                                DiscoveredCandidate {
                                    root_index,
                                    source,
                                    collection,
                                    candidate,
                                }
                            }));
                        }
                        Err(error) => {
                            let status = availability_from_scan_error(&error);
                            if status == AvailabilityStatus::Missing {
                                scanned_collections.push(ScannedCollection {
                                    source,
                                    collection: collection.to_owned(),
                                });
                            }
                            diagnostics.push(AvailabilityDiagnostic {
                                source,
                                collection: collection.to_owned(),
                                status,
                            });
                        }
                    }
                }
            }
        }

        discovered.sort_by_cached_key(|candidate| {
            (
                candidate.source,
                candidate.collection.to_ascii_lowercase(),
                path_sort_key(&candidate.candidate.relative_path),
            )
        });
        diagnostics.sort_by(|left, right| {
            left.source.cmp(&right.source).then_with(|| {
                left.collection
                    .to_ascii_lowercase()
                    .cmp(&right.collection.to_ascii_lowercase())
            })
        });
        scanned_collections.sort_by(|left, right| {
            left.source
                .cmp(&right.source)
                .then_with(|| left.collection.cmp(&right.collection))
        });
        scanned_collections.dedup();

        let mut records = HashMap::with_capacity(discovered.len());
        let mut entries = Vec::with_capacity(discovered.len());
        for discovered in discovered {
            let id = self.id_generator.generate(self.next_sequence);
            self.next_sequence = self.next_sequence.wrapping_add(1);
            records.insert(
                id.as_str().to_owned(),
                PrivateCatalogRecord {
                    root_index: discovered.root_index,
                    candidate_path: discovered.candidate.absolute_path,
                    expected_identity: discovered.candidate.identity,
                    source: discovered.source,
                    collection: discovered.collection,
                    format: discovered.candidate.format,
                    modified_unix_ms: discovered.candidate.modified_unix_ms,
                    size_bytes: discovered.candidate.size_bytes,
                },
            );
            entries.push(CatalogEntry {
                id,
                source: discovered.source,
                display: CandidateDisplay {
                    collection: discovered.collection.to_owned(),
                    title: safe_title(&discovered.candidate.relative_path),
                    file_name: safe_component(
                        discovered
                            .candidate
                            .relative_path
                            .file_name()
                            .unwrap_or_default(),
                    ),
                    modified_unix_ms: discovered.candidate.modified_unix_ms,
                    size_bytes: discovered.candidate.size_bytes,
                },
            });
        }

        self.roots = roots;
        self.records = records;
        CatalogSnapshot {
            entries,
            diagnostics,
            scanned_collections,
        }
    }

    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "catalog resolution is consumed by the discovery-to-load command in task 9"
        )
    )]
    pub(crate) fn resolve(&self, id: &str) -> Option<ResolvedCatalogRecord<'_>> {
        let record = self.records.get(id)?;
        let root = self.roots.get(record.root_index)?;
        Some(ResolvedCatalogRecord {
            root: &root.root,
            candidate_path: &record.candidate_path,
            expected_identity: record.expected_identity,
            source: record.source,
            format: record.format,
        })
    }

    pub(crate) fn private_metadata(&self, id: &str) -> Option<PrivateCandidateMetadata> {
        let record = self.records.get(id)?;
        let root = self.roots.get(record.root_index)?;
        Some(PrivateCandidateMetadata {
            canonical_path: record.candidate_path.clone(),
            root_identity: root.root.identity(),
            file_identity: record.expected_identity,
            source: record.source,
            collection: record.collection.to_owned(),
            modified_unix_ms: record.modified_unix_ms,
            file_size_bytes: record.size_bytes,
        })
    }
}

#[derive(Debug)]
struct DiscoveredCandidate {
    root_index: usize,
    source: DiscoverySource,
    collection: &'static str,
    candidate: CandidateMetadata,
}

#[derive(Debug)]
struct CandidateMetadata {
    absolute_path: PathBuf,
    relative_path: PathBuf,
    identity: (u64, [u8; 16]),
    modified_unix_ms: Option<u64>,
    size_bytes: u64,
    format: CandidateFormat,
}

type PathSortKey = (Vec<u16>, Vec<u16>);

#[derive(Debug)]
enum ScanError {
    Io(io::Error),
    Snapshot(SnapshotError),
}

impl From<io::Error> for ScanError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

fn scan_root(
    root: &Path,
    local_root: &LocalRoot,
    matcher: CandidateMatcher,
    limits: DiscoveryLimits,
    candidate_limit: usize,
) -> Result<ScanRootResult, ScanError> {
    if candidate_limit == 0 || limits.max_depth == 0 || limits.max_entries_per_root == 0 {
        let complete = directory_is_empty(root)?;
        return Ok(ScanRootResult {
            candidates: Vec::new(),
            complete,
            incomplete_status: (!complete).then_some(AvailabilityStatus::Unsupported),
        });
    }

    let mut state = ScanState {
        root,
        local_root,
        matcher,
        limits,
        candidate_limit,
        visited: 0,
        candidates: Vec::new(),
        incomplete_status: None,
    };
    state.walk_directory(root, 1)?;

    // Codex: prefer plain .jsonl over sibling .jsonl.zst
    match matcher {
        CandidateMatcher::Codex => deduplicate_codex_siblings(&mut state.candidates),
        CandidateMatcher::VsCodeWorkspace | CandidateMatcher::VsCodeSessionDirectory => {
            deduplicate_vscode_siblings(&mut state.candidates);
        }
        CandidateMatcher::ClaudeProject | CandidateMatcher::CopilotCli => {}
    }

    Ok(ScanRootResult {
        candidates: state.candidates,
        complete: state.incomplete_status.is_none(),
        incomplete_status: state.incomplete_status,
    })
}

struct ScanRootResult {
    candidates: Vec<CandidateMetadata>,
    complete: bool,
    incomplete_status: Option<AvailabilityStatus>,
}

/// When both `foo.jsonl` and `foo.jsonl.zst` exist in the same directory,
/// keep only the plain `.jsonl` (consistent with official Codex behavior).
fn deduplicate_codex_siblings(candidates: &mut Vec<CandidateMetadata>) {
    use std::collections::HashSet;
    let plain_paths: HashSet<String> = candidates
        .iter()
        .filter(|c| c.format == CandidateFormat::JsonLines)
        .map(|c| c.relative_path.to_string_lossy().to_ascii_lowercase())
        .collect();

    candidates.retain(|c| {
        if c.format != CandidateFormat::ZstdJsonLines {
            return true;
        }
        // Check if a plain sibling exists: strip .zst suffix
        let lower = c.relative_path.to_string_lossy().to_ascii_lowercase();
        if let Some(plain_path) = lower.strip_suffix(".zst") {
            !plain_paths.contains(plain_path)
        } else {
            true
        }
    });
}

/// VS Code reads the append log first and falls back to the legacy flat JSON
/// file only when the log is absent.
/// Source: https://github.com/microsoft/vscode/blob/f9a71837c3e6a2b974948bd06b6f9c80b377e01e/src/vs/workbench/contrib/chat/common/model/chatSessionStore.ts#L635-L649
fn deduplicate_vscode_siblings(candidates: &mut Vec<CandidateMetadata>) {
    use std::collections::HashSet;
    let current_logs: HashSet<String> = candidates
        .iter()
        .filter(|candidate| candidate.format == CandidateFormat::JsonLines)
        .map(|candidate| {
            candidate
                .relative_path
                .to_string_lossy()
                .to_ascii_lowercase()
        })
        .collect();

    candidates.retain(|candidate| {
        if candidate.format != CandidateFormat::Json {
            return true;
        }
        let lower = candidate
            .relative_path
            .to_string_lossy()
            .to_ascii_lowercase();
        let Some(stem) = lower.strip_suffix(".json") else {
            return true;
        };
        !current_logs.contains(&format!("{stem}.jsonl"))
    });
}

struct ScanState<'a> {
    root: &'a Path,
    local_root: &'a LocalRoot,
    matcher: CandidateMatcher,
    limits: DiscoveryLimits,
    candidate_limit: usize,
    visited: usize,
    candidates: Vec<CandidateMetadata>,
    incomplete_status: Option<AvailabilityStatus>,
}

impl ScanState<'_> {
    fn walk_directory(&mut self, directory: &Path, depth: usize) -> Result<(), ScanError> {
        if self.candidate_limit_reached() {
            return Ok(());
        }
        if self.visited >= self.limits.max_entries_per_root {
            if !directory_is_empty(directory)? {
                self.mark_incomplete(AvailabilityStatus::Unsupported);
            }
            return Ok(());
        }
        // Keeping ancestor handles open without delete sharing prevents path swaps
        // from redirecting enumeration through a newly-created reparse point.
        let _guard = open_guarded_directory(directory)?;
        let capacity = self
            .limits
            .max_entries_per_root
            .saturating_sub(self.visited);
        let mut entries = BTreeMap::<PathSortKey, PathBuf>::new();
        let mut observed = 0;
        for entry in fs::read_dir(directory)? {
            if observed >= capacity {
                self.mark_incomplete(AvailabilityStatus::Unsupported);
                break;
            }
            observed += 1;
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) if is_disappearance(&error) => {
                    self.mark_incomplete(AvailabilityStatus::Locked);
                    continue;
                }
                Err(error) => return Err(ScanError::Io(error)),
            };
            let path = entry.path();
            let relative = match path.strip_prefix(self.root) {
                Ok(relative) => relative,
                Err(_) => continue,
            };
            entries.insert(path_sort_key(relative), path);
        }
        self.visited += observed;

        for (_, path) in entries {
            if self.candidate_limit_reached() {
                self.mark_incomplete(AvailabilityStatus::Unsupported);
                break;
            }
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if is_disappearance(&error) => {
                    self.mark_incomplete(AvailabilityStatus::Locked);
                    continue;
                }
                Err(error) => return Err(ScanError::Io(error)),
            };
            if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                continue;
            }
            if metadata.is_dir() {
                // Copilot documents one events.jsonl directly inside each
                // session directory; deeper entries are workspace artifacts.
                // Source: https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference#session-state
                if self.matcher == CandidateMatcher::CopilotCli && depth >= 2 {
                    continue;
                }
                let relative = match path.strip_prefix(self.root) {
                    Ok(relative) => relative,
                    Err(_) => continue,
                };
                if !self.matcher.should_descend(relative) {
                    continue;
                }
                if depth < self.limits.max_depth {
                    match self.walk_directory(&path, depth + 1) {
                        Ok(()) => {}
                        Err(ScanError::Io(error)) if is_disappearance(&error) => {
                            self.mark_incomplete(AvailabilityStatus::Locked);
                        }
                        Err(error) => {
                            let status = availability_from_scan_error(&error);
                            self.mark_incomplete(if status == AvailabilityStatus::Missing {
                                AvailabilityStatus::Locked
                            } else {
                                status
                            });
                        }
                    }
                } else {
                    match directory_is_empty(&path) {
                        Ok(false) => self.mark_incomplete(AvailabilityStatus::Unsupported),
                        Ok(true) => {}
                        Err(error) => {
                            let status = availability_from_scan_error(&error);
                            self.mark_incomplete(if status == AvailabilityStatus::Missing {
                                AvailabilityStatus::Locked
                            } else {
                                status
                            });
                        }
                    }
                }
                continue;
            }
            if !metadata.is_file() {
                continue;
            }

            let relative_path = match path.strip_prefix(self.root) {
                Ok(relative) => relative.to_path_buf(),
                Err(_) => continue,
            };
            let Some(format) = self.matcher.format_for(&relative_path) else {
                continue;
            };
            let opened = match self.local_root.open_file(&path) {
                Ok(opened) => opened,
                Err(SnapshotError::Io(error)) if is_disappearance(&error) => {
                    self.mark_incomplete(AvailabilityStatus::Locked);
                    continue;
                }
                Err(error) => {
                    let scan_error = ScanError::Snapshot(error);
                    let status = availability_from_scan_error(&scan_error);
                    self.mark_incomplete(if status == AvailabilityStatus::Missing {
                        AvailabilityStatus::Locked
                    } else {
                        status
                    });
                    continue;
                }
            };
            let modified_unix_ms = opened
                .file
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .and_then(|duration| u64::try_from(duration.as_millis()).ok());
            self.candidates.push(CandidateMetadata {
                absolute_path: opened.canonical_path,
                relative_path,
                identity: (opened.stamp.volume_serial, opened.stamp.file_id),
                modified_unix_ms,
                size_bytes: opened.stamp.length,
                format,
            });
        }
        Ok(())
    }

    fn candidate_limit_reached(&self) -> bool {
        self.candidates.len() >= self.candidate_limit
    }

    fn mark_incomplete(&mut self, status: AvailabilityStatus) {
        if self.incomplete_status.is_none() {
            self.incomplete_status = Some(status);
        }
    }
}

fn open_guarded_directory(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    let directory = options
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .security_qos_flags(SECURITY_IDENTIFICATION)
        .open(path)?;
    let metadata = directory.metadata()?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "directory is not an ordinary directory",
        ));
    }
    Ok(directory)
}

impl CandidateMatcher {
    fn should_descend(self, relative_directory: &Path) -> bool {
        match self {
            Self::VsCodeSessionDirectory => false,
            Self::VsCodeWorkspace => {
                let components = relative_directory.components().collect::<Vec<_>>();
                match components.as_slice() {
                    [Component::Normal(workspace_id)] => !workspace_id.is_empty(),
                    [Component::Normal(workspace_id), Component::Normal(folder)] => {
                        !workspace_id.is_empty()
                            && folder
                                .to_string_lossy()
                                .eq_ignore_ascii_case("chatSessions")
                    }
                    _ => false,
                }
            }
            Self::ClaudeProject | Self::Codex | Self::CopilotCli => true,
        }
    }

    fn format_for(self, relative_path: &Path) -> Option<CandidateFormat> {
        let file_name = relative_path.file_name()?.to_string_lossy();
        let lower_name = file_name.to_ascii_lowercase();
        match self {
            Self::ClaudeProject if lower_name.ends_with(".jsonl") => {
                Some(CandidateFormat::JsonLines)
            }
            Self::Codex if lower_name.ends_with(".jsonl.zst") => {
                Some(CandidateFormat::ZstdJsonLines)
            }
            Self::Codex if lower_name.ends_with(".jsonl") => Some(CandidateFormat::JsonLines),
            Self::CopilotCli if is_copilot_session_event_log(relative_path) => {
                Some(CandidateFormat::JsonLines)
            }
            Self::VsCodeWorkspace
                if is_vscode_workspace_session(relative_path) && lower_name.ends_with(".jsonl") =>
            {
                Some(CandidateFormat::JsonLines)
            }
            Self::VsCodeWorkspace
                if is_vscode_workspace_session(relative_path) && lower_name.ends_with(".json") =>
            {
                Some(CandidateFormat::Json)
            }
            Self::VsCodeSessionDirectory
                if is_direct_vscode_session(relative_path) && lower_name.ends_with(".jsonl") =>
            {
                Some(CandidateFormat::JsonLines)
            }
            Self::VsCodeSessionDirectory
                if is_direct_vscode_session(relative_path) && lower_name.ends_with(".json") =>
            {
                Some(CandidateFormat::Json)
            }
            _ => None,
        }
    }
}

fn is_copilot_session_event_log(path: &Path) -> bool {
    let mut components = path.components();
    let Some(Component::Normal(session_id)) = components.next() else {
        return false;
    };
    let Some(Component::Normal(file_name)) = components.next() else {
        return false;
    };

    !session_id.is_empty()
        && file_name
            .to_string_lossy()
            .eq_ignore_ascii_case("events.jsonl")
        && components.next().is_none()
}

fn is_vscode_workspace_session(path: &Path) -> bool {
    let mut components = path.components();
    let Some(Component::Normal(workspace_id)) = components.next() else {
        return false;
    };
    let Some(Component::Normal(chat_sessions)) = components.next() else {
        return false;
    };
    let Some(Component::Normal(_file_name)) = components.next() else {
        return false;
    };
    !workspace_id.is_empty()
        && chat_sessions
            .to_string_lossy()
            .eq_ignore_ascii_case("chatSessions")
        && components.next().is_none()
}

fn is_direct_vscode_session(path: &Path) -> bool {
    matches!(
        path.components().collect::<Vec<_>>().as_slice(),
        [Component::Normal(file_name)] if !file_name.is_empty()
    )
}

fn path_sort_key(path: &Path) -> PathSortKey {
    let original: Vec<u16> = path.as_os_str().encode_wide().collect();
    let folded: Vec<u16> = path
        .as_os_str()
        .to_string_lossy()
        .to_lowercase()
        .encode_utf16()
        .collect();
    (folded, original)
}

fn safe_title(path: &Path) -> String {
    let file_name = safe_component(path.file_name().unwrap_or_default());
    for suffix in [".jsonl.zst", ".jsonl", ".json"] {
        if file_name.to_ascii_lowercase().ends_with(suffix) {
            return file_name[..file_name.len() - suffix.len()].to_owned();
        }
    }
    file_name
}

fn safe_component(value: &std::ffi::OsStr) -> String {
    let safe: String = value
        .to_string_lossy()
        .chars()
        .filter(|character| !character.is_control())
        .take(160)
        .collect();
    if safe.is_empty() {
        "(unnamed)".to_owned()
    } else {
        safe
    }
}

fn availability_from_root_error(path: &Path, error: &SnapshotError) -> AvailabilityStatus {
    if matches!(
        path.components().next(),
        Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::UNC(_, _) | Prefix::VerbatimUNC(_, _))
    ) {
        return AvailabilityStatus::RemoteOnly;
    }
    match error {
        SnapshotError::NonLocalVolume => AvailabilityStatus::RemoteOnly,
        SnapshotError::Io(error) => availability_from_io_error(error),
        SnapshotError::InvalidPath | SnapshotError::OutsideRoot | SnapshotError::NotFile => {
            AvailabilityStatus::Unsupported
        }
        _ => AvailabilityStatus::Denied,
    }
}

fn availability_from_io_error(error: &io::Error) -> AvailabilityStatus {
    if matches!(error.raw_os_error(), Some(32 | 33)) {
        return AvailabilityStatus::Locked;
    }
    match error.kind() {
        io::ErrorKind::NotFound => AvailabilityStatus::Missing,
        io::ErrorKind::PermissionDenied => AvailabilityStatus::Denied,
        io::ErrorKind::Unsupported => AvailabilityStatus::Unsupported,
        _ => AvailabilityStatus::Denied,
    }
}
