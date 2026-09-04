use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::adapters::contract::{AdapterContext, AdapterError, RecordAccountant, SessionAdapter};
use crate::adapters::deletion::{DeletionArtifactDeclaration, SourceDeletionAdapter};
use crate::adapters::normalize::{
    EventIdGenerator, RawTimedRecord, RawTimestamp, clamp_long_running_session_timestamps,
    compute_duration, finalize_rich_session, normalize_timestamps, raw_timestamp, structural_order,
};
use crate::io::SessionSnapshotBundle;
use crate::model::{
    NormalizedEntryV2, NormalizedSessionV1, NormalizedSessionV2, NormalizedToolDetailV2,
    NormalizedToolStatusV2, ReplayEvent, SessionRelationship, SessionSource, SourceDiagnostic,
    ToolStatus, migrate_normalized_session_v1,
};

const MAX_TEXT_CHARS: usize = 2_000_000;
const MAX_PATH_CHARS: usize = 32_768;
const MAX_SUMMARY_CHARS: usize = 2_000_000;
const MAX_BOUNDED_STR: usize = 256;

pub struct CodexAdapter;

impl SourceDeletionAdapter for CodexAdapter {
    fn deletion_artifacts(&self) -> DeletionArtifactDeclaration {
        DeletionArtifactDeclaration::PrimaryFile
    }
}

impl SessionAdapter for CodexAdapter {
    fn normalize(
        &self,
        bundle: &SessionSnapshotBundle,
        context: &AdapterContext,
        accountant: &RecordAccountant,
    ) -> Result<NormalizedSessionV1, AdapterError> {
        parse_codex_session(bundle, context, accountant)
    }

    fn normalize_rich(
        &self,
        bundle: &SessionSnapshotBundle,
        context: &AdapterContext,
        accountant: &RecordAccountant,
    ) -> Result<NormalizedSessionV2, AdapterError> {
        let legacy = parse_codex_session(bundle, context, accountant)?;
        let migrated =
            migrate_normalized_session_v1(legacy, context.terminal_hold_ms()).map_err(|_| {
                AdapterError::InvalidOutput {
                    reason: crate::adapters::contract::OutputReason::SessionInvariant,
                }
            })?;
        enrich_codex_session(migrated, bundle, context, accountant)
    }
}

/// Detect format variant from a single record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FormatVariant {
    /// Current: `{timestamp, ordinal, type, payload}`
    CurrentEnveloped,
    /// Legacy enveloped (no ordinal): `{timestamp, type, payload}`
    LegacyEnveloped,
    /// Bare legacy: first record is metadata `{id, timestamp, ...}`,
    /// subsequent records are bare response items.
    BareLegacy,
}

fn detect_variant(first_record: &Value) -> FormatVariant {
    let obj = match first_record.as_object() {
        Some(obj) => obj,
        None => return FormatVariant::BareLegacy,
    };
    if obj.contains_key("type") && obj.contains_key("payload") {
        if obj.contains_key("ordinal") {
            FormatVariant::CurrentEnveloped
        } else {
            FormatVariant::LegacyEnveloped
        }
    } else {
        FormatVariant::BareLegacy
    }
}

fn presented_response_item_id(value: &Value) -> Option<&str> {
    if value.get("type").and_then(Value::as_str) != Some("response_item") {
        return None;
    }
    let payload = value.get("payload")?;
    let item_type = payload.get("type").and_then(Value::as_str).unwrap_or("");
    let is_presented = match item_type {
        "reasoning" | "Reasoning" => {
            explicit_reasoning_text(payload).is_some_and(|text| !text.is_empty())
        }
        "agent_message" | "agentMessage" => {
            visible_text(payload.get("content")).is_some_and(|text| !text.is_empty())
        }
        "function_call_output"
        | "custom_tool_call_output"
        | "tool_search_output"
        | "context_compaction"
        | "ContextCompaction" => false,
        _ => true,
    };
    if !is_presented {
        return None;
    }
    payload.get("id")?.as_str()
}

fn completed_item_id(payload: Option<&Value>) -> Option<&str> {
    let payload = payload?;
    if payload.get("type").and_then(Value::as_str) != Some("item_completed") {
        return None;
    }
    payload.get("item")?.get("id")?.as_str()
}

fn response_user_message_counts(records: &[(usize, usize, Value)]) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for (_, _, record) in records {
        if record.get("type").and_then(Value::as_str) != Some("response_item") {
            continue;
        }
        let Some(text) = record.get("payload").and_then(response_user_message_text) else {
            continue;
        };
        *counts.entry(text).or_insert(0) += 1;
    }
    counts
}

fn response_user_message_text(item: &Value) -> Option<String> {
    if item.get("type").and_then(Value::as_str) != Some("message")
        || item.get("role").and_then(Value::as_str) != Some("user")
    {
        return None;
    }
    visible_text(item.get("content")).filter(|text| !text.is_empty())
}

fn event_user_message_text(payload: Option<&Value>) -> Option<String> {
    let payload = payload?;
    if payload.get("type").and_then(Value::as_str) != Some("user_message") {
        return None;
    }
    visible_text(payload.get("message")).filter(|text| !text.is_empty())
}

fn consume_duplicate_user_message(
    payload: Option<&Value>,
    remaining_response_messages: &mut HashMap<String, usize>,
) -> bool {
    let Some(text) = event_user_message_text(payload) else {
        return false;
    };
    let Some(remaining) = remaining_response_messages.get_mut(&text) else {
        return false;
    };
    if *remaining == 0 {
        return false;
    }
    *remaining -= 1;
    true
}

/// Extract a vendor session ID from the first records for preflight.
pub(crate) fn extract_codex_session_id(
    bundle: &SessionSnapshotBundle,
) -> Result<String, AdapterError> {
    for (member_ordinal, member) in bundle.members().enumerate() {
        for (record_ordinal, record_bytes) in member.records().enumerate() {
            let value: Value = serde_json::from_slice(record_bytes).map_err(|_| {
                AdapterError::MalformedRecord {
                    member_ordinal,
                    record_ordinal,
                    field: "json",
                }
            })?;
            let variant = detect_variant(&value);
            match variant {
                FormatVariant::CurrentEnveloped | FormatVariant::LegacyEnveloped => {
                    if value.get("type").and_then(Value::as_str) == Some("session_meta")
                        && let Some(payload) = value.get("payload")
                    {
                        if let Some(sid) = payload.get("session_id").and_then(Value::as_str) {
                            return Ok(bounded_string(sid, MAX_BOUNDED_STR));
                        }
                        if let Some(id) = payload.get("id").and_then(Value::as_str) {
                            return Ok(bounded_string(id, MAX_BOUNDED_STR));
                        }
                    }
                }
                FormatVariant::BareLegacy => {
                    if let Some(sid) = value.get("session_id").and_then(Value::as_str) {
                        return Ok(bounded_string(sid, MAX_BOUNDED_STR));
                    }
                    if let Some(id) = value.get("id").and_then(Value::as_str) {
                        return Ok(bounded_string(id, MAX_BOUNDED_STR));
                    }
                }
            }
            // Only check first few records for metadata
            if record_ordinal >= 2 {
                break;
            }
        }
    }
    Err(AdapterError::EmptySession)
}

struct ParsedMetadata {
    source_version: Option<String>,
    created_at: Option<String>,
    cwd: Option<String>,
    relationships: Vec<SessionRelationship>,
}

fn parse_codex_session(
    bundle: &SessionSnapshotBundle,
    context: &AdapterContext,
    accountant: &RecordAccountant,
) -> Result<NormalizedSessionV1, AdapterError> {
    let mut raw_records: Vec<(usize, usize, Value)> = Vec::new();
    let mut variant = None;

    // Parse all records from all members
    for (member_ordinal, member) in bundle.members().enumerate() {
        for (record_ordinal, record_bytes) in member.records().enumerate() {
            let value: Value = serde_json::from_slice(record_bytes).map_err(|_| {
                AdapterError::MalformedRecord {
                    member_ordinal,
                    record_ordinal,
                    field: "json",
                }
            })?;
            if variant.is_none() {
                variant = Some(detect_variant(&value));
            }
            raw_records.push((member_ordinal, record_ordinal, value));
        }
    }

    if raw_records.is_empty() {
        return Err(AdapterError::EmptySession);
    }

    let variant = variant.expect("at least one record");
    let response_item_ids = raw_records
        .iter()
        .filter_map(|(_, _, value)| presented_response_item_id(value))
        .collect::<HashSet<_>>();
    let mut remaining_response_user_messages = response_user_message_counts(&raw_records);
    let mut diagnostics: Vec<SourceDiagnostic> = Vec::new();
    let mut metadata = ParsedMetadata {
        source_version: None,
        created_at: None,
        cwd: None,
        relationships: Vec::new(),
    };

    // Build timed records for ordering, extract events, and classify
    let mut timed_records: Vec<RawTimedRecord> = Vec::new();
    let mut pending_events: Vec<PendingEvent> = Vec::new();
    let mut seen_call_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (member_ordinal, record_ordinal, value) in &raw_records {
        let m = *member_ordinal;
        let r = *record_ordinal;

        match variant {
            FormatVariant::CurrentEnveloped | FormatVariant::LegacyEnveloped => {
                let vendor_ordinal = value.get("ordinal").and_then(Value::as_u64);
                let ts_str = value.get("timestamp").and_then(Value::as_str);
                let record_type = value
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let payload = value.get("payload");

                timed_records.push(RawTimedRecord {
                    vendor_ordinal,
                    timestamp_rfc3339: ts_str.map(String::from),
                    structural_position: (m, r),
                    kind: record_type.to_owned(),
                });

                match record_type {
                    "session_meta" => {
                        extract_metadata(payload, &mut metadata);
                        accountant.classify_understood(m, r)?;
                    }
                    "response_item" => {
                        let events = parse_response_item(payload, m, r, &mut seen_call_ids)?;
                        classify_parsed_record(accountant, m, r, &events)?;
                        pending_events.extend(events);
                    }
                    "event_msg" => {
                        let duplicate_presentation = completed_item_id(payload)
                            .is_some_and(|item_id| response_item_ids.contains(item_id))
                            || consume_duplicate_user_message(
                                payload,
                                &mut remaining_response_user_messages,
                            );
                        let events = if duplicate_presentation {
                            vec![PendingEvent {
                                structural_position: (m, r),
                                event: PendingEventKind::DuplicatePresentation,
                            }]
                        } else {
                            parse_event_msg(payload, m, r, &mut seen_call_ids)?
                        };
                        classify_parsed_record(accountant, m, r, &events)?;
                        pending_events.extend(events);
                    }
                    "compacted"
                    | "turn_context"
                    | "world_state"
                    | "inter_agent_communication_metadata" => {
                        // Known types that produce no replay events
                        accountant.classify_understood(m, r)?;
                    }
                    _ => {
                        accountant.classify_unknown(m, r, record_type)?;
                        pending_events.push(PendingEvent {
                            structural_position: (m, r),
                            event: PendingEventKind::Unknown {
                                source_type: bounded_string(record_type, MAX_BOUNDED_STR),
                            },
                        });
                    }
                }
            }
            FormatVariant::BareLegacy => {
                let ts_str = value.get("timestamp").and_then(Value::as_str);

                timed_records.push(RawTimedRecord {
                    vendor_ordinal: None,
                    timestamp_rfc3339: ts_str.map(String::from),
                    structural_position: (m, r),
                    kind: if r == 0 {
                        "session_meta".to_owned()
                    } else {
                        "response_item".to_owned()
                    },
                });

                if r == 0 {
                    // First record is bare metadata
                    extract_bare_metadata(value, &mut metadata);
                    accountant.classify_understood(m, r)?;
                } else {
                    // Subsequent records are bare response items
                    let events = parse_bare_response_item(value, m, r, &mut seen_call_ids)?;
                    classify_parsed_record(accountant, m, r, &events)?;
                    pending_events.extend(events);
                }
            }
        }
    }

    // Determine structural order
    structural_order(&mut timed_records, &mut diagnostics);

    // Build position-to-index map for ordering events
    let position_order: std::collections::HashMap<(usize, usize), usize> = timed_records
        .iter()
        .enumerate()
        .map(|(idx, tr)| (tr.structural_position, idx))
        .collect();

    // Sort pending events by structural order
    pending_events.sort_by_key(|pe| {
        position_order
            .get(&pe.structural_position)
            .copied()
            .unwrap_or(usize::MAX)
    });

    // Normalize every source record so metadata and non-rendered records still
    // participate in the session origin and monotonic timeline.
    let record_timestamps: Vec<RawTimestamp> = timed_records
        .iter()
        .map(|record| raw_timestamp(record.timestamp_rfc3339.as_deref()))
        .collect();
    let mut record_at_ms = normalize_timestamps(&record_timestamps, &mut diagnostics);
    clamp_long_running_session_timestamps(
        &mut record_at_ms,
        context.terminal_hold_ms(),
        &mut diagnostics,
    );
    let at_ms_values: Vec<u64> = pending_events
        .iter()
        .map(|event| {
            position_order
                .get(&event.structural_position)
                .and_then(|index| record_at_ms.get(*index))
                .copied()
                .unwrap_or(0)
        })
        .collect();

    if pending_events.is_empty() {
        return Err(AdapterError::EmptySession);
    }

    // Generate events
    let mut id_gen = EventIdGenerator::new(context.session_id());
    let mut events: Vec<ReplayEvent> = Vec::new();
    let mut unknown_count = 0_u64;

    for (idx, pe) in pending_events.iter().enumerate() {
        let at_ms = at_ms_values[idx];
        let (m, r) = pe.structural_position;

        match &pe.event {
            PendingEventKind::User { text } => {
                events.push(ReplayEvent::User {
                    id: id_gen.next_id_for_content(m, r, "user", &[text]),
                    at_ms,
                    text: text.clone(),
                });
            }
            PendingEventKind::Assistant { markdown } => {
                events.push(ReplayEvent::Assistant {
                    id: id_gen.next_id_for_content(m, r, "assistant", &[markdown]),
                    at_ms,
                    markdown: markdown.clone(),
                });
            }
            PendingEventKind::ToolCall {
                name,
                status,
                summary,
            } => {
                let status_identity = match status {
                    ToolStatus::Running => "running",
                    ToolStatus::Succeeded => "succeeded",
                    ToolStatus::Failed => "failed",
                };
                events.push(ReplayEvent::Tool {
                    id: id_gen.next_id_for_content(m, r, "tool", &[name, status_identity, summary]),
                    at_ms,
                    name: name.clone(),
                    status: status.clone(),
                    summary: summary.clone(),
                });
            }
            PendingEventKind::Command {
                name,
                status,
                summary,
            } => {
                let status_identity = match status {
                    ToolStatus::Running => "running",
                    ToolStatus::Succeeded => "succeeded",
                    ToolStatus::Failed => "failed",
                };
                events.push(ReplayEvent::Tool {
                    id: id_gen.next_id_for_content(m, r, "tool", &[name, status_identity, summary]),
                    at_ms,
                    name: name.clone(),
                    status: status.clone(),
                    summary: summary.clone(),
                });
            }
            PendingEventKind::FileChange { path, summary } => {
                events.push(ReplayEvent::FileChange {
                    id: id_gen.next_id_for_content(m, r, "file-change", &[path, summary]),
                    at_ms,
                    path: path.clone(),
                    summary: summary.clone(),
                });
            }
            PendingEventKind::Unknown { source_type } => {
                unknown_count += 1;
                events.push(ReplayEvent::Unknown {
                    id: id_gen.next_id_for_content(m, r, "unknown", &[source_type]),
                    at_ms,
                    source_type: source_type.clone(),
                });
            }
            PendingEventKind::DuplicatePresentation => {
                // Classified as understood but adds no event
            }
        }
    }

    if events.is_empty() {
        return Err(AdapterError::EmptySession);
    }

    let last_at_ms_values: Vec<u64> = events.iter().map(|e| e.at_ms()).collect();
    let duration_ms = compute_duration(&last_at_ms_values, context.terminal_hold_ms())?;

    // Extract created_at as canonical UTC if valid
    let created_at = metadata.created_at.and_then(|ts| {
        use time::OffsetDateTime;
        use time::format_description::well_known::Rfc3339;
        OffsetDateTime::parse(&ts, &Rfc3339).ok().map(|dt| {
            let utc = dt.to_offset(time::UtcOffset::UTC);
            format!(
                "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
                utc.year(),
                u8::from(utc.month()),
                utc.day(),
                utc.hour(),
                utc.minute(),
                utc.second(),
                utc.millisecond()
            )
        })
    });

    Ok(NormalizedSessionV1 {
        schema_version: 1,
        id: context.session_id().to_owned(),
        source: SessionSource::Codex,
        source_version: metadata.source_version,
        title: bounded_string(context.session_id(), 512),
        created_at,
        cwd: metadata.cwd.map(|c| bounded_string(&c, MAX_PATH_CHARS)),
        relationships: metadata.relationships,
        events,
        duration_ms,
        diagnostics,
        unknown_record_count: unknown_count,
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
        completed: bool,
    },
}

fn enrich_codex_session(
    mut session: NormalizedSessionV2,
    bundle: &SessionSnapshotBundle,
    context: &AdapterContext,
    accountant: &RecordAccountant,
) -> Result<NormalizedSessionV2, AdapterError> {
    let mut records = Vec::new();
    for (member_ordinal, member) in bundle.members().enumerate() {
        for (record_ordinal, bytes) in member.records().enumerate() {
            let value: Value =
                serde_json::from_slice(bytes).map_err(|_| AdapterError::MalformedRecord {
                    member_ordinal,
                    record_ordinal,
                    field: "json",
                })?;
            records.push((member_ordinal, record_ordinal, value));
        }
    }
    let Some((_, _, first)) = records.first() else {
        return Err(AdapterError::EmptySession);
    };
    let variant = detect_variant(first);
    let response_item_ids = records
        .iter()
        .filter_map(|(_, _, value)| presented_response_item_id(value))
        .collect::<HashSet<_>>();
    let mut remaining_response_user_messages = response_user_message_counts(&records);
    let mut overlays = Vec::new();
    let mut calls = HashMap::<String, usize>::new();

    for (member_ordinal, record_ordinal, value) in &records {
        let position = (*member_ordinal, *record_ordinal);
        let vendor_ordinal = value.get("ordinal").and_then(Value::as_u64);
        match variant {
            FormatVariant::CurrentEnveloped | FormatVariant::LegacyEnveloped => {
                let record_type = value
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                match record_type {
                    "session_meta"
                    | "compacted"
                    | "turn_context"
                    | "world_state"
                    | "inter_agent_communication_metadata" => {}
                    "response_item" => collect_rich_item_overlay(
                        value.get("payload"),
                        vendor_ordinal,
                        position,
                        &mut overlays,
                        &mut calls,
                        accountant,
                    )?,
                    "event_msg"
                        if completed_item_id(value.get("payload"))
                            .is_some_and(|item_id| response_item_ids.contains(item_id)) => {}
                    "event_msg"
                        if consume_duplicate_user_message(
                            value.get("payload"),
                            &mut remaining_response_user_messages,
                        ) => {}
                    "event_msg" => collect_rich_event_overlay(
                        value.get("payload"),
                        vendor_ordinal,
                        position,
                        &mut overlays,
                        &mut calls,
                        accountant,
                    )?,
                    _ => overlays.push(PositionedRichOverlay {
                        vendor_ordinal,
                        structural_position: position,
                        overlay: RichOverlay::Keep {
                            key_kind: "unknown",
                            vendor_id: value_id(value),
                        },
                    }),
                }
            }
            FormatVariant::BareLegacy if *record_ordinal == 0 => {}
            FormatVariant::BareLegacy => collect_rich_item_overlay(
                Some(value),
                vendor_ordinal,
                position,
                &mut overlays,
                &mut calls,
                accountant,
            )?,
        }
    }

    sort_rich_overlays(&mut overlays);
    if overlays.len() != session.entries.len() {
        return Err(AdapterError::InvalidOutput {
            reason: crate::adapters::contract::OutputReason::SessionInvariant,
        });
    }

    let mut key_generator = EventIdGenerator::new(context.session_id());
    for (entry, overlay) in session.entries.iter_mut().zip(overlays) {
        let (key_kind, vendor_id) = match &overlay.overlay {
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
        match overlay.overlay {
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
                arguments,
                result,
                completed,
                ..
            } => {
                let NormalizedEntryV2::ToolCall { status, detail, .. } = entry else {
                    return Err(AdapterError::InvalidOutput {
                        reason: crate::adapters::contract::OutputReason::SessionInvariant,
                    });
                };
                if completed {
                    *status = NormalizedToolStatusV2::Succeeded;
                }
                if arguments.is_some() || result.is_some() {
                    *detail = NormalizedToolDetailV2::Available { arguments, result };
                }
            }
        }
    }

    finalize_rich_session(&mut session)?;
    Ok(session)
}

fn collect_rich_item_overlay(
    item: Option<&Value>,
    vendor_ordinal: Option<u64>,
    position: (usize, usize),
    overlays: &mut Vec<PositionedRichOverlay>,
    calls: &mut HashMap<String, usize>,
    accountant: &RecordAccountant,
) -> Result<(), AdapterError> {
    let Some(item) = item else {
        return Ok(());
    };
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
    let vendor_id = value_id(item);
    let overlay = match item_type {
        "message" | "" => {
            let key_kind = match item.get("role").and_then(Value::as_str) {
                Some("assistant" | "model") => "assistant",
                _ => "user",
            };
            Some(RichOverlay::Keep {
                key_kind,
                vendor_id,
            })
        }
        "agent_message" | "agentMessage"
            if visible_text(item.get("content")).is_some_and(|text| !text.is_empty()) =>
        {
            Some(RichOverlay::Keep {
                key_kind: "assistant",
                vendor_id,
            })
        }
        "agent_message" | "agentMessage" => None,
        "reasoning" | "Reasoning" => match explicit_reasoning_text(item) {
            Some(text) if !text.is_empty() => {
                accountant.promote_unknown_to_understood(position.0, position.1)?;
                Some(RichOverlay::Reasoning { vendor_id, text })
            }
            _ => None,
        },
        "local_shell_call" => {
            let call_id = value_string(item.get("call_id"));
            let overlay = RichOverlay::Tool {
                vendor_id: call_id.clone().or(vendor_id),
                arguments: bounded_value(item.get("action")),
                result: bounded_value(item.get("output")),
                completed: false,
            };
            if let Some(call_id) = call_id {
                calls.insert(call_id, overlays.len());
            }
            Some(overlay)
        }
        "function_call" | "custom_tool_call" | "web_search_call" | "tool_search_call" => {
            let call_id = value_string(item.get("call_id"));
            let overlay = RichOverlay::Tool {
                vendor_id: call_id.clone().or(vendor_id),
                arguments: bounded_value(match item_type {
                    "custom_tool_call" => item.get("input"),
                    "web_search_call" => item.get("action").or_else(|| item.get("query")),
                    "tool_search_call" => item.get("query").or_else(|| item.get("arguments")),
                    _ => item.get("arguments"),
                }),
                result: None,
                completed: matches!(
                    item.get("status").and_then(Value::as_str),
                    Some("completed" | "succeeded")
                ),
            };
            if let Some(call_id) = call_id {
                calls.insert(call_id, overlays.len());
            }
            Some(overlay)
        }
        "function_call_output" | "custom_tool_call_output" | "tool_search_output" => {
            let call_id = value_string(item.get("call_id"));
            let result =
                visible_text(item.get("output")).or_else(|| bounded_value(item.get("output")));
            if let Some(index) = call_id
                .as_ref()
                .and_then(|call_id| calls.get(call_id))
                .copied()
            {
                if let Some(PositionedRichOverlay {
                    overlay:
                        RichOverlay::Tool {
                            result: stored_result,
                            completed,
                            ..
                        },
                    ..
                }) = overlays.get_mut(index)
                {
                    *stored_result = result;
                    *completed = true;
                }
                None
            } else {
                Some(RichOverlay::Tool {
                    vendor_id: call_id.or(vendor_id),
                    arguments: None,
                    result,
                    completed: true,
                })
            }
        }
        _ => Some(RichOverlay::Keep {
            key_kind: "unknown",
            vendor_id,
        }),
    };
    if let Some(overlay) = overlay {
        overlays.push(PositionedRichOverlay {
            vendor_ordinal,
            structural_position: position,
            overlay,
        });
    }
    Ok(())
}

fn collect_rich_event_overlay(
    payload: Option<&Value>,
    vendor_ordinal: Option<u64>,
    position: (usize, usize),
    overlays: &mut Vec<PositionedRichOverlay>,
    calls: &mut HashMap<String, usize>,
    accountant: &RecordAccountant,
) -> Result<(), AdapterError> {
    let Some(payload) = payload else {
        return Ok(());
    };
    let event_type = payload
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    if event_type == "agent_reasoning"
        && let Some(text) = value_string(payload.get("text").or_else(|| payload.get("message")))
        && !text.is_empty()
    {
        accountant.promote_unknown_to_understood(position.0, position.1)?;
        overlays.push(PositionedRichOverlay {
            vendor_ordinal,
            structural_position: position,
            overlay: RichOverlay::Reasoning {
                vendor_id: value_id(payload),
                text: bounded_string(&text, MAX_TEXT_CHARS),
            },
        });
        return Ok(());
    }
    if event_type != "item_completed" {
        match event_type {
            "patch_apply_end" => overlays.push(PositionedRichOverlay {
                vendor_ordinal,
                structural_position: position,
                overlay: RichOverlay::Keep {
                    key_kind: "file-change",
                    vendor_id: value_string(payload.get("call_id")).or_else(|| value_id(payload)),
                },
            }),
            "sub_agent_activity" | "turn_aborted" | "error" => {
                overlays.push(PositionedRichOverlay {
                    vendor_ordinal,
                    structural_position: position,
                    overlay: RichOverlay::Keep {
                        key_kind: "tool-call",
                        vendor_id: value_string(payload.get("event_id"))
                            .or_else(|| value_id(payload)),
                    },
                });
            }
            "token_count"
            | "task_started"
            | "task_complete"
            | "thread_settings_applied"
            | "context_compacted"
            | "agent_message"
            | "agent_reasoning"
            | "web_search_end"
            | "mcp_tool_call_end" => {}
            "user_message" => {
                if event_user_message_text(Some(payload)).is_some() {
                    overlays.push(PositionedRichOverlay {
                        vendor_ordinal,
                        structural_position: position,
                        overlay: RichOverlay::Keep {
                            key_kind: "user",
                            vendor_id: value_id(payload),
                        },
                    });
                }
            }
            _ => overlays.push(PositionedRichOverlay {
                vendor_ordinal,
                structural_position: position,
                overlay: RichOverlay::Keep {
                    key_kind: "unknown",
                    vendor_id: value_id(payload),
                },
            }),
        }
        return Ok(());
    }
    let Some(item) = payload.get("item") else {
        overlays.push(PositionedRichOverlay {
            vendor_ordinal,
            structural_position: position,
            overlay: RichOverlay::Keep {
                key_kind: "unknown",
                vendor_id: None,
            },
        });
        return Ok(());
    };
    // These are the persisted spellings of documented Codex ThreadItem
    // families. Source: https://learn.chatgpt.com/docs/app-server#items
    match item.get("type").and_then(Value::as_str).unwrap_or("") {
        "command_execution" | "commandExecution" | "CommandExecution" => {
            let call_id = value_string(item.get("call_id"));
            let result = command_result(item);
            if let Some(index) = call_id
                .as_ref()
                .and_then(|call_id| calls.get(call_id))
                .copied()
            {
                if let Some(PositionedRichOverlay {
                    overlay:
                        RichOverlay::Tool {
                            result: stored_result,
                            completed,
                            ..
                        },
                    ..
                }) = overlays.get_mut(index)
                {
                    if stored_result.is_none() {
                        *stored_result = result;
                    }
                    *completed = true;
                }
            } else {
                overlays.push(PositionedRichOverlay {
                    vendor_ordinal,
                    structural_position: position,
                    overlay: RichOverlay::Tool {
                        vendor_id: call_id.or_else(|| value_id(item)),
                        arguments: bounded_value(item.get("args").or_else(|| item.get("command"))),
                        result,
                        completed: matches!(command_status(item), ToolStatus::Succeeded),
                    },
                });
            }
        }
        "file_change" | "fileChange" | "FileChange" => overlays.push(PositionedRichOverlay {
            vendor_ordinal,
            structural_position: position,
            overlay: RichOverlay::Keep {
                key_kind: "file-change",
                vendor_id: value_id(item),
            },
        }),
        "user_message" | "userMessage" | "UserMessage"
            if visible_text(item.get("content")).is_some_and(|text| !text.is_empty()) =>
        {
            overlays.push(PositionedRichOverlay {
                vendor_ordinal,
                structural_position: position,
                overlay: RichOverlay::Keep {
                    key_kind: "user",
                    vendor_id: value_id(item),
                },
            });
        }
        "user_message" | "userMessage" | "UserMessage" => {}
        "agent_message" | "agentMessage" | "AgentMessage"
            if visible_text(item.get("content")).is_some_and(|text| !text.is_empty()) =>
        {
            overlays.push(PositionedRichOverlay {
                vendor_ordinal,
                structural_position: position,
                overlay: RichOverlay::Keep {
                    key_kind: "assistant",
                    vendor_id: value_id(item),
                },
            });
        }
        "agent_message" | "agentMessage" | "AgentMessage" => {}
        "reasoning" | "Reasoning" => {
            if let Some(text) = explicit_reasoning_text(item).filter(|text| !text.is_empty()) {
                accountant.promote_unknown_to_understood(position.0, position.1)?;
                overlays.push(PositionedRichOverlay {
                    vendor_ordinal,
                    structural_position: position,
                    overlay: RichOverlay::Reasoning {
                        vendor_id: value_id(item),
                        text,
                    },
                });
            }
        }
        "mcp_tool_call" | "mcpToolCall" | "McpToolCall" => overlays.push(PositionedRichOverlay {
            vendor_ordinal,
            structural_position: position,
            overlay: RichOverlay::Tool {
                vendor_id: value_id(item),
                arguments: bounded_value(item.get("arguments")),
                result: bounded_value(item.get("result").or_else(|| item.get("error"))),
                completed: matches!(tool_status(item), ToolStatus::Succeeded),
            },
        }),
        "dynamic_tool_call" | "dynamicToolCall" | "DynamicToolCall" => {
            overlays.push(PositionedRichOverlay {
                vendor_ordinal,
                structural_position: position,
                overlay: RichOverlay::Tool {
                    vendor_id: value_id(item),
                    arguments: bounded_value(item.get("arguments")),
                    result: bounded_value(item.get("content_items")),
                    completed: matches!(tool_status(item), ToolStatus::Succeeded),
                },
            })
        }
        "collab_tool_call" | "collabToolCall" | "CollabToolCall" | "sub_agent_activity"
        | "subAgentActivity" | "SubAgentActivity" | "web_search" | "webSearch" | "WebSearch"
        | "image_view" | "imageView" | "ImageView" => {
            overlays.push(PositionedRichOverlay {
                vendor_ordinal,
                structural_position: position,
                overlay: RichOverlay::Keep {
                    key_kind: "tool-call",
                    vendor_id: value_id(item),
                },
            });
        }
        "context_compaction"
        | "contextCompaction"
        | "ContextCompaction"
        | "entered_review_mode"
        | "enteredReviewMode"
        | "EnteredReviewMode"
        | "exited_review_mode"
        | "exitedReviewMode"
        | "ExitedReviewMode" => {}
        _ => overlays.push(PositionedRichOverlay {
            vendor_ordinal,
            structural_position: position,
            overlay: RichOverlay::Keep {
                key_kind: "unknown",
                vendor_id: value_id(item),
            },
        }),
    }
    Ok(())
}

fn sort_rich_overlays(overlays: &mut [PositionedRichOverlay]) {
    let all_have_unique_ordinals = {
        let mut seen = std::collections::HashSet::new();
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

fn explicit_reasoning_text(item: &Value) -> Option<String> {
    let mut parts = Vec::new();
    for field in ["summary", "summary_text", "content"] {
        if let Some(values) = item.get(field).and_then(Value::as_array) {
            parts.extend(values.iter().filter_map(|value| {
                if let Some(text) = value.as_str() {
                    return Some(text);
                }
                let kind = value.get("type").and_then(Value::as_str)?;
                if matches!(
                    kind,
                    "summary_text" | "reasoning_text" | "SummaryText" | "ReasoningText"
                ) {
                    value.get("text").and_then(Value::as_str)
                } else {
                    None
                }
            }));
        }
    }
    if parts.is_empty()
        && let Some(text) = item.get("text").and_then(Value::as_str)
    {
        parts.push(text);
    }
    (!parts.is_empty()).then(|| bounded_string(&parts.join("\n"), MAX_TEXT_CHARS))
}

fn command_result(item: &Value) -> Option<String> {
    if let Some(output) = item
        .get("aggregated_output")
        .or_else(|| item.get("formatted_output"))
        .and_then(Value::as_str)
        .filter(|output| !output.is_empty())
    {
        return Some(bounded_string(output, MAX_TEXT_CHARS));
    }
    let parts = ["stdout", "stderr"]
        .into_iter()
        .filter_map(|field| item.get(field).and_then(Value::as_str))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    (!parts.is_empty()).then(|| bounded_string(&parts.join("\n"), MAX_TEXT_CHARS))
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

fn value_string(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_owned)
}

fn value_id(value: &Value) -> Option<String> {
    value
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
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

struct PendingEvent {
    structural_position: (usize, usize),
    event: PendingEventKind,
}

enum PendingEventKind {
    User {
        text: String,
    },
    Assistant {
        markdown: String,
    },
    ToolCall {
        name: String,
        status: ToolStatus,
        summary: String,
    },
    Command {
        name: String,
        status: ToolStatus,
        summary: String,
    },
    FileChange {
        path: String,
        summary: String,
    },
    Unknown {
        source_type: String,
    },
    /// Duplicate presentation of a call/item already seen via another record type.
    /// Classified as understood but produces no event.
    DuplicatePresentation,
}

fn classify_parsed_record(
    accountant: &RecordAccountant,
    member_ordinal: usize,
    record_ordinal: usize,
    events: &[PendingEvent],
) -> Result<(), AdapterError> {
    if let Some(source_type) = events.iter().find_map(|event| match &event.event {
        PendingEventKind::Unknown { source_type } => Some(source_type.as_str()),
        _ => None,
    }) {
        accountant.classify_unknown(member_ordinal, record_ordinal, source_type)
    } else {
        accountant.classify_understood(member_ordinal, record_ordinal)
    }
}

fn extract_metadata(payload: Option<&Value>, metadata: &mut ParsedMetadata) {
    let Some(payload) = payload.and_then(Value::as_object) else {
        return;
    };
    if metadata.source_version.is_none() {
        metadata.source_version = payload
            .get("cli_version")
            .and_then(Value::as_str)
            .map(|v| bounded_string(v, 128));
    }
    if metadata.created_at.is_none() {
        metadata.created_at = payload
            .get("timestamp")
            .and_then(Value::as_str)
            .map(String::from);
    }
    if metadata.cwd.is_none() {
        metadata.cwd = payload
            .get("cwd")
            .and_then(Value::as_str)
            .map(|v| bounded_string(v, MAX_PATH_CHARS));
    }
    if let Some(parent) = payload.get("parent_thread_id").and_then(Value::as_str)
        && !parent.is_empty()
    {
        metadata.relationships.push(SessionRelationship {
            kind: crate::model::RelationshipKind::Parent,
            session_id: bounded_string(parent, MAX_BOUNDED_STR),
        });
    }
    if let Some(fork) = payload.get("forked_from_id").and_then(Value::as_str)
        && !fork.is_empty()
    {
        metadata.relationships.push(SessionRelationship {
            kind: crate::model::RelationshipKind::Fork,
            session_id: bounded_string(fork, MAX_BOUNDED_STR),
        });
    }
}

fn extract_bare_metadata(value: &Value, metadata: &mut ParsedMetadata) {
    let Some(obj) = value.as_object() else {
        return;
    };
    if metadata.source_version.is_none() {
        metadata.source_version = obj
            .get("cli_version")
            .and_then(Value::as_str)
            .map(|v| bounded_string(v, 128));
    }
    if metadata.created_at.is_none() {
        metadata.created_at = obj
            .get("timestamp")
            .and_then(Value::as_str)
            .map(String::from);
    }
    if metadata.cwd.is_none() {
        metadata.cwd = obj
            .get("cwd")
            .and_then(Value::as_str)
            .map(|v| bounded_string(v, MAX_PATH_CHARS));
    }
}

fn parse_response_item(
    payload: Option<&Value>,
    member_ordinal: usize,
    record_ordinal: usize,
    seen_call_ids: &mut std::collections::HashSet<String>,
) -> Result<Vec<PendingEvent>, AdapterError> {
    let payload = payload.ok_or(AdapterError::MalformedRecord {
        member_ordinal,
        record_ordinal,
        field: "payload",
    })?;
    parse_item_value(payload, member_ordinal, record_ordinal, seen_call_ids)
}

fn parse_bare_response_item(
    value: &Value,
    member_ordinal: usize,
    record_ordinal: usize,
    seen_call_ids: &mut std::collections::HashSet<String>,
) -> Result<Vec<PendingEvent>, AdapterError> {
    parse_item_value(value, member_ordinal, record_ordinal, seen_call_ids)
}

fn parse_item_value(
    item: &Value,
    member_ordinal: usize,
    record_ordinal: usize,
    seen_call_ids: &mut std::collections::HashSet<String>,
) -> Result<Vec<PendingEvent>, AdapterError> {
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
    let pos = (member_ordinal, record_ordinal);

    match item_type {
        "message" => {
            let role = item.get("role").and_then(Value::as_str).unwrap_or("");
            let text = visible_text(item.get("content")).unwrap_or_default();

            let event = match role {
                "user" => PendingEventKind::User {
                    text: bounded_string(&text, MAX_TEXT_CHARS),
                },
                "assistant" | "model" => PendingEventKind::Assistant {
                    markdown: bounded_string(&text, MAX_TEXT_CHARS),
                },
                _ => PendingEventKind::User {
                    text: bounded_string(&text, MAX_TEXT_CHARS),
                },
            };
            Ok(vec![PendingEvent {
                structural_position: pos,
                event,
            }])
        }
        "agent_message" | "agentMessage" => {
            let Some(markdown) = visible_text(item.get("content")).filter(|text| !text.is_empty())
            else {
                return Ok(Vec::new());
            };
            Ok(vec![PendingEvent {
                structural_position: pos,
                event: PendingEventKind::Assistant { markdown },
            }])
        }
        "reasoning" | "Reasoning" => {
            if explicit_reasoning_text(item).is_some_and(|text| !text.is_empty()) {
                Ok(vec![PendingEvent {
                    structural_position: pos,
                    event: PendingEventKind::Unknown {
                        source_type: "response_item/reasoning".to_owned(),
                    },
                }])
            } else {
                Ok(Vec::new())
            }
        }
        "local_shell_call" => {
            let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
            let status_str = item.get("status").and_then(Value::as_str).unwrap_or("");
            let command = item
                .get("action")
                .and_then(|a| a.get("command"))
                .and_then(Value::as_array)
                .map(|args| {
                    args.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_else(|| "(shell)".to_owned());

            let status = match status_str {
                "completed" => ToolStatus::Succeeded,
                "failed" | "error" => ToolStatus::Failed,
                _ => ToolStatus::Running,
            };

            if !call_id.is_empty() {
                seen_call_ids.insert(call_id.to_owned());
            }

            Ok(vec![PendingEvent {
                structural_position: pos,
                event: PendingEventKind::Command {
                    name: bounded_string(&command, MAX_BOUNDED_STR),
                    status,
                    summary: bounded_string(
                        &format!("Shell: {}", bounded_string(&command, 200)),
                        MAX_SUMMARY_CHARS,
                    ),
                },
            }])
        }
        "function_call" | "custom_tool_call" | "web_search_call" | "tool_search_call" => {
            let name = item
                .get("name")
                .or_else(|| item.get("tool"))
                .and_then(Value::as_str)
                .unwrap_or(match item_type {
                    "web_search_call" => "web_search",
                    "tool_search_call" => "tool_search",
                    _ => "function",
                });
            let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");

            if !call_id.is_empty() {
                seen_call_ids.insert(call_id.to_owned());
            }

            Ok(vec![PendingEvent {
                structural_position: pos,
                event: PendingEventKind::ToolCall {
                    name: bounded_nonempty(name, "tool", MAX_BOUNDED_STR),
                    status: tool_status(item),
                    summary: bounded_string(
                        &format!("Call: {}", bounded_string(name, 200)),
                        MAX_SUMMARY_CHARS,
                    ),
                },
            }])
        }
        "function_call_output" | "custom_tool_call_output" | "tool_search_output" => {
            let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
            // If we already saw the call, this is a duplicate presentation
            if !call_id.is_empty() && seen_call_ids.contains(call_id) {
                Ok(vec![PendingEvent {
                    structural_position: pos,
                    event: PendingEventKind::DuplicatePresentation,
                }])
            } else {
                Ok(vec![PendingEvent {
                    structural_position: pos,
                    event: PendingEventKind::ToolCall {
                        name: if item_type == "function_call_output" {
                            "function_output"
                        } else {
                            "tool_output"
                        }
                        .to_owned(),
                        status: ToolStatus::Succeeded,
                        summary: "Function call completed".to_owned(),
                    },
                }])
            }
        }
        "" => {
            // Bare legacy items without explicit type - treat as message
            let role = item.get("role").and_then(Value::as_str).unwrap_or("");
            let text = visible_text(item.get("content")).unwrap_or_default();

            let event = match role {
                "user" => PendingEventKind::User {
                    text: bounded_string(&text, MAX_TEXT_CHARS),
                },
                "assistant" | "model" => PendingEventKind::Assistant {
                    markdown: bounded_string(&text, MAX_TEXT_CHARS),
                },
                _ => PendingEventKind::User {
                    text: bounded_string(&text, MAX_TEXT_CHARS),
                },
            };
            Ok(vec![PendingEvent {
                structural_position: pos,
                event,
            }])
        }
        _ => {
            // Preserve unknown nested record types as unknown events.
            Ok(vec![PendingEvent {
                structural_position: pos,
                event: PendingEventKind::Unknown {
                    source_type: bounded_string(
                        &format!("response_item/{item_type}"),
                        MAX_BOUNDED_STR,
                    ),
                },
            }])
        }
    }
}

fn parse_event_msg(
    payload: Option<&Value>,
    member_ordinal: usize,
    record_ordinal: usize,
    seen_call_ids: &mut std::collections::HashSet<String>,
) -> Result<Vec<PendingEvent>, AdapterError> {
    let payload = payload.ok_or(AdapterError::MalformedRecord {
        member_ordinal,
        record_ordinal,
        field: "payload",
    })?;
    let pos = (member_ordinal, record_ordinal);

    let event_type = payload
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");

    match event_type {
        "item_completed" => {
            let item = payload.get("item");
            let Some(item) = item else {
                return Ok(vec![PendingEvent {
                    structural_position: pos,
                    event: PendingEventKind::Unknown {
                        source_type: "event_msg/item_completed".to_owned(),
                    },
                }]);
            };
            let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
            match item_type {
                "command_execution" | "commandExecution" | "CommandExecution" => {
                    let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
                    let command = command_text(item).unwrap_or_else(|| "(command)".to_owned());
                    let status = command_status(item);

                    // Check for duplicate with response_item
                    if !call_id.is_empty() && seen_call_ids.contains(call_id) {
                        return Ok(vec![PendingEvent {
                            structural_position: pos,
                            event: PendingEventKind::DuplicatePresentation,
                        }]);
                    }
                    if !call_id.is_empty() {
                        seen_call_ids.insert(call_id.to_owned());
                    }

                    Ok(vec![PendingEvent {
                        structural_position: pos,
                        event: PendingEventKind::Command {
                            name: bounded_nonempty(&command, "command", MAX_BOUNDED_STR),
                            status,
                            summary: bounded_string(
                                &format!("Command: {}", bounded_string(&command, 200)),
                                MAX_SUMMARY_CHARS,
                            ),
                        },
                    }])
                }
                "file_change" | "fileChange" | "FileChange" => {
                    Ok(vec![file_change_event(item, pos)])
                }
                "user_message" | "userMessage" | "UserMessage" => {
                    Ok(visible_text(item.get("content"))
                        .filter(|text| !text.is_empty())
                        .map(|text| PendingEvent {
                            structural_position: pos,
                            event: PendingEventKind::User { text },
                        })
                        .into_iter()
                        .collect())
                }
                "agent_message" | "agentMessage" | "AgentMessage" => {
                    Ok(visible_text(item.get("content"))
                        .filter(|text| !text.is_empty())
                        .map(|markdown| PendingEvent {
                            structural_position: pos,
                            event: PendingEventKind::Assistant { markdown },
                        })
                        .into_iter()
                        .collect())
                }
                "reasoning" | "Reasoning" => Ok(explicit_reasoning_text(item)
                    .filter(|text| !text.is_empty())
                    .map(|_| PendingEvent {
                        structural_position: pos,
                        event: PendingEventKind::Unknown {
                            source_type: "event_msg/item_completed/reasoning".to_owned(),
                        },
                    })
                    .into_iter()
                    .collect()),
                "mcp_tool_call" | "mcpToolCall" | "McpToolCall" => Ok(vec![PendingEvent {
                    structural_position: pos,
                    event: PendingEventKind::ToolCall {
                        name: mcp_tool_name(item),
                        status: tool_status(item),
                        summary: "MCP tool call".to_owned(),
                    },
                }]),
                "dynamic_tool_call" | "dynamicToolCall" | "DynamicToolCall" => {
                    Ok(vec![PendingEvent {
                        structural_position: pos,
                        event: PendingEventKind::ToolCall {
                            name: bounded_nonempty(
                                item.get("tool").and_then(Value::as_str).unwrap_or(""),
                                "dynamic_tool",
                                MAX_BOUNDED_STR,
                            ),
                            status: tool_status(item),
                            summary: "Dynamic tool call".to_owned(),
                        },
                    }])
                }
                "collab_tool_call" | "collabToolCall" | "CollabToolCall" | "sub_agent_activity"
                | "subAgentActivity" | "SubAgentActivity" => Ok(vec![PendingEvent {
                    structural_position: pos,
                    event: PendingEventKind::ToolCall {
                        name: "sub-agent".to_owned(),
                        status: tool_status_with_default(item, ToolStatus::Succeeded),
                        summary: item
                            .get("kind")
                            .and_then(Value::as_str)
                            .map(|kind| bounded_string(&format!("Sub-agent {kind}"), 200))
                            .unwrap_or_else(|| "Sub-agent activity".to_owned()),
                    },
                }]),
                "web_search" | "webSearch" | "WebSearch" => Ok(vec![PendingEvent {
                    structural_position: pos,
                    event: PendingEventKind::ToolCall {
                        name: "web_search".to_owned(),
                        status: tool_status_with_default(item, ToolStatus::Succeeded),
                        summary: "Web search".to_owned(),
                    },
                }]),
                "image_view" | "imageView" | "ImageView" => {
                    let display_path = item
                        .get("path")
                        .and_then(Value::as_str)
                        .map(safe_display_path)
                        .unwrap_or_else(|| "image".to_owned());
                    Ok(vec![PendingEvent {
                        structural_position: pos,
                        event: PendingEventKind::ToolCall {
                            name: "view_image".to_owned(),
                            status: ToolStatus::Succeeded,
                            summary: bounded_string(
                                &format!("Viewed {display_path}"),
                                MAX_SUMMARY_CHARS,
                            ),
                        },
                    }])
                }
                "context_compaction"
                | "contextCompaction"
                | "ContextCompaction"
                | "entered_review_mode"
                | "enteredReviewMode"
                | "EnteredReviewMode"
                | "exited_review_mode"
                | "exitedReviewMode"
                | "ExitedReviewMode" => Ok(Vec::new()),
                _ => {
                    // Unknown completed item type
                    Ok(vec![PendingEvent {
                        structural_position: pos,
                        event: PendingEventKind::Unknown {
                            source_type: bounded_string(
                                &format!("event_msg/item_completed/{item_type}"),
                                MAX_BOUNDED_STR,
                            ),
                        },
                    }])
                }
            }
        }
        "patch_apply_end" => Ok(vec![file_change_event(payload, pos)]),
        "agent_reasoning" => Ok(explicit_reasoning_text(payload)
            .filter(|text| !text.is_empty())
            .map(|_| PendingEvent {
                structural_position: pos,
                event: PendingEventKind::Unknown {
                    source_type: "event_msg/agent_reasoning".to_owned(),
                },
            })
            .into_iter()
            .collect()),
        "sub_agent_activity" => Ok(vec![PendingEvent {
            structural_position: pos,
            event: PendingEventKind::ToolCall {
                name: "sub-agent".to_owned(),
                status: ToolStatus::Succeeded,
                summary: payload
                    .get("kind")
                    .and_then(Value::as_str)
                    .map(|kind| bounded_string(&format!("Sub-agent {kind}"), 200))
                    .unwrap_or_else(|| "Sub-agent activity".to_owned()),
            },
        }]),
        "turn_aborted" | "error" => Ok(vec![PendingEvent {
            structural_position: pos,
            event: PendingEventKind::ToolCall {
                name: "turn".to_owned(),
                status: ToolStatus::Failed,
                summary: "Turn stopped".to_owned(),
            },
        }]),
        "token_count"
        | "task_started"
        | "task_complete"
        | "thread_settings_applied"
        | "context_compacted"
        | "agent_message"
        | "web_search_end"
        | "mcp_tool_call_end" => Ok(Vec::new()),
        "user_message" => Ok(event_user_message_text(Some(payload))
            .map(|text| PendingEvent {
                structural_position: pos,
                event: PendingEventKind::User { text },
            })
            .into_iter()
            .collect()),
        _ => {
            // Unknown event_msg type
            Ok(vec![PendingEvent {
                structural_position: pos,
                event: PendingEventKind::Unknown {
                    source_type: bounded_string(
                        &format!("event_msg/{event_type}"),
                        MAX_BOUNDED_STR,
                    ),
                },
            }])
        }
    }
}

fn visible_text(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(text) = value.as_str() {
        return Some(bounded_string(text, MAX_TEXT_CHARS));
    }
    let parts = value.as_array()?.iter().filter_map(|part| {
        let kind = part.get("type").and_then(Value::as_str)?;
        matches!(
            kind,
            "input_text" | "output_text" | "text" | "Text" | "summary_text" | "reasoning_text"
        )
        .then(|| part.get("text").and_then(Value::as_str))
        .flatten()
    });
    let text = parts.collect::<Vec<_>>().join("\n");
    Some(bounded_string(&text, MAX_TEXT_CHARS))
}

fn command_text(item: &Value) -> Option<String> {
    let command = item.get("command")?;
    if let Some(command) = command.as_str() {
        return Some(bounded_string(command, MAX_TEXT_CHARS));
    }
    let parts = command
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    (!parts.is_empty()).then(|| bounded_string(&parts.join(" "), MAX_TEXT_CHARS))
}

fn tool_status(item: &Value) -> ToolStatus {
    tool_status_with_default(item, ToolStatus::Running)
}

fn command_status(item: &Value) -> ToolStatus {
    let status = tool_status(item);
    if matches!(status, ToolStatus::Succeeded)
        && item
            .get("exit_code")
            .and_then(Value::as_i64)
            .is_some_and(|exit_code| exit_code != 0)
    {
        ToolStatus::Failed
    } else {
        status
    }
}

fn tool_status_with_default(item: &Value, default: ToolStatus) -> ToolStatus {
    if item.get("success").and_then(Value::as_bool) == Some(false) {
        return ToolStatus::Failed;
    }
    match item.get("status").and_then(Value::as_str) {
        Some("completed" | "succeeded") => ToolStatus::Succeeded,
        Some("failed" | "error" | "cancelled" | "declined") => ToolStatus::Failed,
        Some("in_progress" | "running" | "pending") => ToolStatus::Running,
        _ => default,
    }
}

fn mcp_tool_name(item: &Value) -> String {
    let server = item.get("server").and_then(Value::as_str).unwrap_or("");
    let tool = item.get("tool").and_then(Value::as_str).unwrap_or("");
    let name = match (server.is_empty(), tool.is_empty()) {
        (false, false) => format!("{server}/{tool}"),
        (false, true) => server.to_owned(),
        (true, false) => tool.to_owned(),
        (true, true) => "mcp-tool".to_owned(),
    };
    bounded_nonempty(&name, "mcp-tool", MAX_BOUNDED_STR)
}

fn file_change_event(item: &Value, position: (usize, usize)) -> PendingEvent {
    if let Some(changes) = item.get("changes").and_then(Value::as_object) {
        let display_path = if changes.len() == 1 {
            changes
                .keys()
                .next()
                .map(|path| safe_display_path(path))
                .unwrap_or_else(|| "file".to_owned())
        } else {
            format!("{} files", changes.len())
        };
        let noun = if changes.len() == 1 { "file" } else { "files" };
        return PendingEvent {
            structural_position: position,
            event: PendingEventKind::FileChange {
                path: bounded_nonempty(&display_path, "file", MAX_PATH_CHARS),
                summary: format!("Changed {} {noun}", changes.len()),
            },
        };
    }
    let path = item
        .get("path")
        .and_then(Value::as_str)
        .map(safe_display_path)
        .unwrap_or_else(|| "(unknown)".to_owned());
    let change_type = item
        .get("change_type")
        .and_then(Value::as_str)
        .unwrap_or("modified");
    let additions = item.get("additions").and_then(Value::as_u64).unwrap_or(0);
    let deletions = item.get("deletions").and_then(Value::as_u64).unwrap_or(0);
    PendingEvent {
        structural_position: position,
        event: PendingEventKind::FileChange {
            path: bounded_nonempty(&path, "(unknown)", MAX_PATH_CHARS),
            summary: bounded_string(
                &format!("{change_type}: +{additions} -{deletions}"),
                MAX_SUMMARY_CHARS,
            ),
        },
    }
}

fn safe_display_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let is_absolute = normalized.starts_with('/')
        || normalized
            .as_bytes()
            .get(1)
            .is_some_and(|separator| *separator == b':');
    if is_absolute {
        normalized
            .rsplit('/')
            .find(|part| !part.is_empty())
            .map(|part| bounded_nonempty(part, "file", MAX_PATH_CHARS))
            .unwrap_or_else(|| "file".to_owned())
    } else {
        bounded_nonempty(&normalized, "file", MAX_PATH_CHARS)
    }
}

fn bounded_nonempty(input: &str, fallback: &str, max_chars: usize) -> String {
    let bounded = bounded_string(input, max_chars);
    if bounded.trim().is_empty() {
        fallback.to_owned()
    } else {
        bounded
    }
}

fn bounded_string(input: &str, max_chars: usize) -> String {
    input
        .chars()
        .filter(|c| *c != '\0')
        .take(max_chars)
        .collect()
}

#[cfg(test)]
#[path = "codex_tests.rs"]
mod tests;
