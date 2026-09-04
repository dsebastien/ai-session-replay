use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::discovery::catalog::{
    CatalogId, CatalogIdGenerator, DiscoveryCatalog, DiscoveryLimits, DiscoverySource,
};
use crate::discovery::load::load_catalog_session_v2;
use crate::discovery::roots::{CandidateMatcher, FixtureRootProvider};
use crate::model::SessionSource;

struct DeterministicIds;

impl CatalogIdGenerator for DeterministicIds {
    fn generate(&self, sequence: u64) -> CatalogId {
        CatalogId::new(format!("integration-{sequence}"))
    }
}

struct TempTree(PathBuf);

impl TempTree {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should follow the epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "ai-session-integration-test-{}-{nonce}",
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
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn discovers_and_loads_all_sources_through_opaque_ids() {
    let tree = TempTree::new();
    tree.copy_fixture("codex-current.jsonl", r"codex\rollout.jsonl");
    tree.copy_fixture(
        "claude-current.jsonl",
        r"claude\project-a\claude-session-1.jsonl",
    );
    tree.copy_fixture(
        "copilot-cli-events.jsonl",
        r"copilot\session-state\copilot-session-1\events.jsonl",
    );
    tree.copy_fixture("copilot-cli-settings.json", r"copilot\settings.json");
    let settings_path = tree.0.join(r"copilot\settings.json");
    let settings_before = fs::read(&settings_path).expect("settings fixture should be readable");

    let mut provider = FixtureRootProvider::new(&tree.0).expect("fixture root should be valid");
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

    let mut catalog = DiscoveryCatalog::new(DeterministicIds);
    let snapshot = catalog.refresh(&provider, DiscoveryLimits::default());
    assert!(snapshot.diagnostics.is_empty());
    assert_eq!(snapshot.entries.len(), 3);
    assert!(snapshot.entries.iter().all(|entry| {
        entry.id.as_str().starts_with("integration-")
            && !entry.id.as_str().contains("session-state")
            && !entry.id.as_str().contains(".jsonl")
    }));

    let catalog = Arc::new(Mutex::new(catalog));
    let sessions = snapshot
        .entries
        .iter()
        .map(|entry| {
            load_catalog_session_v2(entry.id.as_str(), &catalog)
                .expect("discovered session should load")
        })
        .collect::<Vec<_>>();

    assert_eq!(
        sessions
            .iter()
            .map(|session| (&session.source, session.id.as_str(), session.entries.len()))
            .collect::<Vec<_>>(),
        [
            (&SessionSource::ClaudeCode, "claude-session-1", 4),
            (&SessionSource::Codex, "sess-codex-001", 5),
            (&SessionSource::CopilotCli, "copilot-session-1", 6),
        ]
    );
    assert!(
        sessions[2]
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.code == "copilot-cli-remote-sync-enabled" })
    );
    assert_eq!(
        fs::read(settings_path).expect("settings fixture should remain readable"),
        settings_before
    );
}
