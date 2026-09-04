use sqlx::{Sqlite, Transaction};

use super::{IndexedSessionRepository, RepositoryError, to_i64};
use crate::indexed_library::{
    ContractValidate, RenameIndexedSessionRequestV1, SetEntrySelectionsRequestV1,
    SetSessionPreferencesRequestV1,
};
use crate::model::FontFamily;

impl IndexedSessionRepository {
    pub(crate) async fn rename_session(
        &self,
        request: &RenameIndexedSessionRequestV1,
    ) -> Result<(), RepositoryError> {
        request
            .validate()
            .map_err(|_| RepositoryError::InvalidInput)?;
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
        let updated = sqlx::query("UPDATE sessions SET custom_title = ? WHERE id = ?")
            .bind(&request.title)
            .bind(&request.session_id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
        if updated.rows_affected() != 1 {
            return Err(RepositoryError::SessionNotFound);
        }
        let search_updated = sqlx::query(
            "UPDATE session_search_documents SET title = ?
             WHERE session_id = ? AND search_key = '__session_metadata__'",
        )
        .bind(&request.title)
        .bind(&request.session_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepositoryError::WriteFailed)?;
        if search_updated.rows_affected() != 1 {
            return Err(RepositoryError::WriteFailed);
        }
        transaction
            .commit()
            .await
            .map_err(|_| RepositoryError::WriteFailed)
    }

    pub(crate) async fn set_entry_selections(
        &self,
        request: &SetEntrySelectionsRequestV1,
        updated_at_ms: u64,
    ) -> Result<(), RepositoryError> {
        request
            .validate()
            .map_err(|_| RepositoryError::InvalidInput)?;
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
        assert_current_revision(&mut transaction, &request.session_id, &request.revision_id)
            .await?;

        for change in &request.changes {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM revision_entries
                    WHERE revision_id = ? AND entry_key = ?
                 )",
            )
            .bind(&request.revision_id)
            .bind(&change.entry_key)
            .fetch_one(&mut *transaction)
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
            if !exists {
                return Err(RepositoryError::InvalidInput);
            }
            if change.selected {
                sqlx::query(
                    "DELETE FROM entry_selection_overrides
                     WHERE session_id = ? AND stable_entry_key = ?",
                )
                .bind(&request.session_id)
                .bind(&change.entry_key)
                .execute(&mut *transaction)
                .await
                .map_err(|_| RepositoryError::WriteFailed)?;
            } else {
                sqlx::query(
                    "INSERT INTO entry_selection_overrides (
                        session_id, stable_entry_key, selected, updated_at_ms
                     ) VALUES (?, ?, 0, ?)
                     ON CONFLICT(session_id, stable_entry_key) DO UPDATE SET
                        selected = 0,
                        updated_at_ms = excluded.updated_at_ms",
                )
                .bind(&request.session_id)
                .bind(&change.entry_key)
                .bind(to_i64(updated_at_ms))
                .execute(&mut *transaction)
                .await
                .map_err(|_| RepositoryError::WriteFailed)?;
            }
        }
        transaction
            .commit()
            .await
            .map_err(|_| RepositoryError::WriteFailed)
    }

    pub(crate) async fn set_session_preferences(
        &self,
        request: &SetSessionPreferencesRequestV1,
        updated_at_ms: u64,
    ) -> Result<(), RepositoryError> {
        request
            .validate()
            .map_err(|_| RepositoryError::InvalidInput)?;
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
        assert_current_revision(&mut transaction, &request.session_id, &request.revision_id)
            .await?;
        let preferences = &request.preferences;
        let updated = sqlx::query(
            "UPDATE session_preferences SET
                show_tool_calls = ?, show_tool_details = ?, show_reasoning = ?,
                entry_delay_ms = ?, playback_speed = ?,
                background_color = ?, surface_color = ?, text_color = ?, muted_color = ?,
                accent_color = ?, success_color = ?, error_color = ?,
                font_family = ?, font_size_px = ?, line_height = ?, updated_at_ms = ?
             WHERE session_id = ?",
        )
        .bind(i64::from(preferences.visibility.show_tool_calls))
        .bind(i64::from(preferences.visibility.show_tool_details))
        .bind(i64::from(preferences.visibility.show_reasoning))
        .bind(to_i64(preferences.timing.entry_delay_ms))
        .bind(preferences.timing.playback_speed)
        .bind(&preferences.appearance.theme.background)
        .bind(&preferences.appearance.theme.surface)
        .bind(&preferences.appearance.theme.text)
        .bind(&preferences.appearance.theme.muted)
        .bind(&preferences.appearance.theme.accent)
        .bind(&preferences.appearance.theme.success)
        .bind(&preferences.appearance.theme.error)
        .bind(match preferences.appearance.font.family {
            FontFamily::JetBrainsMono => "JetBrains Mono",
        })
        .bind(i64::from(preferences.appearance.font.size_px))
        .bind(preferences.appearance.font.line_height)
        .bind(to_i64(updated_at_ms))
        .bind(&request.session_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| RepositoryError::WriteFailed)?;
        if updated.rows_affected() != 1 {
            return Err(RepositoryError::SessionNotFound);
        }
        transaction
            .commit()
            .await
            .map_err(|_| RepositoryError::WriteFailed)
    }
}

async fn assert_current_revision(
    transaction: &mut Transaction<'_, Sqlite>,
    session_id: &str,
    revision_id: &str,
) -> Result<(), RepositoryError> {
    let current: Option<String> =
        sqlx::query_scalar("SELECT current_revision_id FROM sessions WHERE id = ?")
            .bind(session_id)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(|_| RepositoryError::WriteFailed)?;
    let current = current.ok_or(RepositoryError::SessionNotFound)?;
    if current != revision_id {
        return Err(RepositoryError::StaleRevision);
    }
    Ok(())
}
