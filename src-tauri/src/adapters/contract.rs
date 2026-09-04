use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;

use crate::io::SessionSnapshotBundle;
use crate::model::{
    NormalizedEntryV2, NormalizedSessionV1, NormalizedSessionV2, ReplayEvent, SessionSource,
    migrate_normalized_session_v1,
};

/// Non-filesystem context provided to adapters alongside the snapshot bundle.
#[derive(Debug, Clone)]
pub struct AdapterContext {
    /// Vendor-assigned session identifier (never a path).
    session_id: String,
    /// Which vendor produced this session.
    source: SessionSource,
    /// Terminal hold added after the last event, in milliseconds.
    terminal_hold_ms: u64,
}

impl AdapterContext {
    pub fn new(
        session_id: String,
        source: SessionSource,
        terminal_hold_ms: u64,
    ) -> Result<Self, AdapterError> {
        let session_id_length = session_id.chars().count();
        if session_id_length == 0 || session_id_length > 256 || session_id.contains('\0') {
            return Err(AdapterError::InvalidContext {
                reason: ContextReason::SessionId,
            });
        }
        if !(250..=10_000).contains(&terminal_hold_ms) {
            return Err(AdapterError::InvalidContext {
                reason: ContextReason::TerminalHold,
            });
        }
        Ok(Self {
            session_id,
            source,
            terminal_hold_ms,
        })
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn source(&self) -> &SessionSource {
        &self.source
    }

    pub fn terminal_hold_ms(&self) -> u64 {
        self.terminal_hold_ms
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountingSummary {
    pub unknown_count: u64,
    pub unknown_types: Vec<String>,
}

#[derive(Debug)]
pub struct RecordAccountant {
    members: RefCell<Vec<Vec<Option<RecordClassification>>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
enum RecordClassification {
    Understood,
    Unknown { source_type: String },
}

#[allow(dead_code)]
impl RecordAccountant {
    pub(crate) fn new(member_record_counts: impl IntoIterator<Item = (usize, usize)>) -> Self {
        let mut members = Vec::new();
        for (member_ordinal, record_count) in member_record_counts {
            if members.len() <= member_ordinal {
                members.resize_with(member_ordinal + 1, Vec::new);
            }
            members[member_ordinal] = vec![None; record_count];
        }
        Self {
            members: RefCell::new(members),
        }
    }

    #[cfg(test)]
    pub(super) fn for_test(member_record_counts: impl IntoIterator<Item = (usize, usize)>) -> Self {
        Self::new(member_record_counts)
    }

    pub(crate) fn classify_understood(
        &self,
        member_ordinal: usize,
        record_ordinal: usize,
    ) -> Result<(), AdapterError> {
        self.classify(
            member_ordinal,
            record_ordinal,
            RecordClassification::Understood,
        )
    }

    pub(crate) fn classify_unknown(
        &self,
        member_ordinal: usize,
        record_ordinal: usize,
        source_type: &str,
    ) -> Result<(), AdapterError> {
        let source_type_length = source_type.chars().count();
        if source_type_length == 0 || source_type_length > 256 || source_type.contains('\0') {
            return Err(AdapterError::MalformedRecord {
                member_ordinal,
                record_ordinal,
                field: "source-type",
            });
        }
        self.classify(
            member_ordinal,
            record_ordinal,
            RecordClassification::Unknown {
                source_type: source_type.to_owned(),
            },
        )
    }

    pub(crate) fn promote_unknown_to_understood(
        &self,
        member_ordinal: usize,
        record_ordinal: usize,
    ) -> Result<(), AdapterError> {
        let mut members = self.members.borrow_mut();
        let records = members
            .get_mut(member_ordinal)
            .ok_or(AdapterError::StructuralViolation {
                reason: StructuralReason::MemberOrdinalOutOfRange { member_ordinal },
            })?;
        let slot = records
            .get_mut(record_ordinal)
            .ok_or(AdapterError::StructuralViolation {
                reason: StructuralReason::RecordOrdinalOutOfRange {
                    member_ordinal,
                    record_ordinal,
                },
            })?;
        match slot {
            Some(RecordClassification::Unknown { .. }) => {
                *slot = Some(RecordClassification::Understood);
                Ok(())
            }
            _ => Err(AdapterError::DuplicateClassification {
                member_ordinal,
                record_ordinal,
            }),
        }
    }

    #[cfg(test)]
    pub(super) fn finish_for_test(self) -> Result<AccountingSummary, AdapterError> {
        self.finish()
    }

    fn finish(self) -> Result<AccountingSummary, AdapterError> {
        let members = self.members.into_inner();
        let mut unknown_count = 0_u64;
        let mut unknown_types = Vec::new();

        for (member_ordinal, records) in members.into_iter().enumerate() {
            for (record_ordinal, classification) in records.into_iter().enumerate() {
                match classification {
                    Some(RecordClassification::Understood) => {}
                    Some(RecordClassification::Unknown { source_type }) => {
                        unknown_count += 1;
                        unknown_types.push(source_type);
                    }
                    None => {
                        return Err(AdapterError::MissingClassification {
                            member_ordinal,
                            record_ordinal,
                        });
                    }
                }
            }
        }

        Ok(AccountingSummary {
            unknown_count,
            unknown_types,
        })
    }

    fn classify(
        &self,
        member_ordinal: usize,
        record_ordinal: usize,
        classification: RecordClassification,
    ) -> Result<(), AdapterError> {
        let mut members = self.members.borrow_mut();
        let records = members
            .get_mut(member_ordinal)
            .ok_or(AdapterError::StructuralViolation {
                reason: StructuralReason::MemberOrdinalOutOfRange { member_ordinal },
            })?;
        let slot = records
            .get_mut(record_ordinal)
            .ok_or(AdapterError::StructuralViolation {
                reason: StructuralReason::RecordOrdinalOutOfRange {
                    member_ordinal,
                    record_ordinal,
                },
            })?;

        if slot.is_some() {
            return Err(AdapterError::DuplicateClassification {
                member_ordinal,
                record_ordinal,
            });
        }

        *slot = Some(classification);
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructuralReason {
    EmptyBundle,
    MemberCountExceeded {
        count: usize,
    },
    MemberOrdinalOutOfRange {
        member_ordinal: usize,
    },
    RecordOrdinalOutOfRange {
        member_ordinal: usize,
        record_ordinal: usize,
    },
}

impl StructuralReason {
    fn code(&self) -> &'static str {
        match self {
            Self::EmptyBundle => "empty-bundle",
            Self::MemberCountExceeded { .. } => "member-count-exceeded",
            Self::MemberOrdinalOutOfRange { .. } => "member-ordinal-out-of-range",
            Self::RecordOrdinalOutOfRange { .. } => "record-ordinal-out-of-range",
        }
    }
}

/// Typed, non-sensitive error returned by adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterError {
    /// Adapter invocation context is outside the normalized-session contract.
    InvalidContext { reason: ContextReason },
    /// The bundle contains no complete records.
    EmptySession,
    /// Event count exceeds the 100,000-event boundary.
    EventLimitExceeded { count: usize },
    /// A source record was never classified as understood or unknown.
    MissingClassification {
        member_ordinal: usize,
        record_ordinal: usize,
    },
    /// A source record was classified more than once.
    DuplicateClassification {
        member_ordinal: usize,
        record_ordinal: usize,
    },
    /// Unknown record metadata does not match the accounting summary.
    UnknownRecordCountMismatch { expected: u64, actual: u64 },
    /// Unknown record source types do not match the accounting summary.
    UnknownRecordTypesMismatch,
    /// Duration arithmetic overflowed.
    DurationOverflow,
    /// Duration exceeds the supported seven-day ceiling.
    DurationLimitExceeded { duration_ms: u64 },
    /// A required field is missing or invalid in a source record.
    MalformedRecord {
        member_ordinal: usize,
        record_ordinal: usize,
        field: &'static str,
    },
    /// A structural invariant was violated.
    StructuralViolation { reason: StructuralReason },
    /// An adapter returned data outside the normalized-session contract.
    InvalidOutput { reason: OutputReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextReason {
    SessionId,
    TerminalHold,
}

impl ContextReason {
    fn code(self) -> &'static str {
        match self {
            Self::SessionId => "session-id",
            Self::TerminalHold => "terminal-hold",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputReason {
    SchemaVersion,
    SessionId,
    SessionSource,
    SessionInvariant,
}

impl OutputReason {
    fn code(self) -> &'static str {
        match self {
            Self::SchemaVersion => "schema-version",
            Self::SessionId => "session-id",
            Self::SessionSource => "session-source",
            Self::SessionInvariant => "session-invariant",
        }
    }
}

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidContext { reason } => {
                write!(f, "adapter context is invalid: {}", reason.code())
            }
            Self::EmptySession => write!(f, "session contains no complete records"),
            Self::EventLimitExceeded { count } => {
                write!(f, "event count {count} exceeds the 100000-event limit")
            }
            Self::MissingClassification {
                member_ordinal,
                record_ordinal,
            } => write!(
                f,
                "record classification missing for member {member_ordinal} record {record_ordinal}"
            ),
            Self::DuplicateClassification {
                member_ordinal,
                record_ordinal,
            } => write!(
                f,
                "record classification duplicated for member {member_ordinal} record {record_ordinal}"
            ),
            Self::UnknownRecordCountMismatch { expected, actual } => write!(
                f,
                "unknown record count mismatch: expected {expected}, actual {actual}"
            ),
            Self::UnknownRecordTypesMismatch => {
                write!(f, "unknown record source types do not match accounting")
            }
            Self::DurationOverflow => write!(f, "session duration overflowed"),
            Self::DurationLimitExceeded { duration_ms } => write!(
                f,
                "session duration {duration_ms}ms exceeds the 604800000ms limit"
            ),
            Self::MalformedRecord {
                member_ordinal,
                record_ordinal,
                field,
            } => write!(
                f,
                "malformed record in member {member_ordinal} record {record_ordinal}: invalid {field}"
            ),
            Self::StructuralViolation { reason } => match reason {
                StructuralReason::EmptyBundle => {
                    write!(f, "structural violation: {}", reason.code())
                }
                StructuralReason::MemberCountExceeded { count } => {
                    write!(f, "structural violation: {} ({count})", reason.code())
                }
                StructuralReason::MemberOrdinalOutOfRange { member_ordinal } => write!(
                    f,
                    "structural violation: {} ({member_ordinal})",
                    reason.code()
                ),
                StructuralReason::RecordOrdinalOutOfRange {
                    member_ordinal,
                    record_ordinal,
                } => write!(
                    f,
                    "structural violation: {} ({member_ordinal}, {record_ordinal})",
                    reason.code()
                ),
            },
            Self::InvalidOutput { reason } => {
                write!(f, "adapter output is invalid: {}", reason.code())
            }
        }
    }
}

impl std::error::Error for AdapterError {}

/// Object-safe adapter trait. Implementations receive only an immutable
/// snapshot bundle (no filesystem capability, no path arguments).
pub trait SessionAdapter {
    /// Normalize a snapshot bundle into a `NormalizedSessionV1`.
    fn normalize(
        &self,
        bundle: &SessionSnapshotBundle,
        context: &AdapterContext,
        records: &RecordAccountant,
    ) -> Result<NormalizedSessionV1, AdapterError>;

    fn normalize_rich(
        &self,
        bundle: &SessionSnapshotBundle,
        context: &AdapterContext,
        records: &RecordAccountant,
    ) -> Result<NormalizedSessionV2, AdapterError> {
        let legacy = self.normalize(bundle, context, records)?;
        migrate_normalized_session_v1(legacy, context.terminal_hold_ms()).map_err(|_| {
            AdapterError::InvalidOutput {
                reason: OutputReason::SessionInvariant,
            }
        })
    }
}

pub fn run_adapter(
    adapter: &dyn SessionAdapter,
    bundle: &SessionSnapshotBundle,
    context: &AdapterContext,
) -> Result<NormalizedSessionV1, AdapterError> {
    if bundle.record_count() == 0 {
        return Err(AdapterError::EmptySession);
    }
    let member_record_counts = bundle
        .members()
        .enumerate()
        .map(|(member_ordinal, member)| (member_ordinal, member.records().len()));
    let records = RecordAccountant::new(member_record_counts);
    let session = adapter.normalize(bundle, context, &records)?;
    let accounting = records.finish()?;

    if session.schema_version != 1 {
        return Err(AdapterError::InvalidOutput {
            reason: OutputReason::SchemaVersion,
        });
    }
    if session.id != context.session_id() {
        return Err(AdapterError::InvalidOutput {
            reason: OutputReason::SessionId,
        });
    }
    if &session.source != context.source() {
        return Err(AdapterError::InvalidOutput {
            reason: OutputReason::SessionSource,
        });
    }
    if session.events.len() > 100_000 {
        return Err(AdapterError::EventLimitExceeded {
            count: session.events.len(),
        });
    }
    session
        .validate(context.terminal_hold_ms())
        .map_err(|_| AdapterError::InvalidOutput {
            reason: OutputReason::SessionInvariant,
        })?;

    if session.unknown_record_count != accounting.unknown_count {
        return Err(AdapterError::UnknownRecordCountMismatch {
            expected: accounting.unknown_count,
            actual: session.unknown_record_count,
        });
    }

    let expected_unknown_types = type_counts(accounting.unknown_types.iter().map(String::as_str));
    let actual_unknown_types = type_counts(session.events.iter().filter_map(|event| match event {
        ReplayEvent::Unknown { source_type, .. } => Some(source_type.as_str()),
        _ => None,
    }));
    if actual_unknown_types != expected_unknown_types {
        return Err(AdapterError::UnknownRecordTypesMismatch);
    }

    Ok(session)
}

pub fn run_rich_adapter(
    adapter: &dyn SessionAdapter,
    bundle: &SessionSnapshotBundle,
    context: &AdapterContext,
) -> Result<NormalizedSessionV2, AdapterError> {
    if bundle.record_count() == 0 {
        return Err(AdapterError::EmptySession);
    }
    let member_record_counts = bundle
        .members()
        .enumerate()
        .map(|(member_ordinal, member)| (member_ordinal, member.records().len()));
    let records = RecordAccountant::new(member_record_counts);
    let session = adapter.normalize_rich(bundle, context, &records)?;
    let accounting = records.finish()?;

    if session.schema_version != 2 {
        return Err(AdapterError::InvalidOutput {
            reason: OutputReason::SchemaVersion,
        });
    }
    if session.id != context.session_id() {
        return Err(AdapterError::InvalidOutput {
            reason: OutputReason::SessionId,
        });
    }
    if &session.source != context.source() {
        return Err(AdapterError::InvalidOutput {
            reason: OutputReason::SessionSource,
        });
    }
    if session.entries.len() > 100_000 {
        return Err(AdapterError::EventLimitExceeded {
            count: session.entries.len(),
        });
    }
    session
        .validate()
        .map_err(|_| AdapterError::InvalidOutput {
            reason: OutputReason::SessionInvariant,
        })?;
    let expected_duration = session
        .entries
        .last()
        .map_or(0, NormalizedEntryV2::at_ms)
        .checked_add(context.terminal_hold_ms())
        .ok_or(AdapterError::DurationOverflow)?;
    if session.duration_ms != expected_duration {
        return Err(AdapterError::InvalidOutput {
            reason: OutputReason::SessionInvariant,
        });
    }

    if session.unknown_record_count != accounting.unknown_count {
        return Err(AdapterError::UnknownRecordCountMismatch {
            expected: accounting.unknown_count,
            actual: session.unknown_record_count,
        });
    }
    let expected_unknown_types = type_counts(accounting.unknown_types.iter().map(String::as_str));
    let actual_unknown_types =
        type_counts(session.entries.iter().filter_map(|entry| match entry {
            NormalizedEntryV2::Unknown { source_type, .. } => Some(source_type.as_str()),
            _ => None,
        }));
    if actual_unknown_types != expected_unknown_types {
        return Err(AdapterError::UnknownRecordTypesMismatch);
    }

    Ok(session)
}

fn type_counts<'a>(values: impl IntoIterator<Item = &'a str>) -> HashMap<&'a str, u64> {
    let mut counts = HashMap::new();
    for value in values {
        *counts.entry(value).or_insert(0) += 1;
    }
    counts
}
