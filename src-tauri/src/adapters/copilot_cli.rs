use std::collections::HashMap;

use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::adapters::contract::{AdapterContext, AdapterError, RecordAccountant, SessionAdapter};
use crate::adapters::deletion::{DeletionArtifactDeclaration, SourceDeletionAdapter};
use crate::adapters::normalize::{
    EventIdGenerator, RawTimedRecord, clamp_long_running_session_timestamps, compute_duration,
    finalize_rich_session, normalize_timestamps, raw_timestamp, structural_order,
};
use crate::io::SessionSnapshotBundle;
use crate::model::{
    DiagnosticSeverity, NormalizedEntryV2, NormalizedSessionV1, NormalizedSessionV2,
    NormalizedToolDetailV2, ReplayEvent, SessionSource, SourceDiagnostic, ToolStatus,
    migrate_normalized_session_v1,
};

// GitHub documents the session-state directory and events.jsonl, but not the
// event schema. Keep this adapter deliberately narrow and preserve unfamiliar
// complete records. Source: https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference
const MAX_TEXT_CHARS: usize = 2_000_000;
const MAX_PATH_CHARS: usize = 32_768;
const MAX_BOUNDED_CHARS: usize = 256;
const FORMAT_WARNING_CODE: &str = "copilot-cli-internal-format-unstable";
const REMOTE_WARNING_CODE: &str = "copilot-cli-remote-sync-enabled";

pub struct CopilotCliAdapter;

impl SourceDeletionAdapter for CopilotCliAdapter {
    fn deletion_artifacts(&self) -> DeletionArtifactDeclaration {
        DeletionArtifactDeclaration::SessionDirectory {
            primary_file_name: "events.jsonl",
            max_artifacts: 64,
            max_depth: 8,
        }
    }
}

impl SessionAdapter for CopilotCliAdapter {
    fn normalize(
        &self,
        bundle: &SessionSnapshotBundle,
        context: &AdapterContext,
        accountant: &RecordAccountant,
    ) -> Result<NormalizedSessionV1, AdapterError> {
        parse_copilot_session(bundle, context, accountant)
    }

    fn normalize_rich(
        &self,
        bundle: &SessionSnapshotBundle,
        context: &AdapterContext,
        accountant: &RecordAccountant,
    ) -> Result<NormalizedSessionV2, AdapterError> {
        let legacy = parse_copilot_session(bundle, context, accountant)?;
        let migrated =
            migrate_normalized_session_v1(legacy, context.terminal_hold_ms()).map_err(|_| {
                AdapterError::InvalidOutput {
                    reason: crate::adapters::contract::OutputReason::SessionInvariant,
                }
            })?;
        enrich_copilot_session(migrated, bundle, context, accountant)
    }
}

#[derive(Default)]
struct ParsedMetadata {
    source_version: Option<String>,
    created_at: Option<String>,
    cwd: Option<String>,
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
    FileChange {
        path: String,
        summary: String,
    },
    Unknown(String),
}

fn parse_copilot_session(
    bundle: &SessionSnapshotBundle,
    context: &AdapterContext,
    accountant: &RecordAccountant,
) -> Result<NormalizedSessionV1, AdapterError> {
    let mut diagnostics = vec![SourceDiagnostic {
        code: FORMAT_WARNING_CODE.to_owned(),
        severity: DiagnosticSeverity::Warning,
        message: "Copilot CLI events use an undocumented, version-unstable format; replay is best-effort."
            .to_owned(),
    }];
    let mut metadata = ParsedMetadata::default();
    let mut timed_records = Vec::new();
    let mut pending_events = Vec::new();
    let mut tool_names = HashMap::new();
    let mut remote_warning_added = false;

    for (member_ordinal, member) in bundle.members().enumerate() {
        for (record_ordinal, bytes) in member.records().enumerate() {
            let position = (member_ordinal, record_ordinal);
            match member.logical_name() {
                "settings.json" => {
                    let value = parse_jsonc(bytes).map_err(|_| AdapterError::MalformedRecord {
                        member_ordinal,
                        record_ordinal,
                        field: "json",
                    })?;
                    if !remote_warning_added && remote_sync_enabled(&value) {
                        diagnostics.push(SourceDiagnostic {
                            code: REMOTE_WARNING_CODE.to_owned(),
                            severity: DiagnosticSeverity::Warning,
                            message: "Copilot CLI remote session synchronization is enabled; replay reads only the supplied local snapshot."
                                .to_owned(),
                        });
                        remote_warning_added = true;
                    }
                    accountant.classify_understood(member_ordinal, record_ordinal)?;
                }
                "events.jsonl" => {
                    let value: Value = serde_json::from_slice(bytes).map_err(|_| {
                        AdapterError::MalformedRecord {
                            member_ordinal,
                            record_ordinal,
                            field: "json",
                        }
                    })?;
                    let record_type = record_type(&value);
                    timed_records.push(RawTimedRecord {
                        vendor_ordinal: value.get("ordinal").and_then(Value::as_u64),
                        timestamp_rfc3339: value
                            .get("timestamp")
                            .and_then(Value::as_str)
                            .map(|value| bounded_string(value, 128)),
                        structural_position: position,
                        kind: record_type.clone(),
                    });

                    let parsed = parse_event_record(
                        &value,
                        &record_type,
                        position,
                        &mut metadata,
                        &mut tool_names,
                    )?;
                    classify_events(accountant, position, &parsed)?;
                    pending_events.extend(parsed);
                }
                _ => {
                    let source_type = "artifact/unsupported".to_owned();
                    accountant.classify_unknown(member_ordinal, record_ordinal, &source_type)?;
                    timed_records.push(RawTimedRecord {
                        vendor_ordinal: None,
                        timestamp_rfc3339: None,
                        structural_position: position,
                        kind: source_type.clone(),
                    });
                    pending_events.push(PendingEvent {
                        structural_position: position,
                        kind: PendingEventKind::Unknown(source_type),
                    });
                }
            }

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
    let mut record_at_ms = normalize_timestamps(&timestamps, &mut diagnostics);
    clamp_long_running_session_timestamps(
        &mut record_at_ms,
        context.terminal_hold_ms(),
        &mut diagnostics,
    );
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
            PendingEventKind::FileChange { path, summary } => {
                let id = id_generator.next_id_for_content(
                    member_ordinal,
                    record_ordinal,
                    "file-change",
                    &[&path],
                );
                ReplayEvent::FileChange {
                    id,
                    at_ms,
                    path,
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
        source: SessionSource::CopilotCli,
        source_version: metadata.source_version,
        title: bounded_string(context.session_id(), 512),
        created_at: metadata.created_at,
        cwd: metadata.cwd,
        relationships: Vec::new(),
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

fn enrich_copilot_session(
    mut session: NormalizedSessionV2,
    bundle: &SessionSnapshotBundle,
    context: &AdapterContext,
    accountant: &RecordAccountant,
) -> Result<NormalizedSessionV2, AdapterError> {
    let mut overlays = Vec::new();
    for (member_ordinal, member) in bundle.members().enumerate() {
        if member.logical_name() != "events.jsonl" {
            continue;
        }
        for (record_ordinal, bytes) in member.records().enumerate() {
            let value: Value =
                serde_json::from_slice(bytes).map_err(|_| AdapterError::MalformedRecord {
                    member_ordinal,
                    record_ordinal,
                    field: "json",
                })?;
            collect_copilot_overlays(
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

fn collect_copilot_overlays(
    value: &Value,
    position: (usize, usize),
    overlays: &mut Vec<PositionedRichOverlay>,
    accountant: &RecordAccountant,
) -> Result<(), AdapterError> {
    let record_type = record_type(value);
    let vendor_ordinal = value.get("ordinal").and_then(Value::as_u64);
    let event_id = value.get("id").and_then(Value::as_str).map(str::to_owned);
    let data = value.get("data");
    match record_type.as_str() {
        "session.start" => {}
        record_type if is_non_display_event(record_type) => {}
        "user.message" => overlays.push(positioned_overlay(
            vendor_ordinal,
            position,
            RichOverlay::Keep {
                key_kind: "user",
                vendor_id: event_id,
            },
        )),
        "assistant.message" => collect_copilot_assistant_overlays(
            data.and_then(|data| data.get("content")),
            vendor_ordinal,
            position,
            event_id,
            overlays,
            accountant,
        )?,
        "assistant.reasoning" => {
            if let Some(text) = data
                .and_then(|data| data.get("content").or_else(|| data.get("text")))
                .and_then(Value::as_str)
                .map(|text| bounded_string(text, MAX_TEXT_CHARS))
                .filter(|text| !text.is_empty())
            {
                accountant.promote_unknown_to_understood(position.0, position.1)?;
                overlays.push(positioned_overlay(
                    vendor_ordinal,
                    position,
                    RichOverlay::Reasoning {
                        vendor_id: event_id,
                        text,
                    },
                ));
            } else {
                overlays.push(positioned_overlay(
                    vendor_ordinal,
                    position,
                    RichOverlay::Keep {
                        key_kind: "unknown",
                        vendor_id: event_id,
                    },
                ));
            }
        }
        "tool.execution_start" => {
            let tool_id = data
                .and_then(|data| data.get("toolCallId"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            overlays.push(positioned_overlay(
                vendor_ordinal,
                position,
                RichOverlay::Tool {
                    vendor_id: tool_id.clone().or_else(|| event_id.clone()),
                    arguments: bounded_value(data.and_then(|data| data.get("arguments"))),
                    result: None,
                },
            ));
            let tool_name = data
                .and_then(|data| data.get("toolName"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if is_file_change_tool(tool_name)
                && data
                    .and_then(|data| data.get("arguments"))
                    .and_then(|arguments| arguments.get("path"))
                    .and_then(Value::as_str)
                    .is_some()
            {
                overlays.push(positioned_overlay(
                    vendor_ordinal,
                    position,
                    RichOverlay::Keep {
                        key_kind: "file-change",
                        vendor_id: event_id,
                    },
                ));
            }
        }
        "tool.execution_complete" => overlays.push(positioned_overlay(
            vendor_ordinal,
            position,
            RichOverlay::Tool {
                vendor_id: data
                    .and_then(|data| data.get("toolCallId"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or(event_id),
                arguments: None,
                result: bounded_value(data.and_then(|data| data.get("result"))),
            },
        )),
        "system.message" | "system.notification" => {
            if optional_data_string(data, "content").is_some() {
                overlays.push(positioned_overlay(
                    vendor_ordinal,
                    position,
                    RichOverlay::Keep {
                        key_kind: "assistant",
                        vendor_id: event_id,
                    },
                ));
            }
        }
        "session.warning" | "session.info" => {
            if optional_data_string(data, "message").is_some() {
                overlays.push(positioned_overlay(
                    vendor_ordinal,
                    position,
                    RichOverlay::Keep {
                        key_kind: "assistant",
                        vendor_id: event_id,
                    },
                ));
            }
        }
        "abort" | "session.error" => overlays.push(positioned_overlay(
            vendor_ordinal,
            position,
            RichOverlay::Keep {
                key_kind: "tool-call",
                vendor_id: event_id,
            },
        )),
        "session.task_complete" => {
            if data
                .and_then(|data| data.get("success"))
                .and_then(Value::as_bool)
                .is_some()
            {
                overlays.push(positioned_overlay(
                    vendor_ordinal,
                    position,
                    RichOverlay::Keep {
                        key_kind: "tool-call",
                        vendor_id: event_id,
                    },
                ));
            }
        }
        "session.workspace_file_changed" => {
            if optional_data_string(data, "path").is_some() {
                overlays.push(positioned_overlay(
                    vendor_ordinal,
                    position,
                    RichOverlay::Keep {
                        key_kind: "file-change",
                        vendor_id: event_id,
                    },
                ));
            }
        }
        "external_tool.requested" | "tool.user_requested" => overlays.push(positioned_overlay(
            vendor_ordinal,
            position,
            RichOverlay::Tool {
                vendor_id: tool_event_id(data).or(event_id),
                arguments: bounded_value(data.and_then(|data| data.get("arguments"))),
                result: None,
            },
        )),
        "external_tool.completed" => overlays.push(positioned_overlay(
            vendor_ordinal,
            position,
            RichOverlay::Tool {
                vendor_id: data
                    .and_then(|data| data.get("requestId"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or(event_id),
                arguments: None,
                result: None,
            },
        )),
        _ => overlays.push(positioned_overlay(
            vendor_ordinal,
            position,
            RichOverlay::Keep {
                key_kind: "unknown",
                vendor_id: event_id,
            },
        )),
    }
    Ok(())
}

fn collect_copilot_assistant_overlays(
    content: Option<&Value>,
    vendor_ordinal: Option<u64>,
    position: (usize, usize),
    vendor_id: Option<String>,
    overlays: &mut Vec<PositionedRichOverlay>,
    accountant: &RecordAccountant,
) -> Result<(), AdapterError> {
    let Some(content) = content else {
        return Ok(());
    };
    if content.is_string() {
        overlays.push(positioned_overlay(
            vendor_ordinal,
            position,
            RichOverlay::Keep {
                key_kind: "assistant",
                vendor_id,
            },
        ));
        return Ok(());
    }
    let Some(blocks) = content.as_array() else {
        return Ok(());
    };
    let mut reasoning = Vec::new();
    let mut has_other_unknown = false;
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => overlays.push(positioned_overlay(
                vendor_ordinal,
                position,
                RichOverlay::Keep {
                    key_kind: "assistant",
                    vendor_id: vendor_id.clone(),
                },
            )),
            Some("tool_use") => {}
            Some("reasoning" | "thinking") => {
                if let Some(text) = block
                    .get("text")
                    .or_else(|| block.get("thinking"))
                    .and_then(Value::as_str)
                    .map(|text| bounded_string(text, MAX_TEXT_CHARS))
                    .filter(|text| !text.is_empty())
                {
                    reasoning.push(text);
                }
            }
            _ => has_other_unknown = true,
        }
    }
    if has_other_unknown || reasoning.is_empty() {
        if has_other_unknown
            || blocks.iter().any(|block| {
                !matches!(
                    block.get("type").and_then(Value::as_str),
                    Some("text" | "tool_use")
                )
            })
        {
            overlays.push(positioned_overlay(
                vendor_ordinal,
                position,
                RichOverlay::Keep {
                    key_kind: "unknown",
                    vendor_id,
                },
            ));
        }
    } else {
        accountant.promote_unknown_to_understood(position.0, position.1)?;
        overlays.push(positioned_overlay(
            vendor_ordinal,
            position,
            RichOverlay::Reasoning {
                vendor_id,
                text: bounded_string(&reasoning.join("\n"), MAX_TEXT_CHARS),
            },
        ));
    }
    Ok(())
}

fn positioned_overlay(
    vendor_ordinal: Option<u64>,
    structural_position: (usize, usize),
    overlay: RichOverlay,
) -> PositionedRichOverlay {
    PositionedRichOverlay {
        vendor_ordinal,
        structural_position,
        overlay,
    }
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

fn parse_event_record(
    value: &Value,
    record_type: &str,
    position: (usize, usize),
    metadata: &mut ParsedMetadata,
    tool_names: &mut HashMap<String, String>,
) -> Result<Vec<PendingEvent>, AdapterError> {
    let data = value.get("data");
    if is_non_display_event(record_type) {
        return Ok(Vec::new());
    }
    match record_type {
        "session.start" => {
            extract_metadata(value, data, metadata);
            Ok(Vec::new())
        }
        "user.message" => {
            let content = required_data_string(data, "content", position, "message-content")?;
            Ok(vec![PendingEvent {
                structural_position: position,
                kind: PendingEventKind::User(bounded_string(content, MAX_TEXT_CHARS)),
            }])
        }
        "assistant.message" => parse_assistant_message(data, position),
        "tool.execution_start" => parse_tool_start(data, position, tool_names),
        "tool.execution_complete" => parse_tool_complete(data, position, tool_names),
        "system.message" | "system.notification" => {
            Ok(optional_assistant_event(data, "content", position))
        }
        "session.warning" | "session.info" => {
            Ok(optional_assistant_event(data, "message", position))
        }
        "abort" => Ok(vec![tool_event(
            position,
            "session",
            ToolStatus::Failed,
            optional_data_string(data, "reason").unwrap_or_else(|| "Session aborted".to_owned()),
        )]),
        "session.error" => Ok(vec![tool_event(
            position,
            "session",
            ToolStatus::Failed,
            optional_data_string(data, "message").unwrap_or_else(|| "Session error".to_owned()),
        )]),
        "external_tool.requested" | "tool.user_requested" => Ok(vec![parse_external_tool_start(
            record_type,
            data,
            position,
            tool_names,
        )]),
        "external_tool.completed" => Ok(vec![parse_external_tool_complete(
            data, position, tool_names,
        )]),
        "session.workspace_file_changed" => Ok(optional_file_change_event(data, position)),
        "session.task_complete" => Ok(optional_task_complete_event(data, position)),
        _ => Ok(vec![PendingEvent {
            structural_position: position,
            kind: PendingEventKind::Unknown(record_type.to_owned()),
        }]),
    }
}

fn parse_assistant_message(
    data: Option<&Value>,
    position: (usize, usize),
) -> Result<Vec<PendingEvent>, AdapterError> {
    let content =
        data.and_then(|value| value.get("content"))
            .ok_or(AdapterError::MalformedRecord {
                member_ordinal: position.0,
                record_ordinal: position.1,
                field: "message-content",
            })?;
    if let Some(text) = content.as_str() {
        return Ok(vec![PendingEvent {
            structural_position: position,
            kind: PendingEventKind::Assistant(bounded_string(text, MAX_TEXT_CHARS)),
        }]);
    }

    let blocks = content.as_array().ok_or(AdapterError::MalformedRecord {
        member_ordinal: position.0,
        record_ordinal: position.1,
        field: "message-content",
    })?;
    let mut events = Vec::new();
    let mut unknown_type = None;
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                let text = block.get("text").and_then(Value::as_str).ok_or(
                    AdapterError::MalformedRecord {
                        member_ordinal: position.0,
                        record_ordinal: position.1,
                        field: "message-text",
                    },
                )?;
                events.push(PendingEvent {
                    structural_position: position,
                    kind: PendingEventKind::Assistant(bounded_string(text, MAX_TEXT_CHARS)),
                });
            }
            Some("tool_use") => {}
            other => {
                unknown_type.get_or_insert_with(|| {
                    let block_type = other
                        .map(|value| bounded_string(value, MAX_BOUNDED_CHARS))
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| "unknown-block".to_owned());
                    bounded_nonempty(
                        &format!("assistant.message/{block_type}"),
                        "assistant.message/unknown-block",
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

fn parse_tool_start(
    data: Option<&Value>,
    position: (usize, usize),
    tool_names: &mut HashMap<String, String>,
) -> Result<Vec<PendingEvent>, AdapterError> {
    let name = required_data_string(data, "toolName", position, "tool-name")?;
    let name = bounded_nonempty(name, "tool", MAX_BOUNDED_CHARS);
    if let Some(call_id) = data
        .and_then(|value| value.get("toolCallId"))
        .and_then(Value::as_str)
    {
        tool_names.insert(bounded_string(call_id, MAX_BOUNDED_CHARS), name.clone());
    }

    let mut events = vec![PendingEvent {
        structural_position: position,
        kind: PendingEventKind::Tool {
            name: name.clone(),
            status: ToolStatus::Running,
            summary: "Tool invocation recorded".to_owned(),
        },
    }];
    if is_file_change_tool(&name)
        && let Some(path) = data
            .and_then(|value| value.get("arguments"))
            .and_then(|value| value.get("path"))
            .and_then(Value::as_str)
            .map(|value| bounded_string(value, MAX_PATH_CHARS))
            .filter(|value| !value.is_empty())
    {
        events.push(PendingEvent {
            structural_position: position,
            kind: PendingEventKind::FileChange {
                path,
                summary: "File change requested".to_owned(),
            },
        });
    }
    Ok(events)
}

fn parse_tool_complete(
    data: Option<&Value>,
    position: (usize, usize),
    tool_names: &HashMap<String, String>,
) -> Result<Vec<PendingEvent>, AdapterError> {
    let data = data.ok_or(AdapterError::MalformedRecord {
        member_ordinal: position.0,
        record_ordinal: position.1,
        field: "data",
    })?;
    let success =
        data.get("success")
            .and_then(Value::as_bool)
            .ok_or(AdapterError::MalformedRecord {
                member_ordinal: position.0,
                record_ordinal: position.1,
                field: "tool-status",
            })?;
    let name = data
        .get("toolCallId")
        .and_then(Value::as_str)
        .map(|value| bounded_string(value, MAX_BOUNDED_CHARS))
        .and_then(|call_id| tool_names.get(&call_id).cloned())
        .or_else(|| {
            data.get("toolName")
                .and_then(Value::as_str)
                .map(|value| bounded_nonempty(value, "tool", MAX_BOUNDED_CHARS))
        })
        .unwrap_or_else(|| "tool".to_owned());
    Ok(vec![PendingEvent {
        structural_position: position,
        kind: PendingEventKind::Tool {
            name,
            status: if success {
                ToolStatus::Succeeded
            } else {
                ToolStatus::Failed
            },
            summary: "Tool result recorded".to_owned(),
        },
    }])
}

fn optional_assistant_event(
    data: Option<&Value>,
    field: &str,
    position: (usize, usize),
) -> Vec<PendingEvent> {
    optional_data_string(data, field)
        .map(|text| {
            vec![PendingEvent {
                structural_position: position,
                kind: PendingEventKind::Assistant(text),
            }]
        })
        .unwrap_or_default()
}

fn parse_external_tool_start(
    record_type: &str,
    data: Option<&Value>,
    position: (usize, usize),
    tool_names: &mut HashMap<String, String>,
) -> PendingEvent {
    let name = optional_data_string(data, "toolName").unwrap_or_else(|| "external tool".to_owned());
    if let Some(data) = data {
        for field in ["requestId", "toolCallId"] {
            if let Some(identifier) = data.get(field).and_then(Value::as_str) {
                tool_names.insert(bounded_string(identifier, MAX_BOUNDED_CHARS), name.clone());
            }
        }
    }
    tool_event(
        position,
        &name,
        ToolStatus::Running,
        if record_type == "tool.user_requested" {
            "User-requested tool"
        } else {
            "External tool requested"
        },
    )
}

fn parse_external_tool_complete(
    data: Option<&Value>,
    position: (usize, usize),
    tool_names: &HashMap<String, String>,
) -> PendingEvent {
    let name = data
        .and_then(|data| data.get("requestId"))
        .and_then(Value::as_str)
        .map(|identifier| bounded_string(identifier, MAX_BOUNDED_CHARS))
        .and_then(|identifier| tool_names.get(&identifier).cloned())
        .unwrap_or_else(|| "external tool".to_owned());
    tool_event(
        position,
        &name,
        ToolStatus::Succeeded,
        "External tool completed",
    )
}

fn optional_file_change_event(data: Option<&Value>, position: (usize, usize)) -> Vec<PendingEvent> {
    let Some(path) = optional_data_string(data, "path")
        .map(|path| bounded_string(&path, MAX_PATH_CHARS))
        .filter(|path| !path.is_empty())
    else {
        return Vec::new();
    };
    let operation = optional_data_string(data, "operation").unwrap_or_else(|| "changed".to_owned());
    vec![PendingEvent {
        structural_position: position,
        kind: PendingEventKind::FileChange {
            path,
            summary: bounded_nonempty(&format!("File {operation}"), "File changed", MAX_TEXT_CHARS),
        },
    }]
}

fn optional_task_complete_event(
    data: Option<&Value>,
    position: (usize, usize),
) -> Vec<PendingEvent> {
    let Some(success) = data
        .and_then(|data| data.get("success"))
        .and_then(Value::as_bool)
    else {
        return Vec::new();
    };
    vec![tool_event(
        position,
        "task",
        if success {
            ToolStatus::Succeeded
        } else {
            ToolStatus::Failed
        },
        optional_data_string(data, "summary").unwrap_or_else(|| "Task completed".to_owned()),
    )]
}

fn tool_event(
    position: (usize, usize),
    name: &str,
    status: ToolStatus,
    summary: impl Into<String>,
) -> PendingEvent {
    PendingEvent {
        structural_position: position,
        kind: PendingEventKind::Tool {
            name: bounded_nonempty(name, "tool", MAX_BOUNDED_CHARS),
            status,
            summary: bounded_string(&summary.into(), MAX_TEXT_CHARS),
        },
    }
}

fn classify_events(
    accountant: &RecordAccountant,
    position: (usize, usize),
    events: &[PendingEvent],
) -> Result<(), AdapterError> {
    if let Some(source_type) = events.iter().find_map(|event| match &event.kind {
        PendingEventKind::Unknown(source_type) => Some(source_type.as_str()),
        _ => None,
    }) {
        accountant.classify_unknown(position.0, position.1, source_type)
    } else {
        accountant.classify_understood(position.0, position.1)
    }
}

fn extract_metadata(value: &Value, data: Option<&Value>, metadata: &mut ParsedMetadata) {
    if metadata.source_version.is_none() {
        metadata.source_version = data
            .and_then(|value| value.get("copilotVersion"))
            .and_then(Value::as_str)
            .map(|value| bounded_string(value, 128));
    }
    if metadata.created_at.is_none() {
        metadata.created_at = data
            .and_then(|value| value.get("startTime"))
            .and_then(Value::as_str)
            .or_else(|| value.get("timestamp").and_then(Value::as_str))
            .and_then(canonical_timestamp);
    }
    if metadata.cwd.is_none() {
        metadata.cwd = data
            .and_then(|value| value.get("context"))
            .and_then(|value| value.get("cwd"))
            .and_then(Value::as_str)
            .map(|value| bounded_string(value, MAX_PATH_CHARS));
    }
}

fn required_data_string<'a>(
    data: Option<&'a Value>,
    field: &str,
    position: (usize, usize),
    error_field: &'static str,
) -> Result<&'a str, AdapterError> {
    data.and_then(|value| value.get(field))
        .and_then(Value::as_str)
        .ok_or(AdapterError::MalformedRecord {
            member_ordinal: position.0,
            record_ordinal: position.1,
            field: error_field,
        })
}

fn optional_data_string(data: Option<&Value>, field: &str) -> Option<String> {
    data.and_then(|value| value.get(field))
        .and_then(Value::as_str)
        .map(|value| bounded_string(value, MAX_TEXT_CHARS))
        .filter(|value| !value.is_empty())
}

fn tool_event_id(data: Option<&Value>) -> Option<String> {
    data.and_then(|data| {
        ["requestId", "toolCallId"]
            .into_iter()
            .find_map(|field| data.get(field).and_then(Value::as_str))
    })
    .map(str::to_owned)
}

fn is_non_display_event(record_type: &str) -> bool {
    matches!(
        record_type,
        "hook.start"
            | "hook.end"
            | "assistant.turn_start"
            | "assistant.turn_end"
            | "skill.invoked"
            | "subagent.started"
            | "subagent.completed"
            | "permission.requested"
            | "permission.completed"
            | "session.usage_checkpoint"
            | "session.model_change"
            | "session.shutdown"
            | "session.compaction_start"
            | "session.compaction_complete"
            | "session.resume"
            | "session.mode_changed"
            | "session.permissions_changed"
            | "session.plan_changed"
            | "session.auto_mode_resolved"
            | "session.context_changed"
    )
}

fn record_type(value: &Value) -> String {
    value
        .get("type")
        .and_then(Value::as_str)
        .map(|value| bounded_nonempty(value, "untyped-record", MAX_BOUNDED_CHARS))
        .unwrap_or_else(|| "untyped-record".to_owned())
}

fn remote_sync_enabled(value: &Value) -> bool {
    value.get("remote").and_then(Value::as_str) != Some("off")
        || value.get("remoteSessions").and_then(Value::as_bool) == Some(true)
}

fn is_file_change_tool(name: &str) -> bool {
    matches!(
        name,
        "edit" | "write_file" | "create_file" | "edit_file" | "replace_string_in_file"
    )
}

fn parse_jsonc(bytes: &[u8]) -> Result<Value, ()> {
    let stripped = strip_json_comments(bytes)?;
    serde_json::from_slice(&stripped).map_err(|_| ())
}

fn strip_json_comments(bytes: &[u8]) -> Result<Vec<u8>, ()> {
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    let mut in_string = false;
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            output.push(byte);
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }

        if byte == b'"' {
            in_string = true;
            output.push(byte);
            index += 1;
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            output.push(b' ');
            index += 2;
            while index < bytes.len() && !matches!(bytes[index], b'\r' | b'\n') {
                index += 1;
            }
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            output.push(b' ');
            index += 2;
            let mut closed = false;
            while index < bytes.len() {
                if bytes[index] == b'\n' {
                    output.push(b'\n');
                }
                if bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'/') {
                    index += 2;
                    closed = true;
                    break;
                }
                index += 1;
            }
            if !closed {
                return Err(());
            }
        } else {
            output.push(byte);
            index += 1;
        }
    }
    Ok(output)
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
        CopilotCliAdapter, RichOverlay, collect_copilot_assistant_overlays, parse_jsonc,
        remote_sync_enabled,
    };
    use crate::adapters::conformance::assert_conforming_fixture;
    use crate::adapters::contract::{
        AdapterContext, AdapterError, RecordAccountant, run_adapter, run_rich_adapter,
    };
    use crate::io::{
        BundleMemberSpec, ContentKind, LocalRoot, MemberCompression, MemberRole,
        SessionSnapshotBundle, SnapshotLimits, SyntheticMember, capture_bundle_with_hook,
    };
    use crate::model::{DiagnosticSeverity, SessionSource};

    const EVENTS_FIXTURE: &str = include_str!("../../../tests/fixtures/copilot-cli-events.jsonl");
    const RICH_FIXTURE: &str = include_str!("../../../tests/fixtures/copilot-cli-rich.jsonl");
    const SETTINGS_FIXTURE: &str =
        include_str!("../../../tests/fixtures/copilot-cli-settings.json");
    const FORMAT_WARNING_CODE: &str = "copilot-cli-internal-format-unstable";
    const REMOTE_WARNING_CODE: &str = "copilot-cli-remote-sync-enabled";

    fn context() -> AdapterContext {
        AdapterContext::new(
            "copilot-session-1".to_owned(),
            SessionSource::CopilotCli,
            1_500,
        )
        .expect("test context should be valid")
    }

    fn member(id: &str, logical_name: &str, content: &str, partial_final: bool) -> SyntheticMember {
        SyntheticMember {
            id: id.to_owned(),
            logical_name: logical_name.to_owned(),
            records: if logical_name.ends_with(".jsonl") {
                content
                    .lines()
                    .map(|line| line.as_bytes().to_vec())
                    .collect()
            } else {
                vec![content.as_bytes().to_vec()]
            },
            partial_final,
        }
    }

    fn fixture_bundle() -> SessionSnapshotBundle {
        SessionSnapshotBundle::synthetic(vec![
            member("events", "events.jsonl", EVENTS_FIXTURE, false),
            member("settings", "settings.json", SETTINGS_FIXTURE, false),
        ])
    }

    #[test]
    fn rich_assistant_overlay_handles_sparse_and_mixed_content() {
        let accountant = RecordAccountant::new([(0, 5)]);
        for record in 0..5 {
            accountant.classify_unknown(0, record, "synthetic").unwrap();
        }
        let mut overlays = Vec::new();
        collect_copilot_assistant_overlays(None, None, (0, 0), None, &mut overlays, &accountant)
            .unwrap();
        collect_copilot_assistant_overlays(
            Some(&serde_json::json!("plain")),
            Some(2),
            (0, 1),
            Some("message-1".to_owned()),
            &mut overlays,
            &accountant,
        )
        .unwrap();
        collect_copilot_assistant_overlays(
            Some(&serde_json::json!({"unexpected":true})),
            None,
            (0, 2),
            None,
            &mut overlays,
            &accountant,
        )
        .unwrap();
        collect_copilot_assistant_overlays(
            Some(&serde_json::json!([{"type":"reasoning","text":"trace"}])),
            Some(1),
            (0, 3),
            Some("reason-1".to_owned()),
            &mut overlays,
            &accountant,
        )
        .unwrap();
        collect_copilot_assistant_overlays(
            Some(&serde_json::json!([
                {"type":"thinking","thinking":"conservative"},
                {"type":"future_block"}
            ])),
            None,
            (0, 4),
            None,
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
    }

    #[test]
    fn current_fixture_normalizes_messages_tools_files_and_unknowns() {
        let session =
            assert_conforming_fixture(&CopilotCliAdapter, &fixture_bundle(), &context(), 6, 1);

        assert_eq!(session.source, SessionSource::CopilotCli);
        assert_eq!(session.source_version.as_deref(), Some("1.0.81"));
        assert_eq!(
            session.created_at.as_deref(),
            Some("2026-08-21T08:30:00.000Z")
        );
        assert_eq!(session.cwd.as_deref(), Some("C:\\synthetic\\project"));
        let event_values = session
            .events
            .iter()
            .map(|event| serde_json::to_value(event).expect("event should serialize"))
            .collect::<Vec<_>>();
        assert_eq!(event_values[0]["kind"], "user");
        assert_eq!(event_values[0]["text"], "Create a synthetic note.");
        assert_eq!(event_values[0]["atMs"], 1_000);
        assert_eq!(event_values[1]["kind"], "assistant");
        assert_eq!(
            event_values[1]["markdown"],
            "I will update the sample file."
        );
        assert_eq!(event_values[1]["atMs"], 2_000);
        assert_eq!(event_values[2]["kind"], "tool");
        assert_eq!(event_values[2]["name"], "edit");
        assert_eq!(event_values[2]["status"], "running");
        assert_eq!(event_values[2]["summary"], "Tool invocation recorded");
        assert_eq!(event_values[2]["atMs"], 3_000);
        assert_eq!(event_values[3]["kind"], "file-change");
        assert_eq!(event_values[3]["path"], "src/synthetic-note.txt");
        assert_eq!(event_values[3]["summary"], "File change requested");
        assert_eq!(event_values[3]["atMs"], 3_000);
        assert_eq!(event_values[4]["kind"], "tool");
        assert_eq!(event_values[4]["name"], "edit");
        assert_eq!(event_values[4]["status"], "succeeded");
        assert_eq!(event_values[4]["summary"], "Tool result recorded");
        assert_eq!(event_values[4]["atMs"], 4_000);
        assert_eq!(event_values[5]["kind"], "unknown");
        assert_eq!(event_values[5]["sourceType"], "future.event");
        assert_eq!(event_values[5]["atMs"], 5_000);
        let serialized = serde_json::to_string(&session).expect("session should serialize");
        assert!(!serialized.contains("private transformed prompt"));
        assert!(!serialized.contains("private tool input"));
        assert!(!serialized.contains("private tool output"));
        assert!(!serialized.contains("private future payload"));
    }

    #[test]
    fn rich_fixture_preserves_only_explicit_reasoning_and_tool_detail() {
        let bundle = SessionSnapshotBundle::synthetic(vec![member(
            "events",
            "events.jsonl",
            RICH_FIXTURE,
            false,
        )]);
        let session = run_rich_adapter(
            &CopilotCliAdapter,
            &bundle,
            &AdapterContext::new(
                "copilot-session-1".to_owned(),
                SessionSource::CopilotCli,
                1_500,
            )
            .unwrap(),
        )
        .expect("rich Copilot fixture should normalize");

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
                if text == "Use the explicit session data."
        )));
        assert_eq!(
            session
                .entries
                .iter()
                .filter(|entry| matches!(
                    entry,
                    crate::model::NormalizedEntryV2::ToolCall {
                        detail: crate::model::NormalizedToolDetailV2::Available { .. },
                        ..
                    }
                ))
                .count(),
            2
        );
        let serialized = serde_json::to_string(&session).expect("session should serialize");
        assert!(serialized.contains("synthetic input"));
        assert!(serialized.contains("synthetic output"));
        assert!(!serialized.contains("never persist transformed input"));
        assert!(!serialized.contains("never persist signature"));
        assert!(!serialized.contains("never persist future payload"));
    }

    #[test]
    fn rich_copilot_keys_survive_append_only_growth() {
        let appended = format!(
            "{RICH_FIXTURE}{}\n",
            r#"{"type":"assistant.message","data":{"content":"Continue."},"id":"event-appended","timestamp":"2026-08-21T08:30:06Z","parentId":"event-006"}"#
        );
        let context = context();
        let before_bundle = SessionSnapshotBundle::synthetic(vec![member(
            "events",
            "events.jsonl",
            RICH_FIXTURE,
            false,
        )]);
        let after_bundle = SessionSnapshotBundle::synthetic(vec![member(
            "events",
            "events.jsonl",
            &appended,
            false,
        )]);
        let before = run_rich_adapter(&CopilotCliAdapter, &before_bundle, &context).unwrap();
        let after = run_rich_adapter(&CopilotCliAdapter, &after_bundle, &context).unwrap();

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
    fn observed_copilot_lifecycle_metadata_is_understood_without_becoming_entries() {
        let hidden_types = [
            "hook.start",
            "hook.end",
            "assistant.turn_start",
            "assistant.turn_end",
            "skill.invoked",
            "subagent.started",
            "subagent.completed",
            "permission.requested",
            "permission.completed",
            "session.usage_checkpoint",
            "session.model_change",
            "session.shutdown",
            "session.compaction_start",
            "session.compaction_complete",
            "session.resume",
            "session.mode_changed",
            "session.permissions_changed",
            "session.plan_changed",
            "session.auto_mode_resolved",
            "session.context_changed",
        ];
        let mut records = vec![serde_json::json!({
            "type": "user.message",
            "data": {"content": "Visible prompt"},
        })];
        records.extend(hidden_types.map(|record_type| {
            serde_json::json!({
                "type": record_type,
                "data": {"privatePayload": "must not be retained"},
            })
        }));
        let content = records
            .iter()
            .map(|record| serde_json::to_string(record).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        let bundle = SessionSnapshotBundle::synthetic(vec![member(
            "events",
            "events.jsonl",
            &content,
            false,
        )]);

        let session = run_rich_adapter(&CopilotCliAdapter, &bundle, &context())
            .expect("observed lifecycle metadata should normalize");

        assert_eq!(session.unknown_record_count, 0);
        assert_eq!(session.entries.len(), 1);
        assert!(matches!(
            &session.entries[0],
            crate::model::NormalizedEntryV2::User { text, .. } if text == "Visible prompt"
        ));
        assert!(
            !serde_json::to_string(&session)
                .unwrap()
                .contains("must not be retained")
        );
    }

    #[test]
    fn observed_copilot_content_events_normalize_without_unknown_placeholders() {
        let records = [
            serde_json::json!({"type":"user.message","data":{"content":"Visible prompt"}}),
            serde_json::json!({"type":"system.message","id":"system-1","data":{"content":"System guidance","role":"system"}}),
            serde_json::json!({"type":"session.warning","id":"warning-1","data":{"message":"Context is nearly full","warningType":"context"}}),
            serde_json::json!({"type":"system.notification","id":"notice-1","data":{"content":"Background work finished","kind":{"type":"agent"}}}),
            serde_json::json!({"type":"session.info","id":"info-1","data":{"message":"Session information","infoType":"status"}}),
            serde_json::json!({"type":"abort","id":"abort-1","data":{"reason":"Stopped by user"}}),
            serde_json::json!({"type":"external_tool.requested","id":"external-1","data":{"requestId":"request-1","toolCallId":"call-1","toolName":"report","arguments":{"title":"Synthetic report"}}}),
            serde_json::json!({"type":"external_tool.completed","id":"external-2","data":{"requestId":"request-1"}}),
            serde_json::json!({"type":"session.error","id":"error-1","data":{"message":"Provider unavailable","errorType":"provider"}}),
            serde_json::json!({"type":"session.workspace_file_changed","id":"file-1","data":{"operation":"modified","path":"src/example.ts"}}),
            serde_json::json!({"type":"tool.user_requested","id":"tool-1","data":{"toolCallId":"call-2","toolName":"shell","arguments":{"command":"synthetic command"}}}),
            serde_json::json!({"type":"session.task_complete","id":"task-1","data":{"success":true,"summary":"Task completed"}}),
        ];
        let content = records
            .iter()
            .map(|record| serde_json::to_string(record).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        let bundle = SessionSnapshotBundle::synthetic(vec![member(
            "events",
            "events.jsonl",
            &content,
            false,
        )]);

        let session = run_rich_adapter(&CopilotCliAdapter, &bundle, &context())
            .expect("observed content-bearing events should normalize");

        assert_eq!(session.unknown_record_count, 0);
        assert!(
            session
                .entries
                .iter()
                .all(|entry| !matches!(entry, crate::model::NormalizedEntryV2::Unknown { .. }))
        );
        assert_eq!(
            session
                .entries
                .iter()
                .filter(|entry| matches!(entry, crate::model::NormalizedEntryV2::Assistant { .. }))
                .count(),
            4
        );
        assert!(session.entries.iter().any(|entry| matches!(
            entry,
            crate::model::NormalizedEntryV2::FileChange { display_path, summary, .. }
                if display_path == "src/example.ts" && summary == "File modified"
        )));
        assert!(session.entries.iter().any(|entry| matches!(
            entry,
            crate::model::NormalizedEntryV2::ToolCall {
                name,
                status: crate::model::NormalizedToolStatusV2::Running,
                detail: crate::model::NormalizedToolDetailV2::Available { arguments: Some(arguments), .. },
                ..
            } if name == "report" && arguments.contains("Synthetic report")
        )));
        assert!(session.entries.iter().any(|entry| matches!(
            entry,
            crate::model::NormalizedEntryV2::ToolCall {
                name,
                status: crate::model::NormalizedToolStatusV2::Succeeded,
                ..
            } if name == "task"
        )));
    }

    #[test]
    fn observed_copilot_optional_event_fields_have_safe_fallbacks() {
        let records = [
            serde_json::json!({"type":"user.message","data":{"content":"Visible prompt"}}),
            serde_json::json!({"type":"system.message","data":{}}),
            serde_json::json!({"type":"system.notification","data":{"content":""}}),
            serde_json::json!({"type":"session.warning","data":{}}),
            serde_json::json!({"type":"session.info","data":{"message":""}}),
            serde_json::json!({"type":"abort","data":{}}),
            serde_json::json!({"type":"session.error","data":{}}),
            serde_json::json!({"type":"external_tool.requested","data":{}}),
            serde_json::json!({"type":"external_tool.completed","data":{}}),
            serde_json::json!({"type":"tool.user_requested","data":{}}),
            serde_json::json!({"type":"session.workspace_file_changed","data":{"path":""}}),
            serde_json::json!({"type":"session.workspace_file_changed","data":{"path":"src/fallback.ts"}}),
            serde_json::json!({"type":"session.task_complete","data":{}}),
            serde_json::json!({"type":"session.task_complete","data":{"success":false,"summary":""}}),
        ];
        let content = records
            .iter()
            .map(|record| serde_json::to_string(record).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        let bundle = SessionSnapshotBundle::synthetic(vec![member(
            "events",
            "events.jsonl",
            &content,
            false,
        )]);

        let session = run_rich_adapter(&CopilotCliAdapter, &bundle, &context())
            .expect("optional operational fields should use safe fallbacks");

        assert_eq!(session.unknown_record_count, 0);
        assert!(session.entries.iter().any(|entry| matches!(
            entry,
            crate::model::NormalizedEntryV2::ToolCall { name, summary, .. }
                if name == "session" && summary == "Session aborted"
        )));
        assert!(session.entries.iter().any(|entry| matches!(
            entry,
            crate::model::NormalizedEntryV2::ToolCall { name, summary, .. }
                if name == "external tool" && summary == "User-requested tool"
        )));
        assert!(session.entries.iter().any(|entry| matches!(
            entry,
            crate::model::NormalizedEntryV2::ToolCall {
                name,
                status: crate::model::NormalizedToolStatusV2::Failed,
                summary,
                ..
            } if name == "task" && summary == "Task completed"
        )));
        assert!(session.entries.iter().any(|entry| matches!(
            entry,
            crate::model::NormalizedEntryV2::FileChange { display_path, summary, .. }
                if display_path == "src/fallback.ts" && summary == "File changed"
        )));
    }

    #[test]
    fn remote_settings_are_warning_only_and_never_leak_values() {
        let settings_before = SETTINGS_FIXTURE.as_bytes().to_vec();

        let session = run_adapter(&CopilotCliAdapter, &fixture_bundle(), &context())
            .expect("fixture should normalize");

        assert_eq!(SETTINGS_FIXTURE.as_bytes(), settings_before);
        let warning = &session.diagnostics[1];
        assert_eq!(warning.code, REMOTE_WARNING_CODE);
        assert_eq!(warning.severity, DiagnosticSeverity::Warning);
        assert_eq!(
            warning.message,
            "Copilot CLI remote session synchronization is enabled; replay reads only the supplied local snapshot."
        );
        let diagnostics = session
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!diagnostics.contains("must-not-appear"));
        assert!(!diagnostics.contains("settings.json"));
        assert!(!diagnostics.contains("C:\\"));
    }

    #[test]
    fn guarded_internal_format_warning_is_always_present() {
        let session = run_adapter(&CopilotCliAdapter, &fixture_bundle(), &context())
            .expect("fixture should normalize");

        assert_eq!(session.diagnostics[0].code, FORMAT_WARNING_CODE);
        assert_eq!(session.diagnostics[0].severity, DiagnosticSeverity::Warning);
    }

    #[test]
    fn jsonc_settings_comments_are_read_without_exposing_comment_text() {
        let settings = concat!(
            "{\n",
            "  // private-comment-marker\n",
            "  \"remote\": \"on\",\n",
            "  /* private-block-marker */ \"remoteExport\": true\n",
            "}\n",
        );
        let bundle = SessionSnapshotBundle::synthetic(vec![
            member(
                "events",
                "events.jsonl",
                "{\"type\":\"user.message\",\"data\":{\"content\":\"hello\"}}\n",
                false,
            ),
            member("settings", "settings.json", settings, false),
        ]);

        let session = run_adapter(&CopilotCliAdapter, &bundle, &context())
            .expect("JSONC settings should normalize");
        let diagnostics =
            serde_json::to_string(&session.diagnostics).expect("diagnostics should serialize");

        assert!(diagnostics.contains(REMOTE_WARNING_CODE));
        assert!(!diagnostics.contains("private-comment-marker"));
        assert!(!diagnostics.contains("private-block-marker"));
    }

    #[test]
    fn jsonc_comment_removal_never_concatenates_adjacent_tokens() {
        assert!(parse_jsonc(br#"{"remote": 1/* private */2}"#).is_err());
    }

    #[test]
    fn jsonc_comment_scanner_preserves_strings_newlines_and_rejects_unclosed_blocks() {
        let parsed = parse_jsonc(
            br#"{
                "remote": "o\"//ff",
                /* private
                   marker */
                "literal": "/* not a comment */"
            }"#,
        )
        .expect("comment markers in strings should remain data");

        assert_eq!(parsed["remote"], "o\"//ff");
        assert_eq!(parsed["literal"], "/* not a comment */");
        assert_eq!(parse_jsonc(br#"{"remote":"on" /* unclosed"#), Err(()));
    }

    #[test]
    fn legacy_remote_sessions_flag_is_warning_metadata_but_disabled_sync_is_not() {
        assert!(!remote_sync_enabled(&serde_json::json!({"remote": "off"})));
        assert!(remote_sync_enabled(&serde_json::json!({
            "remote": "off",
            "remoteSessions": true
        })));
    }

    #[test]
    fn guarded_fallback_shapes_stay_conforming_and_do_not_replace_metadata() {
        let bundle = SessionSnapshotBundle::synthetic(vec![
            member(
                "events",
                "events.jsonl",
                concat!(
                    "{\"type\":\"session.start\",\"data\":{\"copilotVersion\":\"first\",\"startTime\":\"2026-08-01T00:00:00Z\",\"context\":{\"cwd\":\"C:\\\\first\"}}}\n",
                    "{\"type\":\"session.start\",\"data\":{\"copilotVersion\":\"second\",\"startTime\":\"2026-08-02T00:00:00Z\",\"context\":{\"cwd\":\"C:\\\\second\"}}}\n",
                    "{\"type\":\"user.message\",\"data\":{\"content\":\"start\"}}\n",
                    "{\"type\":\"assistant.message\",\"data\":{\"content\":[{\"type\":\"tool_use\"}]}}\n",
                    "{\"type\":\"tool.execution_start\",\"data\":{\"toolName\":\"edit\"}}\n",
                    "{\"type\":\"\",\"data\":{}}\n",
                ),
                false,
            ),
            member(
                "settings",
                "settings.json",
                "{\"remote\":\"off\",\"remoteSessions\":false}",
                false,
            ),
        ]);

        let session = assert_conforming_fixture(&CopilotCliAdapter, &bundle, &context(), 3, 1);
        assert_eq!(session.source_version.as_deref(), Some("first"));
        assert_eq!(session.cwd.as_deref(), Some("C:\\first"));
        assert_eq!(
            session.created_at.as_deref(),
            Some("2026-08-01T00:00:00.000Z")
        );
        let codes = session
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            codes,
            vec![
                FORMAT_WARNING_CODE,
                "ordering-fallback",
                "no-valid-timestamps"
            ]
        );
        let tool = serde_json::to_value(&session.events[1]).expect("event should serialize");
        let unknown = serde_json::to_value(&session.events[2]).expect("event should serialize");
        assert_eq!(tool["kind"], "tool");
        assert_eq!(tool["name"], "edit");
        assert_eq!(unknown["sourceType"], "untyped-record");
    }

    #[test]
    fn repeated_remote_settings_emit_one_generic_warning() {
        let bundle = SessionSnapshotBundle::synthetic(vec![
            member(
                "events",
                "events.jsonl",
                "{\"type\":\"user.message\",\"data\":{\"content\":\"start\"}}\n",
                false,
            ),
            member(
                "settings-one",
                "settings.json",
                "{\"remote\":\"on\"}",
                false,
            ),
            member(
                "settings-two",
                "settings.json",
                "{\"remote\":\"on\"}",
                false,
            ),
        ]);

        let session = run_adapter(&CopilotCliAdapter, &bundle, &context())
            .expect("settings should normalize");
        let codes = session
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            codes,
            vec![
                FORMAT_WARNING_CODE,
                REMOTE_WARNING_CODE,
                "ordering-fallback",
                "no-valid-timestamps"
            ]
        );
    }

    #[test]
    fn included_settings_use_remote_default_but_an_absent_artifact_does_not_assume_state() {
        let events = "{\"type\":\"user.message\",\"data\":{\"content\":\"start\"}}\n";
        let settings_without_remote = SessionSnapshotBundle::synthetic(vec![
            member("events", "events.jsonl", events, false),
            member(
                "settings",
                "settings.json",
                "{\"remoteExport\":true}",
                false,
            ),
        ]);
        let with_settings = run_adapter(&CopilotCliAdapter, &settings_without_remote, &context())
            .expect("settings using documented defaults should normalize");
        let with_settings_codes = with_settings
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            with_settings_codes,
            vec![
                FORMAT_WARNING_CODE,
                REMOTE_WARNING_CODE,
                "ordering-fallback",
                "no-valid-timestamps"
            ]
        );

        let without_settings = run_adapter(
            &CopilotCliAdapter,
            &SessionSnapshotBundle::synthetic(vec![member(
                "events",
                "events.jsonl",
                events,
                false,
            )]),
            &context(),
        )
        .expect("an events-only bundle should normalize");
        let without_settings_codes = without_settings
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            without_settings_codes,
            vec![
                FORMAT_WARNING_CODE,
                "ordering-fallback",
                "no-valid-timestamps"
            ]
        );
    }

    #[test]
    fn malformed_known_shapes_return_typed_fields() {
        let invalid_content = SessionSnapshotBundle::synthetic(vec![member(
            "events",
            "events.jsonl",
            "{\"type\":\"assistant.message\",\"data\":{\"content\":42}}\n",
            false,
        )]);
        assert_eq!(
            run_adapter(&CopilotCliAdapter, &invalid_content, &context()),
            Err(AdapterError::MalformedRecord {
                member_ordinal: 0,
                record_ordinal: 0,
                field: "message-content",
            })
        );

        let missing_text = SessionSnapshotBundle::synthetic(vec![member(
            "events",
            "events.jsonl",
            "{\"type\":\"assistant.message\",\"data\":{\"content\":[{\"type\":\"text\"}]}}\n",
            false,
        )]);
        assert_eq!(
            run_adapter(&CopilotCliAdapter, &missing_text, &context()),
            Err(AdapterError::MalformedRecord {
                member_ordinal: 0,
                record_ordinal: 0,
                field: "message-text",
            })
        );

        let missing_status = SessionSnapshotBundle::synthetic(vec![member(
            "events",
            "events.jsonl",
            "{\"type\":\"tool.execution_complete\",\"data\":{}}\n",
            false,
        )]);
        assert_eq!(
            run_adapter(&CopilotCliAdapter, &missing_status, &context()),
            Err(AdapterError::MalformedRecord {
                member_ordinal: 0,
                record_ordinal: 0,
                field: "tool-status",
            })
        );
    }

    #[test]
    fn assistant_blocks_preserve_text_and_account_for_one_unknown_record() {
        let bundle = SessionSnapshotBundle::synthetic(vec![member(
            "events",
            "events.jsonl",
            concat!(
                "{\"type\":\"user.message\",\"data\":{\"content\":\"start\"}}\n",
                "{\"type\":\"assistant.message\",\"data\":{\"content\":[",
                "{\"type\":\"text\",\"text\":\"safe answer\"},",
                "{\"type\":\"tool_use\",\"id\":\"ignored-duplicate\"},",
                "{\"type\":\"\",\"private\":\"not surfaced\"},",
                "{\"private\":\"also not surfaced\"}",
                "]}}\n",
            ),
            false,
        )]);

        let session = assert_conforming_fixture(&CopilotCliAdapter, &bundle, &context(), 3, 1);
        let assistant = serde_json::to_value(&session.events[1]).expect("event should serialize");
        let unknown = serde_json::to_value(&session.events[2]).expect("event should serialize");

        assert_eq!(assistant["kind"], "assistant");
        assert_eq!(assistant["markdown"], "safe answer");
        assert_eq!(unknown["kind"], "unknown");
        assert_eq!(unknown["sourceType"], "assistant.message/unknown-block");
    }

    #[test]
    fn tool_completions_use_safe_names_and_statuses_without_results() {
        let bundle = SessionSnapshotBundle::synthetic(vec![member(
            "events",
            "events.jsonl",
            concat!(
                "{\"type\":\"user.message\",\"data\":{\"content\":\"start\"}}\n",
                "{\"type\":\"tool.execution_complete\",\"data\":{\"toolName\":\"edit\",\"success\":false,\"result\":{\"privatePayload\":\"private failure\"}}}\n",
                "{\"type\":\"tool.execution_complete\",\"data\":{\"success\":true,\"result\":{\"privatePayload\":\"private success\"}}}\n",
            ),
            false,
        )]);

        let session = assert_conforming_fixture(&CopilotCliAdapter, &bundle, &context(), 3, 0);
        let failed = serde_json::to_value(&session.events[1]).expect("event should serialize");
        let fallback = serde_json::to_value(&session.events[2]).expect("event should serialize");

        assert_eq!(failed["kind"], "tool");
        assert_eq!(failed["name"], "edit");
        assert_eq!(failed["status"], "failed");
        assert_eq!(fallback["kind"], "tool");
        assert_eq!(fallback["name"], "tool");
        assert_eq!(fallback["status"], "succeeded");
        let serialized = serde_json::to_string(&session).expect("session should serialize");
        assert!(!serialized.contains("private failure"));
        assert!(!serialized.contains("private success"));
    }

    #[test]
    fn unsupported_safe_artifact_is_generic_unknown_without_name_or_payload_leakage() {
        let bundle = SessionSnapshotBundle::synthetic(vec![
            member(
                "events",
                "events.jsonl",
                "{\"type\":\"user.message\",\"data\":{\"content\":\"start\"}}\n",
                false,
            ),
            SyntheticMember {
                id: "artifact".to_owned(),
                logical_name: "checkpoint.bin".to_owned(),
                records: vec![b"private artifact bytes".to_vec()],
                partial_final: false,
            },
        ]);

        let session = assert_conforming_fixture(&CopilotCliAdapter, &bundle, &context(), 2, 1);
        let unknown = serde_json::to_value(&session.events[1]).expect("event should serialize");

        assert_eq!(unknown["kind"], "unknown");
        assert_eq!(unknown["sourceType"], "artifact/unsupported");
        let serialized = serde_json::to_string(&session).expect("session should serialize");
        assert!(!serialized.contains("checkpoint.bin"));
        assert!(!serialized.contains("private artifact bytes"));
    }

    #[test]
    fn metadata_only_session_is_empty_and_invalid_settings_are_position_only() {
        let metadata_only = SessionSnapshotBundle::synthetic(vec![member(
            "events",
            "events.jsonl",
            "{\"type\":\"session.start\",\"data\":{}}\n",
            false,
        )]);
        assert_eq!(
            run_adapter(&CopilotCliAdapter, &metadata_only, &context()),
            Err(AdapterError::EmptySession)
        );

        let invalid_settings = SessionSnapshotBundle::synthetic(vec![
            member(
                "events",
                "events.jsonl",
                "{\"type\":\"user.message\",\"data\":{\"content\":\"start\"}}\n",
                false,
            ),
            member(
                "settings",
                "settings.json",
                "{\"remote\":\"on\",\"private\":",
                false,
            ),
        ]);
        assert_eq!(
            run_adapter(&CopilotCliAdapter, &invalid_settings, &context()),
            Err(AdapterError::MalformedRecord {
                member_ordinal: 1,
                record_ordinal: 0,
                field: "json",
            })
        );
    }

    #[test]
    fn long_running_sessions_are_clamped_instead_of_rejected() {
        let bundle = SessionSnapshotBundle::synthetic(vec![member(
            "events",
            "events.jsonl",
            concat!(
                "{\"type\":\"user.message\",\"data\":{\"content\":\"start\"},\"timestamp\":\"2026-08-01T00:00:00Z\"}\n",
                "{\"type\":\"assistant.message\",\"data\":{\"content\":\"late\"},\"timestamp\":\"2026-08-09T00:00:00Z\"}\n",
            ),
            false,
        )]);

        let session = run_adapter(&CopilotCliAdapter, &bundle, &context())
            .expect("a long-running Copilot session should remain indexable");

        assert_eq!(session.duration_ms, 7 * 24 * 60 * 60 * 1_000);
        assert_eq!(
            session.events.last().map(|event| event.at_ms()),
            Some(604_798_500)
        );
        assert!(
            session
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "timeline-span-clamped")
        );
    }

    struct TempFixture(PathBuf);

    impl TempFixture {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should follow the epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "copilot-cli-adapter-fixture-{}-{nonce}",
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
            id: "events".to_owned(),
            logical_name: "events.jsonl".to_owned(),
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
            "events.jsonl",
            b"{\"type\":\"user.message\",\"data\":{\"content\":\"captured\"}}\n",
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
                        b"{\"type\":\"assistant.message\",\"data\":{\"content\":\"late\"}}\n",
                    )
                    .expect("active writer should append");
            },
        )
        .expect("bounded snapshot should succeed");

        assert_eq!(bundle.record_count(), 1);
        let session = assert_conforming_fixture(&CopilotCliAdapter, &bundle, &context(), 1, 0);
        let event = serde_json::to_value(&session.events[0]).expect("event should serialize");
        assert_eq!(event["kind"], "user");
        assert_eq!(event["text"], "captured");
    }

    #[test]
    fn unknown_complete_record_is_preserved_and_accounted_once() {
        let bundle = SessionSnapshotBundle::synthetic(vec![member(
            "events",
            "events.jsonl",
            concat!(
                "{\"type\":\"user.message\",\"data\":{\"content\":\"known\"}}\n",
                "{\"type\":\"future/private-event\",\"data\":{\"privatePayload\":\"not surfaced\"}}\n",
            ),
            false,
        )]);

        let session = assert_conforming_fixture(&CopilotCliAdapter, &bundle, &context(), 2, 1);

        let unknown = serde_json::to_value(&session.events[1]).expect("event should serialize");
        assert_eq!(unknown["kind"], "unknown");
        assert_eq!(unknown["sourceType"], "future/private-event");
    }

    #[test]
    fn unmatched_tool_start_remains_a_safe_running_event() {
        let bundle = SessionSnapshotBundle::synthetic(vec![member(
            "events",
            "events.jsonl",
            concat!(
                "{\"type\":\"user.message\",\"data\":{\"content\":\"start\"}}\n",
                "{\"type\":\"tool.execution_start\",\"data\":{\"toolCallId\":\"orphan\",\"toolName\":\"shell\",\"arguments\":{\"command\":\"private command\"}}}\n",
            ),
            true,
        )]);

        let session = assert_conforming_fixture(&CopilotCliAdapter, &bundle, &context(), 2, 0);

        let tool = serde_json::to_value(&session.events[1]).expect("event should serialize");
        assert_eq!(tool["kind"], "tool");
        assert_eq!(tool["name"], "shell");
        assert_eq!(tool["status"], "running");
        assert_eq!(tool["summary"], "Tool invocation recorded");
    }

    #[test]
    fn malformed_complete_record_returns_only_a_typed_position() {
        let bundle = SessionSnapshotBundle::synthetic(vec![SyntheticMember {
            id: "events".to_owned(),
            logical_name: "events.jsonl".to_owned(),
            records: vec![
                br#"{"type":"user.message","data":{"content":"before"}}"#.to_vec(),
                br#"{"privateText":"synthetic-marker""#.to_vec(),
            ],
            partial_final: false,
        }]);

        let error = run_adapter(&CopilotCliAdapter, &bundle, &context()).unwrap_err();

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
        let bundle = SessionSnapshotBundle::synthetic(vec![member(
            "events",
            "events.jsonl",
            "{\"type\":\"user.message\",\"data\":{\"content\":\"complete\"}}\n",
            true,
        )]);

        assert!(
            bundle
                .member("events")
                .expect("events member should exist")
                .partial_final_record_ignored()
        );
        assert_conforming_fixture(&CopilotCliAdapter, &bundle, &context(), 1, 0);
    }
}
