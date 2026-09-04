use std::collections::HashMap;

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::adapters::contract::AdapterError;
use crate::model::{
    DiagnosticSeverity, NormalizedContentAvailabilityV2, NormalizedContentAvailabilityValueV2,
    NormalizedEntryV2, NormalizedReasoningAvailabilityV2, NormalizedSessionV2,
    NormalizedToolDetailV2, SourceDiagnostic,
};

const MAX_SOURCE_DURATION_MS: u64 = 7 * 24 * 60 * 60 * 1_000;
const MAX_DIAGNOSTIC_COUNT: usize = 1_000;
const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

pub(crate) fn finalize_rich_session(session: &mut NormalizedSessionV2) -> Result<(), AdapterError> {
    session.unknown_record_count = session
        .entries
        .iter()
        .filter(|entry| matches!(entry, NormalizedEntryV2::Unknown { .. }))
        .count() as u64;
    let has_reasoning = session
        .entries
        .iter()
        .any(|entry| matches!(entry, NormalizedEntryV2::Reasoning { .. }));
    let mut available = false;
    let mut unavailable = false;
    for detail in session.entries.iter().filter_map(|entry| match entry {
        NormalizedEntryV2::ToolCall { detail, .. } => Some(detail),
        _ => None,
    }) {
        match detail {
            NormalizedToolDetailV2::Available { .. } => available = true,
            NormalizedToolDetailV2::Unavailable => unavailable = true,
        }
    }
    session.content_availability = NormalizedContentAvailabilityV2 {
        reasoning: if has_reasoning {
            NormalizedReasoningAvailabilityV2::Available
        } else {
            NormalizedReasoningAvailabilityV2::Unavailable
        },
        tool_details: match (available, unavailable) {
            (true, true) => NormalizedContentAvailabilityValueV2::Partial,
            (true, false) => NormalizedContentAvailabilityValueV2::Available,
            _ => NormalizedContentAvailabilityValueV2::Unavailable,
        },
    };
    session.validate().map_err(|_| AdapterError::InvalidOutput {
        reason: crate::adapters::contract::OutputReason::SessionInvariant,
    })
}

/// A raw event with a vendor-assigned ordinal and RFC 3339 timestamp,
/// before normalization.
#[derive(Debug, Clone)]
pub struct RawTimedRecord {
    /// Vendor ordinal, if present and parseable.
    pub vendor_ordinal: Option<u64>,
    /// RFC 3339 timestamp string from the source record (may have offset).
    pub timestamp_rfc3339: Option<String>,
    /// Structural position: (bundle member index, record index within member).
    pub structural_position: (usize, usize),
    /// The event kind string for deterministic ID generation.
    pub kind: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawTimestamp {
    Absent,
    Valid(u64),
    Invalid,
}

pub fn raw_timestamp(value: Option<&str>) -> RawTimestamp {
    match value {
        Some(value) => parse_rfc3339_to_epoch_ms(value)
            .map(RawTimestamp::Valid)
            .unwrap_or(RawTimestamp::Invalid),
        None => RawTimestamp::Absent,
    }
}

/// Determines structural order: vendor ordinals if every record has a valid
/// unique ordinal; otherwise falls back to (member_index, record_index).
pub fn structural_order(records: &mut [RawTimedRecord], diagnostics: &mut Vec<SourceDiagnostic>) {
    let all_have_ordinals =
        !records.is_empty() && records.iter().all(|record| record.vendor_ordinal.is_some());

    if all_have_ordinals {
        let mut seen = std::collections::HashSet::new();
        let all_unique = records
            .iter()
            .all(|record| seen.insert(record.vendor_ordinal.expect("checked above")));

        if all_unique {
            records.sort_by_key(|record| record.vendor_ordinal.expect("checked above"));
            return;
        }
    }

    records.sort_by_key(|record| record.structural_position);
    if !records.is_empty() {
        push_diagnostic(
            diagnostics,
            SourceDiagnostic {
                code: "ordering-fallback".to_owned(),
                severity: DiagnosticSeverity::Warning,
                message:
                    "vendor ordinals are missing or non-unique; using structural position order"
                        .to_owned(),
            },
        );
    }
}

/// Parse an RFC 3339 timestamp to UTC epoch milliseconds.
pub fn parse_rfc3339_to_epoch_ms(input: &str) -> Option<u64> {
    let parsed = OffsetDateTime::parse(input, &Rfc3339).ok()?;
    let unix_ms = parsed.unix_timestamp_nanos().checked_div(1_000_000)?;
    u64::try_from(unix_ms).ok()
}

/// Normalize timestamps to session-relative monotonic milliseconds.
///
/// - The first valid timestamp is the session origin (epoch).
/// - Events before the origin or with missing timestamps before the first
///   valid one get `at_ms = 0`.
/// - Later missing timestamps inherit the previous value.
/// - Decreasing deltas clamp to the previous value and emit a diagnostic.
pub fn normalize_timestamps(
    timestamps: &[RawTimestamp],
    diagnostics: &mut Vec<SourceDiagnostic>,
) -> Vec<u64> {
    let mut result = Vec::with_capacity(timestamps.len());
    let mut origin_ms = None;
    let mut seen_valid = false;
    let mut previous = 0_u64;
    let mut diagnosed_missing = false;
    let mut diagnosed_invalid = false;
    let mut diagnosed_pre_origin = false;
    let mut diagnosed_decrease = false;

    for (index, timestamp) in timestamps.iter().enumerate() {
        let current = match timestamp {
            RawTimestamp::Absent if !seen_valid => 0,
            RawTimestamp::Absent => {
                if !diagnosed_missing {
                    push_diagnostic(
                        diagnostics,
                        SourceDiagnostic {
                            code: "missing-timestamp".to_owned(),
                            severity: DiagnosticSeverity::Info,
                            message: format!(
                                "event at structural index {index} has no timestamp; inheriting {previous}ms"
                            ),
                        },
                    );
                    diagnosed_missing = true;
                }
                previous
            }
            RawTimestamp::Invalid => {
                if !diagnosed_invalid {
                    push_diagnostic(
                        diagnostics,
                        SourceDiagnostic {
                            code: "invalid-timestamp".to_owned(),
                            severity: DiagnosticSeverity::Warning,
                            message: format!(
                                "event at structural index {index} has an invalid timestamp; using {previous}ms"
                            ),
                        },
                    );
                    diagnosed_invalid = true;
                }
                previous
            }
            RawTimestamp::Valid(epoch_ms) => {
                let origin = *origin_ms.get_or_insert(*epoch_ms);
                seen_valid = true;
                if *epoch_ms < origin {
                    if !diagnosed_pre_origin {
                        push_diagnostic(
                            diagnostics,
                            SourceDiagnostic {
                                code: "pre-origin-timestamp".to_owned(),
                                severity: DiagnosticSeverity::Warning,
                                message: format!(
                                    "event at structural index {index} precedes the origin; using 0ms"
                                ),
                            },
                        );
                        diagnosed_pre_origin = true;
                    }
                    0
                } else {
                    epoch_ms - origin
                }
            }
        };

        let clamped = if current < previous {
            if !diagnosed_decrease {
                push_diagnostic(
                    diagnostics,
                    SourceDiagnostic {
                        code: "decreasing-timestamp".to_owned(),
                        severity: DiagnosticSeverity::Warning,
                        message: format!(
                            "event at structural index {index} has decreasing timestamp {current}ms; clamped to {previous}ms"
                        ),
                    },
                );
                diagnosed_decrease = true;
            }
            previous
        } else {
            current
        };

        result.push(clamped);
        previous = clamped;
    }

    if !seen_valid && !timestamps.is_empty() {
        push_diagnostic(
            diagnostics,
            SourceDiagnostic {
                code: "no-valid-timestamps".to_owned(),
                severity: DiagnosticSeverity::Warning,
                message: "no valid timestamps found; all events placed at 0ms".to_owned(),
            },
        );
    }

    result
}

pub(crate) fn clamp_long_running_session_timestamps(
    timestamps: &mut [u64],
    terminal_hold_ms: u64,
    diagnostics: &mut Vec<SourceDiagnostic>,
) {
    let maximum_event_time = MAX_SOURCE_DURATION_MS.saturating_sub(terminal_hold_ms);
    if !timestamps
        .iter()
        .any(|timestamp| *timestamp > maximum_event_time)
    {
        return;
    }
    for timestamp in timestamps {
        *timestamp = (*timestamp).min(maximum_event_time);
    }
    push_diagnostic(
        diagnostics,
        SourceDiagnostic {
            code: "timeline-span-clamped".to_owned(),
            severity: DiagnosticSeverity::Warning,
            message: "session timestamps span more than seven days; later entries use the bounded timeline limit"
                .to_owned(),
        },
    );
}

fn push_diagnostic(diagnostics: &mut Vec<SourceDiagnostic>, diagnostic: SourceDiagnostic) {
    if diagnostics.len() < MAX_DIAGNOSTIC_COUNT {
        diagnostics.push(diagnostic);
    }
}

fn generate_event_id(
    session_hash: u64,
    member_ordinal: usize,
    record_ordinal: usize,
    kind: &str,
    identity_hash: u64,
    collision_index: u32,
) -> String {
    let kind_hash = stable_hash(kind.as_bytes());
    let mut id = format!(
        "e-{session_hash:016x}-{member_ordinal:016x}-{record_ordinal:016x}-{kind_hash:016x}-{identity_hash:016x}"
    );
    if collision_index > 0 {
        id.push('-');
        id.push_str(&collision_index.to_string());
    }
    id
}

#[derive(Debug, Clone)]
pub struct EventIdGenerator {
    session_hash: u64,
    seen: HashMap<(usize, usize, u64, u64), u32>,
}

impl EventIdGenerator {
    pub fn new(session_id: &str) -> Self {
        Self {
            session_hash: stable_hash(session_id.as_bytes()),
            seen: HashMap::new(),
        }
    }

    pub fn next_id_for_content(
        &mut self,
        member_ordinal: usize,
        record_ordinal: usize,
        kind: &str,
        identity_parts: &[&str],
    ) -> String {
        let kind_hash = stable_hash(kind.as_bytes());
        let identity_hash = stable_hash_parts(identity_parts);
        let collision_index = self
            .seen
            .entry((member_ordinal, record_ordinal, kind_hash, identity_hash))
            .and_modify(|count| *count += 1)
            .or_insert(0);
        generate_event_id(
            self.session_hash,
            member_ordinal,
            record_ordinal,
            kind,
            identity_hash,
            *collision_index,
        )
    }

    pub fn next_id_for_vendor(&mut self, kind: &str, vendor_id: &str) -> String {
        let kind_hash = stable_hash(kind.as_bytes());
        let identity_hash = stable_hash_parts(&["vendor", vendor_id]);
        let collision_index = self
            .seen
            .entry((usize::MAX, usize::MAX, kind_hash, identity_hash))
            .and_modify(|count| *count += 1)
            .or_insert(0);
        generate_event_id(
            self.session_hash,
            usize::MAX,
            usize::MAX,
            kind,
            identity_hash,
            *collision_index,
        )
    }

    #[cfg(test)]
    pub fn next_id(&mut self, member_ordinal: usize, record_ordinal: usize, kind: &str) -> String {
        self.next_id_for_content(member_ordinal, record_ordinal, kind, &[])
    }
}

/// Compute `sourceDurationMs` as `max(lastEvent.atMs + terminalHoldMs, terminalHoldMs)`.
pub fn compute_duration(at_ms_values: &[u64], terminal_hold_ms: u64) -> Result<u64, AdapterError> {
    let last_at_ms = at_ms_values.last().copied().unwrap_or(0);
    let duration_ms = last_at_ms
        .checked_add(terminal_hold_ms)
        .ok_or(AdapterError::DurationOverflow)?
        .max(terminal_hold_ms);

    if duration_ms > MAX_SOURCE_DURATION_MS {
        return Err(AdapterError::DurationLimitExceeded { duration_ms });
    }

    Ok(duration_ms)
}

fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn stable_hash_parts(parts: &[&str]) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for part in parts {
        for byte in (part.len() as u64)
            .to_le_bytes()
            .iter()
            .chain(part.as_bytes())
        {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
    }
    hash
}
