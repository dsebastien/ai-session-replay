use std::collections::{HashMap, HashSet};

use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::adapters::contract::{AdapterContext, AdapterError, RecordAccountant, SessionAdapter};
use crate::adapters::deletion::{DeletionArtifactDeclaration, SourceDeletionAdapter};
use crate::adapters::normalize::{
    EventIdGenerator, RawTimedRecord, compute_duration, finalize_rich_session,
    normalize_timestamps, raw_timestamp, structural_order,
};
use crate::io::SessionSnapshotBundle;
use crate::model::{
    DiagnosticSeverity, NormalizedEntryV2, NormalizedSessionV1, NormalizedSessionV2,
    NormalizedToolDetailV2, RelationshipKind, ReplayEvent, SessionRelationship, SessionSource,
    SourceDiagnostic, ToolStatus, migrate_normalized_session_v1,
};

const MAX_TEXT_CHARS: usize = 2_000_000;
const MAX_PATH_CHARS: usize = 32_768;
const MAX_BOUNDED_CHARS: usize = 256;
const MAX_RELATIONSHIPS: usize = 100;
const FORMAT_WARNING_CODE: &str = "claude-internal-format-unstable";

pub struct ClaudeAdapter;

impl SourceDeletionAdapter for ClaudeAdapter {
    fn deletion_artifacts(&self) -> DeletionArtifactDeclaration {
        DeletionArtifactDeclaration::PrimaryFile
    }
}

impl SessionAdapter for ClaudeAdapter {
    fn normalize(
        &self,
        bundle: &SessionSnapshotBundle,
        context: &AdapterContext,
        accountant: &RecordAccountant,
    ) -> Result<NormalizedSessionV1, AdapterError> {
        parse_claude_session(bundle, context, accountant)
    }

    fn normalize_rich(
        &self,
        bundle: &SessionSnapshotBundle,
        context: &AdapterContext,
        accountant: &RecordAccountant,
    ) -> Result<NormalizedSessionV2, AdapterError> {
        let legacy = parse_claude_session(bundle, context, accountant)?;
        let migrated =
            migrate_normalized_session_v1(legacy, context.terminal_hold_ms()).map_err(|_| {
                AdapterError::InvalidOutput {
                    reason: crate::adapters::contract::OutputReason::SessionInvariant,
                }
            })?;
        enrich_claude_session(migrated, bundle, context, accountant)
    }
}

#[derive(Default)]
struct ParsedMetadata {
    source_version: Option<String>,
    created_at: Option<String>,
    cwd: Option<String>,
    relationships: Vec<SessionRelationship>,
    relationship_keys: HashSet<String>,
}

struct PendingEvent {
    structural_position: (usize, usize),
    kind: PendingEventKind,
}

enum PendingEventKind {
    User(String),
    Assistant(String),
    Tool {
        name: String,
        status: ToolStatus,
        summary: String,
    },
    Unknown(String),
}

fn parse_claude_session(
    bundle: &SessionSnapshotBundle,
    context: &AdapterContext,
    accountant: &RecordAccountant,
) -> Result<NormalizedSessionV1, AdapterError> {
    let mut diagnostics = vec![SourceDiagnostic {
        code: FORMAT_WARNING_CODE.to_owned(),
        severity: DiagnosticSeverity::Warning,
        message: "Claude Code session data uses an internal, version-unstable format; replay is best-effort."
            .to_owned(),
    }];
    let mut metadata = ParsedMetadata::default();
    let mut timed_records = Vec::new();
    let mut pending_events = Vec::new();
    let mut tool_names = HashMap::new();

    for (member_ordinal, member) in bundle.members().enumerate() {
        for (record_ordinal, bytes) in member.records().enumerate() {
            let value: Value =
                serde_json::from_slice(bytes).map_err(|_| AdapterError::MalformedRecord {
                    member_ordinal,
                    record_ordinal,
                    field: "json",
                })?;
            let position = (member_ordinal, record_ordinal);
            let record_type = record_type(&value);

            extract_metadata(&value, &mut metadata);
            timed_records.push(RawTimedRecord {
                vendor_ordinal: value.get("ordinal").and_then(Value::as_u64),
                timestamp_rfc3339: value
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .map(|timestamp| bounded_string(timestamp, 128)),
                structural_position: position,
                kind: record_type.clone(),
            });

            let parsed = parse_record(&value, position, &mut tool_names)?;
            if let Some(source_type) = parsed.iter().find_map(|event| match &event.kind {
                PendingEventKind::Unknown(source_type) => Some(source_type.as_str()),
                _ => None,
            }) {
                accountant.classify_unknown(member_ordinal, record_ordinal, source_type)?;
            } else {
                accountant.classify_understood(member_ordinal, record_ordinal)?;
            }
            pending_events.extend(parsed);
            if pending_events.len() > 100_000 {
                return Err(AdapterError::EventLimitExceeded {
                    count: pending_events.len(),
                });
            }
        }
    }

    structural_order(&mut timed_records, &mut diagnostics);
    let position_order: HashMap<(usize, usize), usize> = timed_records
        .iter()
        .enumerate()
        .map(|(index, record)| (record.structural_position, index))
        .collect();
    pending_events.sort_by_key(|event| {
        position_order
            .get(&event.structural_position)
            .copied()
            .unwrap_or(usize::MAX)
    });

    let timestamps = timed_records
        .iter()
        .map(|record| raw_timestamp(record.timestamp_rfc3339.as_deref()))
        .collect::<Vec<_>>();
    let record_at_ms = normalize_timestamps(&timestamps, &mut diagnostics);
    let mut id_generator = EventIdGenerator::new(context.session_id());
    let mut events = Vec::with_capacity(pending_events.len());
    let mut unknown_record_count = 0_u64;

    for pending in pending_events {
        let (member_ordinal, record_ordinal) = pending.structural_position;
        let at_ms = position_order
            .get(&pending.structural_position)
            .and_then(|index| record_at_ms.get(*index))
            .copied()
            .unwrap_or(0);
        let event = match pending.kind {
            PendingEventKind::User(text) => {
                let id = id_generator.next_id_for_content(
                    member_ordinal,
                    record_ordinal,
                    "user",
                    &[&text],
                );
                ReplayEvent::User { id, at_ms, text }
            }
            PendingEventKind::Assistant(markdown) => {
                let id = id_generator.next_id_for_content(
                    member_ordinal,
                    record_ordinal,
                    "assistant",
                    &[&markdown],
                );
                ReplayEvent::Assistant {
                    id,
                    at_ms,
                    markdown,
                }
            }
            PendingEventKind::Tool {
                name,
                status,
                summary,
            } => {
                let status_identity = match status {
                    ToolStatus::Running => "running",
                    ToolStatus::Succeeded => "succeeded",
                    ToolStatus::Failed => "failed",
                };
                let id = id_generator.next_id_for_content(
                    member_ordinal,
                    record_ordinal,
                    "tool",
                    &[&name, status_identity, &summary],
                );
                ReplayEvent::Tool {
                    id,
                    at_ms,
                    name,
                    status,
                    summary,
                }
            }
            PendingEventKind::Unknown(source_type) => {
                unknown_record_count += 1;
                let id = id_generator.next_id_for_content(
                    member_ordinal,
                    record_ordinal,
                    "unknown",
                    &[&source_type],
                );
                ReplayEvent::Unknown {
                    id,
                    at_ms,
                    source_type,
                }
            }
        };
        events.push(event);
    }

    if events.is_empty() {
        return Err(AdapterError::EmptySession);
    }

    let duration_ms = compute_duration(
        &events.iter().map(ReplayEvent::at_ms).collect::<Vec<_>>(),
        context.terminal_hold_ms(),
    )?;

    Ok(NormalizedSessionV1 {
        schema_version: 1,
        id: context.session_id().to_owned(),
        source: SessionSource::ClaudeCode,
        source_version: metadata.source_version,
        title: bounded_string(context.session_id(), 512),
        created_at: metadata.created_at,
        cwd: metadata.cwd,
        relationships: metadata.relationships,
        events,
        duration_ms,
        diagnostics,
        unknown_record_count,
    })
}

#[derive(Debug)]
struct PositionedRichOverlay {
    vendor_ordinal: Option<u64>,
    structural_position: (usize, usize),
    overlay: RichOverlay,
}

#[derive(Debug)]
enum RichOverlay {
    Keep {
        key_kind: &'static str,
        vendor_id: Option<String>,
    },
    Reasoning {
        vendor_id: Option<String>,
        text: String,
    },
    Tool {
        vendor_id: Option<String>,
        arguments: Option<String>,
        result: Option<String>,
    },
}

fn enrich_claude_session(
    mut session: NormalizedSessionV2,
    bundle: &SessionSnapshotBundle,
    context: &AdapterContext,
    accountant: &RecordAccountant,
) -> Result<NormalizedSessionV2, AdapterError> {
    let mut overlays = Vec::new();
    for (member_ordinal, member) in bundle.members().enumerate() {
        for (record_ordinal, bytes) in member.records().enumerate() {
            let value: Value =
                serde_json::from_slice(bytes).map_err(|_| AdapterError::MalformedRecord {
                    member_ordinal,
                    record_ordinal,
                    field: "json",
                })?;
            collect_claude_overlays(
                &value,
                (member_ordinal, record_ordinal),
                &mut overlays,
                accountant,
            )?;
        }
    }
    sort_rich_overlays(&mut overlays);
    if overlays.len() != session.entries.len() {
        return Err(AdapterError::InvalidOutput {
            reason: crate::adapters::contract::OutputReason::SessionInvariant,
        });
    }

    let mut key_generator = EventIdGenerator::new(context.session_id());
    for (entry, positioned) in session.entries.iter_mut().zip(overlays) {
        let (key_kind, vendor_id) = match &positioned.overlay {
            RichOverlay::Keep {
                key_kind,
                vendor_id,
            } => (*key_kind, vendor_id.as_deref()),
            RichOverlay::Reasoning { vendor_id, .. } => ("reasoning", vendor_id.as_deref()),
            RichOverlay::Tool { vendor_id, .. } => ("tool-call", vendor_id.as_deref()),
        };
        if let Some(vendor_id) = vendor_id.filter(|value| !value.is_empty()) {
            set_entry_key(entry, key_generator.next_id_for_vendor(key_kind, vendor_id));
        }
        match positioned.overlay {
            RichOverlay::Keep { .. } => {}
            RichOverlay::Reasoning { text, .. } => {
                let entry_key = entry.entry_key().to_owned();
                let at_ms = entry.at_ms();
                *entry = NormalizedEntryV2::Reasoning {
                    entry_key,
                    at_ms,
                    text,
                };
            }
            RichOverlay::Tool {
                arguments, result, ..
            } => {
                let NormalizedEntryV2::ToolCall { detail, .. } = entry else {
                    return Err(AdapterError::InvalidOutput {
                        reason: crate::adapters::contract::OutputReason::SessionInvariant,
                    });
                };
                if arguments.is_some() || result.is_some() {
                    *detail = NormalizedToolDetailV2::Available { arguments, result };
                }
            }
        }
    }

    finalize_rich_session(&mut session)?;
    Ok(session)
}

fn collect_claude_overlays(
    value: &Value,
    position: (usize, usize),
    overlays: &mut Vec<PositionedRichOverlay>,
    accountant: &RecordAccountant,
) -> Result<(), AdapterError> {
    let record_kind = value.get("type").and_then(Value::as_str);
    let role = match record_kind {
        Some("user" | "assistant") => record_kind,
        None => value.get("role").and_then(Value::as_str),
        _ => None,
    };
    let vendor_ordinal = value.get("ordinal").and_then(Value::as_u64);
    let vendor_id = value.get("uuid").and_then(Value::as_str).map(str::to_owned);
    if let Some(role @ ("user" | "assistant")) = role {
        let content = record_kind
            .and_then(|_| value.get("message"))
            .and_then(|message| message.get("content"))
            .or_else(|| value.get("content"));
        collect_claude_content_overlays(
            content,
            role,
            vendor_ordinal,
            vendor_id,
            position,
            overlays,
            accountant,
        )?;
    } else if !matches!(
        record_kind,
        Some(
            "system"
                | "progress"
                | "summary"
                | "file-history-snapshot"
                | "queue-operation"
                | "permission-mode"
                | "last-prompt"
                | "attachment"
        )
    ) {
        overlays.push(PositionedRichOverlay {
            vendor_ordinal,
            structural_position: position,
            overlay: RichOverlay::Keep {
                key_kind: "unknown",
                vendor_id,
            },
        });
    }
    Ok(())
}

fn collect_claude_content_overlays(
    content: Option<&Value>,
    role: &str,
    vendor_ordinal: Option<u64>,
    vendor_id: Option<String>,
    position: (usize, usize),
    overlays: &mut Vec<PositionedRichOverlay>,
    accountant: &RecordAccountant,
) -> Result<(), AdapterError> {
    let Some(content) = content else {
        return Ok(());
    };
    if content.is_string() {
        overlays.push(PositionedRichOverlay {
            vendor_ordinal,
            structural_position: position,
            overlay: RichOverlay::Keep {
                key_kind: if role == "assistant" {
                    "assistant"
                } else {
                    "user"
                },
                vendor_id,
            },
        });
        return Ok(());
    }
    let Some(blocks) = content.as_array() else {
        return Ok(());
    };
    let mut reasoning = Vec::new();
    let mut has_other_unknown = false;
    for block in blocks {
        let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
        let block_id = block
            .get("id")
            .or_else(|| block.get("tool_use_id"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| vendor_id.clone());
        let overlay = match block_type {
            "text" => Some(RichOverlay::Keep {
                key_kind: if role == "assistant" {
                    "assistant"
                } else {
                    "user"
                },
                vendor_id: block_id,
            }),
            "tool_use" => Some(RichOverlay::Tool {
                vendor_id: block_id,
                arguments: bounded_value(block.get("input")),
                result: None,
            }),
            "tool_result" => Some(RichOverlay::Tool {
                vendor_id: block_id,
                arguments: None,
                result: claude_result_text(block.get("content")),
            }),
            "thinking" => {
                if let Some(text) = block
                    .get("thinking")
                    .or_else(|| block.get("text"))
                    .and_then(Value::as_str)
                    .map(|text| bounded_string(text, MAX_TEXT_CHARS))
                    .filter(|text| !text.is_empty())
                {
                    reasoning.push(text);
                }
                None
            }
            _ => {
                has_other_unknown = true;
                None
            }
        };
        if let Some(overlay) = overlay {
            overlays.push(PositionedRichOverlay {
                vendor_ordinal,
                structural_position: position,
                overlay,
            });
        }
    }
    if has_other_unknown || reasoning.is_empty() {
        if has_other_unknown
            || blocks.iter().any(|block| {
                !matches!(
                    block.get("type").and_then(Value::as_str),
                    Some("text" | "tool_use" | "tool_result")
                )
            })
        {
            overlays.push(PositionedRichOverlay {
                vendor_ordinal,
                structural_position: position,
                overlay: RichOverlay::Keep {
                    key_kind: "unknown",
                    vendor_id,
                },
            });
        }
    } else {
        accountant.promote_unknown_to_understood(position.0, position.1)?;
        overlays.push(PositionedRichOverlay {
            vendor_ordinal,
            structural_position: position,
            overlay: RichOverlay::Reasoning {
                vendor_id,
                text: bounded_string(&reasoning.join("\n"), MAX_TEXT_CHARS),
            },
        });
    }
    Ok(())
}

fn sort_rich_overlays(overlays: &mut [PositionedRichOverlay]) {
    let all_have_unique_ordinals = {
        let mut seen = HashSet::new();
        !overlays.is_empty()
            && overlays.iter().all(|overlay| {
                overlay
                    .vendor_ordinal
                    .is_some_and(|value| seen.insert(value))
            })
    };
    if all_have_unique_ordinals {
        overlays.sort_by_key(|overlay| overlay.vendor_ordinal.unwrap_or_default());
    } else {
        overlays.sort_by_key(|overlay| overlay.structural_position);
    }
}

fn claude_result_text(content: Option<&Value>) -> Option<String> {
    let content = content?;
    if let Some(text) = content.as_str() {
        return Some(bounded_string(text, MAX_TEXT_CHARS));
    }
    let parts = content.as_array()?.iter().filter_map(|part| {
        (part.get("type").and_then(Value::as_str) == Some("text"))
            .then(|| part.get("text").and_then(Value::as_str))
            .flatten()
    });
    let result = parts.collect::<Vec<_>>().join("\n");
    (!result.is_empty()).then(|| bounded_string(&result, MAX_TEXT_CHARS))
}

fn bounded_value(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if value.is_null() {
        return None;
    }
    let text = value
        .as_str()
        .map(str::to_owned)
        .or_else(|| serde_json::to_string(value).ok())?;
    Some(bounded_string(&text, MAX_TEXT_CHARS))
}

fn set_entry_key(entry: &mut NormalizedEntryV2, entry_key: String) {
    match entry {
        NormalizedEntryV2::User {
            entry_key: current, ..
        }
        | NormalizedEntryV2::Assistant {
            entry_key: current, ..
        }
        | NormalizedEntryV2::Reasoning {
            entry_key: current, ..
        }
        | NormalizedEntryV2::ToolCall {
            entry_key: current, ..
        }
        | NormalizedEntryV2::FileChange {
            entry_key: current, ..
        }
        | NormalizedEntryV2::Unknown {
            entry_key: current, ..
        } => *current = entry_key,
    }
}

fn record_type(value: &Value) -> String {
    value
        .get("type")
        .and_then(Value::as_str)
        .or_else(|| value.get("role").and_then(Value::as_str))
        .map(|value| bounded_nonempty(value, "unknown-record", MAX_BOUNDED_CHARS))
        .unwrap_or_else(|| "untyped-record".to_owned())
}

fn parse_record(
    value: &Value,
    position: (usize, usize),
    tool_names: &mut HashMap<String, String>,
) -> Result<Vec<PendingEvent>, AdapterError> {
    let explicit_type = value.get("type").and_then(Value::as_str);
    match explicit_type {
        Some("user") | Some("assistant") => {
            let message = value.get("message").ok_or(AdapterError::MalformedRecord {
                member_ordinal: position.0,
                record_ordinal: position.1,
                field: "message",
            })?;
            let role = explicit_type.expect("matched a message type");
            parse_message_content(message.get("content"), role, position, tool_names)
        }
        Some(
            "system"
            | "progress"
            | "summary"
            | "file-history-snapshot"
            | "queue-operation"
            | "permission-mode"
            | "last-prompt"
            | "attachment",
        ) => Ok(Vec::new()),
        Some(other) => Ok(vec![pending_unknown(position, other)]),
        None => match value.get("role").and_then(Value::as_str) {
            Some(role @ ("user" | "assistant")) => {
                parse_message_content(value.get("content"), role, position, tool_names)
            }
            Some(role) => {
                let role = bounded_string(role, MAX_BOUNDED_CHARS);
                Ok(vec![pending_unknown(position, &format!("legacy/{role}"))])
            }
            None => Ok(vec![pending_unknown(position, "untyped-record")]),
        },
    }
}

fn parse_message_content(
    content: Option<&Value>,
    role: &str,
    position: (usize, usize),
    tool_names: &mut HashMap<String, String>,
) -> Result<Vec<PendingEvent>, AdapterError> {
    let content = content.ok_or(AdapterError::MalformedRecord {
        member_ordinal: position.0,
        record_ordinal: position.1,
        field: "message-content",
    })?;

    if let Some(text) = content.as_str() {
        return Ok(vec![pending_text(position, role, text)]);
    }

    let blocks = content.as_array().ok_or(AdapterError::MalformedRecord {
        member_ordinal: position.0,
        record_ordinal: position.1,
        field: "message-content",
    })?;
    let mut events = Vec::new();
    let mut unknown_type = None;

    for block in blocks {
        let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
        match block_type {
            "text" => {
                let text = block.get("text").and_then(Value::as_str).ok_or(
                    AdapterError::MalformedRecord {
                        member_ordinal: position.0,
                        record_ordinal: position.1,
                        field: "message-text",
                    },
                )?;
                events.push(pending_text(position, role, text));
            }
            "tool_use" => {
                let name = block.get("name").and_then(Value::as_str).ok_or(
                    AdapterError::MalformedRecord {
                        member_ordinal: position.0,
                        record_ordinal: position.1,
                        field: "tool-name",
                    },
                )?;
                let name = bounded_nonempty(name, "tool", MAX_BOUNDED_CHARS);
                if let Some(tool_id) = block.get("id").and_then(Value::as_str) {
                    tool_names.insert(bounded_string(tool_id, MAX_BOUNDED_CHARS), name.clone());
                }
                events.push(PendingEvent {
                    structural_position: position,
                    kind: PendingEventKind::Tool {
                        name,
                        status: ToolStatus::Running,
                        summary: "Tool invocation recorded".to_owned(),
                    },
                });
            }
            "tool_result" => {
                let name = block
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .map(|tool_id| bounded_string(tool_id, MAX_BOUNDED_CHARS))
                    .and_then(|tool_id| tool_names.get(&tool_id).cloned())
                    .unwrap_or_else(|| "tool-result".to_owned());
                let status = if block.get("is_error").and_then(Value::as_bool) == Some(true) {
                    ToolStatus::Failed
                } else {
                    ToolStatus::Succeeded
                };
                events.push(PendingEvent {
                    structural_position: position,
                    kind: PendingEventKind::Tool {
                        name,
                        status,
                        summary: "Tool result recorded".to_owned(),
                    },
                });
            }
            other => {
                unknown_type.get_or_insert_with(|| {
                    let other = bounded_string(other, MAX_BOUNDED_CHARS);
                    bounded_nonempty(
                        &format!("message/{role}/{other}"),
                        "message/unknown-block",
                        MAX_BOUNDED_CHARS,
                    )
                });
            }
        }
    }

    if let Some(source_type) = unknown_type {
        events.push(PendingEvent {
            structural_position: position,
            kind: PendingEventKind::Unknown(source_type),
        });
    }
    Ok(events)
}

fn pending_text(position: (usize, usize), role: &str, text: &str) -> PendingEvent {
    let text = bounded_string(text, MAX_TEXT_CHARS);
    let kind = if role == "assistant" {
        PendingEventKind::Assistant(text)
    } else {
        PendingEventKind::User(text)
    };
    PendingEvent {
        structural_position: position,
        kind,
    }
}

fn pending_unknown(position: (usize, usize), source_type: &str) -> PendingEvent {
    PendingEvent {
        structural_position: position,
        kind: PendingEventKind::Unknown(bounded_nonempty(
            source_type,
            "unknown-record",
            MAX_BOUNDED_CHARS,
        )),
    }
}

fn extract_metadata(value: &Value, metadata: &mut ParsedMetadata) {
    if metadata.source_version.is_none() {
        metadata.source_version = value
            .get("version")
            .or_else(|| value.get("sourceVersion"))
            .and_then(Value::as_str)
            .map(|value| bounded_string(value, 128));
    }
    if metadata.created_at.is_none() {
        metadata.created_at = value
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(canonical_timestamp);
    }
    if metadata.cwd.is_none() {
        metadata.cwd = value
            .get("cwd")
            .and_then(Value::as_str)
            .map(|value| bounded_string(value, MAX_PATH_CHARS));
    }

    add_relationship(
        value,
        &["parentSessionId", "parent_session_id"],
        RelationshipKind::Parent,
        metadata,
    );
    add_relationship(
        value,
        &["forkedFromSessionId", "forked_from_session_id"],
        RelationshipKind::Fork,
        metadata,
    );
}

fn add_relationship(
    value: &Value,
    fields: &[&str],
    kind: RelationshipKind,
    metadata: &mut ParsedMetadata,
) {
    if metadata.relationships.len() >= MAX_RELATIONSHIPS {
        return;
    }
    let Some(session_id) = fields
        .iter()
        .find_map(|field| value.get(*field).and_then(Value::as_str))
        .map(|session_id| bounded_string(session_id, MAX_BOUNDED_CHARS))
        .filter(|session_id| !session_id.is_empty())
    else {
        return;
    };
    let key = format!(
        "{}:{session_id}",
        match kind {
            RelationshipKind::Parent => "parent",
            RelationshipKind::Fork => "fork",
        }
    );
    if metadata.relationship_keys.insert(key) {
        metadata
            .relationships
            .push(SessionRelationship { kind, session_id });
    }
}

fn canonical_timestamp(value: &str) -> Option<String> {
    let parsed = OffsetDateTime::parse(value, &Rfc3339)
        .ok()?
        .to_offset(time::UtcOffset::UTC);
    Some(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        parsed.year(),
        u8::from(parsed.month()),
        parsed.day(),
        parsed.hour(),
        parsed.minute(),
        parsed.second(),
        parsed.millisecond(),
    ))
}

fn bounded_nonempty(input: &str, fallback: &str, max_chars: usize) -> String {
    let value = bounded_string(input, max_chars);
    if value.is_empty() {
        fallback.to_owned()
    } else {
        value
    }
}

fn bounded_string(input: &str, max_chars: usize) -> String {
    input
        .chars()
        .filter(|character| *character != '\0')
        .take(max_chars)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        ClaudeAdapter, PositionedRichOverlay, RichOverlay, collect_claude_content_overlays,
        sort_rich_overlays,
    };
    use crate::adapters::conformance::assert_conforming_fixture;
    use crate::adapters::contract::{
        AdapterContext, AdapterError, RecordAccountant, run_adapter, run_rich_adapter,
    };
    use crate::io::{
        BundleMemberSpec, ContentKind, LocalRoot, MemberCompression, MemberRole,
        SessionSnapshotBundle, SnapshotLimits, SyntheticMember, capture_bundle,
        capture_bundle_with_hook,
    };
    use crate::model::{
        DiagnosticSeverity, RelationshipKind, ReplayEvent, SessionSource, ToolStatus,
    };

    const CURRENT_FIXTURE: &str = include_str!("../../../tests/fixtures/claude-current.jsonl");
    const UNKNOWN_FIXTURE: &str = include_str!("../../../tests/fixtures/claude-unknown.jsonl");
    const RICH_FIXTURE: &str = include_str!("../../../tests/fixtures/claude-rich.jsonl");
    const FORMAT_WARNING_CODE: &str = "claude-internal-format-unstable";

    fn context() -> AdapterContext {
        AdapterContext::new(
            "claude-session-1".to_owned(),
            SessionSource::ClaudeCode,
            1_500,
        )
        .expect("test context should be valid")
    }

    fn member_from_jsonl(id: &str, jsonl: &str, partial_final: bool) -> SyntheticMember {
        SyntheticMember {
            id: id.to_owned(),
            logical_name: format!("{id}.jsonl"),
            records: jsonl.lines().map(|line| line.as_bytes().to_vec()).collect(),
            partial_final,
        }
    }

    fn bundle_from_jsonl(jsonl: &str) -> SessionSnapshotBundle {
        SessionSnapshotBundle::synthetic(vec![member_from_jsonl("primary", jsonl, false)])
    }

    fn assert_format_warning(session: &crate::model::NormalizedSessionV1) {
        assert!(session.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == FORMAT_WARNING_CODE
                && diagnostic.severity == DiagnosticSeverity::Warning
                && !diagnostic.message.contains("C:\\")
        }));
    }

    #[test]
    fn rich_content_overlay_handles_sparse_mixed_and_ordered_blocks() {
        let accountant = RecordAccountant::new([(0, 6)]);
        for record in 0..6 {
            accountant.classify_unknown(0, record, "synthetic").unwrap();
        }
        let mut overlays = Vec::new();
        collect_claude_content_overlays(
            None,
            "assistant",
            None,
            None,
            (0, 0),
            &mut overlays,
            &accountant,
        )
        .unwrap();
        collect_claude_content_overlays(
            Some(&serde_json::json!("plain")),
            "assistant",
            Some(2),
            Some("message-1".to_owned()),
            (0, 1),
            &mut overlays,
            &accountant,
        )
        .unwrap();
        collect_claude_content_overlays(
            Some(&serde_json::json!({"unexpected":true})),
            "user",
            None,
            None,
            (0, 2),
            &mut overlays,
            &accountant,
        )
        .unwrap();
        collect_claude_content_overlays(
            Some(&serde_json::json!([{"type":"thinking","text":"trace"}])),
            "assistant",
            Some(1),
            Some("thought-1".to_owned()),
            (0, 3),
            &mut overlays,
            &accountant,
        )
        .unwrap();
        collect_claude_content_overlays(
            Some(&serde_json::json!([
                {"type":"thinking","thinking":"kept conservative"},
                {"type":"future_block"}
            ])),
            "assistant",
            None,
            None,
            (0, 4),
            &mut overlays,
            &accountant,
        )
        .unwrap();
        collect_claude_content_overlays(
            Some(&serde_json::json!([])),
            "user",
            None,
            None,
            (0, 5),
            &mut overlays,
            &accountant,
        )
        .unwrap();

        assert!(
            overlays
                .iter()
                .any(|item| matches!(&item.overlay, RichOverlay::Reasoning { .. }))
        );
        assert!(overlays.iter().any(|item| matches!(
            &item.overlay,
            RichOverlay::Keep {
                key_kind: "unknown",
                ..
            }
        )));
        let mut sortable = vec![
            PositionedRichOverlay {
                vendor_ordinal: Some(2),
                structural_position: (0, 0),
                overlay: RichOverlay::Keep {
                    key_kind: "user",
                    vendor_id: None,
                },
            },
            PositionedRichOverlay {
                vendor_ordinal: Some(1),
                structural_position: (0, 1),
                overlay: RichOverlay::Keep {
                    key_kind: "user",
                    vendor_id: None,
                },
            },
        ];
        sort_rich_overlays(&mut sortable);
        assert_eq!(sortable[0].vendor_ordinal, Some(1));
        assert_eq!(sortable[1].vendor_ordinal, Some(2));
    }

    #[test]
    fn current_fixture_normalizes_messages_tools_metadata_and_relationships() {
        let bundle = bundle_from_jsonl(CURRENT_FIXTURE);

        let session = assert_conforming_fixture(&ClaudeAdapter, &bundle, &context(), 4, 0);

        assert_eq!(session.source, SessionSource::ClaudeCode);
        assert_eq!(session.source_version.as_deref(), Some("2.1.0"));
        assert_eq!(
            session.created_at.as_deref(),
            Some("2026-08-01T10:00:00.000Z")
        );
        assert_eq!(session.cwd.as_deref(), Some("C:\\synthetic\\project"));
        assert!(matches!(
            session.events.as_slice(),
            [
                ReplayEvent::User { text, at_ms: 0, .. },
                ReplayEvent::Assistant { markdown, at_ms: 500, .. },
                ReplayEvent::Tool { name: call_name, status: ToolStatus::Running, at_ms: 500, .. },
                ReplayEvent::Tool { name: result_name, status: ToolStatus::Succeeded, at_ms: 1_000, .. },
            ] if text == "Build a safe demo."
                && markdown == "I will inspect the synthetic project."
                && call_name == "Read"
                && result_name == "Read"
        ));
        assert_eq!(session.relationships.len(), 2);
        assert!(session.relationships.iter().any(|relationship| {
            relationship.kind == RelationshipKind::Parent
                && relationship.session_id == "session-parent"
        }));
        assert!(session.relationships.iter().any(|relationship| {
            relationship.kind == RelationshipKind::Fork && relationship.session_id == "session-fork"
        }));
        assert!(
            session
                .relationships
                .iter()
                .all(|relationship| relationship.session_id != "message-parent-not-session")
        );
        assert_format_warning(&session);
    }

    #[test]
    fn rich_fixture_preserves_thinking_and_tool_payloads_without_raw_records() {
        let session = run_rich_adapter(
            &ClaudeAdapter,
            &bundle_from_jsonl(RICH_FIXTURE),
            &AdapterContext::new("claude-rich".to_owned(), SessionSource::ClaudeCode, 1_500)
                .unwrap(),
        )
        .expect("rich Claude fixture should normalize");

        assert_eq!(session.unknown_record_count, 1);
        assert_eq!(
            session.content_availability.reasoning,
            crate::model::NormalizedReasoningAvailabilityV2::Available
        );
        assert_eq!(
            session.content_availability.tool_details,
            crate::model::NormalizedContentAvailabilityValueV2::Available
        );
        assert!(session.entries.iter().any(|entry| matches!(
            entry,
            crate::model::NormalizedEntryV2::Reasoning { text, .. }
                if text == "Use only explicit fixture data."
        )));
        assert!(session.entries.iter().any(|entry| matches!(
            entry,
            crate::model::NormalizedEntryV2::ToolCall {
                detail: crate::model::NormalizedToolDetailV2::Available {
                    arguments: Some(arguments),
                    result: None,
                },
                ..
            } if arguments.contains("synthetic.txt")
        )));
        assert!(session.entries.iter().any(|entry| matches!(
            entry,
            crate::model::NormalizedEntryV2::ToolCall {
                detail: crate::model::NormalizedToolDetailV2::Available {
                    arguments: None,
                    result: Some(result),
                },
                ..
            } if result == "Synthetic tool output."
        )));
        let serialized = serde_json::to_string(&session).expect("session should serialize");
        assert!(!serialized.contains("never persist this"));
        assert!(!serialized.contains("do not persist"));
    }

    #[test]
    fn rich_claude_keys_survive_append_only_growth() {
        let appended = format!(
            "{RICH_FIXTURE}{}\n",
            r#"{"type":"assistant","uuid":"message-appended","sessionId":"claude-rich","timestamp":"2026-08-01T10:00:02.000Z","message":{"role":"assistant","content":"Continue."}}"#
        );
        let context =
            AdapterContext::new("claude-rich".to_owned(), SessionSource::ClaudeCode, 1_500)
                .unwrap();
        let before = run_rich_adapter(&ClaudeAdapter, &bundle_from_jsonl(RICH_FIXTURE), &context)
            .expect("base fixture should normalize");
        let after = run_rich_adapter(&ClaudeAdapter, &bundle_from_jsonl(&appended), &context)
            .expect("appended fixture should normalize");

        assert_eq!(before.entries.len() + 1, after.entries.len());
        assert_eq!(
            before
                .entries
                .iter()
                .map(crate::model::NormalizedEntryV2::entry_key)
                .collect::<Vec<_>>(),
            after.entries[..before.entries.len()]
                .iter()
                .map(crate::model::NormalizedEntryV2::entry_key)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn legacy_like_top_level_roles_conform_and_still_warn() {
        let bundle = bundle_from_jsonl(concat!(
            "{\"role\":\"user\",\"content\":\"Legacy hello.\",\"timestamp\":\"2026-08-03T12:00:00Z\",\"version\":\"legacy-like\"}\n",
            "{\"role\":\"assistant\",\"content\":\"Legacy answer.\",\"timestamp\":\"2026-08-03T12:00:02Z\"}\n",
        ));

        let session = assert_conforming_fixture(&ClaudeAdapter, &bundle, &context(), 2, 0);

        assert!(matches!(
            session.events.as_slice(),
            [
                ReplayEvent::User { text, at_ms: 0, .. },
                ReplayEvent::Assistant { markdown, at_ms: 2_000, .. },
            ] if text == "Legacy hello." && markdown == "Legacy answer."
        ));
        assert_format_warning(&session);
    }

    #[test]
    fn unknown_complete_record_is_preserved_by_type_and_accounting() {
        let bundle = bundle_from_jsonl(UNKNOWN_FIXTURE);

        let session = assert_conforming_fixture(&ClaudeAdapter, &bundle, &context(), 2, 1);

        assert!(matches!(
            session.events.as_slice(),
            [ReplayEvent::User { .. }, ReplayEvent::Unknown { source_type, .. }]
                if source_type == "future-record"
        ));
        assert_format_warning(&session);
    }

    #[test]
    fn nested_unknown_and_failed_tool_without_ids_are_bounded_and_accounted() {
        let bundle = bundle_from_jsonl(concat!(
            "{\"type\":\"user\",\"message\":{\"content\":[",
            "{\"type\":\"tool_use\",\"name\":\"SyntheticTool\",\"input\":{}},",
            "{\"type\":\"tool_result\",\"is_error\":true,\"content\":\"ignored\"},",
            "{\"type\":\"future-content\",\"data\":\"ignored\"}",
            "]}}\n",
        ));

        let session = assert_conforming_fixture(&ClaudeAdapter, &bundle, &context(), 3, 1);

        assert_eq!(session.unknown_record_count, 1);
    }

    #[test]
    fn known_metadata_without_replayable_events_is_empty() {
        let bundle = bundle_from_jsonl(
            "{\"type\":\"system\",\"subtype\":\"init\",\"version\":\"synthetic\"}\n",
        );

        assert_eq!(
            run_adapter(&ClaudeAdapter, &bundle, &context()).unwrap_err(),
            AdapterError::EmptySession
        );
    }

    #[test]
    fn known_metadata_records_do_not_become_unsupported_entries() {
        let bundle = bundle_from_jsonl(concat!(
            "{\"type\":\"permission-mode\",\"permissionMode\":\"default\",\"sessionId\":\"claude-session-1\"}\n",
            "{\"type\":\"last-prompt\",\"lastPrompt\":\"synthetic prompt\",\"sessionId\":\"claude-session-1\"}\n",
            "{\"type\":\"attachment\",\"uuid\":\"attachment-1\",\"timestamp\":\"2026-08-03T12:00:00Z\",\"attachment\":{\"type\":\"todo-reminder\",\"content\":[]}}\n",
            "{\"type\":\"user\",\"uuid\":\"user-1\",\"timestamp\":\"2026-08-03T12:00:01Z\",\"message\":{\"role\":\"user\",\"content\":\"Visible message\"}}\n",
        ));

        let legacy = assert_conforming_fixture(&ClaudeAdapter, &bundle, &context(), 1, 0);
        assert!(
            matches!(legacy.events.as_slice(), [ReplayEvent::User { text, .. }] if text == "Visible message")
        );
        let rich = run_rich_adapter(&ClaudeAdapter, &bundle, &context())
            .expect("known Claude metadata should normalize through V2");
        assert_eq!(rich.unknown_record_count, 0);
        assert_eq!(rich.entries.len(), 1);
    }

    #[test]
    fn empty_explicit_record_type_uses_a_nonempty_unknown_fallback() {
        let bundle = bundle_from_jsonl("{\"type\":\"\",\"payload\":{}}\n");

        let session = assert_conforming_fixture(&ClaudeAdapter, &bundle, &context(), 1, 1);

        assert_eq!(session.unknown_record_count, 1);
    }

    #[test]
    fn malformed_complete_record_returns_only_a_typed_position() {
        let bundle = SessionSnapshotBundle::synthetic(vec![SyntheticMember {
            id: "primary".to_owned(),
            logical_name: "primary.jsonl".to_owned(),
            records: vec![
                br#"{"type":"user","message":{"role":"user","content":"before"}}"#.to_vec(),
                br#"{"privateText":"synthetic-marker""#.to_vec(),
            ],
            partial_final: false,
        }]);

        let error = run_adapter(&ClaudeAdapter, &bundle, &context()).unwrap_err();

        assert_eq!(
            error,
            AdapterError::MalformedRecord {
                member_ordinal: 0,
                record_ordinal: 1,
                field: "json",
            }
        );
        assert!(!format!("{error}").contains("synthetic-marker"));
    }

    #[test]
    fn ignored_truncated_tail_does_not_change_complete_record_accounting() {
        let bundle = SessionSnapshotBundle::synthetic(vec![member_from_jsonl(
            "primary",
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"complete\"}}\n",
            true,
        )]);

        assert!(
            bundle
                .member("primary")
                .expect("fixture member should exist")
                .partial_final_record_ignored()
        );
        let session = assert_conforming_fixture(&ClaudeAdapter, &bundle, &context(), 1, 0);
        assert_format_warning(&session);
    }

    struct TempFixture(PathBuf);

    impl TempFixture {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should follow the epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "claude-adapter-fixture-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("temporary fixture root should be created");
            Self(path)
        }

        fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, bytes).expect("synthetic fixture should be written");
            path
        }

        fn root(&self) -> LocalRoot {
            LocalRoot::new(&self.0).expect("temporary fixture root should be valid")
        }
    }

    impl Drop for TempFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn primary_spec(path: &Path) -> BundleMemberSpec {
        BundleMemberSpec {
            id: "primary".to_owned(),
            logical_name: "primary.jsonl".to_owned(),
            path: path.to_owned(),
            role: MemberRole::Primary,
            compression: MemberCompression::None,
            content_kind: ContentKind::JsonLines,
            expected_identity: None,
        }
    }

    #[test]
    fn active_writer_append_after_snapshot_is_not_observed_by_adapter() {
        let tree = TempFixture::new();
        let path = tree.write(
            "session.jsonl",
            b"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"captured\"}}\n",
        );
        let bundle = capture_bundle_with_hook(
            &tree.root(),
            &[primary_spec(&path)],
            SnapshotLimits::default(),
            || {
                OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .expect("active writer should open")
                    .write_all(
                        b"{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":\"late\"}}\n",
                    )
                    .expect("active writer should append");
            },
        )
        .expect("bounded snapshot should succeed");

        assert_eq!(bundle.record_count(), 1);
        let session = assert_conforming_fixture(&ClaudeAdapter, &bundle, &context(), 1, 0);
        assert!(matches!(
            session.events.as_slice(),
            [ReplayEvent::User { text, .. }] if text == "captured"
        ));
    }

    #[test]
    fn filesystem_reader_ignores_a_real_partial_final_line() {
        let tree = TempFixture::new();
        let path = tree.write(
            "session.jsonl",
            b"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"complete\"}}\n{\"type\":\"assistant\"",
        );
        let bundle = capture_bundle(
            &tree.root(),
            &[primary_spec(&path)],
            SnapshotLimits::default(),
        )
        .expect("bounded snapshot should ignore the partial tail");

        assert!(
            bundle
                .member("primary")
                .expect("fixture member should exist")
                .partial_final_record_ignored()
        );
        assert_conforming_fixture(&ClaudeAdapter, &bundle, &context(), 1, 0);
    }
}
