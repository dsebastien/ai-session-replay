use std::sync::Arc;

use tauri::{Emitter, Manager, State};

use crate::database::DatabaseState;
use crate::error::{AppError, CommandError};
use crate::indexed_library::IndexRefreshStateV1;

use super::index::{IndexCoordinator, IndexError, RefreshObserver};

pub(crate) const INDEX_REFRESH_EVENT: &str = "index-refresh-state-v1";

/// Installs the process-wide coordinator after the database plugin has
/// completed migrations, then starts the first refresh without delaying the
/// initial indexed-library read.
pub(crate) fn initialize<R: tauri::Runtime>(app: &mut tauri::App<R>) -> Result<(), IndexError> {
    let repository = app
        .try_state::<DatabaseState>()
        .ok_or(IndexError::Storage)?
        .repository();
    let app_handle = app.handle().clone();
    let observer: RefreshObserver = Arc::new(move |state| {
        // A closed window must not turn an otherwise healthy index scan into a
        // database failure. The next state remains available through the
        // command result and the indexed library remains readable.
        let _ = app_handle.emit(INDEX_REFRESH_EVENT, state);
    });
    let coordinator = IndexCoordinator::new(repository, observer)?;
    register_and_start(coordinator, |coordinator| app.manage(coordinator))
}

pub(super) fn register_and_start(
    coordinator: IndexCoordinator,
    register: impl FnOnce(IndexCoordinator) -> bool,
) -> Result<(), IndexError> {
    let startup = coordinator.clone();
    if !register(coordinator) {
        return Err(IndexError::StateConflict);
    }
    tauri::async_runtime::spawn(async move {
        let _ = startup.refresh().await;
    });
    Ok(())
}

#[tauri::command]
pub(crate) async fn refresh_index(
    state: State<'_, IndexCoordinator>,
) -> Result<IndexRefreshStateV1, CommandError> {
    state
        .refresh()
        .await
        .map_err(|_| CommandError::from(AppError::Internal))
}

#[tauri::command]
pub(crate) async fn get_index_refresh_state(
    state: State<'_, IndexCoordinator>,
) -> Result<IndexRefreshStateV1, CommandError> {
    Ok(state.current_state().await)
}
