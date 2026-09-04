use tauri::Manager;

use super::{
    DATABASE_FILE_NAME, DATABASE_URL, DatabaseBootstrapError, plugin_migrations,
    prepare_repository_state,
};

/// Registers migrations for the same URL preloaded in `tauri.conf.json`.
/// Source: https://v2.tauri.app/plugin/sql/#migrations
pub fn plugin<R: tauri::Runtime>()
-> tauri::plugin::TauriPlugin<R, Option<tauri_plugin_sql::PluginConfig>> {
    tauri_plugin_sql::Builder::default()
        .add_migrations(DATABASE_URL, plugin_migrations())
        .build()
}

/// Adopts the path selected by the plugin and replaces its default pool before
/// commands can run. The pinned plugin resolves SQLite URLs under app config.
/// Source: https://github.com/tauri-apps/plugins-workspace/blob/sql-v2.4.1/plugins/sql/src/wrapper.rs
pub fn initialize<R: tauri::Runtime>(
    app: &mut tauri::App<R>,
) -> Result<(), DatabaseBootstrapError> {
    let database_path = app
        .path()
        .app_config_dir()
        .map_err(|_| DatabaseBootstrapError::PathUnavailable)?
        .join(DATABASE_FILE_NAME);
    let instances = app
        .try_state::<tauri_plugin_sql::DbInstances>()
        .ok_or(DatabaseBootstrapError::PreloadMissing)?;
    let state =
        tauri::async_runtime::block_on(prepare_repository_state(&instances, &database_path))?;

    if app.manage(state) {
        Ok(())
    } else {
        Err(DatabaseBootstrapError::StateConflict)
    }
}
