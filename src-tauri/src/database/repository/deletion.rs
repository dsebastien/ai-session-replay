use sqlx::Row;

use super::{
    IndexedSessionRepository, RepositoryError, source_from_name, source_name, to_i64,
    valid_opaque_id,
};
use crate::indexed_library::{
    ContractValidate, INDEXED_LIBRARY_SCHEMA_VERSION, RestoreSuppressedSourceResultV1,
    SuppressedSourceListRequestV1, SuppressedSourcePageV1, SuppressedSourceV1,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceDeletionRecord {
    pub(crate) session_id: String,
    pub(crate) source: crate::indexed_library::IndexedSessionSourceV1,
    pub(crate) vendor_session_id: String,
    pub(crate) collection: String,
    pub(crate) canonical_path_utf16le: Vec<u8>,
    pub(crate) source_identity_hash: [u8; 32],
    pub(crate) root_identity: Vec<u8>,
    pub(crate) file_identity: Vec<u8>,
}

impl IndexedSessionRepository {
    pub(crate) async fn source_deletion_record(
        &self,
        session_id: &str,
    ) -> Result<SourceDeletionRecord, RepositoryError> {
        if !valid_opaque_id(session_id) {
            return Err(RepositoryError::InvalidInput);
        }
        let row = sqlx::query(
            "SELECT s.id, s.source, s.vendor_session_id, s.collection,
                    l.canonical_path_utf16le, l.source_identity_hash,
                    l.root_identity, l.file_identity
               FROM sessions s
               JOIN source_locations l ON l.session_id = s.id
              WHERE s.id = ?",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| RepositoryError::ReadFailed)?
        .ok_or(RepositoryError::SessionNotFound)?;
        source_deletion_record_from_row(&row)
    }

    pub(crate) async fn delete_indexed_session(
        &self,
        session_id: &str,
        new_suppression_id: &str,
        source_deleted: bool,
        suppressed_at_ms: u64,
        expected_source_identity: Option<&[u8; 32]>,
    ) -> Result<SuppressedSourceV1, RepositoryError> {
        if !valid_opaque_id(session_id)
            || !valid_opaque_id(new_suppression_id)
            || suppressed_at_ms > 9_007_199_254_740_991
        {
            return Err(RepositoryError::InvalidInput);
        }
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
        let row = sqlx::query(
            "SELECT s.id, s.source, s.vendor_session_id, s.collection,
                    l.canonical_path_utf16le, l.source_identity_hash,
                    l.root_identity, l.file_identity
               FROM sessions s
               JOIN source_locations l ON l.session_id = s.id
              WHERE s.id = ?",
        )
        .bind(session_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepositoryError::WriteFailed)?
        .ok_or(RepositoryError::SessionNotFound)?;
        let source_record = source_deletion_record_from_row(&row)?;
        if expected_source_identity
            .is_some_and(|expected| expected != &source_record.source_identity_hash)
        {
            return Err(RepositoryError::SourceStateChanged);
        }

        let existing_id: Option<String> = sqlx::query_scalar(
            "SELECT id FROM suppressed_sources
              WHERE source = ?
                AND (source_identity_hash = ? OR vendor_session_id = ?)
              ORDER BY suppressed_at_ms DESC, id ASC
              LIMIT 1",
        )
        .bind(source_name(&source_record.source))
        .bind(source_record.source_identity_hash.as_slice())
        .bind(&source_record.vendor_session_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepositoryError::WriteFailed)?;
        let suppression_id = existing_id.unwrap_or_else(|| new_suppression_id.to_owned());
        sqlx::query(
            "INSERT INTO suppressed_sources (
                id, source, source_identity_hash, vendor_session_id,
                source_deleted, suppressed_at_ms
             ) VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                source_deleted = max(source_deleted, excluded.source_deleted),
                suppressed_at_ms = max(suppressed_at_ms, excluded.suppressed_at_ms)",
        )
        .bind(&suppression_id)
        .bind(source_name(&source_record.source))
        .bind(source_record.source_identity_hash.as_slice())
        .bind(&source_record.vendor_session_id)
        .bind(i64::from(source_deleted))
        .bind(to_i64(suppressed_at_ms))
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepositoryError::WriteFailed)?;
        let deleted = sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(session_id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
        if deleted.rows_affected() != 1 {
            return Err(RepositoryError::SessionNotFound);
        }
        transaction
            .commit()
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;

        Ok(SuppressedSourceV1 {
            schema_version: INDEXED_LIBRARY_SCHEMA_VERSION,
            suppression_id,
            source: source_record.source,
            source_deleted,
            suppressed_at_ms,
        })
    }

    pub(crate) async fn mark_source_missing(
        &self,
        session_id: &str,
        expected_source_identity: &[u8; 32],
    ) -> Result<(), RepositoryError> {
        if !valid_opaque_id(session_id) {
            return Err(RepositoryError::InvalidInput);
        }
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
        let actual: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT source_identity_hash FROM source_locations WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| RepositoryError::WriteFailed)?;
        let actual = actual.ok_or(RepositoryError::SessionNotFound)?;
        if actual.as_slice() != expected_source_identity {
            return Err(RepositoryError::SourceStateChanged);
        }
        sqlx::query("UPDATE sessions SET source_present = 0 WHERE id = ?")
            .bind(session_id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
        transaction
            .commit()
            .await
            .map_err(|_| RepositoryError::WriteFailed)
    }

    pub(crate) async fn list_suppressed_sources(
        &self,
        request: &SuppressedSourceListRequestV1,
    ) -> Result<SuppressedSourcePageV1, RepositoryError> {
        request
            .validate()
            .map_err(|_| RepositoryError::InvalidInput)?;
        let cursor = match request.cursor.as_deref() {
            Some(id) => Some(
                sqlx::query_as::<_, (i64, String)>(
                    "SELECT suppressed_at_ms, id FROM suppressed_sources WHERE id = ?",
                )
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|_| RepositoryError::ReadFailed)?
                .ok_or(RepositoryError::InvalidInput)?,
            ),
            None => None,
        };
        let mut query = sqlx::QueryBuilder::new(
            "SELECT id, source, source_deleted, suppressed_at_ms FROM suppressed_sources WHERE 1 = 1",
        );
        if let Some((suppressed_at_ms, id)) = cursor {
            query
                .push(" AND (suppressed_at_ms < ")
                .push_bind(suppressed_at_ms)
                .push(" OR (suppressed_at_ms = ")
                .push_bind(suppressed_at_ms)
                .push(" AND id > ")
                .push_bind(id)
                .push("))");
        }
        query
            .push(" ORDER BY suppressed_at_ms DESC, id ASC LIMIT ")
            .push_bind(
                i64::try_from(request.page_size + 1).map_err(|_| RepositoryError::InvalidInput)?,
            );
        let rows = query
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|_| RepositoryError::ReadFailed)?;
        let has_more = rows.len() > request.page_size;
        let items = rows
            .iter()
            .take(request.page_size)
            .map(suppressed_source_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = has_more
            .then(|| items.last().map(|item| item.suppression_id.clone()))
            .flatten();
        let page = SuppressedSourcePageV1 {
            schema_version: INDEXED_LIBRARY_SCHEMA_VERSION,
            items,
            next_cursor,
        };
        page.validate().map_err(|_| RepositoryError::ReadFailed)?;
        Ok(page)
    }

    pub(crate) async fn restore_suppressed_source(
        &self,
        suppression_id: &str,
    ) -> Result<RestoreSuppressedSourceResultV1, RepositoryError> {
        if !valid_opaque_id(suppression_id) {
            return Err(RepositoryError::InvalidInput);
        }
        let result = sqlx::query("DELETE FROM suppressed_sources WHERE id = ?")
            .bind(suppression_id)
            .execute(&self.pool)
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
        if result.rows_affected() != 1 {
            return Err(RepositoryError::SuppressionNotFound);
        }
        Ok(RestoreSuppressedSourceResultV1 {
            schema_version: INDEXED_LIBRARY_SCHEMA_VERSION,
            suppression_id: suppression_id.to_owned(),
        })
    }
}

fn source_deletion_record_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<SourceDeletionRecord, RepositoryError> {
    let identity = row
        .try_get::<Vec<u8>, _>("source_identity_hash")
        .map_err(|_| RepositoryError::ReadFailed)?;
    let source_identity_hash: [u8; 32] = identity
        .try_into()
        .map_err(|_| RepositoryError::ReadFailed)?;
    Ok(SourceDeletionRecord {
        session_id: row.try_get("id").map_err(|_| RepositoryError::ReadFailed)?,
        source: source_from_name(
            &row.try_get::<String, _>("source")
                .map_err(|_| RepositoryError::ReadFailed)?,
        )?,
        vendor_session_id: row
            .try_get("vendor_session_id")
            .map_err(|_| RepositoryError::ReadFailed)?,
        collection: row
            .try_get::<Option<String>, _>("collection")
            .map_err(|_| RepositoryError::ReadFailed)?
            .ok_or(RepositoryError::ReadFailed)?,
        canonical_path_utf16le: row
            .try_get("canonical_path_utf16le")
            .map_err(|_| RepositoryError::ReadFailed)?,
        source_identity_hash,
        root_identity: row
            .try_get("root_identity")
            .map_err(|_| RepositoryError::ReadFailed)?,
        file_identity: row
            .try_get("file_identity")
            .map_err(|_| RepositoryError::ReadFailed)?,
    })
}

fn suppressed_source_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<SuppressedSourceV1, RepositoryError> {
    let source_deleted = match row
        .try_get::<i64, _>("source_deleted")
        .map_err(|_| RepositoryError::ReadFailed)?
    {
        0 => false,
        1 => true,
        _ => return Err(RepositoryError::ReadFailed),
    };
    let suppressed_at_ms = row
        .try_get::<i64, _>("suppressed_at_ms")
        .ok()
        .and_then(|value| u64::try_from(value).ok())
        .ok_or(RepositoryError::ReadFailed)?;
    let tombstone = SuppressedSourceV1 {
        schema_version: INDEXED_LIBRARY_SCHEMA_VERSION,
        suppression_id: row.try_get("id").map_err(|_| RepositoryError::ReadFailed)?,
        source: source_from_name(
            &row.try_get::<String, _>("source")
                .map_err(|_| RepositoryError::ReadFailed)?,
        )?,
        source_deleted,
        suppressed_at_ms,
    };
    tombstone
        .validate()
        .map_err(|_| RepositoryError::ReadFailed)?;
    Ok(tombstone)
}
