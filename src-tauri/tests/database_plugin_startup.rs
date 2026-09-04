use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use ai_session_replay_lib::database::{
    DATABASE_FILE_NAME, DatabaseBootstrapError, DatabaseState, initialize, plugin,
};
use sqlx::Row;
use tauri::Manager;

const STARTUP_CHILD_ENV: &str = "AI_SESSION_REPLAY_DATABASE_STARTUP_CHILD";
const STARTUP_APP_CONFIG_ENV: &str = "AI_SESSION_REPLAY_DATABASE_STARTUP_APP_CONFIG";
const INDEXED_RESTART_CHILD_ENV: &str = "AI_SESSION_REPLAY_INDEXED_RESTART_CHILD";

struct TemporaryAppConfig {
    path: PathBuf,
}

impl TemporaryAppConfig {
    fn new() -> Self {
        let path = std::env::temp_dir().join(temporary_directory_name());
        assert!(!path.exists());
        Self { path }
    }
}

impl Drop for TemporaryAppConfig {
    fn drop(&mut self) {
        if self.path.exists() {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn temporary_directory_name() -> String {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should follow the Unix epoch")
        .as_nanos();
    format!(
        "ai-session-replay-plugin-startup-{}-{nonce}",
        std::process::id()
    )
}

#[test]
fn plugin_startup_migrates_before_repository_initialization() {
    let app_config = TemporaryAppConfig::new();

    let output = Command::new(std::env::current_exe().expect("test executable should resolve"))
        .args([
            "--exact",
            "plugin_startup_child",
            "--ignored",
            "--nocapture",
        ])
        .env(STARTUP_CHILD_ENV, "1")
        .env(STARTUP_APP_CONFIG_ENV, &app_config.path)
        .output()
        .expect("isolated startup test should run");

    assert!(
        output.status.success(),
        "isolated startup failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let database_path = app_config.path.join(DATABASE_FILE_NAME);
    assert!(database_path.is_file());
}

#[test]
#[ignore = "invoked by the isolated startup parent test"]
fn plugin_startup_child() {
    if std::env::var_os(STARTUP_CHILD_ENV).is_none() {
        return;
    }

    let app_config_path = PathBuf::from(
        std::env::var_os(STARTUP_APP_CONFIG_ENV)
            .expect("isolated startup app-config path should be provided"),
    );
    let mut context = tauri::generate_context!();
    context.config_mut().identifier = app_config_path.display().to_string();

    let mut app = tauri::test::mock_builder()
        .plugin(plugin())
        .setup(|app| {
            initialize(app)?;
            Ok(())
        })
        .build(context)
        .expect("mock application should build");

    #[allow(deprecated)]
    app.run_iteration(|_, _| {});

    {
        let state = app.state::<DatabaseState>();
        tauri::async_runtime::block_on(async {
            let migrations =
                sqlx::query("SELECT version, success FROM _sqlx_migrations ORDER BY version")
                    .fetch_all(state.pool())
                    .await
                    .expect("preload migrations should complete before repository initialization");
            assert_eq!(
                migrations
                    .iter()
                    .map(|migration| (
                        migration.get::<i64, _>("version"),
                        migration.get::<i64, _>("success"),
                    ))
                    .collect::<Vec<_>>(),
                [(1, 1), (2, 1), (3, 1), (4, 1), (5, 1), (6, 1)],
            );
        });
    }

    assert_eq!(
        initialize(&mut app),
        Err(DatabaseBootstrapError::StateConflict)
    );

    let mut context_without_plugin = tauri::generate_context!();
    context_without_plugin.config_mut().identifier = app_config_path
        .join("missing-preload")
        .display()
        .to_string();
    let mut app_without_plugin = tauri::test::mock_builder()
        .build(context_without_plugin)
        .expect("mock application without SQL should build");
    assert_eq!(
        initialize(&mut app_without_plugin),
        Err(DatabaseBootstrapError::PreloadMissing)
    );
}

#[test]
fn indexed_detail_command_survives_desktop_restart_and_source_removal() {
    let app_config = TemporaryAppConfig::new();

    let output = Command::new(std::env::current_exe().expect("test executable should resolve"))
        .args([
            "--exact",
            "indexed_detail_restart_child",
            "--ignored",
            "--nocapture",
        ])
        .env(INDEXED_RESTART_CHILD_ENV, "1")
        .env(STARTUP_APP_CONFIG_ENV, &app_config.path)
        .output()
        .expect("isolated restart test should run");

    assert!(
        output.status.success(),
        "isolated restart test failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "invoked by the isolated restart parent test"]
fn indexed_detail_restart_child() {
    if std::env::var_os(INDEXED_RESTART_CHILD_ENV).is_none() {
        return;
    }

    let app_config_path = PathBuf::from(
        std::env::var_os(STARTUP_APP_CONFIG_ENV)
            .expect("isolated startup app-config path should be provided"),
    );
    let source_path = app_config_path.join("synthetic-source.jsonl");
    fs::create_dir_all(&app_config_path).expect("temporary app-config should be created");
    fs::write(&source_path, "synthetic source").expect("source fixture should be written");
    let source_path_utf16le = source_path
        .as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();

    let mut first_context = tauri::generate_context!();
    first_context.config_mut().identifier = app_config_path.display().to_string();
    let mut first_app = tauri::test::mock_builder()
        .plugin(plugin())
        .setup(|app| {
            initialize(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ai_session_replay_lib::commands::indexed_sessions::get_indexed_session
        ])
        .build(first_context)
        .expect("first mock desktop application should build");
    #[allow(deprecated)]
    first_app.run_iteration(|_, _| {});

    {
        let state = first_app.state::<DatabaseState>();
        tauri::async_runtime::block_on(async {
            let mut transaction = state
                .pool()
                .begin()
                .await
                .expect("fixture transaction should begin");
            sqlx::query(
                "INSERT INTO sessions
                    (id, source, vendor_session_id, title, created_at_ms, source_version,
                     collection, display_filename, current_revision_id, source_present,
                     first_indexed_at_ms, last_seen_at_ms)
                 VALUES (?, 'codex', ?, ?, 900, NULL, 'synthetic', 'desktop.jsonl',
                         NULL, 1, 1000, 1000)",
            )
            .bind("session_desktop")
            .bind("vendor-session-desktop")
            .bind("Desktop restart session")
            .execute(&mut *transaction)
            .await
            .expect("session fixture should persist");
            sqlx::query(
                "INSERT INTO session_revisions
                    (id, session_id, content_hash, indexed_at_ms, source_created_at_ms,
                     source_updated_at_ms, entry_count, duration_ms, diagnostic_count,
                     reasoning_availability, tool_detail_availability)
                 VALUES ('revision_desktop', 'session_desktop', ?, 1000, NULL, NULL,
                         1, 0, 0, 'unavailable', 'unavailable')",
            )
            .bind(vec![1_u8; 32])
            .execute(&mut *transaction)
            .await
            .expect("revision fixture should persist");
            sqlx::query(
                "UPDATE sessions SET current_revision_id = 'revision_desktop'
                 WHERE id = 'session_desktop'",
            )
            .execute(&mut *transaction)
            .await
            .expect("current revision should advance");
            sqlx::query(
                "INSERT INTO source_locations
                    (session_id, canonical_path_utf16le, source_identity_hash, root_identity,
                     file_identity, file_size_bytes, modified_at_ms, last_seen_generation)
                 VALUES ('session_desktop', ?, ?, ?, ?, 16, 1000, NULL)",
            )
            .bind(source_path_utf16le)
            .bind(vec![2_u8; 32])
            .bind(vec![3_u8])
            .bind(vec![4_u8])
            .execute(&mut *transaction)
            .await
            .expect("source fixture should persist");
            sqlx::query(
                "INSERT INTO revision_entries
                    (revision_id, entry_key, ordinal, at_ms, kind, body_text, tool_name,
                     tool_status, summary_text, tool_detail_availability, tool_arguments,
                     tool_result, display_path, source_type)
                 VALUES ('revision_desktop', 'entry_desktop', 0, 0, 'user', ?, NULL,
                         NULL, NULL, NULL, NULL, NULL, NULL, NULL)",
            )
            .bind("Synthetic persisted prompt")
            .execute(&mut *transaction)
            .await
            .expect("entry fixture should persist");
            sqlx::query(
                "INSERT INTO session_preferences
                    (session_id, show_tool_calls, show_tool_details, show_reasoning,
                     entry_delay_ms, playback_speed, background_color, surface_color,
                     text_color, muted_color, accent_color, success_color, error_color,
                     font_family, font_size_px, line_height, updated_at_ms)
                 VALUES ('session_desktop', 1, 1, 1, 1000, 1.0, '#101010', '#202020',
                         '#ffffff', '#999999', '#6699ff', '#22aa66', '#dd3344',
                         'JetBrains Mono', 24, 1.5, 1000)",
            )
            .execute(&mut *transaction)
            .await
            .expect("preferences fixture should persist");
            transaction
                .commit()
                .await
                .expect("fixture transaction should commit");
        });
    }
    drop(first_app);
    fs::remove_file(&source_path).expect("source fixture should be removed");

    let mut second_context = tauri::generate_context!();
    second_context.config_mut().identifier = app_config_path.display().to_string();
    let mut second_app = tauri::test::mock_builder()
        .plugin(plugin())
        .setup(|app| {
            initialize(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ai_session_replay_lib::commands::indexed_sessions::get_indexed_session
        ])
        .build(second_context)
        .expect("restarted mock desktop application should build");
    #[allow(deprecated)]
    second_app.run_iteration(|_, _| {});

    let state = second_app.state::<DatabaseState>();
    let detail = tauri::async_runtime::block_on(async {
        sqlx::query("UPDATE sessions SET source_present = 0 WHERE id = 'session_desktop'")
            .execute(state.pool())
            .await
            .expect("completed missing-source scan should update presence");
        ai_session_replay_lib::commands::indexed_sessions::get_indexed_session(
            "session_desktop".to_owned(),
            None,
            state,
        )
        .await
        .expect("indexed detail command should succeed")
    });

    assert_eq!(detail.summary.session_id, "session_desktop");
    assert!(!detail.summary.source_present);
    assert_eq!(detail.entry_page.entries.len(), 1);
    let serialized = serde_json::to_string(&detail).expect("detail should serialize");
    assert!(!serialized.contains("synthetic-source.jsonl"));
    assert!(!serialized.contains(&source_path.to_string_lossy().to_string()));
}
