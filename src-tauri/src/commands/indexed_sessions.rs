use tauri::State;

use crate::adapters::jetbrains::{JetBrainsDetectionRoots, detect_jetbrains_copilot};
use crate::database::DatabaseState;
use crate::database::repository::{IndexedSessionRepository, RepositoryError};
use crate::deletion::{DeletionError, SessionDeletionService};
use crate::discovery::index::{IndexCoordinator, IndexError};
use crate::error::{AppError, CommandError};
use crate::indexed_library::{
    DeleteIndexedSessionRequestV1, DeleteIndexedSessionWithSourceRequestV1, IndexedSessionDetailV1,
    IndexedSessionListRequestV1, IndexedSessionPageV1, JetBrainsCopilotStatusV1,
    PrepareSourceDeletionRequestV1, RenameIndexedSessionRequestV1, ResetLocalDatabaseRequestV1,
    ResetLocalDatabaseResultV1, RestoreSuppressedSourceRequestV1, RestoreSuppressedSourceResultV1,
    SetEntrySelectionsRequestV1, SetSessionPreferencesRequestV1, SourceDeletionConfirmationV1,
    SuppressedSourceListRequestV1, SuppressedSourcePageV1, SuppressedSourceV1, Validated,
};

#[tauri::command]
pub(crate) async fn get_jetbrains_copilot_status() -> Result<JetBrainsCopilotStatusV1, CommandError>
{
    tauri::async_runtime::spawn_blocking(|| {
        detect_jetbrains_copilot(&JetBrainsDetectionRoots::from_environment())
    })
    .await
    .map_err(|_| CommandError::from(AppError::Internal))
}

#[tauri::command]
pub async fn list_indexed_sessions(
    request: Validated<IndexedSessionListRequestV1>,
    state: State<'_, DatabaseState>,
) -> Result<IndexedSessionPageV1, CommandError> {
    list_from_repository(&state.repository(), request.get()).await
}

#[tauri::command]
pub async fn get_indexed_session(
    session_id: String,
    entry_cursor: Option<String>,
    state: State<'_, DatabaseState>,
) -> Result<IndexedSessionDetailV1, CommandError> {
    detail_from_repository(&state.repository(), &session_id, entry_cursor.as_deref()).await
}

#[tauri::command]
pub(crate) async fn delete_indexed_session(
    request: Validated<DeleteIndexedSessionRequestV1>,
    state: State<'_, SessionDeletionService>,
) -> Result<SuppressedSourceV1, CommandError> {
    let session_id = request.get().session_id.clone();
    let result = state
        .delete_library_only(&request.get().session_id)
        .await
        .map_err(deletion_command_error)?;
    crate::commands::runtime::forget_presentation_plans_for_session(&session_id);
    Ok(result)
}

#[tauri::command]
pub(crate) async fn prepare_source_deletion(
    request: Validated<PrepareSourceDeletionRequestV1>,
    state: State<'_, SessionDeletionService>,
) -> Result<SourceDeletionConfirmationV1, CommandError> {
    state
        .prepare_source_deletion(&request.get().session_id)
        .await
        .map_err(deletion_command_error)
}

#[tauri::command]
pub(crate) async fn delete_indexed_session_with_source(
    request: Validated<DeleteIndexedSessionWithSourceRequestV1>,
    state: State<'_, SessionDeletionService>,
) -> Result<SuppressedSourceV1, CommandError> {
    let request = request.get();
    let result = state
        .delete_with_source(&request.session_id, &request.confirmation_token)
        .await
        .map_err(deletion_command_error)?;
    crate::commands::runtime::forget_presentation_plans_for_session(&request.session_id);
    Ok(result)
}

#[tauri::command]
pub(crate) async fn list_suppressed_sources(
    request: Validated<SuppressedSourceListRequestV1>,
    state: State<'_, SessionDeletionService>,
) -> Result<SuppressedSourcePageV1, CommandError> {
    state
        .list_suppressed(request.get())
        .await
        .map_err(deletion_command_error)
}

#[tauri::command]
pub(crate) async fn restore_suppressed_source(
    request: Validated<RestoreSuppressedSourceRequestV1>,
    state: State<'_, SessionDeletionService>,
) -> Result<RestoreSuppressedSourceResultV1, CommandError> {
    state
        .restore(&request.get().suppression_id)
        .await
        .map_err(deletion_command_error)
}

#[tauri::command]
pub(crate) async fn reset_local_database(
    request: Validated<ResetLocalDatabaseRequestV1>,
    state: State<'_, IndexCoordinator>,
) -> Result<ResetLocalDatabaseResultV1, CommandError> {
    let _ = request.get();
    let result = state
        .reset_local_database()
        .await
        .map_err(reset_command_error)?;
    crate::commands::runtime::clear_frozen_plans();
    Ok(result)
}

#[tauri::command]
pub(crate) async fn set_entry_selections(
    request: Validated<SetEntrySelectionsRequestV1>,
    state: State<'_, DatabaseState>,
) -> Result<IndexedSessionDetailV1, CommandError> {
    let request = request.get();
    let repository = state.repository();
    repository
        .set_entry_selections(request, now_ms())
        .await
        .map_err(command_error)?;
    detail_from_repository(&repository, &request.session_id, None).await
}

#[tauri::command]
pub(crate) async fn rename_indexed_session(
    request: Validated<RenameIndexedSessionRequestV1>,
    state: State<'_, DatabaseState>,
) -> Result<IndexedSessionDetailV1, CommandError> {
    let request = request.get();
    let repository = state.repository();
    repository
        .rename_session(request)
        .await
        .map_err(command_error)?;
    detail_from_repository(&repository, &request.session_id, None).await
}

#[tauri::command]
pub(crate) async fn set_session_preferences(
    request: Validated<SetSessionPreferencesRequestV1>,
    state: State<'_, DatabaseState>,
) -> Result<IndexedSessionDetailV1, CommandError> {
    let request = request.get();
    let repository = state.repository();
    repository
        .set_session_preferences(request, now_ms())
        .await
        .map_err(command_error)?;
    detail_from_repository(&repository, &request.session_id, None).await
}

async fn list_from_repository(
    repository: &IndexedSessionRepository,
    request: &IndexedSessionListRequestV1,
) -> Result<IndexedSessionPageV1, CommandError> {
    repository
        .list_indexed_sessions(request)
        .await
        .map_err(command_error)
}

async fn detail_from_repository(
    repository: &IndexedSessionRepository,
    session_id: &str,
    entry_cursor: Option<&str>,
) -> Result<IndexedSessionDetailV1, CommandError> {
    repository
        .get_indexed_session(session_id, entry_cursor)
        .await
        .map_err(command_error)
}

fn command_error(error: RepositoryError) -> CommandError {
    match error {
        RepositoryError::InvalidInput => CommandError {
            code: "INVALID_REQUEST",
            message: "The indexed-session request is invalid".to_owned(),
        },
        RepositoryError::SessionNotFound => CommandError {
            code: "SESSION_NOT_FOUND",
            message: "The indexed session is unavailable".to_owned(),
        },
        RepositoryError::StaleRevision => CommandError {
            code: "STALE_REVISION",
            message: "The indexed session changed; reload it and try again".to_owned(),
        },
        _ => CommandError::from(AppError::Internal),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
        .min(9_007_199_254_740_991)
}

fn deletion_command_error(error: DeletionError) -> CommandError {
    let (code, message) = match error {
        DeletionError::InvalidRequest => ("INVALID_REQUEST", "The deletion request is invalid"),
        DeletionError::SessionNotFound => {
            ("SESSION_NOT_FOUND", "The indexed session is unavailable")
        }
        DeletionError::SuppressionNotFound => (
            "SUPPRESSION_NOT_FOUND",
            "The suppressed source is unavailable",
        ),
        DeletionError::SourceUnavailable => {
            ("SOURCE_UNAVAILABLE", "The source session is unavailable")
        }
        DeletionError::UnsafeSource => (
            "SOURCE_UNSAFE",
            "The source session cannot be deleted safely",
        ),
        DeletionError::SourceStateChanged => (
            "SOURCE_STATE_CHANGED",
            "The source session changed; confirm deletion again",
        ),
        DeletionError::ConfirmationInvalid => (
            "CONFIRMATION_INVALID",
            "The deletion confirmation is invalid",
        ),
        DeletionError::ConfirmationExpired => {
            ("CONFIRMATION_EXPIRED", "The deletion confirmation expired")
        }
        DeletionError::SourceDeleteFailed => (
            "SOURCE_DELETE_FAILED",
            "The source session could not be deleted",
        ),
        DeletionError::Storage | DeletionError::Identifier | DeletionError::StateConflict => {
            return CommandError::from(AppError::Internal);
        }
    };
    CommandError {
        code,
        message: message.to_owned(),
    }
}

fn reset_command_error(error: IndexError) -> CommandError {
    match error {
        IndexError::RefreshInProgress => CommandError {
            code: "REFRESH_IN_PROGRESS",
            message: "Wait for the current refresh to finish before resetting the library"
                .to_owned(),
        },
        _ => CommandError::from(AppError::Internal),
    }
}

#[cfg(test)]
mod tests {
    use super::{command_error, deletion_command_error, reset_command_error};
    use crate::database::repository::RepositoryError;
    use crate::deletion::DeletionError;
    use crate::discovery::index::IndexError;

    #[test]
    fn maps_request_and_missing_session_failures_without_private_details() {
        let invalid = command_error(RepositoryError::InvalidInput);
        assert_eq!(invalid.code, "INVALID_REQUEST");
        assert_eq!(invalid.message, "The indexed-session request is invalid");

        let missing = command_error(RepositoryError::SessionNotFound);
        assert_eq!(missing.code, "SESSION_NOT_FOUND");
        assert_eq!(missing.message, "The indexed session is unavailable");

        let stale = command_error(RepositoryError::StaleRevision);
        assert_eq!(stale.code, "STALE_REVISION");
        assert!(!stale.message.contains(':'));
    }

    #[test]
    fn maps_storage_and_identity_failures_to_one_safe_error() {
        for error in [
            RepositoryError::ReadFailed,
            RepositoryError::WriteFailed,
            RepositoryError::SessionIdentityConflict,
            RepositoryError::RevisionIdentityConflict,
            RepositoryError::SourceIdentityConflict,
            RepositoryError::ScanStateConflict,
        ] {
            let mapped = command_error(error);
            assert_eq!(mapped.code, "INTERNAL_ERROR");
            assert_eq!(mapped.message, "The operation could not be completed");
        }
    }

    #[test]
    fn maps_deletion_failures_to_stable_path_free_errors() {
        let expected = [
            (DeletionError::InvalidRequest, "INVALID_REQUEST"),
            (DeletionError::SessionNotFound, "SESSION_NOT_FOUND"),
            (DeletionError::SuppressionNotFound, "SUPPRESSION_NOT_FOUND"),
            (DeletionError::SourceUnavailable, "SOURCE_UNAVAILABLE"),
            (DeletionError::UnsafeSource, "SOURCE_UNSAFE"),
            (DeletionError::SourceStateChanged, "SOURCE_STATE_CHANGED"),
            (DeletionError::ConfirmationInvalid, "CONFIRMATION_INVALID"),
            (DeletionError::ConfirmationExpired, "CONFIRMATION_EXPIRED"),
            (DeletionError::SourceDeleteFailed, "SOURCE_DELETE_FAILED"),
        ];

        for (error, code) in expected {
            let mapped = deletion_command_error(error);
            assert_eq!(mapped.code, code);
            assert!(!mapped.message.contains(':'));
            assert!(!mapped.message.contains('\\'));
        }

        for error in [
            DeletionError::Storage,
            DeletionError::Identifier,
            DeletionError::StateConflict,
        ] {
            assert_eq!(deletion_command_error(error).code, "INTERNAL_ERROR");
        }
    }

    #[test]
    fn maps_reset_failures_without_database_details() {
        let active = reset_command_error(IndexError::RefreshInProgress);
        assert_eq!(active.code, "REFRESH_IN_PROGRESS");
        assert!(!active.message.contains(':'));

        for error in [
            IndexError::Storage,
            IndexError::Runtime,
            IndexError::Identifier,
            IndexError::StateConflict,
        ] {
            assert_eq!(reset_command_error(error).code, "INTERNAL_ERROR");
        }
    }
}
