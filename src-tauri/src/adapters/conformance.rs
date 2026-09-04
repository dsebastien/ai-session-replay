use crate::adapters::contract::AdapterError;
use crate::adapters::normalize::compute_duration;
use crate::model::{NormalizedSessionV1, ReplayEvent};

#[cfg(test)]
use crate::adapters::contract::{AdapterContext, SessionAdapter, run_adapter};
#[cfg(test)]
use crate::io::SessionSnapshotBundle;

const MAX_SOURCE_DURATION_MS: u64 = 7 * 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConformanceViolation {
    EmptyEvents,
    EventLimitExceeded {
        count: usize,
    },
    NonMonotonicTimestamp {
        index: usize,
        at_ms: u64,
        previous_at_ms: u64,
    },
    DuplicateEventId {
        index: usize,
    },
    DurationMismatch {
        expected: u64,
        actual: u64,
    },
    DurationOverflow,
    DurationLimitExceeded {
        duration_ms: u64,
    },
}

/// Validate that a `NormalizedSessionV1` satisfies the adapter conformance
/// invariants.
pub fn check_conformance(
    session: &NormalizedSessionV1,
    terminal_hold_ms: u64,
) -> Result<(), Vec<ConformanceViolation>> {
    let mut violations = Vec::new();

    if session.events.is_empty() {
        violations.push(ConformanceViolation::EmptyEvents);
    }

    if session.events.len() > 100_000 {
        violations.push(ConformanceViolation::EventLimitExceeded {
            count: session.events.len(),
        });
    }

    let mut previous_at_ms = 0_u64;
    let mut seen_ids = std::collections::HashSet::new();
    let mut at_ms_values = Vec::with_capacity(session.events.len());
    for (index, event) in session.events.iter().enumerate() {
        let at_ms = event_at_ms(event);
        at_ms_values.push(at_ms);
        if index > 0 && at_ms < previous_at_ms {
            violations.push(ConformanceViolation::NonMonotonicTimestamp {
                index,
                at_ms,
                previous_at_ms,
            });
        }
        previous_at_ms = at_ms;

        if !seen_ids.insert(event_id(event)) {
            violations.push(ConformanceViolation::DuplicateEventId { index });
        }
    }

    match compute_duration(&at_ms_values, terminal_hold_ms) {
        Ok(expected) => {
            if session.duration_ms != expected {
                violations.push(ConformanceViolation::DurationMismatch {
                    expected,
                    actual: session.duration_ms,
                });
            }
        }
        Err(AdapterError::DurationOverflow) => {
            violations.push(ConformanceViolation::DurationOverflow)
        }
        Err(AdapterError::DurationLimitExceeded { duration_ms }) => {
            violations.push(ConformanceViolation::DurationLimitExceeded { duration_ms })
        }
        Err(_) => unreachable!("compute_duration only returns duration errors"),
    }

    if session.duration_ms > MAX_SOURCE_DURATION_MS {
        violations.push(ConformanceViolation::DurationLimitExceeded {
            duration_ms: session.duration_ms,
        });
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

fn event_at_ms(event: &ReplayEvent) -> u64 {
    match event {
        ReplayEvent::User { at_ms, .. }
        | ReplayEvent::Assistant { at_ms, .. }
        | ReplayEvent::Tool { at_ms, .. }
        | ReplayEvent::FileChange { at_ms, .. }
        | ReplayEvent::Unknown { at_ms, .. } => *at_ms,
    }
}

fn event_id(event: &ReplayEvent) -> &str {
    match event {
        ReplayEvent::User { id, .. }
        | ReplayEvent::Assistant { id, .. }
        | ReplayEvent::Tool { id, .. }
        | ReplayEvent::FileChange { id, .. }
        | ReplayEvent::Unknown { id, .. } => id,
    }
}

#[cfg(test)]
pub(crate) fn assert_conforming_fixture(
    adapter: &dyn SessionAdapter,
    bundle: &SessionSnapshotBundle,
    context: &AdapterContext,
    expected_event_count: usize,
    expected_unknown_count: u64,
) -> NormalizedSessionV1 {
    let session = run_adapter(adapter, bundle, context).expect("adapter fixture should normalize");
    check_conformance(&session, context.terminal_hold_ms())
        .expect("adapter fixture should conform to replay invariants");
    assert_eq!(session.events.len(), expected_event_count);
    assert_eq!(session.unknown_record_count, expected_unknown_count);
    session
}
