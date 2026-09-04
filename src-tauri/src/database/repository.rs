use std::collections::HashSet;

use sqlx::{QueryBuilder, Sqlite, SqlitePool};

use crate::indexed_library::{
    ContentAvailability, ContentAvailabilityV1, DEFAULT_PRESENTATION_DELAY_MS,
    IndexedSessionSourceV1, IndexedToolStatus, ReasoningAvailability,
};

mod curation;
mod deletion;
mod read;
mod reset;
mod scan;

pub(crate) use deletion::SourceDeletionRecord;

#[cfg_attr(
    not(test),
    allow(
        unused_imports,
        reason = "Task 23 coordinator consumes scan repository contracts in the next increment"
    )
)]
pub(crate) use scan::{ScanCompletionOutcome, ScanCounts, ScanDiagnostic, ScannedCollection};

const DEFAULT_BACKGROUND: &str = "#101211";
const DEFAULT_SURFACE: &str = "#171A18";
const DEFAULT_TEXT: &str = "#F5F5F4";
const DEFAULT_MUTED: &str = "#9B9E9C";
const DEFAULT_ACCENT: &str = "#D6AA68";
const DEFAULT_SUCCESS: &str = "#A8D5BD";
const DEFAULT_ERROR: &str = "#E59A91";
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const MAX_ENTRY_COUNT: usize = 100_000;
const MAX_DURATION_MS: u64 = 604_800_000;
const MAX_ENTRY_TEXT_LENGTH: usize = 2_000_000;
pub(super) const ENTRY_INSERT_BATCH_SIZE: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PersistedSession {
    pub(crate) id: String,
    pub(crate) source: IndexedSessionSourceV1,
    pub(crate) vendor_session_id: String,
    pub(crate) title: String,
    pub(crate) created_at_ms: Option<u64>,
    pub(crate) source_version: Option<String>,
    pub(crate) collection: Option<String>,
    pub(crate) display_filename: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PersistedRevision {
    pub(crate) id: String,
    pub(crate) content_hash: [u8; 32],
    pub(crate) indexed_at_ms: u64,
    pub(crate) source_created_at_ms: Option<u64>,
    pub(crate) source_updated_at_ms: Option<u64>,
    pub(crate) duration_ms: u64,
    pub(crate) diagnostic_count: u64,
    pub(crate) content_availability: ContentAvailabilityV1,
    pub(crate) entries: Vec<PersistedEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PersistedEntry {
    User {
        entry_key: String,
        ordinal: u64,
        at_ms: u64,
        text: String,
    },
    Assistant {
        entry_key: String,
        ordinal: u64,
        at_ms: u64,
        markdown: String,
    },
    #[allow(
        dead_code,
        reason = "the indexed contract supports reasoning before a v1 adapter emits it"
    )]
    Reasoning {
        entry_key: String,
        ordinal: u64,
        at_ms: u64,
        text: String,
    },
    ToolCall {
        entry_key: String,
        ordinal: u64,
        at_ms: u64,
        name: String,
        status: IndexedToolStatus,
        summary: String,
        detail: PersistedToolDetail,
    },
    FileChange {
        entry_key: String,
        ordinal: u64,
        at_ms: u64,
        display_path: String,
        summary: String,
    },
    Unknown {
        entry_key: String,
        ordinal: u64,
        at_ms: u64,
        source_type: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PersistedToolDetail {
    #[allow(
        dead_code,
        reason = "the indexed contract preserves tool detail when an adapter can supply it"
    )]
    Available {
        arguments: Option<String>,
        result: Option<String>,
    },
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PersistedSourceLocation {
    pub(crate) canonical_path_utf16le: Vec<u8>,
    pub(crate) source_identity_hash: [u8; 32],
    pub(crate) root_identity: Vec<u8>,
    pub(crate) file_identity: Vec<u8>,
    pub(crate) file_size_bytes: u64,
    pub(crate) modified_at_ms: u64,
    pub(crate) last_seen_generation: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PersistSessionRevision {
    pub(crate) session: PersistedSession,
    pub(crate) revision: PersistedRevision,
    pub(crate) source_location: PersistedSourceLocation,
    pub(crate) observed_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PersistRevisionOutcome {
    Inserted { revision_id: String },
    Unchanged { revision_id: String },
    Suppressed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RepositoryError {
    #[error("INVALID_REPOSITORY_INPUT")]
    InvalidInput,
    #[error("SESSION_IDENTITY_CONFLICT")]
    SessionIdentityConflict,
    #[error("REVISION_IDENTITY_CONFLICT")]
    RevisionIdentityConflict,
    #[error("SOURCE_IDENTITY_CONFLICT")]
    SourceIdentityConflict,
    #[error("SCAN_STATE_CONFLICT")]
    ScanStateConflict,
    #[error("REPOSITORY_WRITE_FAILED")]
    WriteFailed,
    #[error("SESSION_NOT_FOUND")]
    SessionNotFound,
    #[error("REPOSITORY_READ_FAILED")]
    ReadFailed,
    #[error("SOURCE_STATE_CHANGED")]
    SourceStateChanged,
    #[error("SUPPRESSION_NOT_FOUND")]
    SuppressionNotFound,
    #[error("STALE_REVISION")]
    StaleRevision,
}

#[derive(Clone)]
pub(crate) struct IndexedSessionRepository {
    pool: SqlitePool,
}

impl IndexedSessionRepository {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    #[cfg(test)]
    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    #[cfg(test)]
    pub(crate) async fn close(&self) {
        self.pool.close().await;
    }

    /// Persists one complete session observation. Every early return leaves the
    /// transaction uncommitted, so SQLx rolls it back when it is dropped.
    /// Source: https://docs.rs/sqlx/0.8.6/sqlx/struct.Transaction.html
    pub(crate) async fn persist_revision(
        &self,
        input: &PersistSessionRevision,
    ) -> Result<PersistRevisionOutcome, RepositoryError> {
        input.validate()?;

        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
        if let Some(generation) = input.source_location.last_seen_generation
            && !scan::is_current_running_scan(&mut transaction, generation).await?
        {
            return Err(RepositoryError::ScanStateConflict);
        }
        let source = source_name(&input.session.source);
        let suppressed: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM suppressed_sources
                WHERE source = ?
                  AND (source_identity_hash = ? OR vendor_session_id = ?)
             )",
        )
        .bind(source)
        .bind(input.source_location.source_identity_hash.as_slice())
        .bind(&input.session.vendor_session_id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| RepositoryError::WriteFailed)?;
        if suppressed {
            return Ok(PersistRevisionOutcome::Suppressed);
        }
        let stored_identity: Option<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT source, vendor_session_id, current_revision_id FROM sessions WHERE id = ?",
        )
        .bind(&input.session.id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepositoryError::WriteFailed)?;
        if stored_identity
            .as_ref()
            .is_some_and(|(stored_source, stored_vendor_id, _)| {
                stored_source != source || stored_vendor_id != &input.session.vendor_session_id
            })
        {
            return Err(RepositoryError::SessionIdentityConflict);
        }
        let identity_owner: Option<String> = sqlx::query_scalar(
            "SELECT id FROM sessions WHERE source = ? AND vendor_session_id = ?",
        )
        .bind(source)
        .bind(&input.session.vendor_session_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepositoryError::WriteFailed)?;
        if identity_owner
            .as_deref()
            .is_some_and(|owner| owner != input.session.id)
        {
            return Err(RepositoryError::SessionIdentityConflict);
        }
        let source_identity_owner: Option<String> = sqlx::query_scalar(
            "SELECT session_id FROM source_locations WHERE source_identity_hash = ?",
        )
        .bind(input.source_location.source_identity_hash.as_slice())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepositoryError::WriteFailed)?;
        if source_identity_owner
            .as_deref()
            .is_some_and(|owner| owner != input.session.id)
        {
            return Err(RepositoryError::SourceIdentityConflict);
        }

        let revision = &input.revision;
        let existing_revision_id: Option<String> = sqlx::query_scalar(
            "SELECT id FROM session_revisions WHERE session_id = ? AND content_hash = ?",
        )
        .bind(&input.session.id)
        .bind(revision.content_hash.as_slice())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepositoryError::WriteFailed)?;
        if existing_revision_id.is_none() {
            let revision_owner: Option<String> =
                sqlx::query_scalar("SELECT session_id FROM session_revisions WHERE id = ?")
                    .bind(&revision.id)
                    .fetch_optional(&mut *transaction)
                    .await
                    .map_err(|_| RepositoryError::WriteFailed)?;
            if revision_owner.is_some() {
                return Err(RepositoryError::RevisionIdentityConflict);
            }
        }

        sqlx::query(
            "INSERT INTO sessions (
                id, source, vendor_session_id, title, created_at_ms, source_version,
                collection, display_filename, current_revision_id, source_present,
                first_indexed_at_ms, last_seen_at_ms
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL, 1, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                title = excluded.title,
                created_at_ms = excluded.created_at_ms,
                source_version = excluded.source_version,
                collection = excluded.collection,
                display_filename = excluded.display_filename,
                source_present = 1,
                last_seen_at_ms = max(sessions.last_seen_at_ms, excluded.last_seen_at_ms)",
        )
        .bind(&input.session.id)
        .bind(source)
        .bind(&input.session.vendor_session_id)
        .bind(&input.session.title)
        .bind(input.session.created_at_ms.map(to_i64))
        .bind(&input.session.source_version)
        .bind(&input.session.collection)
        .bind(&input.session.display_filename)
        .bind(to_i64(input.observed_at_ms))
        .bind(to_i64(input.observed_at_ms))
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepositoryError::WriteFailed)?;

        let location = &input.source_location;
        sqlx::query(
            "INSERT INTO source_locations (
                session_id, canonical_path_utf16le, source_identity_hash, root_identity,
                file_identity, file_size_bytes, modified_at_ms, last_seen_generation
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(session_id) DO UPDATE SET
                canonical_path_utf16le = excluded.canonical_path_utf16le,
                source_identity_hash = excluded.source_identity_hash,
                root_identity = excluded.root_identity,
                file_identity = excluded.file_identity,
                file_size_bytes = excluded.file_size_bytes,
                modified_at_ms = excluded.modified_at_ms,
                last_seen_generation = excluded.last_seen_generation",
        )
        .bind(&input.session.id)
        .bind(&location.canonical_path_utf16le)
        .bind(location.source_identity_hash.as_slice())
        .bind(&location.root_identity)
        .bind(&location.file_identity)
        .bind(to_i64(location.file_size_bytes))
        .bind(to_i64(location.modified_at_ms))
        .bind(location.last_seen_generation.map(to_i64))
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepositoryError::WriteFailed)?;

        insert_default_preferences(&mut transaction, &input.session.id, input.observed_at_ms)
            .await?;

        if let Some(revision_id) = existing_revision_id {
            if stored_identity
                .as_ref()
                .and_then(|(_, _, revision_id)| revision_id.as_deref())
                == Some(revision_id.as_str())
            {
                replace_search_metadata(&mut transaction, &input.session).await?;
            } else {
                replace_search_document(&mut transaction, input).await?;
            }
            set_current_revision(&mut transaction, &input.session.id, &revision_id).await?;
            transaction
                .commit()
                .await
                .map_err(|_| RepositoryError::WriteFailed)?;
            return Ok(PersistRevisionOutcome::Unchanged { revision_id });
        }

        sqlx::query(
            "INSERT INTO session_revisions (
                id, session_id, content_hash, indexed_at_ms, source_created_at_ms,
                source_updated_at_ms, entry_count, duration_ms, diagnostic_count,
                reasoning_availability, tool_detail_availability
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&revision.id)
        .bind(&input.session.id)
        .bind(revision.content_hash.as_slice())
        .bind(to_i64(revision.indexed_at_ms))
        .bind(revision.source_created_at_ms.map(to_i64))
        .bind(revision.source_updated_at_ms.map(to_i64))
        .bind(to_i64(revision.entries.len() as u64))
        .bind(to_i64(revision.duration_ms))
        .bind(to_i64(revision.diagnostic_count))
        .bind(reasoning_availability_name(
            &revision.content_availability.reasoning,
        ))
        .bind(content_availability_name(
            &revision.content_availability.tool_details,
        ))
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepositoryError::WriteFailed)?;

        // Bound batches stay below SQLite's parameter limit while avoiding one
        // database execution per entry at the 100,000-entry contract limit.
        // Source: https://docs.rs/sqlx/0.8.6/sqlx/struct.QueryBuilder.html#method.push_values
        for entries in revision.entries.chunks(ENTRY_INSERT_BATCH_SIZE) {
            let mut query = QueryBuilder::<Sqlite>::new(
                "INSERT INTO revision_entries (
                    revision_id, entry_key, ordinal, at_ms, kind, body_text,
                    tool_name, tool_status, summary_text, tool_detail_availability,
                    tool_arguments, tool_result, display_path, source_type
                 ) ",
            );
            query.push_values(entries, |mut row, entry| {
                let columns = EntryColumns::from(entry);
                row.push_bind(&revision.id)
                    .push_bind(columns.entry_key)
                    .push_bind(to_i64(columns.ordinal))
                    .push_bind(to_i64(columns.at_ms))
                    .push_bind(columns.kind)
                    .push_bind(columns.body_text)
                    .push_bind(columns.tool_name)
                    .push_bind(columns.tool_status)
                    .push_bind(columns.summary_text)
                    .push_bind(columns.tool_detail_availability)
                    .push_bind(columns.tool_arguments)
                    .push_bind(columns.tool_result)
                    .push_bind(columns.display_path)
                    .push_bind(columns.source_type);
            });
            query
                .build()
                .execute(&mut *transaction)
                .await
                .map_err(|_| RepositoryError::WriteFailed)?;
        }

        replace_search_document(&mut transaction, input).await?;
        set_current_revision(&mut transaction, &input.session.id, &revision.id).await?;
        transaction
            .commit()
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;

        Ok(PersistRevisionOutcome::Inserted {
            revision_id: revision.id.clone(),
        })
    }
}

impl PersistSessionRevision {
    fn validate(&self) -> Result<(), RepositoryError> {
        let session = &self.session;
        let revision = &self.revision;
        let location = &self.source_location;
        if !valid_opaque_id(&session.id)
            || !valid_string(&session.vendor_session_id, 1, 1_024)
            || !valid_string(&session.title, 1, 512)
            || !valid_optional_string(&session.source_version, 256)
            || !valid_optional_string(&session.collection, 512)
            || !valid_optional_string(&session.display_filename, 512)
            || !valid_optional_timestamp(session.created_at_ms)
            || !valid_opaque_id(&revision.id)
            || revision.indexed_at_ms > MAX_SAFE_INTEGER
            || !valid_optional_timestamp(revision.source_created_at_ms)
            || !valid_optional_timestamp(revision.source_updated_at_ms)
            || revision.entries.is_empty()
            || revision.entries.len() > MAX_ENTRY_COUNT
            || revision.duration_ms > MAX_DURATION_MS
            || revision.diagnostic_count > 1_000
            || self.observed_at_ms > MAX_SAFE_INTEGER
            || !location.is_valid()
        {
            return Err(RepositoryError::InvalidInput);
        }

        let mut entry_keys = HashSet::with_capacity(revision.entries.len());
        let mut previous_ordinal = None;
        let mut previous_at_ms = None;
        for entry in &revision.entries {
            let columns = EntryColumns::from(entry);
            if !entry.validate()
                || !entry_keys.insert(columns.entry_key)
                || previous_ordinal.is_some_and(|ordinal| columns.ordinal <= ordinal)
                || previous_at_ms.is_some_and(|at_ms| columns.at_ms < at_ms)
                || columns.at_ms > revision.duration_ms
            {
                return Err(RepositoryError::InvalidInput);
            }
            previous_ordinal = Some(columns.ordinal);
            previous_at_ms = Some(columns.at_ms);
        }
        Ok(())
    }
}

impl PersistedSourceLocation {
    fn is_valid(&self) -> bool {
        (2..=65_536).contains(&self.canonical_path_utf16le.len())
            && self.canonical_path_utf16le.len().is_multiple_of(2)
            && !self
                .canonical_path_utf16le
                .chunks_exact(2)
                .any(|unit| unit == [0, 0])
            && (1..=512).contains(&self.root_identity.len())
            && (1..=512).contains(&self.file_identity.len())
            && self.file_size_bytes <= MAX_SAFE_INTEGER
            && self.modified_at_ms <= MAX_SAFE_INTEGER
            && self
                .last_seen_generation
                .is_none_or(|value| value != 0 && value <= MAX_SAFE_INTEGER)
    }
}

impl PersistedEntry {
    fn validate(&self) -> bool {
        let columns = EntryColumns::from(self);
        if !valid_opaque_id(columns.entry_key)
            || columns.ordinal >= MAX_ENTRY_COUNT as u64
            || columns.at_ms > MAX_DURATION_MS
        {
            return false;
        }
        match self {
            Self::User { text, .. } | Self::Reasoning { text, .. } => {
                valid_string(text, 0, MAX_ENTRY_TEXT_LENGTH)
            }
            Self::Assistant { markdown, .. } => valid_string(markdown, 0, MAX_ENTRY_TEXT_LENGTH),
            Self::ToolCall {
                name,
                summary,
                detail,
                ..
            } => {
                valid_string(name, 1, 256)
                    && valid_string(summary, 0, MAX_ENTRY_TEXT_LENGTH)
                    && match detail {
                        PersistedToolDetail::Available { arguments, result } => {
                            (arguments.is_some() || result.is_some())
                                && valid_optional_string(arguments, MAX_ENTRY_TEXT_LENGTH)
                                && valid_optional_string(result, MAX_ENTRY_TEXT_LENGTH)
                        }
                        PersistedToolDetail::Unavailable => true,
                    }
            }
            Self::FileChange {
                display_path,
                summary,
                ..
            } => {
                valid_string(display_path, 1, 32_768)
                    && valid_string(summary, 0, MAX_ENTRY_TEXT_LENGTH)
            }
            Self::Unknown { source_type, .. } => valid_string(source_type, 1, 256),
        }
    }
}

struct EntryColumns<'a> {
    entry_key: &'a str,
    ordinal: u64,
    at_ms: u64,
    kind: &'static str,
    body_text: Option<&'a str>,
    tool_name: Option<&'a str>,
    tool_status: Option<&'static str>,
    summary_text: Option<&'a str>,
    tool_detail_availability: Option<&'static str>,
    tool_arguments: Option<&'a str>,
    tool_result: Option<&'a str>,
    display_path: Option<&'a str>,
    source_type: Option<&'a str>,
}

impl<'a> From<&'a PersistedEntry> for EntryColumns<'a> {
    fn from(entry: &'a PersistedEntry) -> Self {
        let empty = Self {
            entry_key: "",
            ordinal: 0,
            at_ms: 0,
            kind: "",
            body_text: None,
            tool_name: None,
            tool_status: None,
            summary_text: None,
            tool_detail_availability: None,
            tool_arguments: None,
            tool_result: None,
            display_path: None,
            source_type: None,
        };
        match entry {
            PersistedEntry::User {
                entry_key,
                ordinal,
                at_ms,
                text,
            } => Self {
                entry_key,
                ordinal: *ordinal,
                at_ms: *at_ms,
                kind: "user",
                body_text: Some(text),
                ..empty
            },
            PersistedEntry::Assistant {
                entry_key,
                ordinal,
                at_ms,
                markdown,
            } => Self {
                entry_key,
                ordinal: *ordinal,
                at_ms: *at_ms,
                kind: "assistant",
                body_text: Some(markdown),
                ..empty
            },
            PersistedEntry::Reasoning {
                entry_key,
                ordinal,
                at_ms,
                text,
            } => Self {
                entry_key,
                ordinal: *ordinal,
                at_ms: *at_ms,
                kind: "reasoning",
                body_text: Some(text),
                ..empty
            },
            PersistedEntry::ToolCall {
                entry_key,
                ordinal,
                at_ms,
                name,
                status,
                summary,
                detail,
            } => {
                let (availability, arguments, result) = match detail {
                    PersistedToolDetail::Available { arguments, result } => {
                        ("available", arguments.as_deref(), result.as_deref())
                    }
                    PersistedToolDetail::Unavailable => ("unavailable", None, None),
                };
                Self {
                    entry_key,
                    ordinal: *ordinal,
                    at_ms: *at_ms,
                    kind: "tool-call",
                    tool_name: Some(name),
                    tool_status: Some(tool_status_name(status)),
                    summary_text: Some(summary),
                    tool_detail_availability: Some(availability),
                    tool_arguments: arguments,
                    tool_result: result,
                    ..empty
                }
            }
            PersistedEntry::FileChange {
                entry_key,
                ordinal,
                at_ms,
                display_path,
                summary,
            } => Self {
                entry_key,
                ordinal: *ordinal,
                at_ms: *at_ms,
                kind: "file-change",
                summary_text: Some(summary),
                display_path: Some(display_path),
                ..empty
            },
            PersistedEntry::Unknown {
                entry_key,
                ordinal,
                at_ms,
                source_type,
            } => Self {
                entry_key,
                ordinal: *ordinal,
                at_ms: *at_ms,
                kind: "unknown",
                source_type: Some(source_type),
                ..empty
            },
        }
    }
}

async fn insert_default_preferences(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    session_id: &str,
    updated_at_ms: u64,
) -> Result<(), RepositoryError> {
    sqlx::query(
        "INSERT INTO session_preferences (
            session_id, show_tool_calls, show_tool_details, show_reasoning,
            entry_delay_ms, playback_speed, background_color, surface_color,
            text_color, muted_color, accent_color, success_color, error_color,
            font_family, font_size_px, line_height, updated_at_ms
         ) VALUES (?, 1, 1, 1, ?, 1.0, ?, ?, ?, ?, ?, ?, ?, 'JetBrains Mono', 34, 1.5, ?)
         ON CONFLICT(session_id) DO NOTHING",
    )
    .bind(session_id)
    .bind(to_i64(DEFAULT_PRESENTATION_DELAY_MS))
    .bind(DEFAULT_BACKGROUND)
    .bind(DEFAULT_SURFACE)
    .bind(DEFAULT_TEXT)
    .bind(DEFAULT_MUTED)
    .bind(DEFAULT_ACCENT)
    .bind(DEFAULT_SUCCESS)
    .bind(DEFAULT_ERROR)
    .bind(to_i64(updated_at_ms))
    .execute(&mut **transaction)
    .await
    .map_err(|_| RepositoryError::WriteFailed)?;
    Ok(())
}

async fn set_current_revision(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    session_id: &str,
    revision_id: &str,
) -> Result<(), RepositoryError> {
    sqlx::query("UPDATE sessions SET current_revision_id = ? WHERE id = ?")
        .bind(revision_id)
        .bind(session_id)
        .execute(&mut **transaction)
        .await
        .map_err(|_| RepositoryError::WriteFailed)?;
    Ok(())
}

async fn replace_search_metadata(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    session: &PersistedSession,
) -> Result<(), RepositoryError> {
    sqlx::query(
        "DELETE FROM session_search_documents
         WHERE session_id = ? AND search_key = '__session_metadata__'",
    )
    .bind(&session.id)
    .execute(&mut **transaction)
    .await
    .map_err(|_| RepositoryError::WriteFailed)?;
    insert_search_metadata(transaction, session).await
}

async fn replace_search_document(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    input: &PersistSessionRevision,
) -> Result<(), RepositoryError> {
    sqlx::query("DELETE FROM session_search_documents WHERE session_id = ?")
        .bind(&input.session.id)
        .execute(&mut **transaction)
        .await
        .map_err(|_| RepositoryError::WriteFailed)?;
    insert_search_metadata(transaction, &input.session).await?;

    for entries in input.revision.entries.chunks(ENTRY_INSERT_BATCH_SIZE) {
        let mut query = QueryBuilder::<Sqlite>::new(
            "INSERT INTO session_search_documents (
                session_id, search_key, source, title, body_text, tool_name,
                tool_status, summary_text, tool_arguments, tool_result,
                display_path, source_type
             ) ",
        );
        query.push_values(entries, |mut row, entry| {
            let columns = EntryColumns::from(entry);
            row.push_bind(&input.session.id)
                .push_bind(columns.entry_key)
                .push_bind(Option::<&str>::None)
                .push_bind(Option::<&str>::None)
                .push_bind(columns.body_text)
                .push_bind(columns.tool_name)
                .push_bind(columns.tool_status)
                .push_bind(columns.summary_text)
                .push_bind(columns.tool_arguments)
                .push_bind(columns.tool_result)
                .push_bind(columns.display_path)
                .push_bind(columns.source_type);
        });
        query
            .build()
            .execute(&mut **transaction)
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
    }
    Ok(())
}

async fn insert_search_metadata(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    session: &PersistedSession,
) -> Result<(), RepositoryError> {
    let effective_title: String =
        sqlx::query_scalar("SELECT coalesce(custom_title, title) FROM sessions WHERE id = ?")
            .bind(&session.id)
            .fetch_one(&mut **transaction)
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
    sqlx::query(
        "INSERT INTO session_search_documents (
            session_id, search_key, source, title, body_text, tool_name,
            tool_status, summary_text, tool_arguments, tool_result,
            display_path, source_type
         ) VALUES (?, '__session_metadata__', ?, ?, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL)",
    )
    .bind(&session.id)
    .bind(source_search_text(&session.source))
    .bind(effective_title)
    .execute(&mut **transaction)
    .await
    .map_err(|_| RepositoryError::WriteFailed)?;
    Ok(())
}

fn source_search_text(source: &IndexedSessionSourceV1) -> &'static str {
    match source {
        IndexedSessionSourceV1::ClaudeCode => "claude code",
        IndexedSessionSourceV1::Codex => "codex cli",
        IndexedSessionSourceV1::CopilotCli => "copilot cli",
        IndexedSessionSourceV1::VscodeCopilot => "visual studio code copilot",
    }
}

fn source_name(source: &IndexedSessionSourceV1) -> &'static str {
    match source {
        IndexedSessionSourceV1::ClaudeCode => "claude-code",
        IndexedSessionSourceV1::Codex => "codex",
        IndexedSessionSourceV1::CopilotCli => "copilot-cli",
        IndexedSessionSourceV1::VscodeCopilot => "vscode-copilot",
    }
}

fn source_from_name(value: &str) -> Result<IndexedSessionSourceV1, RepositoryError> {
    match value {
        "claude-code" => Ok(IndexedSessionSourceV1::ClaudeCode),
        "codex" => Ok(IndexedSessionSourceV1::Codex),
        "copilot-cli" => Ok(IndexedSessionSourceV1::CopilotCli),
        "vscode-copilot" => Ok(IndexedSessionSourceV1::VscodeCopilot),
        _ => Err(RepositoryError::ReadFailed),
    }
}

fn reasoning_availability_name(value: &ReasoningAvailability) -> &'static str {
    match value {
        ReasoningAvailability::Available => "available",
        ReasoningAvailability::Unavailable => "unavailable",
    }
}

fn content_availability_name(value: &ContentAvailability) -> &'static str {
    match value {
        ContentAvailability::Available => "available",
        ContentAvailability::Partial => "partial",
        ContentAvailability::Unavailable => "unavailable",
    }
}

fn tool_status_name(status: &IndexedToolStatus) -> &'static str {
    match status {
        IndexedToolStatus::Pending => "pending",
        IndexedToolStatus::Running => "running",
        IndexedToolStatus::Succeeded => "succeeded",
        IndexedToolStatus::Failed => "failed",
    }
}

fn valid_opaque_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn valid_string(value: &str, minimum: usize, maximum: usize) -> bool {
    let length = value.chars().count();
    (minimum..=maximum).contains(&length) && !value.contains('\0')
}

fn valid_optional_string(value: &Option<String>, maximum: usize) -> bool {
    value
        .as_deref()
        .is_none_or(|value| valid_string(value, 0, maximum))
}

fn valid_optional_timestamp(value: Option<u64>) -> bool {
    value.is_none_or(|value| value <= MAX_SAFE_INTEGER)
}

fn to_i64(value: u64) -> i64 {
    value as i64
}
