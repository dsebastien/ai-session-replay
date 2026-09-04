use std::collections::{HashMap, VecDeque};
use std::fs;
use std::fs::OpenOptions;
use std::io::{Read, Seek, Write};
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager, State};
use windows_sys::Win32::System::SystemInformation::GetWindowsDirectoryW;

use crate::database::DatabaseState;
use crate::database::repository::{IndexedSessionRepository, RepositoryError};
use crate::discovery::roots::ProductionRootProvider;
use crate::error::{AppError, CommandError};
use crate::indexed_library::{
    ContractValidate, INDEXED_LIBRARY_SCHEMA_VERSION, PresentationEntryContentV1,
    PresentationPlanEntryV1, PresentationPlanV1, PresentedToolDetailV1, SessionEntryV1,
    SessionPreferencesV1, ToolDetailV1,
};
#[cfg(test)]
use crate::io::validate_new_local_file;
use crate::io::{LocalRoot, lock_new_local_file_destination, volume_supports_atomic_links};

use crate::export::runtime::{
    MediaSummary, RuntimeError, RuntimePaths, VerifiedMediaSummary, run_worker_process,
    run_worker_process_cancellable_with_progress, valid_job_id,
};

pub(crate) const RUNTIME_SMOKE_JOB_ENV: &str = "AI_SESSION_REPLAY_RUNTIME_SMOKE_JOB";
static EXPORT_CANCELLATIONS: LazyLock<Mutex<HashMap<String, Arc<AtomicBool>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static COMPLETED_EXPORTS: LazyLock<Mutex<CompletedExports>> =
    LazyLock::new(|| Mutex::new(CompletedExports::default()));
static PRESENTATION_PLANS: LazyLock<Mutex<FrozenPlans>> =
    LazyLock::new(|| Mutex::new(FrozenPlans::default()));
const MAX_COMPLETED_EXPORTS: usize = 32;
const MAX_FROZEN_PLANS: usize = 2;
const MAX_PRESENTATION_TEXT_BYTES: usize = 48 * 1_024 * 1_024;
const MAX_SERIALIZED_PLAN_BYTES: usize = 60 * 1_024 * 1_024;
const EXPORT_PROGRESS_EVENT: &str = "export-progress-v1";

#[link(name = "advapi32")]
unsafe extern "system" {
    #[link_name = "SystemFunction036"]
    fn rtl_gen_random(buffer: *mut u8, length: u32) -> u8;
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportProgressV1 {
    schema_version: u8,
    job_id: String,
    rendered_frames: u32,
    total_frames: u32,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExportCleanupMarker {
    schema_version: u8,
    staging_path: String,
}

#[derive(Default)]
struct CompletedExports {
    paths: HashMap<String, PathBuf>,
    order: VecDeque<String>,
}

#[derive(Default)]
struct FrozenPlans {
    plans: HashMap<String, Arc<PresentationPlanV1>>,
    order: VecDeque<String>,
    epoch: u64,
}

struct ExportExecution<'a> {
    resource_directory: &'a Path,
    executable_directory: &'a Path,
    output_path: &'a Path,
    staging_path: &'a Path,
    job_id: &'a str,
    plan: &'a PresentationPlanV1,
}

#[derive(Debug)]
struct ExportDestination {
    root: LocalRoot,
    output_path: PathBuf,
    staging_path: PathBuf,
}

#[derive(Debug)]
struct CleanupRegistration {
    root: LocalRoot,
    marker_path: PathBuf,
}

pub(crate) fn configured_smoke_job() -> Option<String> {
    std::env::var(RUNTIME_SMOKE_JOB_ENV)
        .ok()
        .filter(|job_id| valid_job_id(job_id))
}

pub(crate) async fn run_installed_runtime_smoke(
    app: AppHandle,
    job_id: String,
) -> Result<MediaSummary, RuntimeError> {
    let resource_directory = app
        .path()
        .resource_dir()
        .map_err(|_| RuntimeError::InvalidRuntime)?;
    let executable = std::env::current_exe().map_err(|_| RuntimeError::InvalidRuntime)?;
    let executable_directory = executable
        .parent()
        .ok_or(RuntimeError::InvalidRuntime)?
        .to_path_buf();
    let output_directory = smoke_output_directory();

    tauri::async_runtime::spawn_blocking(move || {
        run_runtime_smoke_blocking(
            &resource_directory,
            &executable_directory,
            &output_directory,
            &job_id,
        )
    })
    .await
    .map_err(|_| RuntimeError::Process)?
}

#[tauri::command]
pub(crate) async fn create_presentation_plan(
    session_id: String,
    revision_id: String,
    state: State<'_, DatabaseState>,
) -> Result<PresentationPlanV1, CommandError> {
    if !valid_opaque_identifier(&session_id) || !valid_opaque_identifier(&revision_id) {
        return Err(CommandError {
            code: "INVALID_REQUEST",
            message: "The presentation request is invalid".to_owned(),
        });
    }
    let created_at_ms = now_ms();
    let plan_epoch = presentation_plan_epoch()?;
    let plan_id = random_identifier("plan").map_err(|_| CommandError::from(AppError::Internal))?;
    let plan = build_presentation_plan(
        &state.repository(),
        &session_id,
        &revision_id,
        plan_id,
        created_at_ms,
    )
    .await?;
    remember_presentation_plan(plan.clone(), plan_epoch)?;
    Ok(plan)
}

#[tauri::command]
pub(crate) async fn export_presentation(
    app: AppHandle,
    plan_id: String,
    output_path: String,
    job_id: String,
) -> Result<MediaSummary, CommandError> {
    let plan = frozen_presentation_plan(&plan_id)?;
    let protected_roots = ProductionRootProvider::from_environment().configured_data_roots();
    let destination = prepare_export_paths(Path::new(&output_path), &protected_roots)?;
    let output_path = destination.output_path.clone();
    let staging_path = destination.staging_path.clone();
    let resource_directory = app
        .path()
        .resource_dir()
        .map_err(|_| CommandError::from(AppError::Internal))?;
    let executable = std::env::current_exe().map_err(|_| CommandError::from(AppError::Internal))?;
    let executable_directory = executable
        .parent()
        .ok_or_else(|| CommandError::from(AppError::Internal))?
        .to_path_buf();
    let app_config_directory = app
        .path()
        .app_config_dir()
        .map_err(|_| CommandError::from(AppError::Internal))?;
    if !valid_job_id(&job_id) {
        return Err(CommandError {
            code: "INVALID_REQUEST",
            message: "The export request is invalid".to_owned(),
        });
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    {
        let mut jobs = EXPORT_CANCELLATIONS
            .lock()
            .map_err(|_| CommandError::from(AppError::Internal))?;
        if jobs.len() >= 8 || jobs.contains_key(&job_id) {
            return Err(CommandError {
                code: "EXPORT_BUSY",
                message: "The export queue is busy".to_owned(),
            });
        }
        jobs.insert(job_id.clone(), Arc::clone(&cancelled));
    }
    let cleanup_registration =
        match register_staging_cleanup(&app_config_directory, &job_id, &staging_path) {
            Ok(marker) => marker,
            Err(error) => {
                if let Ok(mut jobs) = EXPORT_CANCELLATIONS.lock() {
                    jobs.remove(&job_id);
                }
                return Err(error);
            }
        };
    let worker_job_id = job_id.clone();
    let completed_output_path = output_path.clone();
    let cleanup_staging_path = staging_path.clone();
    let worker_output_path = output_path.clone();
    let worker_staging_path = staging_path.clone();
    let process_cancelled = Arc::clone(&cancelled);
    let progress_app = app.clone();
    let progress_job_id = job_id.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        run_export_blocking_with_progress(
            ExportExecution {
                resource_directory: &resource_directory,
                executable_directory: &executable_directory,
                output_path: &worker_output_path,
                staging_path: &worker_staging_path,
                job_id: &worker_job_id,
                plan: &plan,
            },
            &process_cancelled,
            Arc::new(move |rendered_frames, total_frames| {
                let _ = progress_app.emit(
                    EXPORT_PROGRESS_EVENT,
                    ExportProgressV1 {
                        schema_version: 1,
                        job_id: progress_job_id.clone(),
                        rendered_frames,
                        total_frames,
                    },
                );
            }),
        )
    })
    .await;
    let verified = match joined {
        Err(_) => {
            unregister_export(&job_id);
            cleanup_staging_registration(
                &cleanup_staging_path,
                &cleanup_registration,
                &protected_roots,
            );
            return Err(CommandError::from(AppError::Internal));
        }
        Ok(Err(error)) => {
            unregister_export(&job_id);
            cleanup_staging_registration(
                &cleanup_staging_path,
                &cleanup_registration,
                &protected_roots,
            );
            return Err(export_runtime_error(error));
        }
        Ok(Ok(verified)) => verified,
    };
    if cancelled.load(Ordering::Acquire) {
        unregister_export(&job_id);
        cleanup_staging_registration(
            &cleanup_staging_path,
            &cleanup_registration,
            &protected_roots,
        );
        return Err(CommandError {
            code: "EXPORT_CANCELLED",
            message: "The export was cancelled".to_owned(),
        });
    }
    let publication_job_id = job_id.clone();
    let publication_cancelled = Arc::clone(&cancelled);
    let publication_staging_path = cleanup_staging_path.clone();
    let publication_output_path = completed_output_path.clone();
    let staging_sha256 = verified.staging_sha256;
    let publication = tauri::async_runtime::spawn_blocking(move || {
        publish_verified_staging(
            &destination.root,
            &publication_staging_path,
            &publication_output_path,
            staging_sha256,
            Some((&publication_job_id, &publication_cancelled)),
        )
    })
    .await;
    let publication_result = publication.unwrap_or(Err(RuntimeError::Process));
    if let Err(error) = publication_result {
        unregister_export(&job_id);
        cleanup_staging_registration(
            &cleanup_staging_path,
            &cleanup_registration,
            &protected_roots,
        );
        return Err(export_runtime_error(error));
    }
    cleanup_staging_registration(
        &cleanup_staging_path,
        &cleanup_registration,
        &protected_roots,
    );
    remember_completed_export(job_id, completed_output_path);
    Ok(verified.media)
}

fn export_runtime_error(error: RuntimeError) -> CommandError {
    let (code, message) = match error {
        RuntimeError::Cancelled => ("EXPORT_CANCELLED", "The export was cancelled"),
        RuntimeError::InvalidRuntime
        | RuntimeError::Worker(crate::export::runtime::WorkerFailureCode::InvalidRuntime) => (
            "EXPORT_RUNTIME_UNAVAILABLE",
            "The installed export runtime is unavailable",
        ),
        RuntimeError::Worker(crate::export::runtime::WorkerFailureCode::OutputNotAvailable) => (
            "EXPORT_OUTPUT_UNAVAILABLE",
            "The export destination is unavailable",
        ),
        RuntimeError::Worker(crate::export::runtime::WorkerFailureCode::RenderFailed) => {
            ("EXPORT_RENDER_FAILED", "The video renderer failed")
        }
        RuntimeError::Worker(crate::export::runtime::WorkerFailureCode::MediaAssertionFailed) => (
            "EXPORT_VERIFICATION_FAILED",
            "The rendered video failed verification",
        ),
        RuntimeError::Process => (
            "EXPORT_PROCESS_FAILED",
            "The export process could not start or finish",
        ),
        RuntimeError::Protocol | RuntimeError::OutputLimit => (
            "EXPORT_PROTOCOL_FAILED",
            "The export worker returned an invalid response",
        ),
        RuntimeError::Worker(crate::export::runtime::WorkerFailureCode::RequestLimitExceeded) => (
            "EXPORT_CONTENT_TOO_LARGE",
            "The selected conversation is too large to export",
        ),
        RuntimeError::Worker(crate::export::runtime::WorkerFailureCode::InvalidRequest) => (
            "EXPORT_WORKER_REQUEST_FAILED",
            "The export worker rejected the request",
        ),
        RuntimeError::Worker(crate::export::runtime::WorkerFailureCode::ProtocolLimitExceeded) => (
            "EXPORT_PROTOCOL_FAILED",
            "The export worker exceeded its protocol limits",
        ),
        RuntimeError::Worker(crate::export::runtime::WorkerFailureCode::WorkerInternal) => {
            ("EXPORT_WORKER_FAILED", "The export worker failed")
        }
    };
    CommandError {
        code,
        message: message.to_owned(),
    }
}

fn unregister_export(job_id: &str) {
    if let Ok(mut jobs) = EXPORT_CANCELLATIONS.lock() {
        jobs.remove(job_id);
    }
}

fn cleanup_staging_registration(
    staging_path: &Path,
    registration: &CleanupRegistration,
    protected_roots: &[PathBuf],
) {
    delete_safe_staging_path(staging_path, protected_roots);
    if matches!(staging_path.try_exists(), Ok(false))
        && let Ok(marker) = registration
            .root
            .open_file_for_deletion(&registration.marker_path)
    {
        let _ = marker.delete();
    }
}

fn publish_verified_staging(
    root: &LocalRoot,
    staging_path: &Path,
    output_path: &Path,
    expected_sha256: [u8; 32],
    cancellation: Option<(&str, &Arc<AtomicBool>)>,
) -> Result<(), RuntimeError> {
    if staging_path.parent() != output_path.parent()
        || output_path
            .try_exists()
            .map_err(|_| RuntimeError::Process)?
    {
        return Err(RuntimeError::Protocol);
    }
    root.revalidate().map_err(|_| RuntimeError::Process)?;
    let mut staging = root
        .open_file_for_publication(staging_path)
        .map_err(|_| RuntimeError::Process)?;
    let actual_sha256 = sha256_reader(staging.file_mut())?;
    staging.revalidate(1).map_err(|_| RuntimeError::Process)?;
    if actual_sha256 != expected_sha256 {
        return Err(RuntimeError::Protocol);
    }
    let mut jobs = if let Some((job_id, cancelled)) = cancellation {
        let mut jobs = EXPORT_CANCELLATIONS
            .lock()
            .map_err(|_| RuntimeError::Process)?;
        if cancelled.load(Ordering::Acquire)
            || !jobs
                .get(job_id)
                .is_some_and(|registered| Arc::ptr_eq(registered, cancelled))
        {
            jobs.remove(job_id);
            return Err(RuntimeError::Cancelled);
        }
        Some((jobs, job_id))
    } else {
        None
    };
    fs::hard_link(staging.canonical_path(), output_path).map_err(|_| RuntimeError::Process)?;
    if let Some((jobs, job_id)) = &mut jobs {
        jobs.remove(*job_id);
    }
    let _ = staging.delete_link();
    Ok(())
}

fn sha256_reader(file: &mut fs::File) -> Result<[u8; 32], RuntimeError> {
    file.rewind().map_err(|_| RuntimeError::Process)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1_024];
    loop {
        let read = file.read(&mut buffer).map_err(|_| RuntimeError::Process)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(digest.finalize().into())
}

pub(crate) fn cleanup_abandoned_exports(app: &AppHandle) {
    let Ok(app_config_directory) = app.path().app_config_dir() else {
        return;
    };
    cleanup_abandoned_exports_in(
        &app_config_directory,
        &ProductionRootProvider::from_environment().configured_data_roots(),
    );
}

fn cleanup_abandoned_exports_in(app_config_directory: &Path, protected_roots: &[PathBuf]) {
    let marker_directory = app_config_directory.join("export-cleanup");
    let Ok(app_root) = LocalRoot::new(app_config_directory) else {
        return;
    };
    let Ok(marker_root) = LocalRoot::new(&marker_directory) else {
        return;
    };
    if !app_root.contains_canonical_path(marker_root.canonical_path()) {
        return;
    }
    let Ok(entries) = fs::read_dir(marker_root.canonical_path()) else {
        return;
    };
    for entry in entries.take(128).flatten() {
        let marker_path = entry.path();
        let file_name = marker_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if !file_name.starts_with("export-") || !file_name.ends_with(".json") {
            continue;
        }
        let marker = marker_root
            .open_file(&marker_path)
            .ok()
            .and_then(|opened| {
                let mut bytes = Vec::new();
                opened.file.take(65_537).read_to_end(&mut bytes).ok()?;
                (bytes.len() <= 65_536).then_some(bytes)
            })
            .and_then(|bytes| serde_json::from_slice::<ExportCleanupMarker>(&bytes).ok());
        let remove_marker = if let Some(marker) = marker
            && marker.schema_version == 1
        {
            let staging_path = PathBuf::from(marker.staging_path);
            delete_safe_staging_path(&staging_path, protected_roots)
                || matches!(staging_path.try_exists(), Ok(false))
        } else {
            true
        };
        if remove_marker && let Ok(marker) = marker_root.open_file_for_deletion(&marker_path) {
            let _ = marker.delete();
        }
    }
}

fn register_staging_cleanup(
    app_config_directory: &Path,
    job_id: &str,
    staging_path: &Path,
) -> Result<CleanupRegistration, CommandError> {
    let staging_path = staging_path
        .to_str()
        .ok_or_else(|| CommandError::from(AppError::Internal))?;
    let marker_directory = app_config_directory.join("export-cleanup");
    fs::create_dir_all(&marker_directory).map_err(|_| CommandError::from(AppError::Internal))?;
    let app_root =
        LocalRoot::new(app_config_directory).map_err(|_| CommandError::from(AppError::Internal))?;
    let marker_root = LocalRoot::new_locked(&marker_directory)
        .map_err(|_| CommandError::from(AppError::Internal))?;
    if !app_root.contains_canonical_path(marker_root.canonical_path()) {
        return Err(CommandError::from(AppError::Internal));
    }
    let marker_path = marker_root
        .canonical_path()
        .join(format!("export-{job_id}.json"));
    let marker = serde_json::to_vec(&ExportCleanupMarker {
        schema_version: 1,
        staging_path: staging_path.to_owned(),
    })
    .map_err(|_| CommandError::from(AppError::Internal))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker_path)
        .map_err(|_| CommandError::from(AppError::Internal))?;
    file.write_all(&marker)
        .and_then(|_| file.sync_all())
        .map_err(|_| CommandError::from(AppError::Internal))?;
    Ok(CleanupRegistration {
        root: marker_root,
        marker_path,
    })
}

fn safe_staging_path(path: &Path, protected_roots: &[PathBuf]) -> bool {
    let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let suffix = file_name
        .strip_prefix(".ai-session-replay-")
        .and_then(|value| value.strip_suffix(".partial.mp4"));
    if !suffix.is_some_and(|value| {
        value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
    }) {
        return false;
    }
    let Some(parent) = path.parent() else {
        return false;
    };
    let Ok(parent_root) = LocalRoot::new(parent) else {
        return false;
    };
    if parent_root.canonical_path().join(file_name) != path {
        return false;
    }
    !protected_roots.iter().any(|configured_root| {
        LocalRoot::new(configured_root).is_ok_and(|root| root.contains_canonical_path(path))
    })
}

fn delete_safe_staging_path(path: &Path, protected_roots: &[PathBuf]) -> bool {
    if !safe_staging_path(path, protected_roots) {
        return false;
    }
    let Some(parent) = path.parent() else {
        return false;
    };
    let Ok(parent_root) = LocalRoot::new(parent) else {
        return false;
    };
    parent_root
        .open_staging_link_for_deletion(path)
        .and_then(|target| target.delete())
        .is_ok()
}

#[tauri::command]
pub(crate) fn cancel_export(job_id: String) -> Result<(), CommandError> {
    if !valid_job_id(&job_id) {
        return Err(CommandError {
            code: "INVALID_REQUEST",
            message: "The export request is invalid".to_owned(),
        });
    }
    let jobs = EXPORT_CANCELLATIONS
        .lock()
        .map_err(|_| CommandError::from(AppError::Internal))?;
    if let Some(cancelled) = jobs.get(&job_id) {
        cancelled.store(true, Ordering::Release);
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn reveal_export(job_id: String) -> Result<(), CommandError> {
    let output_path = resolve_completed_export(&job_id)?;
    Command::new(trusted_explorer_path()?)
        .arg("/select,")
        .arg(output_path)
        .spawn()
        .map_err(|_| CommandError {
            code: "EXPORT_REVEAL_FAILED",
            message: "The exported video could not be shown".to_owned(),
        })?;
    Ok(())
}

fn trusted_explorer_path() -> Result<PathBuf, CommandError> {
    let mut buffer = vec![0_u16; 32_768];
    // SAFETY: the writable buffer and its length are valid for this OS call.
    let length = unsafe { GetWindowsDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) };
    if length == 0 || length as usize >= buffer.len() {
        return Err(CommandError::from(AppError::Internal));
    }
    let windows = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length as usize]));
    let explorer = windows.join("explorer.exe");
    if !explorer.is_file() {
        return Err(CommandError::from(AppError::Internal));
    }
    Ok(explorer)
}

fn remember_completed_export(job_id: String, output_path: PathBuf) {
    let Ok(mut exports) = COMPLETED_EXPORTS.lock() else {
        return;
    };
    if exports.paths.insert(job_id.clone(), output_path).is_none() {
        exports.order.push_back(job_id);
    }
    while exports.paths.len() > MAX_COMPLETED_EXPORTS {
        if let Some(expired) = exports.order.pop_front() {
            exports.paths.remove(&expired);
        }
    }
}

fn remember_presentation_plan(
    plan: PresentationPlanV1,
    expected_epoch: u64,
) -> Result<(), CommandError> {
    let mut plans = PRESENTATION_PLANS
        .lock()
        .map_err(|_| CommandError::from(AppError::Internal))?;
    if plans.epoch != expected_epoch {
        return Err(CommandError {
            code: "STALE_REVISION",
            message: "The indexed session changed; reload it and try again".to_owned(),
        });
    }
    let plan_id = plan.plan_id.clone();
    if plans
        .plans
        .insert(plan_id.clone(), Arc::new(plan))
        .is_none()
    {
        plans.order.push_back(plan_id);
    }
    while plans.plans.len() > MAX_FROZEN_PLANS {
        if let Some(expired) = plans.order.pop_front() {
            plans.plans.remove(&expired);
        }
    }
    Ok(())
}

pub(crate) fn clear_frozen_plans() {
    if let Ok(mut plans) = PRESENTATION_PLANS.lock() {
        plans.epoch = plans.epoch.wrapping_add(1);
        plans.plans.clear();
        plans.order.clear();
    }
}

pub(crate) fn forget_presentation_plans_for_session(session_id: &str) {
    if let Ok(mut plans) = PRESENTATION_PLANS.lock() {
        plans.epoch = plans.epoch.wrapping_add(1);
        plans.plans.retain(|_, plan| plan.session_id != session_id);
        let retained = plans
            .plans
            .keys()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        plans.order.retain(|plan_id| retained.contains(plan_id));
    }
}

fn presentation_plan_epoch() -> Result<u64, CommandError> {
    PRESENTATION_PLANS
        .lock()
        .map(|plans| plans.epoch)
        .map_err(|_| CommandError::from(AppError::Internal))
}

fn frozen_presentation_plan(plan_id: &str) -> Result<Arc<PresentationPlanV1>, CommandError> {
    if !valid_job_id(plan_id) {
        return Err(invalid_presentation());
    }
    PRESENTATION_PLANS
        .lock()
        .map_err(|_| CommandError::from(AppError::Internal))?
        .plans
        .get(plan_id)
        .cloned()
        .ok_or_else(invalid_presentation)
}

pub(crate) async fn build_presentation_plan(
    repository: &IndexedSessionRepository,
    session_id: &str,
    expected_revision_id: &str,
    plan_id: String,
    created_at_ms: u64,
) -> Result<PresentationPlanV1, CommandError> {
    let source = repository
        .get_presentation_source(session_id, expected_revision_id)
        .await
        .map_err(presentation_repository_error)?;
    let preferences = source.preferences;
    let mut entries = Vec::new();
    let mut text_bytes = 0usize;
    for selectable in source.entries {
        if !selectable.selected || !entry_is_visible(&selectable.entry, &preferences) {
            continue;
        }
        let entry = project_entry(selectable.entry, preferences.visibility.show_tool_details);
        text_bytes = text_bytes
            .checked_add(presentation_entry_text_bytes(&entry))
            .ok_or_else(invalid_presentation)?;
        if text_bytes > MAX_PRESENTATION_TEXT_BYTES {
            return Err(CommandError {
                code: "PRESENTATION_TOO_LARGE",
                message: "The presentation is too large to export".to_owned(),
            });
        }
        let reveal_at_ms = ((entries.len() as f64 * preferences.timing.entry_delay_ms as f64)
            / preferences.timing.playback_speed)
            .round() as u64;
        entries.push(PresentationPlanEntryV1 {
            entry,
            reveal_at_ms,
        });
    }
    let duration_ms = ((entries.len() as f64 * preferences.timing.entry_delay_ms as f64)
        / preferences.timing.playback_speed)
        .round() as u64;
    let plan = PresentationPlanV1 {
        schema_version: INDEXED_LIBRARY_SCHEMA_VERSION,
        plan_id,
        session_id: session_id.to_owned(),
        revision_id: source.revision_id,
        session_title: source.title,
        created_at_ms,
        entries,
        preferences,
        duration_ms,
        fps: 30,
        width: 1_920,
        height: 1_080,
    };
    plan.validate().map_err(|error| CommandError {
        code: error.code,
        message: error.message.to_owned(),
    })?;
    if serde_json::to_vec(&plan)
        .map_err(|_| invalid_presentation())?
        .len()
        > MAX_SERIALIZED_PLAN_BYTES
    {
        return Err(CommandError {
            code: "PRESENTATION_TOO_LARGE",
            message: "The presentation is too large to export".to_owned(),
        });
    }
    Ok(plan)
}

fn entry_is_visible(entry: &SessionEntryV1, preferences: &SessionPreferencesV1) -> bool {
    match entry {
        SessionEntryV1::Reasoning { .. } => preferences.visibility.show_reasoning,
        SessionEntryV1::ToolCall { .. } => preferences.visibility.show_tool_calls,
        _ => true,
    }
}

fn project_entry(entry: SessionEntryV1, show_tool_details: bool) -> PresentationEntryContentV1 {
    match entry {
        SessionEntryV1::User {
            entry_key,
            ordinal,
            at_ms,
            text,
        } => PresentationEntryContentV1::User {
            entry_key,
            ordinal,
            at_ms,
            text,
        },
        SessionEntryV1::Assistant {
            entry_key,
            ordinal,
            at_ms,
            markdown,
        } => PresentationEntryContentV1::Assistant {
            entry_key,
            ordinal,
            at_ms,
            markdown,
        },
        SessionEntryV1::Reasoning {
            entry_key,
            ordinal,
            at_ms,
            text,
        } => PresentationEntryContentV1::Reasoning {
            entry_key,
            ordinal,
            at_ms,
            text,
        },
        SessionEntryV1::ToolCall {
            entry_key,
            ordinal,
            at_ms,
            name,
            status,
            summary,
            detail,
        } => PresentationEntryContentV1::ToolCall {
            entry_key,
            ordinal,
            at_ms,
            name,
            status,
            summary,
            detail: match detail {
                ToolDetailV1::Available { arguments, result } if show_tool_details => {
                    Some(PresentedToolDetailV1 { arguments, result })
                }
                ToolDetailV1::Unavailable | ToolDetailV1::Available { .. } => None,
            },
        },
        SessionEntryV1::FileChange {
            entry_key,
            ordinal,
            at_ms,
            display_path,
            summary,
        } => PresentationEntryContentV1::FileChange {
            entry_key,
            ordinal,
            at_ms,
            display_path,
            summary,
        },
        SessionEntryV1::Unknown {
            entry_key,
            ordinal,
            at_ms,
            source_type,
        } => PresentationEntryContentV1::Unknown {
            entry_key,
            ordinal,
            at_ms,
            source_type,
        },
    }
}

fn presentation_entry_text_bytes(entry: &PresentationEntryContentV1) -> usize {
    match entry {
        PresentationEntryContentV1::User { text, .. }
        | PresentationEntryContentV1::Reasoning { text, .. } => text.len(),
        PresentationEntryContentV1::Assistant { markdown, .. } => markdown.len(),
        PresentationEntryContentV1::ToolCall {
            name,
            summary,
            detail,
            ..
        } => {
            name.len()
                + summary.len()
                + detail.as_ref().map_or(0, |detail| {
                    detail.arguments.as_ref().map_or(0, String::len)
                        + detail.result.as_ref().map_or(0, String::len)
                })
        }
        PresentationEntryContentV1::FileChange {
            display_path,
            summary,
            ..
        } => display_path.len() + summary.len(),
        PresentationEntryContentV1::Unknown { source_type, .. } => source_type.len(),
    }
}

fn presentation_repository_error(error: RepositoryError) -> CommandError {
    match error {
        RepositoryError::InvalidInput => invalid_presentation(),
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

fn invalid_presentation() -> CommandError {
    CommandError {
        code: "PRESENTATION_UNAVAILABLE",
        message: "The presentation could not be created".to_owned(),
    }
}

fn valid_opaque_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
        .min(9_007_199_254_740_991)
}

fn random_identifier(prefix: &str) -> Result<String, ()> {
    let mut random = [0_u8; 16];
    // SAFETY: the buffer is writable for exactly the length passed to the OS.
    if unsafe { rtl_gen_random(random.as_mut_ptr(), random.len() as u32) } == 0 {
        return Err(());
    }
    let mut value = String::with_capacity(prefix.len() + 1 + random.len() * 2);
    value.push_str(prefix);
    value.push('_');
    use std::fmt::Write as _;
    for byte in random {
        write!(&mut value, "{byte:02x}").map_err(|_| ())?;
    }
    Ok(value)
}

fn resolve_completed_export(job_id: &str) -> Result<PathBuf, CommandError> {
    if !valid_job_id(job_id) {
        return Err(CommandError {
            code: "INVALID_REQUEST",
            message: "The export request is invalid".to_owned(),
        });
    }
    let output_path = COMPLETED_EXPORTS
        .lock()
        .map_err(|_| CommandError::from(AppError::Internal))?
        .paths
        .get(job_id)
        .cloned()
        .ok_or_else(|| CommandError {
            code: "EXPORT_NOT_FOUND",
            message: "The completed export is no longer available".to_owned(),
        })?;
    let canonical = fs::canonicalize(output_path).map_err(|_| CommandError {
        code: "EXPORT_NOT_FOUND",
        message: "The completed export is no longer available".to_owned(),
    })?;
    let valid = canonical.is_file()
        && canonical
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("mp4"));
    if !valid {
        return Err(CommandError {
            code: "EXPORT_NOT_FOUND",
            message: "The completed export is no longer available".to_owned(),
        });
    }
    Ok(canonical)
}

#[cfg(test)]
fn run_export_blocking(
    resource_directory: &Path,
    executable_directory: &Path,
    output_path: &Path,
    job_id: &str,
    plan: &PresentationPlanV1,
    cancelled: &AtomicBool,
) -> Result<MediaSummary, RuntimeError> {
    let output_path = validate_new_local_file(output_path).map_err(|_| RuntimeError::Protocol)?;
    let staging_path = export_staging_path(&output_path).map_err(|_| RuntimeError::Protocol)?;
    run_export_blocking_with_progress(
        ExportExecution {
            resource_directory,
            executable_directory,
            output_path: &output_path,
            staging_path: &staging_path,
            job_id,
            plan,
        },
        cancelled,
        Arc::new(|_, _| {}),
    )
    .map(|verified| verified.media)
}

fn run_export_blocking_with_progress(
    execution: ExportExecution<'_>,
    cancelled: &AtomicBool,
    on_progress: Arc<dyn Fn(u32, u32) + Send + Sync>,
) -> Result<VerifiedMediaSummary, RuntimeError> {
    if !valid_job_id(execution.job_id)
        || execution.output_path.exists()
        || execution.staging_path.exists()
    {
        return Err(RuntimeError::Protocol);
    }
    if execution.output_path.parent() != execution.staging_path.parent()
        || !execution
            .output_path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("mp4"))
    {
        return Err(RuntimeError::Protocol);
    }
    let paths =
        RuntimePaths::resolve(execution.resource_directory, execution.executable_directory)?;
    let value = serde_json::json!({
        "schemaVersion": 1,
        "type": "render",
        "jobId": execution.job_id,
        "outputPath": execution.output_path,
        "stagingPath": execution.staging_path,
        "plan": execution.plan,
    });
    let mut request = serde_json::to_vec(&value).map_err(|_| RuntimeError::Protocol)?;
    request.push(b'\n');
    run_worker_process_cancellable_with_progress(
        &paths.worker_launch_spec(),
        &request,
        execution.job_id,
        cancelled,
        on_progress,
    )
}

fn prepare_export_paths(
    requested_output: &Path,
    protected_roots: &[PathBuf],
) -> Result<ExportDestination, CommandError> {
    if !requested_output
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mp4"))
    {
        return Err(export_destination_error(
            "EXPORT_OUTPUT_FORMAT",
            "The export destination must be an MP4 file",
        ));
    }
    match requested_output.try_exists() {
        Ok(true) => {
            return Err(export_destination_error(
                "EXPORT_OUTPUT_EXISTS",
                "The export destination already exists",
            ));
        }
        Ok(false) => {}
        Err(_) => return Err(invalid_export_destination()),
    }
    let (output_root, output_path) =
        lock_new_local_file_destination(requested_output).map_err(|_| {
            export_destination_error(
                "EXPORT_OUTPUT_UNAVAILABLE",
                "The export destination is unavailable",
            )
        })?;
    if output_path
        .try_exists()
        .map_err(|_| invalid_export_destination())?
    {
        return Err(invalid_export_destination());
    }
    if !volume_supports_atomic_links(&output_path).map_err(|_| invalid_export_destination())? {
        return Err(invalid_export_destination());
    }
    for configured_root in protected_roots {
        if let Ok(root) = LocalRoot::new_resolved(configured_root)
            && root.contains_canonical_path(&output_path)
        {
            return Err(export_destination_error(
                "EXPORT_OUTPUT_UNSAFE",
                "The export destination is inside a protected source folder",
            ));
        }
    }
    let staging_path =
        export_staging_path(&output_path).map_err(|_| invalid_export_destination())?;
    Ok(ExportDestination {
        root: output_root,
        output_path,
        staging_path,
    })
}

fn export_staging_path(output_path: &Path) -> Result<PathBuf, ()> {
    let parent = output_path.parent().ok_or(())?;
    for _ in 0..4 {
        let mut random = [0_u8; 16];
        // SAFETY: the buffer is writable for exactly the length passed to the OS.
        if unsafe { rtl_gen_random(random.as_mut_ptr(), random.len() as u32) } == 0 {
            return Err(());
        }
        let mut suffix = String::with_capacity(random.len() * 2);
        use std::fmt::Write as _;
        for byte in random {
            write!(&mut suffix, "{byte:02x}").map_err(|_| ())?;
        }
        let candidate = parent.join(format!(".ai-session-replay-{suffix}.partial.mp4"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(())
}

fn invalid_export_destination() -> CommandError {
    export_destination_error(
        "EXPORT_OUTPUT_UNAVAILABLE",
        "The export destination is unavailable",
    )
}

fn export_destination_error(code: &'static str, message: &'static str) -> CommandError {
    CommandError {
        code,
        message: message.to_owned(),
    }
}

pub(crate) fn run_runtime_smoke_blocking(
    resource_directory: &Path,
    executable_directory: &Path,
    output_directory: &Path,
    job_id: &str,
) -> Result<MediaSummary, RuntimeError> {
    if !valid_job_id(job_id) {
        return Err(RuntimeError::Protocol);
    }
    fs::create_dir_all(output_directory).map_err(|_| RuntimeError::Process)?;
    let output_path = output_directory.join(format!("{job_id}.mp4"));
    let staging_path =
        output_directory.join(".ai-session-replay-00000000000000000000000000000000.partial.mp4");
    if output_path.exists() || staging_path.exists() {
        return Err(RuntimeError::Process);
    }
    let output_root = LocalRoot::new_locked(output_directory).map_err(|_| RuntimeError::Process)?;

    let paths = RuntimePaths::resolve(resource_directory, executable_directory)?;
    let request = smoke_request(&output_path, &staging_path, job_id)?;
    let result =
        run_worker_process(&paths.worker_launch_spec(), &request, job_id).and_then(|verified| {
            publish_verified_staging(
                &output_root,
                &staging_path,
                &output_path,
                verified.staging_sha256,
                None,
            )?;
            Ok(verified.media)
        });
    let _ = fs::remove_file(&staging_path);
    let _ = fs::remove_file(output_path);
    result
}

pub(crate) fn smoke_output_directory() -> PathBuf {
    std::env::temp_dir().join("ai-session-replay-runtime-smoke")
}

fn smoke_request(
    output_path: &Path,
    staging_path: &Path,
    job_id: &str,
) -> Result<Vec<u8>, RuntimeError> {
    let output_path = output_path.to_str().ok_or(RuntimeError::Protocol)?;
    let staging_path = staging_path.to_str().ok_or(RuntimeError::Protocol)?;
    let value = serde_json::json!({
        "schemaVersion": 1,
        "type": "render",
        "jobId": job_id,
        "outputPath": output_path,
        "stagingPath": staging_path,
        "plan": {
            "schemaVersion": 1,
            "planId": "smoke-plan",
            "sessionId": "smoke-session",
            "revisionId": "smoke-revision",
            "sessionTitle": "Synthetic runtime smoke",
            "createdAtMs": 0,
            "entries": [{
                "entry": {"entryKey": "smoke-entry", "ordinal": 0, "atMs": 0, "kind": "user", "text": "Synthetic packaged-runtime check"},
                "revealAtMs": 0
            }],
            "preferences": {
                "schemaVersion": 1,
                "visibility": {"showToolCalls": true, "showToolDetails": false, "showReasoning": true},
                "timing": {"entryDelayMs": 267, "playbackSpeed": 1},
                "appearance": {
                    "theme": {"background": "#101211", "surface": "#171A18", "text": "#F5F5F4", "muted": "#9B9E9C", "accent": "#D6AA68", "success": "#A8D5BD", "error": "#E59A91"},
                    "font": {"family": "JetBrains Mono", "sizePx": 34, "lineHeight": 1.5}
                }
            },
            "durationMs": 267,
            "fps": 30,
            "width": 1920,
            "height": 1080
        }
    });
    let mut request = serde_json::to_vec(&value).map_err(|_| RuntimeError::Protocol)?;
    request.push(b'\n');
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::{
        COMPLETED_EXPORTS, ExportProgressV1, MAX_COMPLETED_EXPORTS, PRESENTATION_PLANS,
        cancel_export, cleanup_abandoned_exports_in, export_runtime_error,
        frozen_presentation_plan, prepare_export_paths, publish_verified_staging,
        register_staging_cleanup, remember_completed_export, remember_presentation_plan,
        resolve_completed_export, run_export_blocking, smoke_request, trusted_explorer_path,
    };
    use crate::export::runtime::RuntimeError;
    use crate::indexed_library::PresentationPlanV1;
    use sha2::Digest;
    use std::path::Path;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn builds_a_bounded_synthetic_request_without_session_paths() {
        let request = smoke_request(
            Path::new(r"C:\Temp\runtime-smoke\job-123.mp4"),
            Path::new(r"C:\Temp\runtime-smoke\.ai-session-replay-00000000000000000000000000000000.partial.mp4"),
            "job-123",
        )
            .expect("smoke request should serialize");
        let value: serde_json::Value = serde_json::from_slice(&request)
            .expect("smoke request should be valid JSON with trailing whitespace");

        assert_eq!(value["jobId"], "job-123");
        assert!(value["plan"].get("sourcePath").is_none());
        assert_eq!(value["plan"]["entries"].as_array().unwrap().len(), 1);
        assert!(request.ends_with(b"\n"));
    }

    #[test]
    fn maps_export_runtime_failures_to_actionable_safe_codes() {
        assert_eq!(
            export_runtime_error(RuntimeError::Cancelled).code,
            "EXPORT_CANCELLED"
        );
        assert_eq!(
            export_runtime_error(RuntimeError::InvalidRuntime).code,
            "EXPORT_RUNTIME_UNAVAILABLE"
        );
        assert_eq!(
            export_runtime_error(RuntimeError::Worker(
                crate::export::runtime::WorkerFailureCode::OutputNotAvailable,
            ))
            .code,
            "EXPORT_OUTPUT_UNAVAILABLE"
        );
        assert_eq!(
            export_runtime_error(RuntimeError::Worker(
                crate::export::runtime::WorkerFailureCode::MediaAssertionFailed,
            ))
            .code,
            "EXPORT_VERIFICATION_FAILED"
        );
        assert_eq!(
            export_runtime_error(RuntimeError::Worker(
                crate::export::runtime::WorkerFailureCode::RenderFailed,
            ))
            .code,
            "EXPORT_RENDER_FAILED"
        );
        assert_eq!(
            export_runtime_error(RuntimeError::Process).code,
            "EXPORT_PROCESS_FAILED"
        );
        assert_eq!(
            export_runtime_error(RuntimeError::Protocol).code,
            "EXPORT_PROTOCOL_FAILED"
        );
    }

    #[test]
    fn rejects_an_invalid_export_destination_before_runtime_launch() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/indexed-library-contracts-v1.json"
        ))
        .unwrap();
        let plan: PresentationPlanV1 =
            serde_json::from_value(fixture["presentationPlan"].clone()).unwrap();
        assert_eq!(
            run_export_blocking(
                Path::new(r"C:\missing-runtime"),
                Path::new(r"C:\missing-bin"),
                Path::new(r"C:\Temp\bad.webm"),
                "job-123",
                &plan,
                &AtomicBool::new(false),
            ),
            Err(RuntimeError::Protocol)
        );
    }

    #[test]
    fn cancellation_is_validated_and_idempotent_after_completion() {
        assert_eq!(
            cancel_export("bad/job".to_owned()).unwrap_err().code,
            "INVALID_REQUEST"
        );
        cancel_export("missing-job".to_owned()).unwrap();
    }

    #[test]
    fn completed_exports_are_bounded_and_resolved_without_returning_paths_to_the_webview() {
        let directory = std::env::temp_dir().join(format!(
            "ai-session-replay-completed-export-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let output = directory.join("synthetic.mp4");
        std::fs::write(&output, b"synthetic media fixture").unwrap();

        COMPLETED_EXPORTS.lock().unwrap().paths.clear();
        COMPLETED_EXPORTS.lock().unwrap().order.clear();
        for index in 0..=MAX_COMPLETED_EXPORTS {
            remember_completed_export(format!("completed_{index}"), output.clone());
        }
        let exports = COMPLETED_EXPORTS.lock().unwrap();
        assert_eq!(exports.paths.len(), MAX_COMPLETED_EXPORTS);
        assert!(!exports.paths.contains_key("completed_0"));
        drop(exports);
        assert_eq!(
            resolve_completed_export(&format!("completed_{MAX_COMPLETED_EXPORTS}")).unwrap(),
            std::fs::canonicalize(&output).unwrap()
        );
        assert_eq!(
            resolve_completed_export("completed_0").unwrap_err().code,
            "EXPORT_NOT_FOUND"
        );

        COMPLETED_EXPORTS.lock().unwrap().paths.clear();
        COMPLETED_EXPORTS.lock().unwrap().order.clear();
        std::fs::remove_file(output).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn export_progress_is_bounded_path_free_metadata() {
        let progress = serde_json::to_value(ExportProgressV1 {
            schema_version: 1,
            job_id: "export_1".to_owned(),
            rendered_frames: 30,
            total_frames: 60,
        })
        .unwrap();
        assert_eq!(progress["renderedFrames"], 30);
        assert_eq!(progress["totalFrames"], 60);
        assert!(progress.get("path").is_none());
    }

    #[test]
    fn export_paths_are_local_normalized_randomized_and_outside_vendor_roots() {
        let directory = std::env::temp_dir().join(format!(
            "ai-session-replay-export-path-{}",
            std::process::id()
        ));
        let vendor = directory.join("vendor");
        std::fs::create_dir_all(&vendor).unwrap();

        let destination =
            prepare_export_paths(&directory.join("Replay.MP4"), std::slice::from_ref(&vendor))
                .unwrap();
        let output = &destination.output_path;
        let staging = &destination.staging_path;
        assert!(!output.to_string_lossy().starts_with(r"\\?\"));
        assert_eq!(staging.parent(), output.parent());
        assert!(
            staging
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".ai-session-replay-")
        );
        assert_eq!(
            prepare_export_paths(&vendor.join("private.mp4"), std::slice::from_ref(&vendor))
                .unwrap_err()
                .code,
            "EXPORT_OUTPUT_UNSAFE"
        );

        let existing = directory.join("existing.mp4");
        std::fs::write(&existing, b"existing video").unwrap();
        assert_eq!(
            prepare_export_paths(&existing, std::slice::from_ref(&vendor))
                .unwrap_err()
                .code,
            "EXPORT_OUTPUT_EXISTS"
        );
        assert_eq!(
            prepare_export_paths(&directory.join("bad.webm"), std::slice::from_ref(&vendor))
                .unwrap_err()
                .code,
            "EXPORT_OUTPUT_FORMAT"
        );

        drop(destination);
        std::fs::remove_file(existing).unwrap();
        std::fs::remove_dir(vendor).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn explorer_is_resolved_from_the_windows_directory_not_path() {
        let explorer = trusted_explorer_path().unwrap();
        assert!(explorer.is_absolute());
        assert_eq!(
            explorer.file_name().unwrap().to_string_lossy(),
            "explorer.exe"
        );
    }

    #[test]
    fn backend_publishes_verified_staging_without_overwriting() {
        let directory = std::env::temp_dir().join(format!(
            "ai-session-replay-publish-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let staging =
            directory.join(".ai-session-replay-0123456789abcdef0123456789abcdef.partial.mp4");
        let output = directory.join("replay.mp4");
        std::fs::write(&staging, b"verified").unwrap();
        let root = crate::io::LocalRoot::new_locked(&directory).unwrap();
        let digest: [u8; 32] = sha2::Sha256::digest(b"verified").into();

        publish_verified_staging(&root, &staging, &output, digest, None).unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), b"verified");
        assert!(!staging.exists());

        let second_staging =
            directory.join(".ai-session-replay-fedcba9876543210fedcba9876543210.partial.mp4");
        std::fs::write(&second_staging, b"must not replace").unwrap();
        assert_eq!(
            publish_verified_staging(&root, &second_staging, &output, digest, None),
            Err(RuntimeError::Protocol)
        );
        assert_eq!(std::fs::read(&output).unwrap(), b"verified");

        let wrong_staging =
            directory.join(".ai-session-replay-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.partial.mp4");
        let wrong_output = directory.join("wrong.mp4");
        std::fs::write(&wrong_staging, b"swapped").unwrap();
        assert_eq!(
            publish_verified_staging(&root, &wrong_staging, &wrong_output, digest, None),
            Err(RuntimeError::Protocol)
        );
        assert!(!wrong_output.exists());
        assert!(wrong_staging.exists());

        let cancelled_staging =
            directory.join(".ai-session-replay-cccccccccccccccccccccccccccccccc.partial.mp4");
        let cancelled_output = directory.join("cancelled.mp4");
        std::fs::write(&cancelled_staging, b"verified").unwrap();
        let cancelled = std::sync::Arc::new(AtomicBool::new(true));
        super::EXPORT_CANCELLATIONS.lock().unwrap().insert(
            "publication_cancel_test".to_owned(),
            std::sync::Arc::clone(&cancelled),
        );
        assert_eq!(
            publish_verified_staging(
                &root,
                &cancelled_staging,
                &cancelled_output,
                digest,
                Some(("publication_cancel_test", &cancelled)),
            ),
            Err(RuntimeError::Cancelled)
        );
        assert!(!cancelled_output.exists());
        assert!(cancelled_staging.exists());
        assert!(
            !super::EXPORT_CANCELLATIONS
                .lock()
                .unwrap()
                .contains_key("publication_cancel_test")
        );

        let linked_source = directory.join("linked-source.mp4");
        let linked_staging =
            directory.join(".ai-session-replay-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.partial.mp4");
        let linked_output = directory.join("linked-output.mp4");
        std::fs::write(&linked_source, b"verified").unwrap();
        std::fs::hard_link(&linked_source, &linked_staging).unwrap();
        assert_eq!(
            publish_verified_staging(&root, &linked_staging, &linked_output, digest, None),
            Err(RuntimeError::Process)
        );
        assert!(!linked_output.exists());

        let renamed = directory.with_extension("renamed");
        assert!(std::fs::rename(&directory, &renamed).is_err());

        std::fs::remove_file(output).unwrap();
        std::fs::remove_file(second_staging).unwrap();
        std::fs::remove_file(wrong_staging).unwrap();
        std::fs::remove_file(cancelled_staging).unwrap();
        std::fs::remove_file(linked_staging).unwrap();
        std::fs::remove_file(linked_source).unwrap();
        drop(root);
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn startup_cleanup_removes_only_registered_staging_artifacts() {
        let root = std::env::temp_dir().join(format!(
            "ai-session-replay-cleanup-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let output = root.join("output");
        std::fs::create_dir_all(&output).unwrap();
        let staging =
            output.join(".ai-session-replay-0123456789abcdef0123456789abcdef.partial.mp4");
        let unrelated = output.join("keep.mp4");
        let linked_source = output.join("partial-source.mp4");
        std::fs::write(&linked_source, b"partial").unwrap();
        std::fs::hard_link(&linked_source, &staging).unwrap();
        std::fs::write(&unrelated, b"keep").unwrap();
        let marker = register_staging_cleanup(&root, "cleanup_job", &staging).unwrap();

        cleanup_abandoned_exports_in(&root, &[]);

        assert!(!staging.exists());
        assert!(!marker.marker_path.exists());
        assert_eq!(std::fs::read(&linked_source).unwrap(), b"partial");
        assert_eq!(std::fs::read(&unrelated).unwrap(), b"keep");
        drop(marker);
        std::fs::remove_file(linked_source).unwrap();
        std::fs::remove_file(unrelated).unwrap();
        std::fs::remove_dir(output).unwrap();
        std::fs::remove_dir(root.join("export-cleanup")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn startup_cleanup_retains_a_marker_until_a_locked_staging_file_can_be_removed() {
        let root = std::env::temp_dir().join(format!(
            "ai-session-replay-cleanup-retry-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let output = root.join("output");
        std::fs::create_dir_all(&output).unwrap();
        let staging =
            output.join(".ai-session-replay-0123456789abcdef0123456789abcdef.partial.mp4");
        std::fs::write(&staging, b"partial").unwrap();
        let registration = register_staging_cleanup(&root, "retry_job", &staging).unwrap();
        let output_root = crate::io::LocalRoot::new(&output).unwrap();
        let locked = output_root.open_file_for_publication(&staging).unwrap();

        cleanup_abandoned_exports_in(&root, &[]);
        assert!(staging.exists());
        assert!(registration.marker_path.exists());

        drop(locked);
        drop(registration);
        cleanup_abandoned_exports_in(&root, &[]);
        assert!(!staging.exists());
        assert!(!root.join("export-cleanup/export-retry_job.json").exists());

        std::fs::remove_dir(output).unwrap();
        std::fs::remove_dir(root.join("export-cleanup")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn frozen_plans_are_retrieved_only_by_bounded_opaque_id() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/indexed-library-contracts-v1.json"
        ))
        .unwrap();
        let plan: PresentationPlanV1 =
            serde_json::from_value(fixture["presentationPlan"].clone()).unwrap();
        PRESENTATION_PLANS.lock().unwrap().plans.clear();
        PRESENTATION_PLANS.lock().unwrap().order.clear();
        let epoch = PRESENTATION_PLANS.lock().unwrap().epoch;
        remember_presentation_plan(plan.clone(), epoch).unwrap();

        assert_eq!(
            frozen_presentation_plan(&plan.plan_id).unwrap().as_ref(),
            &plan
        );
        assert_eq!(
            frozen_presentation_plan("../forged").unwrap_err().code,
            "PRESENTATION_UNAVAILABLE"
        );
        assert_eq!(
            frozen_presentation_plan("missing_plan").unwrap_err().code,
            "PRESENTATION_UNAVAILABLE"
        );

        let stale_epoch = PRESENTATION_PLANS.lock().unwrap().epoch;
        super::clear_frozen_plans();
        assert_eq!(
            remember_presentation_plan(plan.clone(), stale_epoch)
                .unwrap_err()
                .code,
            "STALE_REVISION"
        );
        let current_epoch = PRESENTATION_PLANS.lock().unwrap().epoch;
        remember_presentation_plan(plan.clone(), current_epoch).unwrap();
        super::forget_presentation_plans_for_session(&plan.session_id);
        assert_eq!(
            frozen_presentation_plan(&plan.plan_id).unwrap_err().code,
            "PRESENTATION_UNAVAILABLE"
        );

        PRESENTATION_PLANS.lock().unwrap().plans.clear();
        PRESENTATION_PLANS.lock().unwrap().order.clear();
    }
}
