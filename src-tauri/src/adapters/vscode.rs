use serde_json::Value;
use time::OffsetDateTime;

use crate::adapters::contract::{
    AdapterContext, AdapterError, RecordAccountant, SessionAdapter, StructuralReason,
};
use crate::adapters::deletion::{DeletionArtifactDeclaration, SourceDeletionAdapter};
use crate::adapters::normalize::{
    EventIdGenerator, clamp_long_running_session_timestamps, compute_duration,
    finalize_rich_session,
};
use crate::adapters::vscode_mutation::{MutationError, replay_mutations};
use crate::io::SessionSnapshotBundle;
use crate::model::{
    DiagnosticSeverity, NormalizedContentAvailabilityV2, NormalizedContentAvailabilityValueV2,
    NormalizedEntryV2, NormalizedReasoningAvailabilityV2, NormalizedSessionV1, NormalizedSessionV2,
    NormalizedToolDetailV2, NormalizedToolStatusV2, ReplayEvent, SessionSource, SourceDiagnostic,
    ToolStatus,
};

const MAX_TEXT_CHARS: usize = 2_000_000;
const MAX_TITLE_CHARS: usize = 512;
const MAX_SOURCE_VERSION_CHARS: usize = 128;
const MAX_DIAGNOSTICS: usize = 1_000;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const FORMAT_WARNING_CODE: &str = "vscode-copilot-internal-format-unstable";

pub struct VscodeCopilotAdapter;

impl SourceDeletionAdapter for VscodeCopilotAdapter {
    fn deletion_artifacts(&self) -> DeletionArtifactDeclaration {
        DeletionArtifactDeclaration::SiblingFiles {
            extensions: &["json", "jsonl"],
        }
    }
}

impl SessionAdapter for VscodeCopilotAdapter {
    fn normalize(
        &self,
        bundle: &SessionSnapshotBundle,
        context: &AdapterContext,
        accountant: &RecordAccountant,
    ) -> Result<NormalizedSessionV1, AdapterError> {
        let rich = parse_vscode_session(bundle, context, accountant)?;
        let events = rich.entries.into_iter().map(downgrade_entry).collect();
        Ok(NormalizedSessionV1 {
            schema_version: 1,
            id: rich.id,
            source: rich.source,
            source_version: rich.source_version,
            title: rich.title,
            created_at: rich.created_at,
            cwd: rich.cwd,
            relationships: rich.relationships,
            events,
            duration_ms: rich.duration_ms,
            diagnostics: rich.diagnostics,
            unknown_record_count: rich.unknown_record_count,
        })
    }

    fn normalize_rich(
        &self,
        bundle: &SessionSnapshotBundle,
        context: &AdapterContext,
        accountant: &RecordAccountant,
    ) -> Result<NormalizedSessionV2, AdapterError> {
        parse_vscode_session(bundle, context, accountant)
    }
}

#[derive(Debug)]
struct PendingEntry {
    entry_key: String,
    timestamp_ms: Option<u64>,
    kind: PendingEntryKind,
}

#[derive(Debug)]
enum PendingEntryKind {
    User(String),
    Assistant(String),
    Reasoning(String),
    Tool {
        name: String,
        status: NormalizedToolStatusV2,
        summary: String,
        detail: NormalizedToolDetailV2,
    },
    FileChange {
        display_path: String,
        summary: String,
    },
}

fn parse_vscode_session(
    bundle: &SessionSnapshotBundle,
    context: &AdapterContext,
    accountant: &RecordAccountant,
) -> Result<NormalizedSessionV2, AdapterError> {
    let members = bundle.members().collect::<Vec<_>>();
    if members.len() != 1 {
        return Err(AdapterError::StructuralViolation {
            reason: StructuralReason::MemberCountExceeded {
                count: members.len(),
            },
        });
    }
    let member = members[0];
    let records = member.records().collect::<Vec<_>>();
    let current_log = member
        .logical_name()
        .to_ascii_lowercase()
        .ends_with(".jsonl");
    let (state, reset_count) = if current_log {
        let replay = replay_mutations(records.iter().copied().enumerate())
            .map_err(mutation_adapter_error)?;
        (replay.value, replay.reset_count)
    } else {
        if records.len() != 1 {
            return Err(AdapterError::MalformedRecord {
                member_ordinal: 0,
                record_ordinal: 0,
                field: "legacy-json",
            });
        }
        let value =
            serde_json::from_slice(records[0]).map_err(|_| AdapterError::MalformedRecord {
                member_ordinal: 0,
                record_ordinal: 0,
                field: "json",
            })?;
        (value, 0)
    };
    for record_ordinal in 0..records.len() {
        accountant.classify_understood(0, record_ordinal)?;
    }

    let mut diagnostics = vec![SourceDiagnostic {
        code: FORMAT_WARNING_CODE.to_owned(),
        severity: DiagnosticSeverity::Warning,
        message: "VS Code Copilot Chat data uses an internal, version-unstable format; replay is best-effort."
            .to_owned(),
    }];
    if reset_count > 0 {
        push_diagnostic(
            &mut diagnostics,
            "vscode-log-reset",
            DiagnosticSeverity::Info,
            "The mutation log contained a replacement snapshot; only the final reconstructed state was used.",
        );
    }
    if member.partial_final_record_ignored() {
        push_diagnostic(
            &mut diagnostics,
            "vscode-partial-record-ignored",
            DiagnosticSeverity::Info,
            "An unterminated final mutation was ignored while VS Code was writing the session.",
        );
    }

    let object = state.as_object().ok_or(AdapterError::MalformedRecord {
        member_ordinal: 0,
        record_ordinal: 0,
        field: "session",
    })?;
    let session_id = object
        .get("sessionId")
        .and_then(Value::as_str)
        .filter(|value| *value == context.session_id())
        .ok_or(AdapterError::MalformedRecord {
            member_ordinal: 0,
            record_ordinal: 0,
            field: "session-id",
        })?;
    let version = match object.get("version") {
        None => 1,
        Some(value) => value
            .as_u64()
            .filter(|value| matches!(value, 2 | 3))
            .ok_or(AdapterError::MalformedRecord {
                member_ordinal: 0,
                record_ordinal: 0,
                field: "version",
            })?,
    };
    let creation_date = object.get("creationDate").and_then(valid_epoch_ms).ok_or(
        AdapterError::MalformedRecord {
            member_ordinal: 0,
            record_ordinal: 0,
            field: "creation-date",
        },
    )?;
    let requests =
        object
            .get("requests")
            .and_then(Value::as_array)
            .ok_or(AdapterError::MalformedRecord {
                member_ordinal: 0,
                record_ordinal: 0,
                field: "requests",
            })?;

    let mut key_generator = EventIdGenerator::new(session_id);
    let mut pending = Vec::new();
    for (request_index, request) in requests.iter().enumerate() {
        collect_request_entries(
            request,
            request_index,
            &mut key_generator,
            &mut pending,
            &mut diagnostics,
        )?;
        if pending.len() > 100_000 {
            return Err(AdapterError::EventLimitExceeded {
                count: pending.len(),
            });
        }
    }
    if pending.is_empty() {
        return Err(AdapterError::EmptySession);
    }

    let mut timestamps = normalize_epoch_timestamps(
        &pending
            .iter()
            .map(|entry| entry.timestamp_ms)
            .collect::<Vec<_>>(),
        creation_date,
        &mut diagnostics,
    );
    clamp_long_running_session_timestamps(
        &mut timestamps,
        context.terminal_hold_ms(),
        &mut diagnostics,
    );
    let duration_ms = compute_duration(&timestamps, context.terminal_hold_ms())?;
    let entries = pending
        .into_iter()
        .zip(timestamps)
        .map(|(entry, at_ms)| pending_to_entry(entry, at_ms))
        .collect();
    let title = object
        .get(if version == 2 {
            "computedTitle"
        } else {
            "customTitle"
        })
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(|value| bounded(value.trim(), MAX_TITLE_CHARS))
        .or_else(|| first_request_title(requests))
        .unwrap_or_else(|| "VS Code Copilot session".to_owned());

    let mut session = NormalizedSessionV2 {
        schema_version: 2,
        id: context.session_id().to_owned(),
        source: SessionSource::VscodeCopilot,
        source_version: Some(bounded(
            &format!("chat-session-v{version}"),
            MAX_SOURCE_VERSION_CHARS,
        )),
        title,
        created_at: epoch_ms_to_rfc3339(creation_date),
        cwd: None,
        relationships: Vec::new(),
        entries,
        duration_ms,
        diagnostics,
        unknown_record_count: 0,
        content_availability: NormalizedContentAvailabilityV2 {
            reasoning: NormalizedReasoningAvailabilityV2::Unavailable,
            tool_details: NormalizedContentAvailabilityValueV2::Unavailable,
        },
    };
    finalize_rich_session(&mut session)?;
    Ok(session)
}

fn collect_request_entries(
    request: &Value,
    request_index: usize,
    keys: &mut EventIdGenerator,
    pending: &mut Vec<PendingEntry>,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Result<(), AdapterError> {
    let object = request.as_object().ok_or(AdapterError::MalformedRecord {
        member_ordinal: 0,
        record_ordinal: request_index,
        field: "request",
    })?;
    let request_id = object
        .get("requestId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.chars().count() <= 256)
        .ok_or(AdapterError::MalformedRecord {
            member_ordinal: 0,
            record_ordinal: request_index,
            field: "request-id",
        })?;
    if object
        .get("hiddenFromTranscript")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || object
            .get("isHidden")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return Ok(());
    }
    let timestamp = object.get("timestamp").and_then(valid_epoch_ms);
    let hide_request = object
        .get("requestHiddenFromTranscript")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !hide_request && let Some(text) = object.get("message").and_then(message_text) {
        pending.push(PendingEntry {
            entry_key: keys.next_id_for_vendor("user", request_id),
            timestamp_ms: timestamp,
            kind: PendingEntryKind::User(text),
        });
    }

    let response_timestamp = object
        .get("responseTimestamp")
        .and_then(valid_epoch_ms)
        .or_else(|| {
            object
                .get("modelState")
                .and_then(|state| state.get("completedAt"))
                .and_then(valid_epoch_ms)
        })
        .or(timestamp);
    let response_entry_start = pending.len();
    let response = object.get("response");
    match response {
        Some(Value::Array(parts)) => {
            for (part_index, part) in parts.iter().enumerate() {
                collect_response_part(
                    part,
                    request_id,
                    part_index,
                    response_timestamp,
                    keys,
                    pending,
                    diagnostics,
                );
            }
        }
        Some(part @ (Value::String(_) | Value::Object(_))) => collect_response_part(
            part,
            request_id,
            0,
            response_timestamp,
            keys,
            pending,
            diagnostics,
        ),
        Some(Value::Null) | None => {}
        Some(_) => push_unsupported_part(diagnostics, "invalid-response"),
    }
    if pending.len() == response_entry_start
        && let Some(message) = object
            .get("result")
            .and_then(|result| result.get("errorDetails"))
            .and_then(|details| details.get("message"))
            .and_then(Value::as_str)
            .and_then(non_empty_text)
    {
        pending.push(PendingEntry {
            entry_key: keys.next_id_for_vendor("assistant", &format!("{request_id}:error")),
            timestamp_ms: response_timestamp,
            kind: PendingEntryKind::Assistant(message),
        });
    }
    Ok(())
}

fn collect_response_part(
    part: &Value,
    request_id: &str,
    part_index: usize,
    timestamp_ms: Option<u64>,
    keys: &mut EventIdGenerator,
    pending: &mut Vec<PendingEntry>,
    diagnostics: &mut Vec<SourceDiagnostic>,
) {
    let identity = format!("{request_id}:part:{part_index}");
    if let Some(text) = markdown_text(part) {
        pending.push(PendingEntry {
            entry_key: keys.next_id_for_vendor("assistant", &identity),
            timestamp_ms,
            kind: PendingEntryKind::Assistant(text),
        });
        return;
    }
    let Some(kind) = part.get("kind").and_then(Value::as_str) else {
        push_unsupported_part(diagnostics, "untagged-object");
        return;
    };
    match kind {
        "thinking" => {
            if let Some(text) = thinking_text(part) {
                pending.push(PendingEntry {
                    entry_key: keys.next_id_for_vendor("reasoning", &identity),
                    timestamp_ms,
                    kind: PendingEntryKind::Reasoning(text),
                });
            }
        }
        "toolInvocationSerialized" => {
            let tool_call_id = part
                .get("toolCallId")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or(&identity);
            pending.push(PendingEntry {
                entry_key: keys.next_id_for_vendor("tool-call", tool_call_id),
                timestamp_ms,
                kind: tool_entry(part),
            });
        }
        "textEditGroup" | "notebookEditGroup" | "externalEdit" => {
            pending.push(PendingEntry {
                entry_key: keys.next_id_for_vendor("file-change", &identity),
                timestamp_ms,
                kind: PendingEntryKind::FileChange {
                    display_path: display_path(part.get("uri"))
                        .unwrap_or_else(|| "File".to_owned()),
                    summary: file_change_summary(kind),
                },
            });
        }
        "workspaceEdit" => {
            let edits = part.get("edits").and_then(Value::as_array);
            if let Some(edits) = edits.filter(|edits| !edits.is_empty()) {
                for (edit_index, edit) in edits.iter().enumerate() {
                    let edit_identity = format!("{identity}:edit:{edit_index}");
                    pending.push(PendingEntry {
                        entry_key: keys.next_id_for_vendor("file-change", &edit_identity),
                        timestamp_ms,
                        kind: PendingEntryKind::FileChange {
                            display_path: display_path(
                                edit.get("newResource").or_else(|| edit.get("oldResource")),
                            )
                            .unwrap_or_else(|| "File".to_owned()),
                            summary: "Workspace file changed".to_owned(),
                        },
                    });
                }
            } else {
                push_unsupported_part(diagnostics, kind);
            }
        }
        "warning"
        | "info"
        | "progressMessage"
        | "systemNotification"
        | "progressTaskSerialized" => {
            if let Some(text) = part.get("content").and_then(message_text) {
                pending.push(PendingEntry {
                    entry_key: keys.next_id_for_vendor("assistant", &identity),
                    timestamp_ms,
                    kind: PendingEntryKind::Assistant(text),
                });
            }
        }
        _ => push_unsupported_part(diagnostics, kind),
    }
}

fn tool_entry(part: &Value) -> PendingEntryKind {
    let complete = part
        .get("isComplete")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let failed = part
        .get("resultDetails")
        .and_then(|details| details.get("isError"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let name = part
        .get("generatedTitle")
        .and_then(Value::as_str)
        .and_then(non_empty_text)
        .or_else(|| {
            part.get("toolId")
                .and_then(Value::as_str)
                .and_then(non_empty_text)
        })
        .unwrap_or_else(|| "Tool".to_owned());
    let summary = part
        .get(if complete {
            "pastTenseMessage"
        } else {
            "invocationMessage"
        })
        .and_then(message_text)
        .unwrap_or_else(|| {
            if complete {
                "Tool completed"
            } else {
                "Tool running"
            }
            .to_owned()
        });
    let arguments = terminal_command(part).or_else(|| {
        part.get("resultDetails")
            .and_then(|details| details.get("input"))
            .and_then(Value::as_str)
            .and_then(non_empty_text)
    });
    let result = terminal_output(part).or_else(|| tool_text_output(part));
    PendingEntryKind::Tool {
        name,
        status: if failed {
            NormalizedToolStatusV2::Failed
        } else if complete {
            NormalizedToolStatusV2::Succeeded
        } else {
            NormalizedToolStatusV2::Running
        },
        summary,
        detail: if arguments.is_some() || result.is_some() {
            NormalizedToolDetailV2::Available { arguments, result }
        } else {
            NormalizedToolDetailV2::Unavailable
        },
    }
}

fn terminal_command(part: &Value) -> Option<String> {
    let line = part.get("toolSpecificData")?.get("commandLine")?;
    ["forDisplay", "userEdited", "toolEdited", "original"]
        .into_iter()
        .find_map(|field| {
            line.get(field)
                .and_then(Value::as_str)
                .and_then(non_empty_text)
        })
}

fn terminal_output(part: &Value) -> Option<String> {
    part.get("toolSpecificData")?
        .get("terminalCommandOutput")?
        .get("text")?
        .as_str()
        .and_then(non_empty_text)
}

fn tool_text_output(part: &Value) -> Option<String> {
    let output = part.get("resultDetails")?.get("output")?;
    if let Some(text) = output.as_str().and_then(non_empty_text) {
        return Some(text);
    }
    let text = output
        .as_array()?
        .iter()
        .filter(|item| item.get("isText").and_then(Value::as_bool) == Some(true))
        .filter_map(|item| item.get("value").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    non_empty_text(&text)
}

fn message_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => non_empty_text(text),
        Value::Object(object) => object
            .get("text")
            .and_then(Value::as_str)
            .and_then(non_empty_text)
            .or_else(|| {
                object
                    .get("value")
                    .and_then(Value::as_str)
                    .and_then(non_empty_text)
            })
            .or_else(|| object.get("content").and_then(message_text)),
        _ => None,
    }
}

fn markdown_text(value: &Value) -> Option<String> {
    match value {
        Value::String(_) => message_text(value),
        Value::Object(object) if !object.contains_key("kind") => message_text(value),
        Value::Object(object)
            if object.get("kind").and_then(Value::as_str) == Some("markdownContent") =>
        {
            object.get("content").and_then(message_text)
        }
        _ => None,
    }
}

fn thinking_text(part: &Value) -> Option<String> {
    match part.get("value")? {
        Value::String(text) => non_empty_text(text),
        Value::Array(parts) => non_empty_text(
            &parts
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        _ => None,
    }
}

fn display_path(uri: Option<&Value>) -> Option<String> {
    let value = uri
        .and_then(|uri| uri.get("path").or_else(|| uri.get("fsPath")))
        .and_then(Value::as_str)?;
    value
        .rsplit(['/', '\\'])
        .find(|component| !component.is_empty())
        .map(|component| bounded(component, 512))
}

fn file_change_summary(kind: &str) -> String {
    match kind {
        "externalEdit" => "External file change".to_owned(),
        "notebookEditGroup" => "Notebook changed".to_owned(),
        _ => "File changed".to_owned(),
    }
}

fn first_request_title(requests: &[Value]) -> Option<String> {
    requests
        .iter()
        .filter(|request| {
            !request
                .get("hiddenFromTranscript")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                && !request
                    .get("isHidden")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                && !request
                    .get("requestHiddenFromTranscript")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
        })
        .filter_map(|request| request.get("message"))
        .find_map(message_text)
        .map(|text| bounded(text.trim(), MAX_TITLE_CHARS))
}

fn normalize_epoch_timestamps(
    timestamps: &[Option<u64>],
    origin_ms: u64,
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Vec<u64> {
    let mut previous = 0;
    let mut diagnosed_missing = false;
    let mut diagnosed_decrease = false;
    timestamps
        .iter()
        .map(|timestamp| {
            let current = match timestamp {
                Some(timestamp) => timestamp.saturating_sub(origin_ms),
                None => {
                    if !diagnosed_missing {
                        push_diagnostic(
                            diagnostics,
                            "missing-timestamp",
                            DiagnosticSeverity::Info,
                            "A VS Code chat entry had no timestamp and inherited the preceding time.",
                        );
                        diagnosed_missing = true;
                    }
                    previous
                }
            };
            if current < previous {
                if !diagnosed_decrease {
                    push_diagnostic(
                        diagnostics,
                        "decreasing-timestamp",
                        DiagnosticSeverity::Warning,
                        "A VS Code chat timestamp moved backwards and was clamped to preserve order.",
                    );
                    diagnosed_decrease = true;
                }
                previous
            } else {
                previous = current;
                current
            }
        })
        .collect()
}

fn pending_to_entry(pending: PendingEntry, at_ms: u64) -> NormalizedEntryV2 {
    match pending.kind {
        PendingEntryKind::User(text) => NormalizedEntryV2::User {
            entry_key: pending.entry_key,
            at_ms,
            text,
        },
        PendingEntryKind::Assistant(markdown) => NormalizedEntryV2::Assistant {
            entry_key: pending.entry_key,
            at_ms,
            markdown,
        },
        PendingEntryKind::Reasoning(text) => NormalizedEntryV2::Reasoning {
            entry_key: pending.entry_key,
            at_ms,
            text,
        },
        PendingEntryKind::Tool {
            name,
            status,
            summary,
            detail,
        } => NormalizedEntryV2::ToolCall {
            entry_key: pending.entry_key,
            at_ms,
            name,
            status,
            summary,
            detail,
        },
        PendingEntryKind::FileChange {
            display_path,
            summary,
        } => NormalizedEntryV2::FileChange {
            entry_key: pending.entry_key,
            at_ms,
            display_path,
            summary,
        },
    }
}

fn downgrade_entry(entry: NormalizedEntryV2) -> ReplayEvent {
    match entry {
        NormalizedEntryV2::User {
            entry_key,
            at_ms,
            text,
        } => ReplayEvent::User {
            id: entry_key,
            at_ms,
            text,
        },
        NormalizedEntryV2::Assistant {
            entry_key,
            at_ms,
            markdown,
        }
        | NormalizedEntryV2::Reasoning {
            entry_key,
            at_ms,
            text: markdown,
        } => ReplayEvent::Assistant {
            id: entry_key,
            at_ms,
            markdown,
        },
        NormalizedEntryV2::ToolCall {
            entry_key,
            at_ms,
            name,
            status,
            summary,
            ..
        } => ReplayEvent::Tool {
            id: entry_key,
            at_ms,
            name,
            status: match status {
                NormalizedToolStatusV2::Pending | NormalizedToolStatusV2::Running => {
                    ToolStatus::Running
                }
                NormalizedToolStatusV2::Succeeded => ToolStatus::Succeeded,
                NormalizedToolStatusV2::Failed => ToolStatus::Failed,
            },
            summary,
        },
        NormalizedEntryV2::FileChange {
            entry_key,
            at_ms,
            display_path,
            summary,
        } => ReplayEvent::FileChange {
            id: entry_key,
            at_ms,
            path: display_path,
            summary,
        },
        NormalizedEntryV2::Unknown {
            entry_key,
            at_ms,
            source_type,
        } => ReplayEvent::Unknown {
            id: entry_key,
            at_ms,
            source_type,
        },
    }
}

fn mutation_adapter_error(error: MutationError) -> AdapterError {
    let (record_ordinal, field) = match error {
        MutationError::InvalidJson { record_ordinal } => (record_ordinal, "json"),
        MutationError::InvalidKind { record_ordinal } => (record_ordinal, "mutation-kind"),
        MutationError::MissingInitial { record_ordinal } => (record_ordinal, "initial-mutation"),
        MutationError::InvalidPath { record_ordinal } => (record_ordinal, "mutation-path"),
        MutationError::InvalidValue { record_ordinal } => (record_ordinal, "mutation-value"),
    };
    AdapterError::MalformedRecord {
        member_ordinal: 0,
        record_ordinal,
        field,
    }
}

fn valid_epoch_ms(value: &Value) -> Option<u64> {
    value.as_u64().filter(|value| *value <= MAX_SAFE_INTEGER)
}

fn epoch_ms_to_rfc3339(milliseconds: u64) -> Option<String> {
    let nanoseconds = i128::from(milliseconds).checked_mul(1_000_000)?;
    let value = OffsetDateTime::from_unix_timestamp_nanos(nanoseconds).ok()?;
    let year = value.year();
    if !(0..=9_999).contains(&year) {
        return None;
    }
    Some(format!(
        "{year:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        u8::from(value.month()),
        value.day(),
        value.hour(),
        value.minute(),
        value.second(),
        value.millisecond(),
    ))
}

fn non_empty_text(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| bounded(value, MAX_TEXT_CHARS))
}

fn bounded(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn push_unsupported_part(diagnostics: &mut Vec<SourceDiagnostic>, kind: &str) {
    let kind = bounded(kind, 128);
    push_diagnostic(
        diagnostics,
        "vscode-unsupported-response-part",
        DiagnosticSeverity::Info,
        &format!(
            "VS Code response part '{kind}' was omitted because it has no safe replay mapping."
        ),
    );
}

fn push_diagnostic(
    diagnostics: &mut Vec<SourceDiagnostic>,
    code: &str,
    severity: DiagnosticSeverity,
    message: &str,
) {
    if diagnostics.len() < MAX_DIAGNOSTICS {
        diagnostics.push(SourceDiagnostic {
            code: code.to_owned(),
            severity,
            message: bounded(message, 4_096),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_DIAGNOSTICS, MAX_SAFE_INTEGER, PendingEntryKind, VscodeCopilotAdapter, display_path,
        epoch_ms_to_rfc3339, file_change_summary, markdown_text, message_text, push_diagnostic,
        thinking_text, tool_entry, valid_epoch_ms,
    };
    use crate::adapters::contract::{AdapterContext, AdapterError, run_rich_adapter};
    use crate::io::{SessionSnapshotBundle, SyntheticMember};
    use crate::model::{
        NormalizedContentAvailabilityValueV2, NormalizedEntryV2, NormalizedReasoningAvailabilityV2,
        NormalizedToolDetailV2, NormalizedToolStatusV2, SessionSource,
    };

    const CURRENT: &str = include_str!("../../../tests/fixtures/vscode-copilot-current.jsonl");
    const LEGACY: &str = include_str!("../../../tests/fixtures/vscode-copilot-legacy.json");

    fn context(id: &str) -> AdapterContext {
        AdapterContext::new(id.to_owned(), SessionSource::VscodeCopilot, 1_500)
            .expect("VS Code context should be valid")
    }

    fn log_bundle(lines: impl IntoIterator<Item = &'static str>) -> SessionSnapshotBundle {
        SessionSnapshotBundle::synthetic(vec![SyntheticMember {
            id: "primary".to_owned(),
            logical_name: "session.jsonl".to_owned(),
            records: lines
                .into_iter()
                .map(|line| line.as_bytes().to_vec())
                .collect(),
            partial_final: false,
        }])
    }

    fn legacy_bundle(contents: &'static str) -> SessionSnapshotBundle {
        SessionSnapshotBundle::synthetic(vec![SyntheticMember {
            id: "primary".to_owned(),
            logical_name: "session.json".to_owned(),
            records: vec![contents.as_bytes().to_vec()],
            partial_final: false,
        }])
    }

    fn value_bundle(
        value: serde_json::Value,
        logical_name: &str,
        partial_final: bool,
    ) -> SessionSnapshotBundle {
        SessionSnapshotBundle::synthetic(vec![SyntheticMember {
            id: "primary".to_owned(),
            logical_name: logical_name.to_owned(),
            records: vec![serde_json::to_vec(&value).unwrap()],
            partial_final,
        }])
    }

    #[test]
    fn reconstructs_current_logs_and_maps_supported_chat_parts() {
        let session = run_rich_adapter(
            &VscodeCopilotAdapter,
            &log_bundle(CURRENT.lines()),
            &context("vscode-session-1"),
        )
        .expect("current VS Code chat should normalize");

        assert_eq!(session.title, "Synthetic VS Code chat");
        assert_eq!(session.source_version.as_deref(), Some("chat-session-v3"));
        assert_eq!(
            session.created_at.as_deref(),
            Some("2026-09-01T12:00:00.000Z")
        );
        assert_eq!(session.entries.len(), 8);
        assert!(
            matches!(&session.entries[0], NormalizedEntryV2::User { text, at_ms: 1_000, .. } if text == "Inspect the synthetic workspace")
        );
        assert!(
            matches!(&session.entries[1], NormalizedEntryV2::Assistant { markdown, at_ms: 2_000, .. } if markdown == "I will inspect it.")
        );
        assert!(
            matches!(&session.entries[2], NormalizedEntryV2::Reasoning { text, .. } if text == "Checking files\nPlanning changes")
        );
        assert!(matches!(
            &session.entries[3],
            NormalizedEntryV2::ToolCall {
                name,
                status: NormalizedToolStatusV2::Succeeded,
                summary,
                detail: NormalizedToolDetailV2::Available { arguments: Some(arguments), result: Some(result) },
                ..
            } if name == "run_in_terminal" && summary == "Ran tests" && arguments == "bun test" && result == "2 tests passed"
        ));
        assert!(
            matches!(&session.entries[6], NormalizedEntryV2::FileChange { display_path, .. } if display_path == "example.ts")
        );
        assert!(
            session
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "vscode-unsupported-response-part")
        );
        assert_eq!(session.unknown_record_count, 0);
        assert_eq!(
            session.content_availability.reasoning,
            NormalizedReasoningAvailabilityV2::Available
        );
        assert_eq!(
            session.content_availability.tool_details,
            NormalizedContentAvailabilityValueV2::Available
        );
        let serialized = serde_json::to_string(&session).expect("session should serialize");
        assert!(!serialized.contains("must-not-leak"));
    }

    #[test]
    fn normalizes_legacy_flat_json_with_version_dispatch() {
        let session = run_rich_adapter(
            &VscodeCopilotAdapter,
            &legacy_bundle(LEGACY),
            &context("vscode-legacy-1"),
        )
        .expect("legacy VS Code chat should normalize");

        assert_eq!(session.title, "Legacy synthetic chat");
        assert_eq!(session.source_version.as_deref(), Some("chat-session-v2"));
        assert!(
            matches!(&session.entries[0], NormalizedEntryV2::User { text, .. } if text == "Legacy question")
        );
        assert!(
            matches!(&session.entries[1], NormalizedEntryV2::Assistant { markdown, .. } if markdown == "Legacy answer")
        );
    }

    #[test]
    fn stable_request_and_part_keys_survive_append_only_growth() {
        let lines = CURRENT.lines().collect::<Vec<_>>();
        let before = run_rich_adapter(
            &VscodeCopilotAdapter,
            &log_bundle(lines[..1].iter().copied()),
            &context("vscode-session-1"),
        )
        .expect("initial VS Code state should normalize");
        let after = run_rich_adapter(
            &VscodeCopilotAdapter,
            &log_bundle(lines.iter().copied()),
            &context("vscode-session-1"),
        )
        .expect("appended VS Code state should normalize");

        assert_eq!(
            before
                .entries
                .iter()
                .map(NormalizedEntryV2::entry_key)
                .collect::<Vec<_>>(),
            after.entries[..before.entries.len()]
                .iter()
                .map(NormalizedEntryV2::entry_key)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn rejects_malformed_mutations_and_session_identity_mismatches() {
        let malformed = log_bundle([
            r#"{"kind":0,"v":{"version":3,"creationDate":1,"sessionId":"vscode-session-1","responderUsername":"Copilot","requests":[]}}"#,
            r#"{"kind":1,"k":["requests",4,"message"],"v":"invalid"}"#,
        ]);
        assert!(matches!(
            run_rich_adapter(
                &VscodeCopilotAdapter,
                &malformed,
                &context("vscode-session-1")
            ),
            Err(AdapterError::MalformedRecord {
                field: "mutation-path",
                ..
            })
        ));

        assert!(matches!(
            run_rich_adapter(
                &VscodeCopilotAdapter,
                &legacy_bundle(LEGACY),
                &context("different-session")
            ),
            Err(AdapterError::MalformedRecord {
                field: "session-id",
                ..
            })
        ));
    }

    #[test]
    fn maps_alternate_response_shapes_without_exposing_unknown_payloads() {
        let state = serde_json::json!({
            "creationDate": 2_000,
            "sessionId": "vscode-shapes",
            "responderUsername": "Copilot",
            "requests": [
                {"requestId":"hidden-1","timestamp":2_001,"message":"hidden","hiddenFromTranscript":true,"response":[{"value":"hidden"}]},
                {"requestId":"hidden-2","timestamp":2_002,"message":"hidden","isHidden":true,"response":[{"value":"hidden"}]},
                {"requestId":"response-only","timestamp":2_003,"message":"hidden prompt","requestHiddenFromTranscript":true,"response":[{"kind":"markdownContent","content":{"value":"Visible response"}}]},
                {"requestId":"error","message":"Failure request","response":null,"result":{"errorDetails":{"message":"Safe failure"}}},
                {"requestId":"parts","timestamp":1_999,"message":{"text":"Shape request"},"responseTimestamp":2_004,"response":[
                    {"kind":"thinking","value":"Thinking"},
                    {"kind":"toolInvocationSerialized","isComplete":false},
                    {"kind":"toolInvocationSerialized","toolId":"failed_tool","isComplete":true,"resultDetails":{"isError":true,"input":"input","output":"output"}},
                    {"kind":"externalEdit","uri":{"fsPath":"C:\\synthetic\\external.rs"}},
                    {"kind":"notebookEditGroup"},
                    {"kind":"workspaceEdit","edits":[{"newResource":{"path":"/workspace/new.ts"}},{"oldResource":{"path":"/workspace/old.ts"}}]},
                    {"kind":"workspaceEdit","edits":[]},
                    {"kind":"progressMessage","content":"Progress"},
                    {"kind":"info","content":{"value":"Info"}},
                    {"kind":"systemNotification","content":{"text":"Notice"}},
                    {"kind":"progressTaskSerialized","content":{"content":{"value":"Task"}}},
                    {"unexpected":"private-payload"}
                ]}
            ]
        });
        let session = run_rich_adapter(
            &VscodeCopilotAdapter,
            &value_bundle(state, "vscode-shapes.json", false),
            &context("vscode-shapes"),
        )
        .expect("supported alternate shapes should normalize");

        assert_eq!(session.source_version.as_deref(), Some("chat-session-v1"));
        assert_eq!(session.title, "Failure request");
        assert!(session.entries.iter().any(|entry| matches!(entry, NormalizedEntryV2::Assistant { markdown, .. } if markdown == "Safe failure")));
        assert!(session.entries.iter().any(|entry| matches!(
            entry,
            NormalizedEntryV2::ToolCall {
                status: NormalizedToolStatusV2::Running,
                detail: NormalizedToolDetailV2::Unavailable,
                ..
            }
        )));
        assert!(session.entries.iter().any(|entry| matches!(entry, NormalizedEntryV2::ToolCall { status: NormalizedToolStatusV2::Failed, detail: NormalizedToolDetailV2::Available { arguments: Some(arguments), result: Some(result) }, .. } if arguments == "input" && result == "output")));
        assert!(session.entries.iter().any(|entry| matches!(entry, NormalizedEntryV2::FileChange { display_path, summary, .. } if display_path == "external.rs" && summary == "External file change")));
        assert!(session.entries.iter().any(|entry| matches!(entry, NormalizedEntryV2::FileChange { display_path, summary, .. } if display_path == "File" && summary == "Notebook changed")));
        assert!(session.entries.iter().any(|entry| matches!(entry, NormalizedEntryV2::FileChange { display_path, summary, .. } if display_path == "new.ts" && summary == "Workspace file changed")));
        assert!(
            session
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "missing-timestamp")
        );
        assert!(
            session
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "decreasing-timestamp")
        );
        let serialized = serde_json::to_string(&session).unwrap();
        assert!(!serialized.contains("private-payload"));
    }

    #[test]
    fn reports_resets_partial_tails_and_safe_fallback_titles() {
        let first = r#"{"kind":0,"v":{"version":3,"creationDate":1000,"sessionId":"vscode-reset","responderUsername":"Copilot","requests":[{"requestId":"old","message":"Old","response":[{"value":"Old response"}]}]}}"#;
        let replacement = r#"{"kind":0,"v":{"version":3,"creationDate":1000,"sessionId":"vscode-reset","responderUsername":"Copilot","requests":[{"requestId":"new","message":"Fallback title","response":[{"value":"New response"}]}]}}"#;
        let bundle = SessionSnapshotBundle::synthetic(vec![SyntheticMember {
            id: "primary".to_owned(),
            logical_name: "session.jsonl".to_owned(),
            records: vec![first.as_bytes().to_vec(), replacement.as_bytes().to_vec()],
            partial_final: true,
        }]);
        let session = run_rich_adapter(&VscodeCopilotAdapter, &bundle, &context("vscode-reset"))
            .expect("replacement snapshot should normalize");

        assert_eq!(session.title, "Fallback title");
        assert_eq!(session.entries.len(), 2);
        assert!(
            session
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "vscode-log-reset")
        );
        assert!(
            session
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "vscode-partial-record-ignored")
        );
    }

    #[test]
    fn rejects_invalid_flat_session_shapes_without_payload_details() {
        let cases = [
            (serde_json::json!([]), "session"),
            (
                serde_json::json!({"version":4,"creationDate":1,"sessionId":"bad","requests":[]}),
                "version",
            ),
            (
                serde_json::json!({"version":3,"creationDate":-1,"sessionId":"bad","requests":[]}),
                "creation-date",
            ),
            (
                serde_json::json!({"version":3,"creationDate":1,"sessionId":"bad","requests":{}}),
                "requests",
            ),
            (
                serde_json::json!({"version":3,"creationDate":1,"sessionId":"bad","requests":[null]}),
                "request",
            ),
            (
                serde_json::json!({"version":3,"creationDate":1,"sessionId":"bad","requests":[{"message":"x"}]}),
                "request-id",
            ),
        ];
        for (state, field) in cases {
            assert!(matches!(
                run_rich_adapter(
                    &VscodeCopilotAdapter,
                    &value_bundle(state, "bad.json", false),
                    &context("bad")
                ),
                Err(AdapterError::MalformedRecord { field: actual, .. }) if actual == field
            ));
        }

        let invalid_json = SessionSnapshotBundle::synthetic(vec![SyntheticMember {
            id: "primary".to_owned(),
            logical_name: "bad.json".to_owned(),
            records: vec![b"not-json".to_vec()],
            partial_final: false,
        }]);
        assert!(matches!(
            run_rich_adapter(&VscodeCopilotAdapter, &invalid_json, &context("bad")),
            Err(AdapterError::MalformedRecord { field: "json", .. })
        ));
    }

    #[test]
    fn bounds_diagnostics_and_rejects_multiple_or_empty_members() {
        let mut diagnostics = Vec::new();
        for _ in 0..=MAX_DIAGNOSTICS {
            push_diagnostic(
                &mut diagnostics,
                "bounded",
                crate::model::DiagnosticSeverity::Info,
                "safe",
            );
        }
        assert_eq!(diagnostics.len(), MAX_DIAGNOSTICS);

        let two_members = SessionSnapshotBundle::synthetic(vec![
            SyntheticMember {
                id: "one".to_owned(),
                logical_name: "one.json".to_owned(),
                records: vec![LEGACY.as_bytes().to_vec()],
                partial_final: false,
            },
            SyntheticMember {
                id: "two".to_owned(),
                logical_name: "two.json".to_owned(),
                records: vec![LEGACY.as_bytes().to_vec()],
                partial_final: false,
            },
        ]);
        assert!(matches!(
            run_rich_adapter(
                &VscodeCopilotAdapter,
                &two_members,
                &context("vscode-legacy-1")
            ),
            Err(AdapterError::StructuralViolation { .. })
        ));
        let multiple_records = SessionSnapshotBundle::synthetic(vec![SyntheticMember {
            id: "primary".to_owned(),
            logical_name: "legacy.json".to_owned(),
            records: vec![LEGACY.as_bytes().to_vec(), LEGACY.as_bytes().to_vec()],
            partial_final: false,
        }]);
        assert!(matches!(
            run_rich_adapter(
                &VscodeCopilotAdapter,
                &multiple_records,
                &context("vscode-legacy-1")
            ),
            Err(AdapterError::MalformedRecord {
                field: "legacy-json",
                ..
            })
        ));
    }

    #[test]
    fn bounded_extractors_cover_safe_fallbacks_without_serializing_arbitrary_values() {
        assert_eq!(message_text(&serde_json::json!(true)), None);
        assert_eq!(
            message_text(&serde_json::json!({"content":{"value":"Nested"}})),
            Some("Nested".to_owned())
        );
        assert_eq!(
            markdown_text(&serde_json::json!({"kind":"markdownContent","content":"Markdown"})),
            Some("Markdown".to_owned())
        );
        assert_eq!(markdown_text(&serde_json::json!({"kind":"future"})), None);
        assert_eq!(thinking_text(&serde_json::json!({"value":[]})), None);
        assert_eq!(thinking_text(&serde_json::json!({"value":7})), None);
        assert_eq!(display_path(None), None);
        assert_eq!(display_path(Some(&serde_json::json!({"path":"///"}))), None);
        assert_eq!(file_change_summary("externalEdit"), "External file change");
        assert_eq!(file_change_summary("notebookEditGroup"), "Notebook changed");
        assert_eq!(file_change_summary("textEditGroup"), "File changed");
        assert_eq!(
            valid_epoch_ms(&serde_json::json!(MAX_SAFE_INTEGER + 1)),
            None
        );
        assert_eq!(epoch_ms_to_rfc3339(MAX_SAFE_INTEGER), None);

        assert!(matches!(
            tool_entry(&serde_json::json!({
                "isComplete": true,
                "generatedTitle": "Generated",
                "pastTenseMessage": "",
                "resultDetails": {
                    "output": [
                        {"type":"embed","value":"visible","isText":true},
                        {"type":"embed","value":"hidden","isText":false}
                    ]
                }
            })),
            PendingEntryKind::Tool {
                name,
                status: NormalizedToolStatusV2::Succeeded,
                summary,
                detail: NormalizedToolDetailV2::Available { arguments: None, result: Some(result) },
            } if name == "Generated" && summary == "Tool completed" && result == "visible"
        ));
        assert!(matches!(
            tool_entry(&serde_json::json!({"isComplete":false,"invocationMessage":""})),
            PendingEntryKind::Tool {
                name,
                status: NormalizedToolStatusV2::Running,
                summary,
                detail: NormalizedToolDetailV2::Unavailable,
            } if name == "Tool" && summary == "Tool running"
        ));
    }

    #[test]
    fn handles_empty_hidden_and_incomplete_response_content_explicitly() {
        let empty = serde_json::json!({
            "version": 3,
            "creationDate": 1,
            "sessionId": "empty",
            "requests": []
        });
        assert_eq!(
            run_rich_adapter(
                &VscodeCopilotAdapter,
                &value_bundle(empty, "empty.json", false),
                &context("empty")
            ),
            Err(AdapterError::EmptySession)
        );

        for request_id in ["", &"x".repeat(257)] {
            let invalid = serde_json::json!({
                "version": 3,
                "creationDate": 1,
                "sessionId": "invalid-request",
                "requests": [{"requestId":request_id,"message":"Prompt"}]
            });
            assert!(matches!(
                run_rich_adapter(
                    &VscodeCopilotAdapter,
                    &value_bundle(invalid, "invalid-request.json", false),
                    &context("invalid-request")
                ),
                Err(AdapterError::MalformedRecord {
                    field: "request-id",
                    ..
                })
            ));
        }

        let incomplete = serde_json::json!({
            "version": 3,
            "creationDate": 1,
            "sessionId": "incomplete",
            "requests": [{
                "requestId":"request",
                "message":"Prompt",
                "response":[
                    {"kind":"thinking"},
                    {"kind":"warning"},
                    {"kind":"toolInvocationSerialized","toolId":"only-result","isComplete":true,"resultDetails":{"output":"result"}}
                ]
            }]
        });
        let session = run_rich_adapter(
            &VscodeCopilotAdapter,
            &value_bundle(incomplete, "incomplete.json", false),
            &context("incomplete"),
        )
        .expect("incomplete optional content should be omitted safely");
        assert_eq!(session.entries.len(), 2);
        assert!(matches!(
            &session.entries[1],
            NormalizedEntryV2::ToolCall {
                detail: NormalizedToolDetailV2::Available { arguments: None, result: Some(result) },
                ..
            } if result == "result"
        ));
    }
}
