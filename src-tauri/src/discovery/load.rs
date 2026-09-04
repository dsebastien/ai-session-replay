use std::sync::{Arc, Mutex};

use crate::adapters::claude::ClaudeAdapter;
use crate::adapters::codex::{CodexAdapter, extract_codex_session_id};
use crate::adapters::contract::{AdapterContext, run_rich_adapter};
use crate::adapters::copilot_cli::CopilotCliAdapter;
use crate::adapters::vscode::VscodeCopilotAdapter;
use crate::io::{
    BundleMemberSpec, ContentKind, MemberCompression, MemberRole, SessionSnapshotBundle,
    SnapshotLimits, capture_bundle,
};
use crate::model::{NormalizedSessionV2, SessionSource};

use super::catalog::{CandidateFormat, DiscoveryCatalog, DiscoverySource, ResolvedCatalogRecord};

const DEFAULT_TERMINAL_HOLD_MS: u64 = 1_500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum SessionLoadError {
    #[error("CATALOG_STATE_UNAVAILABLE")]
    CatalogStateUnavailable,
    #[error("CATALOG_ENTRY_UNAVAILABLE")]
    CatalogEntryUnavailable,
    #[error("UNSUPPORTED_SESSION_SOURCE")]
    UnsupportedSource,
    #[error("SOURCE_READ_FAILED")]
    SourceReadFailed,
    #[error("SOURCE_METADATA_INVALID")]
    SourceMetadataInvalid,
    #[error("SOURCE_SESSION_IDENTIFIER_INVALID")]
    SessionIdentifierInvalid,
    #[error("SOURCE_NORMALIZATION_FAILED")]
    SourceNormalizationFailed,
}

impl SessionLoadError {
    pub(crate) fn diagnostic_code(self) -> &'static str {
        match self {
            Self::CatalogStateUnavailable => "CATALOG_STATE_UNAVAILABLE",
            Self::CatalogEntryUnavailable => "CATALOG_ENTRY_UNAVAILABLE",
            Self::UnsupportedSource => "UNSUPPORTED_SESSION_SOURCE",
            Self::SourceReadFailed => "SOURCE_READ_FAILED",
            Self::SourceMetadataInvalid => "SOURCE_METADATA_INVALID",
            Self::SessionIdentifierInvalid => "SOURCE_SESSION_IDENTIFIER_INVALID",
            Self::SourceNormalizationFailed => "SOURCE_NORMALIZATION_FAILED",
        }
    }
}

pub(crate) fn load_catalog_session_v2(
    catalog_id: &str,
    catalog: &Arc<Mutex<DiscoveryCatalog>>,
) -> Result<NormalizedSessionV2, SessionLoadError> {
    let catalog = catalog
        .lock()
        .map_err(|_| SessionLoadError::CatalogStateUnavailable)?;
    let record = catalog
        .resolve(catalog_id)
        .ok_or(SessionLoadError::CatalogEntryUnavailable)?;
    load_resolved_session_v2(&record)
}

fn load_resolved_session_v2(
    record: &ResolvedCatalogRecord<'_>,
) -> Result<NormalizedSessionV2, SessionLoadError> {
    let (source, bundle, context) = prepare_adapter_input(record)?;
    let normalized = match source {
        DiscoverySource::Codex => run_rich_adapter(&CodexAdapter, &bundle, &context),
        DiscoverySource::ClaudeCode => run_rich_adapter(&ClaudeAdapter, &bundle, &context),
        DiscoverySource::CopilotCli => run_rich_adapter(&CopilotCliAdapter, &bundle, &context),
        DiscoverySource::VscodeCopilot => {
            run_rich_adapter(&VscodeCopilotAdapter, &bundle, &context)
        }
        DiscoverySource::JetBrains => return Err(SessionLoadError::UnsupportedSource),
    };
    normalized.map_err(|_| SessionLoadError::SourceNormalizationFailed)
}

fn prepare_adapter_input(
    record: &ResolvedCatalogRecord<'_>,
) -> Result<(DiscoverySource, SessionSnapshotBundle, AdapterContext), SessionLoadError> {
    let candidate_path = record.candidate_path.to_path_buf();
    let source = record.source;
    let (compression, content_kind) = match record.format {
        CandidateFormat::JsonLines => (MemberCompression::None, ContentKind::JsonLines),
        CandidateFormat::ZstdJsonLines => (MemberCompression::Zstandard, ContentKind::JsonLines),
        CandidateFormat::Json => (MemberCompression::None, ContentKind::Bytes),
    };

    let mut specs = vec![BundleMemberSpec {
        id: "primary".to_owned(),
        logical_name: candidate_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        path: candidate_path.clone(),
        role: MemberRole::Primary,
        compression,
        content_kind,
        expected_identity: Some(record.expected_identity),
    }];

    if source == DiscoverySource::CopilotCli
        && let Some(settings_spec) = copilot_settings_spec(record, &candidate_path)
    {
        specs.push(settings_spec);
    }

    let bundle = capture_bundle(record.root, &specs, snapshot_limits(record.format))
        .map_err(|_| SessionLoadError::SourceReadFailed)?;
    let vendor_id = match source {
        DiscoverySource::Codex => match codex_rollout_id(candidate_path.file_name()) {
            Some(session_id) => session_id,
            None => extract_codex_session_id(&bundle)
                .map_err(|_| SessionLoadError::SourceMetadataInvalid)?,
        },
        DiscoverySource::ClaudeCode => path_identifier(candidate_path.file_stem())?,
        DiscoverySource::CopilotCli => {
            path_identifier(candidate_path.parent().and_then(std::path::Path::file_name))?
        }
        DiscoverySource::VscodeCopilot => path_identifier(candidate_path.file_stem())?,
        DiscoverySource::JetBrains => return Err(SessionLoadError::UnsupportedSource),
    };
    let session_source = match source {
        DiscoverySource::Codex => SessionSource::Codex,
        DiscoverySource::ClaudeCode => SessionSource::ClaudeCode,
        DiscoverySource::CopilotCli => SessionSource::CopilotCli,
        DiscoverySource::VscodeCopilot => SessionSource::VscodeCopilot,
        DiscoverySource::JetBrains => return Err(SessionLoadError::UnsupportedSource),
    };
    let context = AdapterContext::new(vendor_id, session_source, DEFAULT_TERMINAL_HOLD_MS)
        .map_err(|_| SessionLoadError::SessionIdentifierInvalid)?;
    Ok((source, bundle, context))
}

fn codex_rollout_id(component: Option<&std::ffi::OsStr>) -> Option<String> {
    let file_name = component?.to_str()?.to_ascii_lowercase();
    let stem = file_name
        .strip_suffix(".jsonl.zst")
        .or_else(|| file_name.strip_suffix(".jsonl"))?;
    let rollout = stem.strip_prefix("rollout-")?;
    let candidate = rollout.get(rollout.len().checked_sub(36)?..)?;
    let valid = candidate.bytes().enumerate().all(|(index, byte)| {
        if matches!(index, 8 | 13 | 18 | 23) {
            byte == b'-'
        } else {
            byte.is_ascii_hexdigit()
        }
    });
    valid.then(|| candidate.to_owned())
}

fn snapshot_limits(format: CandidateFormat) -> SnapshotLimits {
    let mut limits = SnapshotLimits::default();
    if format == CandidateFormat::JsonLines {
        // Plaintext JSONL has identical raw and decoded sizes. Keep compressed
        // inputs at the smaller raw ceiling while accepting plaintext up to
        // the existing bounded decoded-file ceiling.
        limits.max_raw_file_bytes = limits.max_decoded_file_bytes;
    }
    limits
}

fn path_identifier(component: Option<&std::ffi::OsStr>) -> Result<String, SessionLoadError> {
    component
        .and_then(std::ffi::OsStr::to_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or(SessionLoadError::SourceMetadataInvalid)
}

fn copilot_settings_spec(
    record: &ResolvedCatalogRecord<'_>,
    events_path: &std::path::Path,
) -> Option<BundleMemberSpec> {
    let session_state = events_path.parent()?.parent()?;
    if !session_state
        .file_name()?
        .to_string_lossy()
        .eq_ignore_ascii_case("session-state")
    {
        return None;
    }
    let settings_path = session_state.parent()?.join("settings.json");
    let opened = record.root.open_file(&settings_path).ok()?;
    Some(BundleMemberSpec {
        id: "settings".to_owned(),
        logical_name: "settings.json".to_owned(),
        path: opened.canonical_path,
        role: MemberRole::Metadata,
        compression: MemberCompression::None,
        content_kind: ContentKind::Bytes,
        expected_identity: Some((opened.stamp.volume_serial, opened.stamp.file_id)),
    })
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::{codex_rollout_id, snapshot_limits};
    use crate::discovery::catalog::CandidateFormat;

    #[test]
    fn plaintext_logs_use_the_bounded_decoded_file_ceiling() {
        let copilot = snapshot_limits(CandidateFormat::JsonLines);
        let codex = snapshot_limits(CandidateFormat::JsonLines);
        let compressed_codex = snapshot_limits(CandidateFormat::ZstdJsonLines);

        assert_eq!(copilot.max_raw_file_bytes, 128 * 1024 * 1024);
        assert_eq!(copilot.max_raw_file_bytes, copilot.max_decoded_file_bytes);
        assert_eq!(copilot.max_raw_total_bytes, 256 * 1024 * 1024);
        assert_eq!(codex.max_raw_file_bytes, codex.max_decoded_file_bytes);
        assert_eq!(compressed_codex.max_raw_file_bytes, 64 * 1024 * 1024);
    }

    #[test]
    fn codex_rollout_identity_comes_from_the_artifact_filename() {
        assert_eq!(
            codex_rollout_id(Some(OsStr::new(
                "rollout-2026-09-02T08-00-00-22222222-2222-4222-8222-222222222222.jsonl"
            ))),
            Some("22222222-2222-4222-8222-222222222222".to_owned())
        );
        assert_eq!(
            codex_rollout_id(Some(OsStr::new(
                "rollout-2026-09-02T08-00-00-AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA.JSONL.ZST"
            ))),
            Some("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_owned())
        );
        assert_eq!(codex_rollout_id(Some(OsStr::new("rollout.jsonl"))), None);
        assert_eq!(
            codex_rollout_id(Some(OsStr::new(
                "unrelated-22222222-2222-4222-8222-222222222222.jsonl"
            ))),
            None
        );
    }
}
