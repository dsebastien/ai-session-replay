//! Transactional scan bookkeeping for the indexed-session repository.
//! SQLx rolls back an in-progress transaction when it is dropped, which keeps
//! stale or invalid scan completion from partially changing presence state.
//! Source: https://docs.rs/sqlx/0.8.6/sqlx/struct.Transaction.html

use std::collections::HashSet;

use sqlx::{Sqlite, Transaction};

use crate::indexed_library::IndexedSessionSourceV1;

use super::{
    IndexedSessionRepository, MAX_SAFE_INTEGER, PersistedSourceLocation, RepositoryError,
    source_name, to_i64, valid_opaque_id, valid_string,
};

const MAX_SCAN_COUNT: u64 = 100_000;
const MAX_SCAN_DIAGNOSTICS: usize = 64;
const MAX_DIAGNOSTIC_OCCURRENCES: u64 = 1_000;
const MAX_RETAINED_SCAN_RUNS: u64 = 100;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ScanCounts {
    pub(crate) discovered: u64,
    pub(crate) processed: u64,
    pub(crate) indexed: u64,
    pub(crate) unchanged: u64,
    pub(crate) failed: u64,
    pub(crate) skipped: u64,
    pub(crate) warnings: u64,
}

impl ScanCounts {
    fn is_valid(self) -> bool {
        self.discovered <= MAX_SCAN_COUNT
            && self.processed <= self.discovered
            && self.indexed <= self.processed
            && self.unchanged <= self.processed
            && self.failed <= self.processed
            && self.skipped <= self.processed
            && self.warnings <= MAX_SCAN_COUNT
            && self
                .indexed
                .checked_add(self.unchanged)
                .and_then(|value| value.checked_add(self.failed))
                .and_then(|value| value.checked_add(self.skipped))
                .is_some_and(|value| value <= self.processed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScannedCollection {
    source: IndexedSessionSourceV1,
    collection: String,
}

impl ScannedCollection {
    pub(crate) fn new(source: IndexedSessionSourceV1, collection: impl Into<String>) -> Self {
        Self {
            source,
            collection: collection.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScanDiagnostic {
    session_id: Option<String>,
    source: IndexedSessionSourceV1,
    code: String,
    occurrence_count: u64,
}

impl ScanDiagnostic {
    pub(crate) fn new(
        session_id: Option<String>,
        source: IndexedSessionSourceV1,
        code: impl Into<String>,
        occurrence_count: u64,
    ) -> Self {
        Self {
            session_id,
            source,
            code: code.into(),
            occurrence_count,
        }
    }

    fn is_valid(&self) -> bool {
        self.session_id.as_deref().is_none_or(valid_opaque_id)
            && valid_safe_code(&self.code)
            && (1..=MAX_DIAGNOSTIC_OCCURRENCES).contains(&self.occurrence_count)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScanCompletionOutcome {
    Applied,
    Stale,
}

impl IndexedSessionRepository {
    pub(crate) async fn begin_scan(&self, started_at_ms: u64) -> Result<u64, RepositoryError> {
        if started_at_ms > MAX_SAFE_INTEGER {
            return Err(RepositoryError::InvalidInput);
        }
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
        let previous: Option<i64> = sqlx::query_scalar("SELECT MAX(generation) FROM scan_runs")
            .fetch_one(&mut *transaction)
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
        let generation = previous
            .unwrap_or(0)
            .checked_add(1)
            .and_then(|value| u64::try_from(value).ok())
            .filter(|value| *value <= MAX_SAFE_INTEGER)
            .ok_or(RepositoryError::InvalidInput)?;

        sqlx::query(
            "UPDATE scan_runs
             SET status = 'failed',
                 completed_at_ms = max(started_at_ms, ?),
                 error_code = 'INDEX_SCAN_INTERRUPTED'
             WHERE status = 'running'",
        )
        .bind(to_i64(started_at_ms))
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepositoryError::WriteFailed)?;
        sqlx::query(
            "INSERT INTO scan_runs (
                generation, status, started_at_ms, completed_at_ms,
                discovered_count, processed_count, indexed_count,
                unchanged_count, failed_count, error_code
             ) VALUES (?, 'running', ?, NULL, 0, 0, 0, 0, 0, NULL)",
        )
        .bind(to_i64(generation))
        .bind(to_i64(started_at_ms))
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepositoryError::WriteFailed)?;

        if generation > MAX_RETAINED_SCAN_RUNS {
            sqlx::query("DELETE FROM scan_runs WHERE generation <= ?")
                .bind(to_i64(generation - MAX_RETAINED_SCAN_RUNS))
                .execute(&mut *transaction)
                .await
                .map_err(|_| RepositoryError::WriteFailed)?;
        }
        transaction
            .commit()
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
        Ok(generation)
    }

    pub(crate) async fn update_scan_progress(
        &self,
        generation: u64,
        counts: ScanCounts,
    ) -> Result<ScanCompletionOutcome, RepositoryError> {
        validate_generation_and_counts(generation, counts)?;
        let result = sqlx::query(
            "UPDATE scan_runs SET
                discovered_count = ?, processed_count = ?, indexed_count = ?,
                unchanged_count = ?, failed_count = ?
             WHERE generation = ? AND status = 'running'
               AND generation = (SELECT MAX(generation) FROM scan_runs)",
        )
        .bind(to_i64(counts.discovered))
        .bind(to_i64(counts.processed))
        .bind(to_i64(counts.indexed))
        .bind(to_i64(counts.unchanged))
        .bind(to_i64(counts.failed))
        .bind(to_i64(generation))
        .execute(&self.pool)
        .await
        .map_err(|_| RepositoryError::WriteFailed)?;
        Ok(if result.rows_affected() == 1 {
            ScanCompletionOutcome::Applied
        } else {
            ScanCompletionOutcome::Stale
        })
    }

    pub(crate) async fn complete_scan(
        &self,
        generation: u64,
        completed_at_ms: u64,
        counts: ScanCounts,
        scanned_collections: &[ScannedCollection],
        diagnostics: &[ScanDiagnostic],
    ) -> Result<ScanCompletionOutcome, RepositoryError> {
        validate_generation_and_counts(generation, counts)?;
        if completed_at_ms > MAX_SAFE_INTEGER
            || scanned_collections.len() > 64
            || diagnostics.len() > MAX_SCAN_DIAGNOSTICS
            || scanned_collections
                .iter()
                .any(|scope| !valid_string(&scope.collection, 1, 512))
            || diagnostics.iter().any(|diagnostic| !diagnostic.is_valid())
        {
            return Err(RepositoryError::InvalidInput);
        }
        let mut unique_collections = HashSet::with_capacity(scanned_collections.len());
        if scanned_collections.iter().any(|scope| {
            !unique_collections.insert((source_name(&scope.source), scope.collection.as_str()))
        }) {
            return Err(RepositoryError::InvalidInput);
        }

        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
        if !is_current_running_scan(&mut transaction, generation).await? {
            return Ok(ScanCompletionOutcome::Stale);
        }
        let started_at_ms: i64 =
            sqlx::query_scalar("SELECT started_at_ms FROM scan_runs WHERE generation = ?")
                .bind(to_i64(generation))
                .fetch_one(&mut *transaction)
                .await
                .map_err(|_| RepositoryError::WriteFailed)?;
        if completed_at_ms < started_at_ms as u64 {
            return Err(RepositoryError::InvalidInput);
        }

        for diagnostic in diagnostics {
            sqlx::query(
                "INSERT INTO source_diagnostics (
                    scan_generation, session_id, source, code,
                    occurrence_count, recorded_at_ms
                 ) VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(to_i64(generation))
            .bind(&diagnostic.session_id)
            .bind(source_name(&diagnostic.source))
            .bind(&diagnostic.code)
            .bind(to_i64(diagnostic.occurrence_count))
            .bind(to_i64(completed_at_ms))
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
        }
        for scope in scanned_collections {
            sqlx::query(
                "UPDATE sessions
                 SET source_present = 0
                 WHERE source = ? AND collection = ?
                   AND EXISTS (
                       SELECT 1 FROM source_locations
                       WHERE source_locations.session_id = sessions.id
                         AND (source_locations.last_seen_generation IS NULL
                              OR source_locations.last_seen_generation <> ?)
                   )",
            )
            .bind(source_name(&scope.source))
            .bind(&scope.collection)
            .bind(to_i64(generation))
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
        }
        update_scan_terminal(
            &mut transaction,
            generation,
            completed_at_ms,
            counts,
            "completed",
            None,
        )
        .await?;
        transaction
            .commit()
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
        Ok(ScanCompletionOutcome::Applied)
    }

    pub(crate) async fn fail_scan(
        &self,
        generation: u64,
        completed_at_ms: u64,
        error_code: &str,
        counts: ScanCounts,
    ) -> Result<ScanCompletionOutcome, RepositoryError> {
        validate_generation_and_counts(generation, counts)?;
        if completed_at_ms > MAX_SAFE_INTEGER || !valid_safe_code(error_code) {
            return Err(RepositoryError::InvalidInput);
        }
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
        if !is_current_running_scan(&mut transaction, generation).await? {
            return Ok(ScanCompletionOutcome::Stale);
        }
        let started_at_ms: i64 =
            sqlx::query_scalar("SELECT started_at_ms FROM scan_runs WHERE generation = ?")
                .bind(to_i64(generation))
                .fetch_one(&mut *transaction)
                .await
                .map_err(|_| RepositoryError::WriteFailed)?;
        if completed_at_ms < started_at_ms as u64 {
            return Err(RepositoryError::InvalidInput);
        }
        update_scan_terminal(
            &mut transaction,
            generation,
            completed_at_ms,
            counts,
            "failed",
            Some(error_code),
        )
        .await?;
        transaction
            .commit()
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
        Ok(ScanCompletionOutcome::Applied)
    }

    pub(crate) async fn observe_existing_source(
        &self,
        generation: u64,
        source: &IndexedSessionSourceV1,
        location: &PersistedSourceLocation,
        observed_at_ms: u64,
    ) -> Result<Option<String>, RepositoryError> {
        if generation == 0
            || generation > MAX_SAFE_INTEGER
            || observed_at_ms > MAX_SAFE_INTEGER
            || !location.is_valid()
        {
            return Err(RepositoryError::InvalidInput);
        }
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
        if !is_current_running_scan(&mut transaction, generation).await? {
            return Err(RepositoryError::ScanStateConflict);
        }
        let session_id: Option<String> = sqlx::query_scalar(
            "SELECT source_locations.session_id
             FROM source_locations
             JOIN sessions ON sessions.id = source_locations.session_id
             WHERE sessions.source = ?
               AND (source_locations.source_identity_hash = ?
                    OR source_locations.canonical_path_utf16le = ?)
             ORDER BY CASE WHEN source_locations.source_identity_hash = ? THEN 0 ELSE 1 END
             LIMIT 1",
        )
        .bind(source_name(source))
        .bind(location.source_identity_hash.as_slice())
        .bind(&location.canonical_path_utf16le)
        .bind(location.source_identity_hash.as_slice())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepositoryError::WriteFailed)?;
        if let Some(session_id) = &session_id {
            sqlx::query(
                "UPDATE source_locations SET last_seen_generation = ? WHERE session_id = ?",
            )
            .bind(to_i64(generation))
            .bind(session_id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
            sqlx::query(
                "UPDATE sessions
                 SET source_present = 1, last_seen_at_ms = max(last_seen_at_ms, ?)
                 WHERE id = ?",
            )
            .bind(to_i64(observed_at_ms))
            .bind(session_id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
        }
        transaction
            .commit()
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
        Ok(session_id)
    }

    pub(crate) async fn is_source_suppressed(
        &self,
        source: &IndexedSessionSourceV1,
        source_identity_hash: &[u8; 32],
        vendor_session_id: Option<&str>,
    ) -> Result<bool, RepositoryError> {
        if vendor_session_id.is_some_and(|value| !valid_string(value, 1, 1_024)) {
            return Err(RepositoryError::InvalidInput);
        }
        sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM suppressed_sources
                WHERE source = ?
                  AND (source_identity_hash = ?
                       OR (? IS NOT NULL AND vendor_session_id = ?))
             )",
        )
        .bind(source_name(source))
        .bind(source_identity_hash.as_slice())
        .bind(vendor_session_id)
        .bind(vendor_session_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|_| RepositoryError::WriteFailed)
    }

    pub(crate) async fn find_session_id(
        &self,
        source: &IndexedSessionSourceV1,
        vendor_session_id: &str,
    ) -> Result<Option<String>, RepositoryError> {
        if !valid_string(vendor_session_id, 1, 1_024) {
            return Err(RepositoryError::InvalidInput);
        }
        sqlx::query_scalar("SELECT id FROM sessions WHERE source = ? AND vendor_session_id = ?")
            .bind(source_name(source))
            .bind(vendor_session_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| RepositoryError::WriteFailed)
    }
}

pub(super) async fn is_current_running_scan(
    transaction: &mut Transaction<'_, Sqlite>,
    generation: u64,
) -> Result<bool, RepositoryError> {
    let current: Option<String> = sqlx::query_scalar(
        "SELECT status FROM scan_runs
         WHERE generation = ? AND generation = (SELECT MAX(generation) FROM scan_runs)",
    )
    .bind(to_i64(generation))
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| RepositoryError::WriteFailed)?;
    Ok(current.as_deref() == Some("running"))
}

async fn update_scan_terminal(
    transaction: &mut Transaction<'_, Sqlite>,
    generation: u64,
    completed_at_ms: u64,
    counts: ScanCounts,
    status: &str,
    error_code: Option<&str>,
) -> Result<(), RepositoryError> {
    let result = sqlx::query(
        "UPDATE scan_runs SET
            status = ?, completed_at_ms = ?, discovered_count = ?,
            processed_count = ?, indexed_count = ?, unchanged_count = ?,
            failed_count = ?, error_code = ?
         WHERE generation = ? AND status = 'running'
           AND generation = (SELECT MAX(generation) FROM scan_runs)",
    )
    .bind(status)
    .bind(to_i64(completed_at_ms))
    .bind(to_i64(counts.discovered))
    .bind(to_i64(counts.processed))
    .bind(to_i64(counts.indexed))
    .bind(to_i64(counts.unchanged))
    .bind(to_i64(counts.failed))
    .bind(error_code)
    .bind(to_i64(generation))
    .execute(&mut **transaction)
    .await
    .map_err(|_| RepositoryError::WriteFailed)?;
    if result.rows_affected() != 1 {
        return Err(RepositoryError::ScanStateConflict);
    }
    Ok(())
}

fn validate_generation_and_counts(
    generation: u64,
    counts: ScanCounts,
) -> Result<(), RepositoryError> {
    if generation == 0 || generation > MAX_SAFE_INTEGER || !counts.is_valid() {
        return Err(RepositoryError::InvalidInput);
    }
    Ok(())
}

fn valid_safe_code(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value.as_bytes()[0].is_ascii_uppercase()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}
