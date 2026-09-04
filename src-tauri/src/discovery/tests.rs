use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, LOCKFILE_EXCLUSIVE_LOCK,
    LOCKFILE_FAIL_IMMEDIATELY, LockFileEx, UnlockFileEx,
};
use windows_sys::Win32::System::IO::OVERLAPPED;

use super::catalog::{
    AvailabilityStatus, CandidateFormat, CatalogId, CatalogIdGenerator, DiscoveryCatalog,
    DiscoveryLimits, DiscoverySource, ScannedCollection, WindowsCatalogIdGenerator,
};
use super::roots::{
    CandidateMatcher, FixtureRootError, FixtureRootProvider, ProductionRootProvider, RootProvider,
    RootSpec,
};

struct TempTree(PathBuf);

impl TempTree {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should follow the epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "ai-session-discovery-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary fixture should be created");
        Self(path)
    }

    fn write(&self, relative: &str, bytes: &[u8]) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().expect("fixture file should have a parent"))
            .expect("fixture directory should be created");
        fs::write(path, bytes).expect("fixture file should be written");
    }
}

struct BodyReadGuard {
    file: File,
    length: u64,
}

impl BodyReadGuard {
    fn lock(path: &Path) -> Self {
        let file = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(path)
            .expect("fixture should open for locking");
        let length = file
            .metadata()
            .expect("fixture metadata should be readable")
            .len();
        let mut overlapped = OVERLAPPED::default();
        // SAFETY: the handle is live and `overlapped` is writable for the synchronous lock.
        let locked = unsafe {
            LockFileEx(
                file.as_raw_handle() as HANDLE,
                LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                0,
                length as u32,
                (length >> 32) as u32,
                &raw mut overlapped,
            )
        };
        assert_ne!(locked, 0, "fixture body lock should succeed");
        Self { file, length }
    }
}

impl Drop for BodyReadGuard {
    fn drop(&mut self) {
        let mut overlapped = OVERLAPPED::default();
        // SAFETY: the handle remains live until this destructor returns.
        unsafe {
            UnlockFileEx(
                self.file.as_raw_handle() as HANDLE,
                0,
                self.length as u32,
                (self.length >> 32) as u32,
                &raw mut overlapped,
            );
        }
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct DeterministicIds;

impl CatalogIdGenerator for DeterministicIds {
    fn generate(&self, sequence: u64) -> CatalogId {
        CatalogId::new(format!("test-{sequence:04}"))
    }
}

fn fixture_provider(tree: &TempTree) -> FixtureRootProvider {
    FixtureRootProvider::new(&tree.0).expect("fixture root should be accepted")
}

fn discover(
    provider: &FixtureRootProvider,
    limits: DiscoveryLimits,
) -> super::catalog::CatalogSnapshot {
    let mut catalog = DiscoveryCatalog::new(DeterministicIds);
    catalog.refresh(provider, limits)
}

#[test]
fn fixture_provider_rejects_roots_outside_its_base() {
    let tree = TempTree::new("escape");
    let mut provider = fixture_provider(&tree);

    let parent_escape = provider.add_root(
        DiscoverySource::ClaudeCode,
        "projects",
        Path::new(r"..\outside"),
        CandidateMatcher::ClaudeProject,
    );
    let absolute_escape = provider.add_root(
        DiscoverySource::ClaudeCode,
        "projects",
        &tree.0,
        CandidateMatcher::ClaudeProject,
    );

    assert_eq!(parent_escape, Err(FixtureRootError::OutsideFixture));
    assert_eq!(absolute_escape, Err(FixtureRootError::OutsideFixture));
}

#[test]
fn fixture_provider_rejects_a_reparse_root_that_escapes_its_base() {
    let tree = TempTree::new("root-link");
    let outside = TempTree::new("root-link-outside");
    let link = tree.0.join("linked-root");
    if let Err(error) = std::os::windows::fs::symlink_dir(&outside.0, &link) {
        if error.kind() == std::io::ErrorKind::PermissionDenied
            || error.raw_os_error() == Some(1_314)
        {
            return;
        }
        panic!("directory link should be created: {error}");
    }
    let mut provider = fixture_provider(&tree);

    let result = provider.add_root(
        DiscoverySource::Codex,
        "active",
        Path::new("linked-root"),
        CandidateMatcher::Codex,
    );

    assert_eq!(result, Err(FixtureRootError::OutsideFixture));
}

#[test]
fn production_provider_declares_vscode_stable_and_insiders_chat_roots() {
    let provider =
        ProductionRootProvider::with_environment_paths(PathBuf::from(r"C:\Users\fixture-user"));
    let roots = provider.roots();
    let local_paths: Vec<_> = roots
        .iter()
        .filter_map(|root| match root {
            RootSpec::Local { path, .. } => Some(path.as_path()),
            RootSpec::Unavailable { .. } => None,
        })
        .collect();
    let read_root_paths: Vec<_> = roots
        .iter()
        .filter_map(|root| match root {
            RootSpec::Local { read_root_path, .. } => Some(read_root_path.as_path()),
            RootSpec::Unavailable { .. } => None,
        })
        .collect();

    assert_eq!(
        local_paths,
        [
            Path::new(r"C:\Users\fixture-user\.claude\projects"),
            Path::new(r"C:\Users\fixture-user\.codex\sessions"),
            Path::new(r"C:\Users\fixture-user\.codex\archived_sessions"),
            Path::new(r"C:\Users\fixture-user\.copilot\session-state"),
            Path::new(r"C:\Users\fixture-user\AppData\Roaming\Code\User\workspaceStorage"),
            Path::new(
                r"C:\Users\fixture-user\AppData\Roaming\Code\User\globalStorage\emptyWindowChatSessions"
            ),
            Path::new(
                r"C:\Users\fixture-user\AppData\Roaming\Code - Insiders\User\workspaceStorage"
            ),
            Path::new(
                r"C:\Users\fixture-user\AppData\Roaming\Code - Insiders\User\globalStorage\emptyWindowChatSessions"
            ),
        ]
    );
    assert_eq!(
        read_root_paths,
        [
            Path::new(r"C:\Users\fixture-user\.claude\projects"),
            Path::new(r"C:\Users\fixture-user\.codex\sessions"),
            Path::new(r"C:\Users\fixture-user\.codex\archived_sessions"),
            Path::new(r"C:\Users\fixture-user\.copilot"),
            Path::new(r"C:\Users\fixture-user\AppData\Roaming\Code\User\workspaceStorage"),
            Path::new(
                r"C:\Users\fixture-user\AppData\Roaming\Code\User\globalStorage\emptyWindowChatSessions"
            ),
            Path::new(
                r"C:\Users\fixture-user\AppData\Roaming\Code - Insiders\User\workspaceStorage"
            ),
            Path::new(
                r"C:\Users\fixture-user\AppData\Roaming\Code - Insiders\User\globalStorage\emptyWindowChatSessions"
            ),
        ]
    );
    assert!(roots.iter().all(|root| matches!(
        root,
        RootSpec::Local {
            source: DiscoverySource::ClaudeCode
                | DiscoverySource::Codex
                | DiscoverySource::CopilotCli
                | DiscoverySource::VscodeCopilot,
            ..
        }
    )));
}

#[test]
fn production_provider_honors_vendor_data_root_overrides() {
    let provider =
        ProductionRootProvider::with_environment_paths(PathBuf::from(r"C:\Users\fixture-user"))
            .with_vendor_overrides(
                PathBuf::from(r"D:\Claude"),
                PathBuf::from(r"E:\Codex"),
                PathBuf::from(r"F:\Copilot"),
            );
    let roots = provider.roots();
    let local_paths: Vec<_> = roots
        .iter()
        .filter_map(|root| match root {
            RootSpec::Local { path, .. } => Some(path.as_path()),
            RootSpec::Unavailable { .. } => None,
        })
        .collect();
    let read_root_paths: Vec<_> = roots
        .iter()
        .filter_map(|root| match root {
            RootSpec::Local { read_root_path, .. } => Some(read_root_path.as_path()),
            RootSpec::Unavailable { .. } => None,
        })
        .collect();

    assert_eq!(local_paths[0], Path::new(r"D:\Claude\projects"));
    assert_eq!(local_paths[1], Path::new(r"E:\Codex\sessions"));
    assert_eq!(local_paths[2], Path::new(r"E:\Codex\archived_sessions"));
    assert_eq!(local_paths[3], Path::new(r"F:\Copilot\session-state"));
    assert_eq!(read_root_paths[3], Path::new(r"F:\Copilot"));
}

#[test]
fn production_provider_discovers_copilot_sessions_from_its_broader_read_scope() {
    let tree = TempTree::new("production-copilot-scan");
    tree.write(
        r"copilot\session-state\session\events.jsonl",
        b"synthetic copilot session",
    );
    tree.write(r"copilot\settings.json", b"{}");
    let provider = ProductionRootProvider::with_environment_paths(&tree.0).with_vendor_overrides(
        tree.0.join("claude"),
        tree.0.join("codex"),
        tree.0.join("copilot"),
    );
    let mut catalog = DiscoveryCatalog::new(DeterministicIds);

    let snapshot = catalog.refresh(&provider, DiscoveryLimits::default());
    let copilot_entries = snapshot
        .entries
        .iter()
        .filter(|entry| entry.source == DiscoverySource::CopilotCli)
        .count();
    let copilot_diagnostics = snapshot
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.source == DiscoverySource::CopilotCli)
        .count();

    assert_eq!(copilot_entries, 1);
    assert_eq!(copilot_diagnostics, 0);
}

#[test]
fn copilot_read_scope_does_not_expand_or_hide_the_session_scan_root() {
    let available = TempTree::new("copilot-scan-scope");
    available.write(r"copilot\session-state\session\events.jsonl", b"copilot");
    available.write(r"copilot\settings.json", b"{}");
    available.write(r"copilot\unrelated\one.json", b"ignored");
    available.write(r"copilot\unrelated\two.json", b"ignored");
    let mut available_provider = fixture_provider(&available);
    available_provider
        .add_root_with_read_scope(
            DiscoverySource::CopilotCli,
            "sessions",
            Path::new("copilot/session-state"),
            Path::new("copilot"),
            CandidateMatcher::CopilotCli,
        )
        .expect("Copilot roots should be accepted");

    let available_snapshot = discover(
        &available_provider,
        DiscoveryLimits {
            max_entries_per_root: 2,
            ..DiscoveryLimits::default()
        },
    );
    assert_eq!(available_snapshot.entries.len(), 1);
    assert!(available_snapshot.diagnostics.is_empty());

    let missing = TempTree::new("copilot-missing-sessions");
    missing.write(r"copilot\settings.json", b"{}");
    let mut missing_provider = fixture_provider(&missing);
    missing_provider
        .add_root_with_read_scope(
            DiscoverySource::CopilotCli,
            "sessions",
            Path::new("copilot/session-state"),
            Path::new("copilot"),
            CandidateMatcher::CopilotCli,
        )
        .expect("Copilot roots should be accepted");

    let missing_snapshot = discover(&missing_provider, DiscoveryLimits::default());
    assert!(missing_snapshot.entries.is_empty());
    assert_eq!(
        missing_snapshot.diagnostics,
        [super::catalog::AvailabilityDiagnostic {
            source: DiscoverySource::CopilotCli,
            collection: "sessions".to_owned(),
            status: AvailabilityStatus::Missing,
        }]
    );
}

#[test]
fn copilot_discovery_does_not_descend_into_session_workspace_artifacts() {
    let tree = TempTree::new("copilot-workspace-artifacts");
    tree.write(
        r"copilot\session-state\session\events.jsonl",
        b"synthetic copilot session",
    );
    tree.write(
        r"copilot\session-state\session\workspace\one\two\three\four\five\six\artifact.txt",
        b"irrelevant workspace artifact",
    );
    let mut provider = fixture_provider(&tree);
    provider
        .add_root_with_read_scope(
            DiscoverySource::CopilotCli,
            "sessions",
            Path::new("copilot/session-state"),
            Path::new("copilot"),
            CandidateMatcher::CopilotCli,
        )
        .expect("Copilot roots should be accepted");

    let snapshot = discover(&provider, DiscoveryLimits::default());

    assert_eq!(snapshot.entries.len(), 1);
    assert!(snapshot.diagnostics.is_empty());
    assert_eq!(
        snapshot.scanned_collections,
        [ScannedCollection {
            source: DiscoverySource::CopilotCli,
            collection: "sessions".to_owned(),
        }]
    );
}

#[test]
fn availability_statuses_serialize_to_the_closed_vocabulary() {
    let statuses = [
        AvailabilityStatus::Missing,
        AvailabilityStatus::Disabled,
        AvailabilityStatus::Denied,
        AvailabilityStatus::Locked,
        AvailabilityStatus::Unsupported,
        AvailabilityStatus::RemoteOnly,
    ];

    assert_eq!(
        serde_json::to_value(statuses).expect("statuses should serialize"),
        serde_json::json!([
            "missing",
            "disabled",
            "denied",
            "locked",
            "unsupported",
            "remote-only"
        ])
    );
}

#[test]
fn production_ids_are_opaque_and_unique_within_a_generator() {
    let generator = WindowsCatalogIdGenerator::new().expect("Windows entropy should be available");

    let first = generator.generate(0);
    let second = generator.generate(1);

    assert_ne!(first, second);
    assert_eq!(first.as_str().len(), 49);
    assert!(
        first
            .as_str()
            .chars()
            .all(|character| { character.is_ascii_hexdigit() || character == '-' })
    );
}

#[test]
fn discovers_only_supported_candidate_names() {
    let tree = TempTree::new("supported");
    tree.write(r"claude\project-a\session.jsonl", b"claude");
    tree.write(r"claude\project-a\session.json", b"ignored");
    tree.write(r"codex\rollout.jsonl", b"codex");
    tree.write(r"codex\rollout.JSONL.ZST", b"codex-zstd");
    tree.write(r"codex\rollout.zst", b"ignored");
    tree.write(r"copilot\session-state\session\events.jsonl", b"copilot");
    tree.write(r"copilot\session-state\session\other.jsonl", b"ignored");
    tree.write(r"copilot\events.jsonl", b"ignored");
    tree.write(r"copilot\other\events.jsonl", b"ignored");
    tree.write(r"vscode\workspace\chatSessions\one.json", b"vscode");
    tree.write(r"vscode\workspace\chatSessions\two.jsonl", b"vscode");
    tree.write(
        r"vscode\workspace\chatSessions\nested\ignored.jsonl",
        b"ignored",
    );
    tree.write(r"vscode\workspace\outside.json", b"ignored");
    tree.write(r"vscode-empty\three.jsonl", b"vscode");
    tree.write(r"vscode-empty\nested\ignored.json", b"ignored");

    let mut provider = fixture_provider(&tree);
    provider
        .add_root(
            DiscoverySource::ClaudeCode,
            "projects",
            Path::new("claude"),
            CandidateMatcher::ClaudeProject,
        )
        .expect("Claude root should be accepted");
    provider
        .add_root(
            DiscoverySource::Codex,
            "active",
            Path::new("codex"),
            CandidateMatcher::Codex,
        )
        .expect("Codex root should be accepted");
    provider
        .add_root_with_read_scope(
            DiscoverySource::CopilotCli,
            "sessions",
            Path::new("copilot/session-state"),
            Path::new("copilot"),
            CandidateMatcher::CopilotCli,
        )
        .expect("Copilot root should be accepted");
    provider
        .add_root(
            DiscoverySource::VscodeCopilot,
            "chatSessions",
            Path::new("vscode"),
            CandidateMatcher::VsCodeWorkspace,
        )
        .expect("VS Code root should be accepted");
    provider
        .add_root(
            DiscoverySource::VscodeCopilot,
            "stable-empty-window",
            Path::new("vscode-empty"),
            CandidateMatcher::VsCodeSessionDirectory,
        )
        .expect("VS Code empty-window root should be accepted");

    let snapshot = discover(&provider, DiscoveryLimits::default());
    let names: Vec<_> = snapshot
        .entries
        .iter()
        .map(|entry| entry.display.file_name.as_str())
        .collect();

    assert_eq!(
        names,
        [
            "session.jsonl",
            "rollout.jsonl",
            "events.jsonl",
            "one.json",
            "two.jsonl",
            "three.jsonl"
        ]
    );
}

#[test]
fn vscode_discovery_prefers_current_logs_over_legacy_json_siblings() {
    let tree = TempTree::new("vscode-format-precedence");
    tree.write(r"stable\workspace\chatSessions\same.json", b"legacy");
    tree.write(r"stable\workspace\chatSessions\same.jsonl", b"current");
    tree.write(r"insiders\workspace\chatSessions\insiders.json", b"legacy");

    let mut provider = fixture_provider(&tree);
    provider
        .add_root(
            DiscoverySource::VscodeCopilot,
            "stable-workspaces",
            Path::new("stable"),
            CandidateMatcher::VsCodeWorkspace,
        )
        .expect("VS Code Stable root should be accepted");
    provider
        .add_root(
            DiscoverySource::VscodeCopilot,
            "insiders-workspaces",
            Path::new("insiders"),
            CandidateMatcher::VsCodeWorkspace,
        )
        .expect("VS Code Insiders root should be accepted");

    let snapshot = discover(&provider, DiscoveryLimits::default());
    assert_eq!(
        snapshot
            .entries
            .iter()
            .map(|entry| (
                entry.display.collection.as_str(),
                entry.display.file_name.as_str()
            ))
            .collect::<Vec<_>>(),
        [
            ("insiders-workspaces", "insiders.json"),
            ("stable-workspaces", "same.jsonl"),
        ]
    );
}

#[test]
fn candidate_order_is_stable_across_creation_order() {
    let first = TempTree::new("order-a");
    first.write(r"sessions\z.jsonl", b"z");
    first.write(r"sessions\a.jsonl", b"a");
    first.write(r"sessions\m.jsonl", b"m");
    let second = TempTree::new("order-b");
    second.write(r"sessions\m.jsonl", b"m");
    second.write(r"sessions\a.jsonl", b"a");
    second.write(r"sessions\z.jsonl", b"z");

    let paths = |tree: &TempTree| {
        let mut provider = fixture_provider(tree);
        provider
            .add_root(
                DiscoverySource::Codex,
                "active",
                Path::new("sessions"),
                CandidateMatcher::Codex,
            )
            .expect("fixture root should be accepted");
        discover(&provider, DiscoveryLimits::default())
            .entries
            .into_iter()
            .map(|entry| entry.display.file_name)
            .collect::<Vec<_>>()
    };

    assert_eq!(paths(&first), paths(&second));
    assert_eq!(
        paths(&first),
        ["a.jsonl", "m.jsonl", "z.jsonl"].map(str::to_owned)
    );
}

#[test]
fn enforces_candidate_traversal_and_total_caps() {
    let tree = TempTree::new("limits");
    for name in ["a", "b", "c", "d"] {
        tree.write(&format!(r"one\{name}.jsonl"), name.as_bytes());
        tree.write(&format!(r"two\{name}.jsonl"), name.as_bytes());
    }

    let mut provider = fixture_provider(&tree);
    for (collection, root) in [("one", "one"), ("two", "two")] {
        provider
            .add_root(
                DiscoverySource::Codex,
                collection,
                Path::new(root),
                CandidateMatcher::Codex,
            )
            .expect("fixture root should be accepted");
    }

    let candidate_limited = discover(
        &provider,
        DiscoveryLimits {
            max_candidates_per_root: 2,
            max_total_candidates: 3,
            ..DiscoveryLimits::default()
        },
    );
    let traversal_limited = discover(
        &provider,
        DiscoveryLimits {
            max_entries_per_root: 2,
            ..DiscoveryLimits::default()
        },
    );

    assert_eq!(candidate_limited.entries.len(), 3);
    assert_eq!(traversal_limited.entries.len(), 4);
    assert!(
        traversal_limited
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.status == AvailabilityStatus::Unsupported)
    );
}

#[test]
fn entry_limit_retains_a_bounded_non_authoritative_subset() {
    let first = TempTree::new("limit-order-a");
    first.write(r"sessions\z.jsonl", b"z");
    first.write(r"sessions\a.jsonl", b"a");
    first.write(r"sessions\m.jsonl", b"m");
    let second = TempTree::new("limit-order-b");
    second.write(r"sessions\m.jsonl", b"m");
    second.write(r"sessions\a.jsonl", b"a");
    second.write(r"sessions\z.jsonl", b"z");

    let discover_limited = |tree: &TempTree| {
        let mut provider = fixture_provider(tree);
        provider
            .add_root(
                DiscoverySource::Codex,
                "active",
                Path::new("sessions"),
                CandidateMatcher::Codex,
            )
            .expect("fixture root should be accepted");
        discover(
            &provider,
            DiscoveryLimits {
                max_entries_per_root: 2,
                ..DiscoveryLimits::default()
            },
        )
    };

    let first_snapshot = discover_limited(&first);
    let second_snapshot = discover_limited(&second);
    assert_eq!(first_snapshot.entries.len(), 2);
    assert_eq!(second_snapshot.entries.len(), 2);
    assert_eq!(
        first_snapshot.diagnostics[0].status,
        AvailabilityStatus::Unsupported
    );
}

#[test]
fn exact_entry_limit_still_processes_every_buffered_candidate() {
    let tree = TempTree::new("exact-entry-limit");
    tree.write(r"sessions\b.jsonl", b"b");
    tree.write(r"sessions\a.jsonl", b"a");
    let mut provider = fixture_provider(&tree);
    provider
        .add_root(
            DiscoverySource::Codex,
            "active",
            Path::new("sessions"),
            CandidateMatcher::Codex,
        )
        .expect("fixture root should be accepted");

    let snapshot = discover(
        &provider,
        DiscoveryLimits {
            max_entries_per_root: 2,
            ..DiscoveryLimits::default()
        },
    );

    assert_eq!(
        snapshot
            .entries
            .iter()
            .map(|entry| entry.display.file_name.as_str())
            .collect::<Vec<_>>(),
        ["a.jsonl", "b.jsonl"]
    );
    assert!(snapshot.diagnostics.is_empty());
}

#[test]
fn exact_entry_limit_rejects_a_nonempty_unvisited_subtree() {
    let tree = TempTree::new("exact-nested-entry-limit");
    tree.write(r"sessions\nested\session.jsonl", b"nested");
    let mut provider = fixture_provider(&tree);
    provider
        .add_root(
            DiscoverySource::Codex,
            "active",
            Path::new("sessions"),
            CandidateMatcher::Codex,
        )
        .expect("fixture root should be accepted");

    let snapshot = discover(
        &provider,
        DiscoveryLimits {
            max_entries_per_root: 1,
            ..DiscoveryLimits::default()
        },
    );

    assert!(snapshot.entries.is_empty());
    assert_eq!(snapshot.diagnostics.len(), 1);
    assert_eq!(
        snapshot.diagnostics[0].status,
        AvailabilityStatus::Unsupported
    );
}

#[test]
fn enforces_the_traversal_depth_cap() {
    let tree = TempTree::new("depth");
    tree.write(r"sessions\root.jsonl", b"root");
    tree.write(r"sessions\one\nested.jsonl", b"nested");
    let mut provider = fixture_provider(&tree);
    provider
        .add_root(
            DiscoverySource::Codex,
            "active",
            Path::new("sessions"),
            CandidateMatcher::Codex,
        )
        .expect("fixture root should be accepted");

    let snapshot = discover(
        &provider,
        DiscoveryLimits {
            max_depth: 1,
            ..DiscoveryLimits::default()
        },
    );

    assert_eq!(snapshot.entries.len(), 1);
    assert_eq!(snapshot.entries[0].display.file_name, "root.jsonl");
}

#[test]
fn reports_a_missing_root_without_exposing_its_path() {
    let tree = TempTree::new("missing");
    let mut provider = fixture_provider(&tree);
    provider
        .add_root(
            DiscoverySource::ClaudeCode,
            "projects",
            Path::new("not-created"),
            CandidateMatcher::ClaudeProject,
        )
        .expect("missing in-fixture root should be accepted");

    let snapshot = discover(&provider, DiscoveryLimits::default());

    assert!(snapshot.entries.is_empty());
    assert_eq!(snapshot.diagnostics.len(), 1);
    assert_eq!(snapshot.diagnostics[0].status, AvailabilityStatus::Missing);
    assert_eq!(
        snapshot.scanned_collections,
        [ScannedCollection {
            source: DiscoverySource::ClaudeCode,
            collection: "projects".to_owned(),
        }]
    );
}

#[test]
fn serialized_output_contains_neither_absolute_paths_nor_transcript_bodies() {
    let tree = TempTree::new("serialization");
    let transcript_body = "PRIVATE_TRANSCRIPT_BODY_SENTINEL";
    tree.write(r"sessions\private.jsonl", transcript_body.as_bytes());
    let mut provider = fixture_provider(&tree);
    provider
        .add_root(
            DiscoverySource::Codex,
            "active",
            Path::new("sessions"),
            CandidateMatcher::Codex,
        )
        .expect("fixture root should be accepted");

    let snapshot = discover(&provider, DiscoveryLimits::default());
    let value = serde_json::to_value(&snapshot).expect("catalog output should serialize");
    let serialized = serde_json::to_string(&snapshot).expect("catalog output should serialize");
    let fixture_path = tree.0.to_string_lossy();

    assert!(value["entries"][0]["display"].get("relativePath").is_none());

    fn assert_strings_are_path_free(value: &serde_json::Value, fixture_path: &str) {
        match value {
            serde_json::Value::Array(values) => {
                for value in values {
                    assert_strings_are_path_free(value, fixture_path);
                }
            }
            serde_json::Value::Object(values) => {
                for value in values.values() {
                    assert_strings_are_path_free(value, fixture_path);
                }
            }
            serde_json::Value::String(value) => assert!(!value.contains(fixture_path)),
            _ => {}
        }
    }

    assert_strings_are_path_free(&value, &fixture_path);
    assert!(!serialized.contains(transcript_body));
}

#[test]
fn discovery_succeeds_while_transcript_body_reads_are_exclusively_locked() {
    let tree = TempTree::new("metadata-only");
    let relative = Path::new(r"sessions\private.jsonl");
    let absolute = tree.0.join(relative);
    tree.write(
        relative
            .to_str()
            .expect("fixture relative path should be Unicode"),
        b"PRIVATE_TRANSCRIPT_BODY_SENTINEL",
    );
    let _body_read_guard = BodyReadGuard::lock(&absolute);
    let mut provider = fixture_provider(&tree);
    provider
        .add_root(
            DiscoverySource::Codex,
            "active",
            Path::new("sessions"),
            CandidateMatcher::Codex,
        )
        .expect("fixture root should be accepted");

    let snapshot = discover(&provider, DiscoveryLimits::default());

    assert_eq!(snapshot.entries.len(), 1);
    let mut bytes = Vec::new();
    let read_error = File::open(&absolute)
        .expect("a second handle should open")
        .read_to_end(&mut bytes)
        .expect_err("the fixture lock should reject body reads");
    assert_eq!(read_error.raw_os_error(), Some(33));
}

#[test]
fn reports_a_candidate_that_cannot_be_opened_as_locked() {
    let tree = TempTree::new("locked-candidate");
    let relative = Path::new(r"sessions\locked.jsonl");
    let absolute = tree.0.join(relative);
    tree.write(
        relative
            .to_str()
            .expect("fixture relative path should be Unicode"),
        b"locked",
    );
    let _exclusive_handle = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&absolute)
        .expect("fixture should open without sharing");
    let mut provider = fixture_provider(&tree);
    provider
        .add_root(
            DiscoverySource::Codex,
            "active",
            Path::new("sessions"),
            CandidateMatcher::Codex,
        )
        .expect("fixture root should be accepted");

    let snapshot = discover(&provider, DiscoveryLimits::default());

    assert!(snapshot.entries.is_empty());
    assert_eq!(snapshot.diagnostics.len(), 1);
    assert_eq!(snapshot.diagnostics[0].status, AvailabilityStatus::Locked);
}

#[test]
fn one_locked_candidate_does_not_hide_readable_siblings_or_mark_the_scan_authoritative() {
    let tree = TempTree::new("partially-locked-candidates");
    tree.write(r"sessions\a-readable.jsonl", b"readable");
    let locked_path = tree.0.join(r"sessions\z-locked.jsonl");
    tree.write(r"sessions\z-locked.jsonl", b"locked");
    let _exclusive_handle = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&locked_path)
        .expect("fixture should open without sharing");
    let mut provider = fixture_provider(&tree);
    provider
        .add_root(
            DiscoverySource::Codex,
            "active",
            Path::new("sessions"),
            CandidateMatcher::Codex,
        )
        .expect("fixture root should be accepted");

    let snapshot = discover(&provider, DiscoveryLimits::default());

    assert_eq!(snapshot.entries.len(), 1);
    assert_eq!(snapshot.entries[0].display.file_name, "a-readable.jsonl");
    assert_eq!(snapshot.diagnostics.len(), 1);
    assert_eq!(snapshot.diagnostics[0].status, AvailabilityStatus::Locked);
    assert!(snapshot.scanned_collections.is_empty());
}

#[test]
fn candidate_and_depth_limits_keep_found_sessions_but_are_not_authoritative() {
    let tree = TempTree::new("partial-limits");
    tree.write(r"sessions\a.jsonl", b"a");
    tree.write(r"sessions\b.jsonl", b"b");
    tree.write(r"sessions\nested\c.jsonl", b"c");
    let mut provider = fixture_provider(&tree);
    provider
        .add_root(
            DiscoverySource::Codex,
            "active",
            Path::new("sessions"),
            CandidateMatcher::Codex,
        )
        .expect("fixture root should be accepted");

    let candidate_limited = discover(
        &provider,
        DiscoveryLimits {
            max_candidates_per_root: 1,
            ..DiscoveryLimits::default()
        },
    );
    assert_eq!(candidate_limited.entries.len(), 1);
    assert!(candidate_limited.scanned_collections.is_empty());
    assert_eq!(
        candidate_limited.diagnostics[0].status,
        AvailabilityStatus::Unsupported
    );

    let depth_limited = discover(
        &provider,
        DiscoveryLimits {
            max_depth: 1,
            ..DiscoveryLimits::default()
        },
    );
    assert_eq!(depth_limited.entries.len(), 2);
    assert!(depth_limited.scanned_collections.is_empty());
    assert_eq!(
        depth_limited.diagnostics[0].status,
        AvailabilityStatus::Unsupported
    );
}

#[test]
fn codex_prefers_plain_jsonl_over_sibling_zst() {
    let tree = TempTree::new("codex-sibling");
    tree.write(r"sessions\2026\08\24\abc123.jsonl", b"{}\n");
    tree.write(r"sessions\2026\08\24\abc123.jsonl.zst", b"\x28\xb5\x2f\xfd");

    let mut provider = fixture_provider(&tree);
    provider
        .add_root(
            DiscoverySource::Codex,
            "active",
            Path::new("sessions"),
            CandidateMatcher::Codex,
        )
        .expect("fixture root should be accepted");

    let snapshot = discover(&provider, DiscoveryLimits::default());

    // Only the plain .jsonl should appear
    assert_eq!(snapshot.entries.len(), 1);
    assert_eq!(snapshot.entries[0].display.file_name, "abc123.jsonl");
    assert_eq!(snapshot.entries[0].source, DiscoverySource::Codex);
}

#[test]
fn codex_keeps_zst_when_no_plain_sibling_exists() {
    let tree = TempTree::new("codex-zst-only");
    tree.write(r"sessions\2026\08\24\abc456.jsonl.zst", b"\x28\xb5\x2f\xfd");

    let mut provider = fixture_provider(&tree);
    provider
        .add_root(
            DiscoverySource::Codex,
            "active",
            Path::new("sessions"),
            CandidateMatcher::Codex,
        )
        .expect("fixture root should be accepted");

    let snapshot = discover(&provider, DiscoveryLimits::default());

    assert_eq!(snapshot.entries.len(), 1);
    assert_eq!(snapshot.entries[0].display.file_name, "abc456.jsonl.zst");
}

#[test]
fn opaque_ids_resolve_only_through_private_catalog_records() {
    let tree = TempTree::new("resolve");
    tree.write(r"sessions\rollout.jsonl.zst", b"compressed");
    let mut provider = fixture_provider(&tree);
    provider
        .add_root(
            DiscoverySource::Codex,
            "archived",
            Path::new("sessions"),
            CandidateMatcher::Codex,
        )
        .expect("fixture root should be accepted");
    let mut catalog = DiscoveryCatalog::new(DeterministicIds);

    let snapshot = catalog.refresh(&provider, DiscoveryLimits::default());
    let resolved = catalog
        .resolve(snapshot.entries[0].id.as_str())
        .expect("catalog ID should resolve");

    assert_eq!(resolved.source, DiscoverySource::Codex);
    assert_eq!(resolved.format, CandidateFormat::ZstdJsonLines);
    assert!(
        resolved
            .candidate_path
            .ends_with(r"sessions\rollout.jsonl.zst")
    );
    let reopened = resolved
        .root
        .open_file(resolved.candidate_path)
        .expect("catalog candidate should remain safe to open");
    assert_eq!(
        resolved.expected_identity,
        (reopened.stamp.volume_serial, reopened.stamp.file_id)
    );
    let metadata = catalog
        .private_metadata(snapshot.entries[0].id.as_str())
        .expect("private source metadata should resolve");
    assert_eq!(metadata.source, DiscoverySource::Codex);
    assert_eq!(metadata.collection, "archived");
    assert_eq!(metadata.file_size_bytes, reopened.stamp.length);
    assert_eq!(metadata.file_identity, resolved.expected_identity);
    assert_eq!(metadata.canonical_path, resolved.candidate_path);
    assert_ne!(metadata.root_identity, metadata.file_identity);
    assert!(catalog.resolve("unknown-id").is_none());
    assert!(catalog.private_metadata("unknown-id").is_none());
}

#[test]
fn skips_reparse_directories_without_requiring_link_privileges() {
    let tree = TempTree::new("reparse");
    let outside = TempTree::new("outside");
    outside.write("escaped.jsonl", b"outside");
    let link = tree.0.join("sessions").join("linked");
    fs::create_dir_all(link.parent().expect("link should have a parent"))
        .expect("fixture directory should be created");
    if let Err(error) = std::os::windows::fs::symlink_dir(&outside.0, &link) {
        if error.kind() == std::io::ErrorKind::PermissionDenied
            || error.raw_os_error() == Some(1_314)
        {
            return;
        }
        panic!("directory link should be created: {error}");
    }
    let mut provider = fixture_provider(&tree);
    provider
        .add_root(
            DiscoverySource::Codex,
            "active",
            Path::new("sessions"),
            CandidateMatcher::Codex,
        )
        .expect("fixture root should be accepted");

    let snapshot = discover(&provider, DiscoveryLimits::default());

    assert!(snapshot.entries.is_empty());
}
