pub mod adapters;
pub mod commands;
pub mod database;
pub mod deletion;
pub mod discovery;
pub mod error;
pub mod export;
pub mod indexed_library;
pub mod io;
pub mod model;

use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    version: &'static str,
    platform: &'static str,
}

#[tauri::command]
fn get_app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        platform: "windows-x64",
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(database::plugin())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            commands::runtime::cleanup_abandoned_exports(app.handle());
            database::initialize(app).inspect_err(|error| {
                eprintln!("AI Session Replay startup failed: {error}");
            })?;
            deletion::initialize(app).inspect_err(|error| {
                eprintln!("AI Session Replay startup failed: {error}");
            })?;
            discovery::index_runtime::initialize(app).inspect_err(|error| {
                eprintln!("AI Session Replay startup failed: {error}");
            })?;
            if let Some(job_id) = commands::runtime::configured_smoke_job() {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let exit_code = match commands::runtime::run_installed_runtime_smoke(
                        app_handle.clone(),
                        job_id,
                    )
                    .await
                    {
                        Ok(_) => 0,
                        Err(_) => 1,
                    };
                    app_handle.exit(exit_code);
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_app_info,
            commands::indexed_sessions::list_indexed_sessions,
            commands::indexed_sessions::get_indexed_session,
            commands::indexed_sessions::get_jetbrains_copilot_status,
            commands::indexed_sessions::delete_indexed_session,
            commands::indexed_sessions::prepare_source_deletion,
            commands::indexed_sessions::delete_indexed_session_with_source,
            commands::indexed_sessions::list_suppressed_sources,
            commands::indexed_sessions::restore_suppressed_source,
            commands::indexed_sessions::reset_local_database,
            commands::indexed_sessions::set_entry_selections,
            commands::indexed_sessions::rename_indexed_session,
            commands::indexed_sessions::set_session_preferences,
            discovery::index_runtime::get_index_refresh_state,
            discovery::index_runtime::refresh_index,
            commands::runtime::create_presentation_plan,
            commands::runtime::export_presentation,
            commands::runtime::cancel_export,
            commands::runtime::reveal_export
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|error| {
            panic!(
                "failed to run AI Session Replay: {}",
                safe_runtime_error_code(&error)
            )
        });
}

fn safe_runtime_error_code(error: &tauri::Error) -> &'static str {
    match error {
        tauri::Error::Runtime(_) => "TAURI_RUNTIME_FAILED",
        tauri::Error::Setup(_) => "SETUP_FAILED",
        tauri::Error::PluginInitialization(name, _) if name == "sql" => {
            "SQL_PLUGIN_INITIALIZATION_FAILED"
        }
        tauri::Error::PluginInitialization(name, _) if name == "dialog" => {
            "DIALOG_PLUGIN_INITIALIZATION_FAILED"
        }
        tauri::Error::PluginInitialization(_, _) => "PLUGIN_INITIALIZATION_FAILED",
        tauri::Error::Io(_) => "APPLICATION_IO_FAILED",
        tauri::Error::InvalidWebviewUrl(_) => "INVALID_WEBVIEW_URL",
        tauri::Error::InvalidWindowHandle => "INVALID_WINDOW_HANDLE",
        tauri::Error::WebviewNotFound => "WEBVIEW_NOT_FOUND",
        _ => "APPLICATION_RUNTIME_FAILED",
    }
}

#[cfg(test)]
mod tests {
    use super::{get_app_info, safe_runtime_error_code};

    #[test]
    fn reports_the_supported_platform() {
        let info = get_app_info();

        assert_eq!(info.platform, "windows-x64");
        assert!(!info.version.is_empty());
    }

    #[test]
    fn startup_errors_are_reduced_to_path_free_codes() {
        assert_eq!(
            safe_runtime_error_code(&tauri::Error::PluginInitialization(
                "sql".to_owned(),
                "C:\\private\\database failure".to_owned(),
            )),
            "SQL_PLUGIN_INITIALIZATION_FAILED"
        );
        assert_eq!(
            safe_runtime_error_code(&tauri::Error::Io(std::io::Error::other("private details",))),
            "APPLICATION_IO_FAILED"
        );
    }
}

#[cfg(test)]
mod integration_tests;

#[cfg(test)]
mod runtime_integration_tests;
