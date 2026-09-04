use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::adapters::conformance::{
    ConformanceViolation, assert_conforming_fixture, check_conformance,
};
use crate::adapters::contract::{
    AdapterContext, AdapterError, ContextReason, OutputReason, RecordAccountant, SessionAdapter,
    StructuralReason, run_adapter, run_rich_adapter,
};
use crate::adapters::deletion::{DeletionArtifactDeclaration, artifact_declaration};
use crate::adapters::normalize::{
    EventIdGenerator, RawTimedRecord, RawTimestamp, compute_duration, normalize_timestamps,
    parse_rfc3339_to_epoch_ms, structural_order,
};
use crate::indexed_library::IndexedSessionSourceV1;
use crate::io::{
    BundleMemberSpec, ContentKind, LocalRoot, MemberCompression, MemberRole, SessionSnapshotBundle,
    SnapshotLimits, SyntheticMember, capture_bundle, capture_bundle_with_hook,
};
use crate::model::{
    DiagnosticSeverity, NormalizedSessionV1, NormalizedSessionV2, ReplayEvent, SessionSource,
    migrate_normalized_session_v1,
};

fn make_bundle(members: Vec<SyntheticMember>) -> SessionSnapshotBundle {
    SessionSnapshotBundle::synthetic(members)
}

fn default_context() -> AdapterContext {
    AdapterContext::new("test-session-1".to_owned(), SessionSource::ClaudeCode, 1500)
        .expect("default adapter context should be valid")
}

fn minimal_valid_session(events: Vec<ReplayEvent>, terminal_hold_ms: u64) -> NormalizedSessionV1 {
    let last_at = events.last().map(ReplayEvent::at_ms).unwrap_or(0);
    NormalizedSessionV1 {
        schema_version: 1,
        id: "test-session-1".to_owned(),
        source: SessionSource::ClaudeCode,
        source_version: None,
        title: "Test Session".to_owned(),
        created_at: None,
        cwd: None,
        relationships: vec![],
        duration_ms: last_at + terminal_hold_ms,
        diagnostics: vec![],
        unknown_record_count: events
            .iter()
            .filter(|event| matches!(event, ReplayEvent::Unknown { .. }))
            .count() as u64,
        events,
    }
}

fn member(id: &str, records: Vec<&[u8]>, partial_final: bool) -> SyntheticMember {
    SyntheticMember {
        id: id.to_owned(),
        logical_name: format!("{id}.jsonl"),
        records: records.into_iter().map(|record| record.to_vec()).collect(),
        partial_final,
    }
}

fn spec(id: &str, path: &Path) -> BundleMemberSpec {
    BundleMemberSpec {
        id: id.to_owned(),
        logical_name: format!("{id}.jsonl"),
        path: path.to_path_buf(),
        role: MemberRole::Primary,
        compression: MemberCompression::None,
        content_kind: ContentKind::JsonLines,
        expected_identity: None,
    }
}

fn byte_spec(id: &str, path: &Path) -> BundleMemberSpec {
    BundleMemberSpec {
        id: id.to_owned(),
        logical_name: format!("{id}.json"),
        path: path.to_path_buf(),
        role: MemberRole::Primary,
        compression: MemberCompression::None,
        content_kind: ContentKind::Bytes,
        expected_identity: None,
    }
}

struct RepoTree(PathBuf);

impl RepoTree {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should follow the epoch")
            .as_nanos();
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("adapter-fixture-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path).expect("fixture root should be created");
        Self(path)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, bytes).expect("fixture should be written");
        path
    }

    fn root(&self) -> LocalRoot {
        LocalRoot::new(&self.0).expect("root should be valid")
    }
}

impl Drop for RepoTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct ReferenceAdapter;

#[derive(Clone, Copy)]
enum RichFault {
    Schema,
    Id,
    Source,
    EntryLimit,
    Duration,
    UnknownCount,
    UnknownType,
}

struct InvalidRichAdapter(RichFault);

impl SessionAdapter for InvalidRichAdapter {
    fn normalize(
        &self,
        bundle: &SessionSnapshotBundle,
        context: &AdapterContext,
        records: &RecordAccountant,
    ) -> Result<NormalizedSessionV1, AdapterError> {
        ReferenceAdapter.normalize(bundle, context, records)
    }

    fn normalize_rich(
        &self,
        bundle: &SessionSnapshotBundle,
        context: &AdapterContext,
        records: &RecordAccountant,
    ) -> Result<NormalizedSessionV2, AdapterError> {
        let legacy = self.normalize(bundle, context, records)?;
        let mut session = migrate_normalized_session_v1(legacy, context.terminal_hold_ms())
            .map_err(|_| AdapterError::InvalidOutput {
                reason: OutputReason::SessionInvariant,
            })?;
        match self.0 {
            RichFault::Schema => session.schema_version = 1,
            RichFault::Id => session.id = "wrong-session".to_owned(),
            RichFault::Source => session.source = SessionSource::Codex,
            RichFault::EntryLimit => session.entries = vec![session.entries[0].clone(); 100_001],
            RichFault::Duration => session.duration_ms += 1,
            RichFault::UnknownCount => {
                session.unknown_record_count = 0;
                session.entries[0] = crate::model::NormalizedEntryV2::User {
                    entry_key: "replacement".to_owned(),
                    at_ms: 0,
                    text: "safe".to_owned(),
                };
            }
            RichFault::UnknownType => {
                if let crate::model::NormalizedEntryV2::Unknown { source_type, .. } =
                    &mut session.entries[0]
                {
                    *source_type = "different".to_owned();
                }
            }
        }
        Ok(session)
    }
}

impl SessionAdapter for ReferenceAdapter {
    fn normalize(
        &self,
        bundle: &SessionSnapshotBundle,
        context: &AdapterContext,
        records: &RecordAccountant,
    ) -> Result<NormalizedSessionV1, AdapterError> {
        let mut events = Vec::new();
        let mut id_generator = EventIdGenerator::new(context.session_id());

        for (member_ordinal, member) in bundle.members().enumerate() {
            for (record_ordinal, _) in member.records().enumerate() {
                records.classify_unknown(member_ordinal, record_ordinal, "reference")?;
                events.push(ReplayEvent::Unknown {
                    id: id_generator.next_id(member_ordinal, record_ordinal, "unknown"),
                    at_ms: 0,
                    source_type: "reference".to_owned(),
                });
            }
        }

        if events.is_empty() {
            return Err(AdapterError::EmptySession);
        }

        Ok(NormalizedSessionV1 {
            schema_version: 1,
            id: context.session_id().to_owned(),
            source: context.source().clone(),
            source_version: None,
            title: "Reference Session".to_owned(),
            created_at: None,
            cwd: None,
            relationships: vec![],
            duration_ms: compute_duration(&[0], context.terminal_hold_ms())?,
            diagnostics: vec![],
            unknown_record_count: events.len() as u64,
            events,
        })
    }
}

struct MultiEventAdapter;

impl SessionAdapter for MultiEventAdapter {
    fn normalize(
        &self,
        bundle: &SessionSnapshotBundle,
        context: &AdapterContext,
        records: &RecordAccountant,
    ) -> Result<NormalizedSessionV1, AdapterError> {
        let mut events = Vec::new();
        let mut id_generator = EventIdGenerator::new(context.session_id());

        for (member_ordinal, member) in bundle.members().enumerate() {
            for (record_ordinal, _) in member.records().enumerate() {
                records.classify_understood(member_ordinal, record_ordinal)?;
                events.push(ReplayEvent::User {
                    id: id_generator.next_id(member_ordinal, record_ordinal, "user"),
                    at_ms: 0,
                    text: "hello".to_owned(),
                });
                events.push(ReplayEvent::Assistant {
                    id: id_generator.next_id(member_ordinal, record_ordinal, "assistant"),
                    at_ms: 0,
                    markdown: "world".to_owned(),
                });
            }
        }

        Ok(NormalizedSessionV1 {
            schema_version: 1,
            id: context.session_id().to_owned(),
            source: context.source().clone(),
            source_version: None,
            title: "Multi Event Session".to_owned(),
            created_at: None,
            cwd: None,
            relationships: vec![],
            duration_ms: compute_duration(&[0], context.terminal_hold_ms())?,
            diagnostics: vec![],
            unknown_record_count: 0,
            events,
        })
    }
}

struct DroppingAdapter;

impl SessionAdapter for DroppingAdapter {
    fn normalize(
        &self,
        bundle: &SessionSnapshotBundle,
        context: &AdapterContext,
        records: &RecordAccountant,
    ) -> Result<NormalizedSessionV1, AdapterError> {
        let mut events = Vec::new();
        let mut id_generator = EventIdGenerator::new(context.session_id());

        for (member_ordinal, member) in bundle.members().enumerate() {
            for (record_ordinal, _) in member.records().enumerate() {
                if member_ordinal == 0 && record_ordinal == 0 {
                    continue;
                }
                records.classify_unknown(member_ordinal, record_ordinal, "dropping")?;
                events.push(ReplayEvent::Unknown {
                    id: id_generator.next_id(member_ordinal, record_ordinal, "unknown"),
                    at_ms: 0,
                    source_type: "dropping".to_owned(),
                });
            }
        }

        Ok(NormalizedSessionV1 {
            schema_version: 1,
            id: context.session_id().to_owned(),
            source: context.source().clone(),
            source_version: None,
            title: "Dropping Session".to_owned(),
            created_at: None,
            cwd: None,
            relationships: vec![],
            duration_ms: compute_duration(&[0], context.terminal_hold_ms())?,
            diagnostics: vec![],
            unknown_record_count: events.len() as u64,
            events,
        })
    }
}

struct DuplicateClassificationAdapter;

impl SessionAdapter for DuplicateClassificationAdapter {
    fn normalize(
        &self,
        _bundle: &SessionSnapshotBundle,
        _context: &AdapterContext,
        records: &RecordAccountant,
    ) -> Result<NormalizedSessionV1, AdapterError> {
        records.classify_understood(0, 0)?;
        records.classify_understood(0, 0)?;
        unreachable!("duplicate classification should error before this point");
    }
}

struct WrongUnknownCountAdapter;

impl SessionAdapter for WrongUnknownCountAdapter {
    fn normalize(
        &self,
        bundle: &SessionSnapshotBundle,
        context: &AdapterContext,
        records: &RecordAccountant,
    ) -> Result<NormalizedSessionV1, AdapterError> {
        let session = ReferenceAdapter.normalize(bundle, context, records)?;
        Ok(NormalizedSessionV1 {
            unknown_record_count: 0,
            ..session
        })
    }
}

struct WrongUnknownTypeAdapter;

impl SessionAdapter for WrongUnknownTypeAdapter {
    fn normalize(
        &self,
        bundle: &SessionSnapshotBundle,
        context: &AdapterContext,
        records: &RecordAccountant,
    ) -> Result<NormalizedSessionV1, AdapterError> {
        let mut session = ReferenceAdapter.normalize(bundle, context, records)?;
        session.events[0] = ReplayEvent::Unknown {
            id: "rewritten".to_owned(),
            at_ms: 0,
            source_type: "rewritten".to_owned(),
        };
        Ok(session)
    }
}

struct FixtureAdapter;

impl SessionAdapter for FixtureAdapter {
    fn normalize(
        &self,
        bundle: &SessionSnapshotBundle,
        context: &AdapterContext,
        records: &RecordAccountant,
    ) -> Result<NormalizedSessionV1, AdapterError> {
        let mut events = Vec::new();
        let mut id_generator = EventIdGenerator::new(context.session_id());
        let mut unknown_record_count = 0_u64;

        for (member_ordinal, member) in bundle.members().enumerate() {
            for (record_ordinal, record) in member.records().enumerate() {
                let value: serde_json::Value =
                    serde_json::from_slice(record).map_err(|_| AdapterError::MalformedRecord {
                        member_ordinal,
                        record_ordinal,
                        field: "json",
                    })?;
                let current_kind = value.get("type").and_then(serde_json::Value::as_str);
                let legacy_kind = value.get("role").and_then(serde_json::Value::as_str);

                match current_kind.or(legacy_kind) {
                    Some("user") => {
                        records.classify_understood(member_ordinal, record_ordinal)?;
                        let text = value
                            .get("text")
                            .or_else(|| value.get("content"))
                            .and_then(serde_json::Value::as_str)
                            .ok_or(AdapterError::MalformedRecord {
                                member_ordinal,
                                record_ordinal,
                                field: "message",
                            })?;
                        events.push(ReplayEvent::User {
                            id: id_generator.next_id(member_ordinal, record_ordinal, "user"),
                            at_ms: 0,
                            text: text.to_owned(),
                        });
                    }
                    Some("assistant") => {
                        records.classify_understood(member_ordinal, record_ordinal)?;
                        let markdown = value
                            .get("text")
                            .or_else(|| value.get("content"))
                            .and_then(serde_json::Value::as_str)
                            .ok_or(AdapterError::MalformedRecord {
                                member_ordinal,
                                record_ordinal,
                                field: "message",
                            })?;
                        events.push(ReplayEvent::Assistant {
                            id: id_generator.next_id(member_ordinal, record_ordinal, "assistant"),
                            at_ms: 0,
                            markdown: markdown.to_owned(),
                        });
                    }
                    Some(source_type) => {
                        records.classify_unknown(member_ordinal, record_ordinal, source_type)?;
                        unknown_record_count += 1;
                        events.push(ReplayEvent::Unknown {
                            id: id_generator.next_id(member_ordinal, record_ordinal, "unknown"),
                            at_ms: 0,
                            source_type: source_type.to_owned(),
                        });
                    }
                    None => {
                        records.classify_unknown(member_ordinal, record_ordinal, "untyped")?;
                        unknown_record_count += 1;
                        events.push(ReplayEvent::Unknown {
                            id: id_generator.next_id(member_ordinal, record_ordinal, "unknown"),
                            at_ms: 0,
                            source_type: "untyped".to_owned(),
                        });
                    }
                }
            }
        }

        let at_ms_values: Vec<u64> = events.iter().map(ReplayEvent::at_ms).collect();
        Ok(NormalizedSessionV1 {
            schema_version: 1,
            id: context.session_id().to_owned(),
            source: context.source().clone(),
            source_version: Some("fixture".to_owned()),
            title: "Fixture Session".to_owned(),
            created_at: None,
            cwd: None,
            relationships: vec![],
            duration_ms: compute_duration(&at_ms_values, context.terminal_hold_ms())?,
            diagnostics: vec![],
            unknown_record_count,
            events,
        })
    }
}

#[derive(Clone, Copy)]
enum InvalidOutput {
    SchemaVersion,
    SessionId,
    SessionSource,
    DuplicateEventId,
    OversizedTitle,
    EventLimit,
}

struct InvalidOutputAdapter(InvalidOutput);

impl SessionAdapter for InvalidOutputAdapter {
    fn normalize(
        &self,
        bundle: &SessionSnapshotBundle,
        context: &AdapterContext,
        records: &RecordAccountant,
    ) -> Result<NormalizedSessionV1, AdapterError> {
        let mut session = MultiEventAdapter.normalize(bundle, context, records)?;
        match self.0 {
            InvalidOutput::SchemaVersion => session.schema_version = 2,
            InvalidOutput::SessionId => session.id = "other-session".to_owned(),
            InvalidOutput::SessionSource => session.source = SessionSource::Codex,
            InvalidOutput::DuplicateEventId => {
                let first_id = match &session.events[0] {
                    ReplayEvent::User { id, .. } => id.clone(),
                    _ => unreachable!("multi-event adapter starts with a user event"),
                };
                match &mut session.events[1] {
                    ReplayEvent::Assistant { id, .. } => *id = first_id,
                    _ => unreachable!("multi-event adapter continues with an assistant event"),
                }
            }
            InvalidOutput::OversizedTitle => session.title = "x".repeat(513),
            InvalidOutput::EventLimit => {
                for index in session.events.len()..=100_000 {
                    session.events.push(ReplayEvent::User {
                        id: format!("overflow-{index}"),
                        at_ms: 0,
                        text: "x".to_owned(),
                    });
                }
            }
        }
        Ok(session)
    }
}

struct ReorderedUnknownAdapter;

impl SessionAdapter for ReorderedUnknownAdapter {
    fn normalize(
        &self,
        bundle: &SessionSnapshotBundle,
        context: &AdapterContext,
        records: &RecordAccountant,
    ) -> Result<NormalizedSessionV1, AdapterError> {
        let source_types = ["first-type", "second-type"];
        for (record_ordinal, _) in bundle
            .member("primary")
            .expect("fixture has primary member")
            .records()
            .enumerate()
        {
            records.classify_unknown(0, record_ordinal, source_types[record_ordinal])?;
        }
        let mut id_generator = EventIdGenerator::new(context.session_id());
        let events = vec![
            ReplayEvent::Unknown {
                id: id_generator.next_id(0, 1, "unknown"),
                at_ms: 0,
                source_type: source_types[1].to_owned(),
            },
            ReplayEvent::Unknown {
                id: id_generator.next_id(0, 0, "unknown"),
                at_ms: 0,
                source_type: source_types[0].to_owned(),
            },
        ];
        Ok(NormalizedSessionV1 {
            schema_version: 1,
            id: context.session_id().to_owned(),
            source: context.source().clone(),
            source_version: None,
            title: "Reordered Unknown Session".to_owned(),
            created_at: None,
            cwd: None,
            relationships: vec![],
            duration_ms: context.terminal_hold_ms(),
            diagnostics: vec![],
            unknown_record_count: 2,
            events,
        })
    }
}

#[test]
fn parses_rfc3339_utc_timestamp() {
    let ms = parse_rfc3339_to_epoch_ms("2024-01-15T10:30:00Z").unwrap();
    assert_eq!(ms, 1_705_314_600_000);
}

#[test]
fn parses_rfc3339_with_negative_offset_and_fraction() {
    let ms = parse_rfc3339_to_epoch_ms("2024-01-15T05:30:00.123-05:00").unwrap();
    assert_eq!(ms, 1_705_314_600_123);
}

#[test]
fn rejects_invalid_rfc3339() {
    assert!(parse_rfc3339_to_epoch_ms("not-a-date").is_none());
    assert!(parse_rfc3339_to_epoch_ms("2024-13-15T10:30:00Z").is_none());
    assert!(parse_rfc3339_to_epoch_ms("2024-01-32T10:30:00Z").is_none());
}

#[test]
fn uses_vendor_ordinals_when_all_unique() {
    let mut records = vec![
        RawTimedRecord {
            vendor_ordinal: Some(3),
            timestamp_rfc3339: None,
            structural_position: (0, 0),
            kind: "user".to_owned(),
        },
        RawTimedRecord {
            vendor_ordinal: Some(1),
            timestamp_rfc3339: None,
            structural_position: (0, 1),
            kind: "assistant".to_owned(),
        },
        RawTimedRecord {
            vendor_ordinal: Some(2),
            timestamp_rfc3339: None,
            structural_position: (0, 2),
            kind: "tool".to_owned(),
        },
    ];
    let mut diagnostics = vec![];

    structural_order(&mut records, &mut diagnostics);

    assert_eq!(records[0].vendor_ordinal, Some(1));
    assert_eq!(records[1].vendor_ordinal, Some(2));
    assert_eq!(records[2].vendor_ordinal, Some(3));
    assert!(diagnostics.is_empty());
}

#[test]
fn falls_back_to_structural_position_when_ordinals_missing() {
    let mut records = vec![
        RawTimedRecord {
            vendor_ordinal: Some(1),
            timestamp_rfc3339: None,
            structural_position: (0, 0),
            kind: "user".to_owned(),
        },
        RawTimedRecord {
            vendor_ordinal: None,
            timestamp_rfc3339: None,
            structural_position: (0, 1),
            kind: "assistant".to_owned(),
        },
    ];
    let mut diagnostics = vec![];

    structural_order(&mut records, &mut diagnostics);

    assert_eq!(records[0].structural_position, (0, 0));
    assert_eq!(records[1].structural_position, (0, 1));
    assert_eq!(diagnostics[0].code, "ordering-fallback");
}

#[test]
fn falls_back_to_structural_position_when_ordinals_are_duplicated() {
    let mut records = vec![
        RawTimedRecord {
            vendor_ordinal: Some(1),
            timestamp_rfc3339: None,
            structural_position: (0, 1),
            kind: "assistant".to_owned(),
        },
        RawTimedRecord {
            vendor_ordinal: Some(1),
            timestamp_rfc3339: None,
            structural_position: (0, 0),
            kind: "user".to_owned(),
        },
    ];
    let mut diagnostics = vec![];

    structural_order(&mut records, &mut diagnostics);

    assert_eq!(records[0].structural_position, (0, 0));
    assert_eq!(records[1].structural_position, (0, 1));
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, "ordering-fallback");
}

#[test]
fn ordering_fallback_respects_the_model_diagnostic_limit() {
    let mut records = vec![RawTimedRecord {
        vendor_ordinal: None,
        timestamp_rfc3339: None,
        structural_position: (0, 0),
        kind: "user".to_owned(),
    }];
    let mut diagnostics = (0..1000)
        .map(|index| crate::model::SourceDiagnostic {
            code: format!("existing-{index}"),
            severity: DiagnosticSeverity::Info,
            message: "existing diagnostic".to_owned(),
        })
        .collect();

    structural_order(&mut records, &mut diagnostics);

    assert_eq!(diagnostics.len(), 1000);
}

#[test]
fn adapter_context_rejects_oversized_session_ids_before_normalization() {
    let error = AdapterContext::new("x".repeat(257), SessionSource::Codex, 1500).unwrap_err();

    assert!(matches!(
        error,
        AdapterError::InvalidContext {
            reason: ContextReason::SessionId
        }
    ));
}

#[test]
fn adapter_context_rejects_empty_and_nul_session_ids() {
    for session_id in ["", "session\0id"] {
        let error =
            AdapterContext::new(session_id.to_owned(), SessionSource::Codex, 1500).unwrap_err();
        assert!(matches!(
            error,
            AdapterError::InvalidContext {
                reason: ContextReason::SessionId
            }
        ));
    }
}

#[test]
fn adapter_context_rejects_terminal_holds_outside_render_limits() {
    for terminal_hold_ms in [249, 10_001] {
        let error = AdapterContext::new(
            "test-session".to_owned(),
            SessionSource::Codex,
            terminal_hold_ms,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            AdapterError::InvalidContext {
                reason: ContextReason::TerminalHold
            }
        ));
    }
}

#[test]
fn missing_timestamp_after_origin_inherits_previous_even_when_previous_is_zero() {
    let mut diagnostics = vec![];

    let result = normalize_timestamps(
        &[RawTimestamp::Valid(1000), RawTimestamp::Absent],
        &mut diagnostics,
    );

    assert_eq!(result, vec![0, 0]);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "missing-timestamp")
    );
}

#[test]
fn timestamps_before_origin_emit_a_pre_origin_diagnostic() {
    let mut diagnostics = vec![];

    let result = normalize_timestamps(
        &[RawTimestamp::Valid(1000), RawTimestamp::Valid(500)],
        &mut diagnostics,
    );

    assert_eq!(result, vec![0, 0]);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "pre-origin-timestamp")
    );
}

#[test]
fn repeated_missing_timestamps_emit_one_bounded_diagnostic() {
    let mut timestamps = vec![RawTimestamp::Valid(1000)];
    timestamps.extend(std::iter::repeat_n(RawTimestamp::Absent, 1001));
    let mut diagnostics = vec![];

    let result = normalize_timestamps(&timestamps, &mut diagnostics);

    assert_eq!(result.len(), 1002);
    assert!(result.iter().all(|at_ms| *at_ms == 0));
    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == "missing-timestamp")
            .count(),
        1
    );
}

#[test]
fn timestamp_normalization_respects_the_model_diagnostic_limit() {
    let mut diagnostics = (0..1000)
        .map(|index| crate::model::SourceDiagnostic {
            code: format!("existing-{index}"),
            severity: DiagnosticSeverity::Info,
            message: "existing diagnostic".to_owned(),
        })
        .collect();

    normalize_timestamps(
        &[
            RawTimestamp::Valid(1000),
            RawTimestamp::Absent,
            RawTimestamp::Invalid,
        ],
        &mut diagnostics,
    );

    assert_eq!(diagnostics.len(), 1000);
}

#[test]
fn invalid_timestamp_after_origin_inherits_previous_with_invalid_diagnostic() {
    let mut diagnostics = vec![];

    let result = normalize_timestamps(
        &[
            RawTimestamp::Valid(1000),
            RawTimestamp::Invalid,
            RawTimestamp::Valid(2500),
        ],
        &mut diagnostics,
    );

    assert_eq!(result, vec![0, 0, 1500]);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "invalid-timestamp")
    );
}

#[test]
fn all_missing_timestamps_produce_zeroes_with_diagnostic() {
    let mut diagnostics = vec![];

    let result = normalize_timestamps(
        &[
            RawTimestamp::Absent,
            RawTimestamp::Absent,
            RawTimestamp::Absent,
        ],
        &mut diagnostics,
    );

    assert_eq!(result, vec![0, 0, 0]);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "no-valid-timestamps")
    );
}

#[test]
fn deterministic_event_ids_use_structural_positions_instead_of_call_order() {
    let mut first = EventIdGenerator::new("session-123");
    let id_a = first.next_id_for_content(2, 5, "assistant", &["answer a"]);
    let id_b = first.next_id_for_content(0, 1, "assistant", &["answer b"]);

    let mut second = EventIdGenerator::new("session-123");
    let id_b_again = second.next_id_for_content(0, 1, "assistant", &["answer b"]);
    let id_a_again = second.next_id_for_content(2, 5, "assistant", &["answer a"]);

    assert_eq!(id_a, id_a_again);
    assert_eq!(id_b, id_b_again);
}

#[test]
fn deterministic_event_ids_suffix_collisions_for_same_record_and_kind() {
    let mut generator = EventIdGenerator::new("session-123");

    let first = generator.next_id_for_content(1, 9, "tool", &["same"]);
    let second = generator.next_id_for_content(1, 9, "tool", &["same"]);

    assert_ne!(first, second);
    assert!(second.ends_with("-1"));
}

#[test]
fn deterministic_event_ids_are_bounded_and_do_not_embed_raw_session_text() {
    let session_id = "session-".to_owned() + &"x".repeat(512);
    let mut generator = EventIdGenerator::new(&session_id);

    let id = generator.next_id_for_content(123, 456, "assistant", &["private response"]);

    assert!(id.len() <= 256);
    assert!(!id.contains(&"x".repeat(32)));
    assert!(!id.contains("private response"));
}

#[test]
fn derived_entry_keys_survive_append_only_growth_and_change_on_rewrite() {
    let contents = (0..128)
        .map(|index| format!("synthetic entry {index}"))
        .collect::<Vec<_>>();
    let prefix_keys = {
        let mut generator = EventIdGenerator::new("session-123");
        contents[..64]
            .iter()
            .enumerate()
            .map(|(index, content)| {
                generator.next_id_for_content(0, index, "assistant", &[content])
            })
            .collect::<Vec<_>>()
    };
    let grown_keys = {
        let mut generator = EventIdGenerator::new("session-123");
        contents
            .iter()
            .enumerate()
            .map(|(index, content)| {
                generator.next_id_for_content(0, index, "assistant", &[content])
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(prefix_keys, grown_keys[..prefix_keys.len()]);

    let mut rewritten = EventIdGenerator::new("session-123");
    let rewritten_key = rewritten.next_id_for_content(0, 12, "assistant", &["rewritten content"]);
    assert_ne!(grown_keys[12], rewritten_key);

    let mut pending = EventIdGenerator::new("session-123");
    let pending_key = pending.next_id_for_content(0, 7, "tool", &["read", "pending", "file"]);
    let mut completed = EventIdGenerator::new("session-123");
    let completed_key = completed.next_id_for_content(0, 7, "tool", &["read", "succeeded", "file"]);
    assert_ne!(pending_key, completed_key);
}

#[test]
fn vendor_entry_keys_survive_position_and_content_changes() {
    let mut first = EventIdGenerator::new("session-123");
    let original = first.next_id_for_vendor("tool", "call-42");
    let mut second = EventIdGenerator::new("session-123");
    let updated = second.next_id_for_vendor("tool", "call-42");
    let different = second.next_id_for_vendor("tool", "call-43");

    assert_eq!(original, updated);
    assert_ne!(original, different);
    assert!(!original.contains("call-42"));
}

#[test]
fn computes_duration_with_terminal_hold() {
    assert_eq!(compute_duration(&[0, 500, 1000], 1500).unwrap(), 2500);
}

#[test]
fn empty_events_get_terminal_hold() {
    assert_eq!(compute_duration(&[], 1500).unwrap(), 1500);
}

#[test]
fn duration_overflow_is_rejected() {
    let error = compute_duration(&[u64::MAX], 1).unwrap_err();
    assert!(matches!(error, AdapterError::DurationOverflow));
}

#[test]
fn duration_limit_is_enforced() {
    let seven_days_ms = 7 * 24 * 60 * 60 * 1000_u64;
    let error = compute_duration(&[seven_days_ms - 1000], 1500).unwrap_err();
    assert!(
        matches!(error, AdapterError::DurationLimitExceeded { duration_ms } if duration_ms == seven_days_ms + 500)
    );
}

#[test]
fn accountant_finishes_with_unknown_summary_in_structural_order() {
    let accountant = RecordAccountant::for_test([(0, 2), (1, 1)]);

    accountant.classify_understood(0, 0).unwrap();
    accountant.classify_unknown(0, 1, "tool-call").unwrap();
    accountant.classify_unknown(1, 0, "vendor-gap").unwrap();

    let summary = accountant.finish_for_test().unwrap();
    assert_eq!(summary.unknown_count, 2);
    assert_eq!(summary.unknown_types, vec!["tool-call", "vendor-gap"]);
}

#[test]
fn accountant_rejects_duplicate_classification() {
    let accountant = RecordAccountant::for_test([(0, 1)]);

    accountant.classify_understood(0, 0).unwrap();
    let error = accountant.classify_unknown(0, 0, "duplicate").unwrap_err();

    assert!(matches!(
        error,
        AdapterError::DuplicateClassification {
            member_ordinal: 0,
            record_ordinal: 0
        }
    ));
}

#[test]
fn accountant_rejects_oversized_unknown_source_type() {
    let accountant = RecordAccountant::for_test([(0, 1)]);

    let error = accountant
        .classify_unknown(0, 0, &"x".repeat(257))
        .unwrap_err();

    assert!(matches!(
        error,
        AdapterError::MalformedRecord {
            member_ordinal: 0,
            record_ordinal: 0,
            field: "source-type"
        }
    ));
}

#[test]
fn accountant_rejects_empty_and_nul_unknown_source_types() {
    for source_type in ["", "future\0record"] {
        let accountant = RecordAccountant::for_test([(0, 1)]);
        let error = accountant.classify_unknown(0, 0, source_type).unwrap_err();
        assert!(matches!(
            error,
            AdapterError::MalformedRecord {
                member_ordinal: 0,
                record_ordinal: 0,
                field: "source-type"
            }
        ));
    }
}

#[test]
fn accountant_rejects_missing_classification_on_finish() {
    let accountant = RecordAccountant::for_test([(0, 2)]);

    accountant.classify_understood(0, 0).unwrap();
    let error = accountant.finish_for_test().unwrap_err();

    assert!(matches!(
        error,
        AdapterError::MissingClassification {
            member_ordinal: 0,
            record_ordinal: 1
        }
    ));
}

#[test]
fn run_adapter_allows_multiple_events_from_one_understood_record() {
    let bundle = make_bundle(vec![member(
        "primary",
        vec![br#"{"type":"message"}"#],
        false,
    )]);
    let context = default_context();

    let session = run_adapter(&MultiEventAdapter, &bundle, &context).unwrap();

    assert_eq!(session.events.len(), 2);
    assert_eq!(session.unknown_record_count, 0);
}

#[test]
fn run_adapter_rejects_missing_classification() {
    let bundle = make_bundle(vec![member(
        "primary",
        vec![br#"{"type":"message"}"#, br#"{"type":"response"}"#],
        false,
    )]);
    let context = default_context();

    let error = run_adapter(&DroppingAdapter, &bundle, &context).unwrap_err();

    assert!(matches!(
        error,
        AdapterError::MissingClassification {
            member_ordinal: 0,
            record_ordinal: 0
        }
    ));
}

#[test]
fn run_adapter_rejects_duplicate_classification() {
    let bundle = make_bundle(vec![member(
        "primary",
        vec![br#"{"type":"message"}"#],
        false,
    )]);
    let context = default_context();

    let error = run_adapter(&DuplicateClassificationAdapter, &bundle, &context).unwrap_err();

    assert!(matches!(
        error,
        AdapterError::DuplicateClassification {
            member_ordinal: 0,
            record_ordinal: 0
        }
    ));
}

#[test]
fn run_adapter_rejects_unknown_record_count_mismatch() {
    let bundle = make_bundle(vec![member(
        "primary",
        vec![br#"{"type":"message"}"#],
        false,
    )]);
    let context = default_context();

    let error = run_adapter(&WrongUnknownCountAdapter, &bundle, &context).unwrap_err();

    assert!(matches!(
        error,
        AdapterError::UnknownRecordCountMismatch {
            expected: 1,
            actual: 0
        }
    ));
}

#[test]
fn run_adapter_rejects_unknown_record_type_mismatch() {
    let bundle = make_bundle(vec![member(
        "primary",
        vec![br#"{"type":"message"}"#],
        false,
    )]);
    let context = default_context();

    let error = run_adapter(&WrongUnknownTypeAdapter, &bundle, &context).unwrap_err();

    assert!(matches!(error, AdapterError::UnknownRecordTypesMismatch));
}

#[test]
fn reusable_fixture_assertion_accepts_current_records() {
    let bundle = make_bundle(vec![member(
        "primary",
        vec![
            br#"{"type":"user","text":"hello"}"#,
            br#"{"type":"assistant","text":"hi"}"#,
        ],
        false,
    )]);
    let context = default_context();

    let session = assert_conforming_fixture(&FixtureAdapter, &bundle, &context, 2, 0);

    assert!(matches!(
        session.events.as_slice(),
        [
            ReplayEvent::User { text, .. },
            ReplayEvent::Assistant { markdown, .. }
        ] if text == "hello" && markdown == "hi"
    ));
}

#[test]
fn reusable_fixture_assertion_accepts_legacy_records() {
    let bundle = make_bundle(vec![member(
        "primary",
        vec![
            br#"{"role":"user","content":"legacy hello"}"#,
            br#"{"role":"assistant","content":"legacy hi"}"#,
        ],
        false,
    )]);
    let context = default_context();

    let session = assert_conforming_fixture(&FixtureAdapter, &bundle, &context, 2, 0);

    assert!(matches!(
        session.events.as_slice(),
        [
            ReplayEvent::User { text, .. },
            ReplayEvent::Assistant { markdown, .. }
        ] if text == "legacy hello" && markdown == "legacy hi"
    ));
}

#[test]
fn reusable_fixture_assertion_preserves_unknown_records() {
    let bundle = make_bundle(vec![member(
        "primary",
        vec![br#"{"type":"future-record","payload":"ignored"}"#],
        false,
    )]);
    let context = default_context();

    let session = assert_conforming_fixture(&FixtureAdapter, &bundle, &context, 1, 1);

    assert!(matches!(
        session.events.as_slice(),
        [ReplayEvent::Unknown { source_type, .. }] if source_type == "future-record"
    ));
}

#[test]
fn malformed_interior_record_returns_a_typed_position_without_payload_text() {
    let bundle = make_bundle(vec![member(
        "primary",
        vec![
            br#"{"type":"user","text":"before"}"#,
            br#"{"secret":"must-not-appear""#,
            br#"{"type":"assistant","text":"after"}"#,
        ],
        false,
    )]);
    let context = default_context();

    let error = run_adapter(&FixtureAdapter, &bundle, &context).unwrap_err();

    assert!(matches!(
        error,
        AdapterError::MalformedRecord {
            member_ordinal: 0,
            record_ordinal: 1,
            field: "json"
        }
    ));
    assert!(!error.to_string().contains("must-not-appear"));
}

#[test]
fn run_adapter_accepts_unknown_types_emitted_in_vendor_order() {
    let bundle = make_bundle(vec![member(
        "primary",
        vec![br#"{"ordinal":2}"#, br#"{"ordinal":1}"#],
        false,
    )]);
    let context = default_context();

    let session = run_adapter(&ReorderedUnknownAdapter, &bundle, &context).unwrap();

    assert!(matches!(
        session.events.as_slice(),
        [
            ReplayEvent::Unknown { source_type: first, .. },
            ReplayEvent::Unknown { source_type: second, .. }
        ] if first == "second-type" && second == "first-type"
    ));
}

#[test]
fn run_adapter_rejects_wrong_schema_version() {
    let bundle = make_bundle(vec![member("primary", vec![b"{}"], false)]);
    let error = run_adapter(
        &InvalidOutputAdapter(InvalidOutput::SchemaVersion),
        &bundle,
        &default_context(),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        AdapterError::InvalidOutput {
            reason: OutputReason::SchemaVersion
        }
    ));
}

#[test]
fn run_adapter_rejects_wrong_session_id() {
    let bundle = make_bundle(vec![member("primary", vec![b"{}"], false)]);
    let error = run_adapter(
        &InvalidOutputAdapter(InvalidOutput::SessionId),
        &bundle,
        &default_context(),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        AdapterError::InvalidOutput {
            reason: OutputReason::SessionId
        }
    ));
}

#[test]
fn run_adapter_rejects_wrong_session_source() {
    let bundle = make_bundle(vec![member("primary", vec![b"{}"], false)]);
    let error = run_adapter(
        &InvalidOutputAdapter(InvalidOutput::SessionSource),
        &bundle,
        &default_context(),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        AdapterError::InvalidOutput {
            reason: OutputReason::SessionSource
        }
    ));
}

#[test]
fn run_adapter_rejects_duplicate_output_event_ids() {
    let bundle = make_bundle(vec![member("primary", vec![b"{}"], false)]);
    let error = run_adapter(
        &InvalidOutputAdapter(InvalidOutput::DuplicateEventId),
        &bundle,
        &default_context(),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        AdapterError::InvalidOutput {
            reason: OutputReason::SessionInvariant
        }
    ));
}

#[test]
fn run_adapter_rejects_oversized_output_metadata() {
    let bundle = make_bundle(vec![member("primary", vec![b"{}"], false)]);
    let error = run_adapter(
        &InvalidOutputAdapter(InvalidOutput::OversizedTitle),
        &bundle,
        &default_context(),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        AdapterError::InvalidOutput {
            reason: OutputReason::SessionInvariant
        }
    ));
}

#[test]
fn run_adapter_rejects_output_event_limit_overflow() {
    let bundle = make_bundle(vec![member("primary", vec![b"{}"], false)]);
    let error = run_adapter(
        &InvalidOutputAdapter(InvalidOutput::EventLimit),
        &bundle,
        &default_context(),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        AdapterError::EventLimitExceeded { count: 100_001 }
    ));
}

#[test]
fn conformance_passes_for_valid_session() {
    let session = minimal_valid_session(
        vec![
            ReplayEvent::User {
                id: "e-0".to_owned(),
                at_ms: 0,
                text: "hello".to_owned(),
            },
            ReplayEvent::Assistant {
                id: "e-1".to_owned(),
                at_ms: 500,
                markdown: "hi".to_owned(),
            },
        ],
        1500,
    );

    assert!(check_conformance(&session, 1500).is_ok());
}

#[test]
fn conformance_returns_typed_non_monotonic_timestamp_violation() {
    let session = minimal_valid_session(
        vec![
            ReplayEvent::User {
                id: "e-0".to_owned(),
                at_ms: 1000,
                text: "x".to_owned(),
            },
            ReplayEvent::User {
                id: "e-1".to_owned(),
                at_ms: 500,
                text: "y".to_owned(),
            },
        ],
        1500,
    );

    let violations = check_conformance(&session, 1500).unwrap_err();
    assert!(violations.iter().any(|violation| matches!(
        violation,
        ConformanceViolation::NonMonotonicTimestamp {
            index: 1,
            at_ms: 500,
            previous_at_ms: 1000
        }
    )));
}

#[test]
fn conformance_returns_typed_duplicate_id_violation() {
    let session = minimal_valid_session(
        vec![
            ReplayEvent::User {
                id: "dup".to_owned(),
                at_ms: 0,
                text: "x".to_owned(),
            },
            ReplayEvent::User {
                id: "dup".to_owned(),
                at_ms: 0,
                text: "y".to_owned(),
            },
        ],
        1500,
    );

    let violations = check_conformance(&session, 1500).unwrap_err();
    assert!(violations.iter().any(|violation| matches!(
        violation,
        ConformanceViolation::DuplicateEventId { index: 1 }
    )));
}

#[test]
fn conformance_returns_typed_duration_violation() {
    let mut session = minimal_valid_session(
        vec![ReplayEvent::User {
            id: "e-0".to_owned(),
            at_ms: 200,
            text: "x".to_owned(),
        }],
        1500,
    );
    session.duration_ms = 200;

    let violations = check_conformance(&session, 1500).unwrap_err();
    assert!(violations.iter().any(|violation| matches!(
        violation,
        ConformanceViolation::DurationMismatch {
            expected: 1700,
            actual: 200
        }
    )));
}

#[test]
fn conformance_reports_empty_overflowing_and_over_limit_sessions() {
    let empty = minimal_valid_session(vec![], 1500);
    assert!(matches!(
        check_conformance(&empty, 1500).unwrap_err().as_slice(),
        [ConformanceViolation::EmptyEvents]
    ));

    let mut overflowing = minimal_valid_session(
        vec![ReplayEvent::User {
            id: "overflow".to_owned(),
            at_ms: u64::MAX,
            text: "synthetic".to_owned(),
        }],
        0,
    );
    overflowing.duration_ms = u64::MAX;
    let violations = check_conformance(&overflowing, 1500).unwrap_err();
    assert!(violations.contains(&ConformanceViolation::DurationOverflow));
    assert!(
        violations.contains(&ConformanceViolation::DurationLimitExceeded {
            duration_ms: u64::MAX,
        })
    );

    let too_many = minimal_valid_session(
        (0..=100_000)
            .map(|index| ReplayEvent::User {
                id: format!("event-{index}"),
                at_ms: 0,
                text: String::new(),
            })
            .collect(),
        1500,
    );
    assert!(
        check_conformance(&too_many, 1500)
            .unwrap_err()
            .contains(&ConformanceViolation::EventLimitExceeded { count: 100_001 })
    );
}

#[test]
fn conformance_fixture_ignores_a_truncated_final_record() {
    let tree = RepoTree::new();
    let path = tree.write(
        "session.jsonl",
        b"{\"type\":\"user\",\"text\":\"hello\"}\n{\"type\":\"assistant\",\"text\":\"hi\"}\n{\"type\":\"assistant\"",
    );
    let root = tree.root();
    let bundle = capture_bundle(&root, &[spec("primary", &path)], SnapshotLimits::default())
        .expect("bundle should be captured");
    let context = default_context();

    assert!(
        bundle
            .member("primary")
            .unwrap()
            .partial_final_record_ignored()
    );
    assert_conforming_fixture(&FixtureAdapter, &bundle, &context, 2, 0);
}

#[test]
fn snapshot_debug_output_redacts_ignored_partial_tail_bytes() {
    let tree = RepoTree::new();
    let path = tree.write(
        "session.jsonl",
        b"{\"type\":\"user\",\"text\":\"hello\"}\nsecret-ignored-tail",
    );
    let root = tree.root();
    let bundle = capture_bundle(&root, &[spec("primary", &path)], SnapshotLimits::default())
        .expect("bundle should be captured");

    let debug = format!("{bundle:?}");

    assert!(!debug.contains("bytes: ["));
    assert!(!debug.contains("secret-ignored-tail"));
    assert!(!debug.contains("[115, 101, 99, 114, 101, 116"));
}

#[test]
fn conformance_fixture_reads_only_the_prefix_captured_before_an_append() {
    let tree = RepoTree::new();
    let path = tree.write("live.jsonl", b"{\"type\":\"user\",\"text\":\"first\"}\n");
    let root = tree.root();
    let bundle = capture_bundle_with_hook(
        &root,
        &[spec("live", &path)],
        SnapshotLimits::default(),
        || {
            let mut writer = OpenOptions::new()
                .append(true)
                .open(&path)
                .expect("active writer should open");
            writer
                .write_all(b"{\"type\":\"assistant\",\"text\":\"appended\"}\n")
                .expect("active writer should append");
        },
    )
    .expect("bundle should be captured");
    let context = default_context();

    assert_eq!(bundle.record_count(), 1);
    let session = assert_conforming_fixture(&FixtureAdapter, &bundle, &context, 1, 0);
    assert!(matches!(
        session.events.as_slice(),
        [ReplayEvent::User { text, .. }] if text == "first"
    ));
    assert!(
        fs::read_to_string(path)
            .expect("live file should remain readable")
            .contains("appended")
    );
}

#[test]
fn run_adapter_accounts_for_a_nonempty_byte_member() {
    let tree = RepoTree::new();
    let path = tree.write("session.json", br#"{"type":"session"}"#);
    let root = tree.root();
    let bundle = capture_bundle(
        &root,
        &[byte_spec("primary", &path)],
        SnapshotLimits::default(),
    )
    .expect("byte bundle should be captured");

    let session = run_adapter(&ReferenceAdapter, &bundle, &default_context()).unwrap();

    assert_eq!(bundle.record_count(), 1);
    assert_eq!(session.events.len(), 1);
}

#[test]
fn rich_adapter_boundary_migrates_legacy_output_without_inventing_content() {
    let bundle = make_bundle(vec![member(
        "events",
        vec![br#"{"type":"user","text":"hello"}"#],
        false,
    )]);
    let context = default_context();

    let session = run_rich_adapter(&FixtureAdapter, &bundle, &context)
        .expect("legacy adapter output should migrate");

    assert_eq!(session.schema_version, 2);
    assert_eq!(session.entries.len(), 1);
    assert_eq!(
        session.content_availability.reasoning,
        crate::model::NormalizedReasoningAvailabilityV2::Unavailable
    );
    assert_eq!(
        session.content_availability.tool_details,
        crate::model::NormalizedContentAvailabilityValueV2::Unavailable
    );
}

#[test]
fn rich_adapter_boundary_rejects_every_invalid_output_class() {
    let empty = make_bundle(vec![]);
    assert_eq!(
        run_rich_adapter(&ReferenceAdapter, &empty, &default_context()),
        Err(AdapterError::EmptySession)
    );
    let bundle = make_bundle(vec![member(
        "events",
        vec![br#"{"type":"unknown"}"#],
        false,
    )]);
    for (fault, expected) in [
        (RichFault::Schema, OutputReason::SchemaVersion),
        (RichFault::Id, OutputReason::SessionId),
        (RichFault::Source, OutputReason::SessionSource),
        (RichFault::Duration, OutputReason::SessionInvariant),
    ] {
        assert_eq!(
            run_rich_adapter(&InvalidRichAdapter(fault), &bundle, &default_context()),
            Err(AdapterError::InvalidOutput { reason: expected })
        );
    }
    assert!(matches!(
        run_rich_adapter(
            &InvalidRichAdapter(RichFault::EntryLimit),
            &bundle,
            &default_context(),
        ),
        Err(AdapterError::EventLimitExceeded { count: 100_001 })
    ));
    assert!(matches!(
        run_rich_adapter(
            &InvalidRichAdapter(RichFault::UnknownCount),
            &bundle,
            &default_context(),
        ),
        Err(AdapterError::UnknownRecordCountMismatch { .. })
    ));
    assert_eq!(
        run_rich_adapter(
            &InvalidRichAdapter(RichFault::UnknownType),
            &bundle,
            &default_context(),
        ),
        Err(AdapterError::UnknownRecordTypesMismatch)
    );
}

#[test]
fn run_adapter_accounts_for_byte_members_in_a_mixed_bundle() {
    let tree = RepoTree::new();
    let metadata = tree.write("metadata.json", br#"{"type":"session"}"#);
    let transcript = tree.write("session.jsonl", b"{\"type\":\"message\"}\n");
    let root = tree.root();
    let bundle = capture_bundle(
        &root,
        &[
            byte_spec("metadata", &metadata),
            spec("transcript", &transcript),
        ],
        SnapshotLimits::default(),
    )
    .expect("mixed bundle should be captured");

    let session = run_adapter(&ReferenceAdapter, &bundle, &default_context()).unwrap();

    assert_eq!(bundle.record_count(), 2);
    assert_eq!(session.events.len(), 2);
}

#[test]
fn dropping_adapter_fails_accounting_on_captured_bundle() {
    let tree = RepoTree::new();
    let path = tree.write(
        "dropping.jsonl",
        b"{\"type\":\"message\"}\n{\"type\":\"response\"}\n",
    );
    let root = tree.root();
    let bundle = capture_bundle(&root, &[spec("primary", &path)], SnapshotLimits::default())
        .expect("bundle should be captured");
    let context = default_context();

    let error = run_adapter(&DroppingAdapter, &bundle, &context).unwrap_err();
    assert!(matches!(
        error,
        AdapterError::MissingClassification {
            member_ordinal: 0,
            record_ordinal: 0
        }
    ));
}

#[test]
fn adapter_error_display_uses_safe_structural_identifiers() {
    let error = AdapterError::MalformedRecord {
        member_ordinal: 3,
        record_ordinal: 42,
        field: "timestamp",
    };

    let message = error.to_string();
    assert!(!message.contains('/'));
    assert!(!message.contains('\\'));
    assert!(message.contains("member 3"));
    assert!(message.contains("record 42"));
}

#[test]
fn structural_violation_error_uses_reason_codes() {
    let error = AdapterError::StructuralViolation {
        reason: StructuralReason::MemberCountExceeded { count: 900 },
    };

    let message = error.to_string();
    assert!(message.contains("member-count-exceeded"));
    assert!(!message.contains("unexpected member count"));
}

#[test]
fn invalid_timestamp_diagnostic_is_typed_and_non_fatal() {
    let mut diagnostics = vec![];

    let result = normalize_timestamps(
        &[RawTimestamp::Invalid, RawTimestamp::Valid(2000)],
        &mut diagnostics,
    );

    assert_eq!(result, vec![0, 0]);
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "invalid-timestamp")
        .expect("invalid timestamp diagnostic should be emitted");
    assert_eq!(diagnostic.severity, DiagnosticSeverity::Warning);
}

#[test]
fn every_v1_adapter_declares_a_bounded_source_deletion_scope() {
    assert_eq!(
        artifact_declaration(&IndexedSessionSourceV1::ClaudeCode),
        DeletionArtifactDeclaration::PrimaryFile,
    );
    assert_eq!(
        artifact_declaration(&IndexedSessionSourceV1::Codex),
        DeletionArtifactDeclaration::PrimaryFile,
    );
    assert_eq!(
        artifact_declaration(&IndexedSessionSourceV1::CopilotCli),
        DeletionArtifactDeclaration::SessionDirectory {
            primary_file_name: "events.jsonl",
            max_artifacts: 64,
            max_depth: 8,
        },
    );
    assert_eq!(
        artifact_declaration(&IndexedSessionSourceV1::VscodeCopilot),
        DeletionArtifactDeclaration::SiblingFiles {
            extensions: &["json", "jsonl"],
        },
    );
}
