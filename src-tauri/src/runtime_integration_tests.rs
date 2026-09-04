use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject};

use crate::export::runtime::{
    LaunchSpec, RuntimeError, RuntimePaths, WorkerFailureCode, parse_worker_stdout,
    run_worker_process, run_worker_process_cancellable_with_progress,
};

struct TempTree(PathBuf);

impl TempTree {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should follow the epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "ai-session-runtime-test-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary fixture directory should be created");
        Self(path)
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn resolves_only_the_installed_sidecar_and_resource_layout() {
    let tree = TempTree::new();
    let executable_directory = tree.0.join("app");
    let resource_directory = executable_directory.join("resources");
    write_runtime_fixture(&resource_directory, &executable_directory);

    let paths = RuntimePaths::resolve(&resource_directory, &executable_directory)
        .expect("installed runtime layout should resolve");

    assert_eq!(
        paths.bun_executable,
        executable_directory.join("render-worker-bun.exe")
    );
    assert_eq!(
        paths.worker_entrypoint,
        resource_directory.join("render-runtime/worker/index.js")
    );
    assert_eq!(
        paths.runtime_directory,
        resource_directory.join("render-runtime")
    );
    assert_eq!(
        paths.browser_executable,
        resource_directory.join("render-runtime/chrome-headless-shell/chrome-headless-shell.exe")
    );
    assert_eq!(
        paths.composition_directory,
        resource_directory.join("render-runtime/composition")
    );
    assert_eq!(
        paths.binaries_directory,
        resource_directory.join("render-runtime/remotion")
    );
    let launch = paths.worker_launch_spec();
    assert_eq!(
        launch.program,
        executable_directory.join("render-worker-bun.exe")
    );
    assert_eq!(
        launch.arguments,
        [
            resource_directory
                .join("render-runtime")
                .join("worker")
                .join("index.js")
                .into_os_string(),
            OsString::from("--runtime-directory"),
            resource_directory.join("render-runtime").into_os_string(),
        ]
    );
}

#[test]
fn rejects_an_altered_worker_receipt() {
    let tree = TempTree::new();
    let executable_directory = tree.0.join("app");
    let resource_directory = executable_directory.join("resources");
    write_runtime_fixture(&resource_directory, &executable_directory);
    fs::write(
        resource_directory.join("render-runtime/worker-layout.json"),
        br#"{"schemaVersion":1,"targetTriple":"aarch64-pc-windows-msvc","worker":{"path":"worker/index.js","sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"bunSidecar":"render-worker-bun.exe"}"#,
    )
    .expect("altered receipt should be written");

    assert_eq!(
        RuntimePaths::resolve(&resource_directory, &executable_directory),
        Err(RuntimeError::InvalidRuntime)
    );
}

#[test]
fn accepts_one_well_formed_job_scoped_protocol_stream() {
    let stdout = concat!(
        "{\"schemaVersion\":1,\"type\":\"started\",\"jobId\":\"job-123\",\"totalFrames\":8}\n",
        "{\"schemaVersion\":1,\"type\":\"progress\",\"jobId\":\"job-123\",\"renderedFrames\":0,\"totalFrames\":8}\n",
        "{\"schemaVersion\":1,\"type\":\"progress\",\"jobId\":\"job-123\",\"renderedFrames\":4,\"totalFrames\":8}\n",
        "{\"schemaVersion\":1,\"type\":\"succeeded\",\"jobId\":\"job-123\",\"media\":{\"codec\":\"h264\",\"width\":1920,\"height\":1080,\"pixelFormat\":\"yuv420p\",\"durationMs\":267,\"frameCount\":8},\"stagingSha256\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"}\n",
    );

    let media = parse_worker_stdout(stdout.as_bytes(), "job-123")
        .expect("valid protocol should be accepted");

    assert_eq!(media.media.codec, "h264");
    assert_eq!(media.media.pixel_format, "yuv420p");
    assert_eq!(media.media.duration_ms, 267);
    assert_eq!(media.media.frame_count, 8);
    assert_eq!(media.staging_sha256, [0xaa; 32]);
}

#[test]
fn accepts_only_allowlisted_worker_failure_codes() {
    let stdout = "{\"schemaVersion\":1,\"type\":\"failed\",\"jobId\":\"job-123\",\"code\":\"RENDER_FAILED\"}\n";
    assert_eq!(
        parse_worker_stdout(stdout.as_bytes(), "job-123"),
        Err(RuntimeError::Worker(WorkerFailureCode::RenderFailed))
    );

    let raw_exception = "{\"schemaVersion\":1,\"type\":\"failed\",\"jobId\":\"job-123\",\"code\":\"RENDER_FAILED\",\"message\":\"raw transcript\"}\n";
    assert_eq!(
        parse_worker_stdout(raw_exception.as_bytes(), "job-123"),
        Err(RuntimeError::Protocol)
    );
}

#[test]
fn rejects_mismatched_jobs_invalid_order_and_protocol_limits() {
    let mismatched_job = concat!(
        "{\"schemaVersion\":1,\"type\":\"started\",\"jobId\":\"other-job\",\"totalFrames\":8}\n",
        "{\"schemaVersion\":1,\"type\":\"failed\",\"jobId\":\"other-job\",\"code\":\"RENDER_FAILED\"}\n",
    );
    assert_eq!(
        parse_worker_stdout(mismatched_job.as_bytes(), "job-123"),
        Err(RuntimeError::Protocol)
    );

    let progress_before_started = concat!(
        "{\"schemaVersion\":1,\"type\":\"progress\",\"jobId\":\"job-123\",\"renderedFrames\":1,\"totalFrames\":8}\n",
        "{\"schemaVersion\":1,\"type\":\"failed\",\"jobId\":\"job-123\",\"code\":\"RENDER_FAILED\"}\n",
    );
    assert_eq!(
        parse_worker_stdout(progress_before_started.as_bytes(), "job-123"),
        Err(RuntimeError::Protocol)
    );

    let duplicate_zero_progress = concat!(
        "{\"schemaVersion\":1,\"type\":\"started\",\"jobId\":\"job-123\",\"totalFrames\":8}\n",
        "{\"schemaVersion\":1,\"type\":\"progress\",\"jobId\":\"job-123\",\"renderedFrames\":0,\"totalFrames\":8}\n",
        "{\"schemaVersion\":1,\"type\":\"progress\",\"jobId\":\"job-123\",\"renderedFrames\":0,\"totalFrames\":8}\n",
    );
    assert_eq!(
        parse_worker_stdout(duplicate_zero_progress.as_bytes(), "job-123"),
        Err(RuntimeError::Protocol)
    );

    let oversized = vec![b'x'; 128 * 1_024 + 1];
    assert_eq!(
        parse_worker_stdout(&oversized, "job-123"),
        Err(RuntimeError::Protocol)
    );
}

#[test]
fn launches_with_scrubbed_path_and_discards_bounded_stderr() {
    let tree = TempTree::new();
    let script = tree.0.join("worker scripts/success.ps1");
    fs::create_dir_all(script.parent().expect("script should have a parent"))
        .expect("script directory should be created");
    fs::write(
        &script,
        r#"if ($null -ne $env:PATH -and $env:PATH -ne '') { exit 9 }
if ($null -ne $env:AI_SESSION_REPLAY_SYNTHETIC_SECRET) { exit 7 }
$request = [Console]::In.ReadLine()
if (-not $request.Contains('"jobId":"job-123"')) { exit 8 }
[Console]::Error.Write('raw transcript must be discarded')
[Console]::Out.Write('{"schemaVersion":1,"type":"started","jobId":"job-123","totalFrames":8}' + "`n")
[Console]::Out.Write('{"schemaVersion":1,"type":"progress","jobId":"job-123","renderedFrames":0,"totalFrames":8}' + "`n")
[Console]::Out.Write('{"schemaVersion":1,"type":"progress","jobId":"job-123","renderedFrames":2,"totalFrames":8}' + "`n")
[Console]::Out.Write('{"schemaVersion":1,"type":"progress","jobId":"job-123","renderedFrames":8,"totalFrames":8}' + "`n")
[Console]::Out.Write('{"schemaVersion":1,"type":"succeeded","jobId":"job-123","media":{"codec":"h264","width":1920,"height":1080,"pixelFormat":"yuv420p","durationMs":267,"frameCount":8},"stagingSha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}' + "`n")
"#,
    )
    .expect("fake worker should be written");
    // SAFETY: this process-scoped synthetic variable is unique to this test
    // and is restored immediately after the child process exits.
    unsafe { std::env::set_var("AI_SESSION_REPLAY_SYNTHETIC_SECRET", "must-not-leak") };

    let progress = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&progress);
    let media_result = run_worker_process_cancellable_with_progress(
        &powershell_spec(&script),
        b"{\"schemaVersion\":1,\"type\":\"render\",\"jobId\":\"job-123\"}\n",
        "job-123",
        &AtomicBool::new(false),
        Arc::new(move |rendered, total| captured.lock().unwrap().push((rendered, total))),
    );
    // SAFETY: see the matching setup immediately above.
    unsafe { std::env::remove_var("AI_SESSION_REPLAY_SYNTHETIC_SECRET") };
    let media = media_result.expect("fake worker should complete");

    assert_eq!(media.media.frame_count, 8);
    assert_eq!(*progress.lock().unwrap(), [(0, 8), (2, 8), (8, 8)]);
}

#[test]
fn terminates_a_worker_process_tree_that_exceeds_the_stderr_limit() {
    let tree = TempTree::new();
    let script = tree.0.join("overflow.ps1");
    fs::write(
        &script,
        r#"$child = Start-Process -FilePath (Join-Path $PSHOME 'powershell.exe') -ArgumentList @('-NoProfile', '-NonInteractive', '-Command', 'Start-Sleep -Seconds 30') -WindowStyle Hidden -PassThru
[IO.File]::WriteAllText((Join-Path $PSScriptRoot 'child.pid'), [string]$child.Id)
[Console]::Error.Write(('x' * 20000))
Start-Sleep -Seconds 30
"#,
    )
    .expect("fake worker should be written");
    let started = Instant::now();

    let result = run_worker_process(&powershell_spec(&script), b"{}\n", "job-123");

    assert_eq!(result, Err(RuntimeError::OutputLimit));
    assert!(started.elapsed() < Duration::from_secs(10));
    let child_pid: u32 = fs::read_to_string(tree.0.join("child.pid"))
        .expect("worker should record its synthetic child")
        .parse()
        .expect("child PID should be numeric");
    assert!(process_has_exited(child_pid));
}

#[test]
fn cancellation_terminates_the_worker_without_waiting_for_completion() {
    let tree = TempTree::new();
    let script = tree.0.join("cancel.ps1");
    fs::write(
        &script,
        "[Console]::Out.Write('{\"schemaVersion\":1,\"type\":\"started\",\"jobId\":\"job-123\",\"totalFrames\":8}' + \"`n\")\nStart-Sleep -Seconds 30\n",
    )
    .expect("fake worker should be written");
    let cancelled = AtomicBool::new(true);
    let started = Instant::now();

    let result = run_worker_process_cancellable_with_progress(
        &powershell_spec(&script),
        b"{}\n",
        "job-123",
        &cancelled,
        Arc::new(|_, _| {}),
    );

    assert_eq!(result, Err(RuntimeError::Cancelled));
    assert!(started.elapsed() < Duration::from_secs(10));
}

fn write_runtime_fixture(resource_directory: &Path, executable_directory: &Path) {
    let runtime = resource_directory.join("render-runtime");
    fs::create_dir_all(runtime.join("worker")).expect("runtime worker directory should be created");
    fs::create_dir_all(runtime.join("chrome-headless-shell"))
        .expect("Chrome directory should be created");
    fs::create_dir_all(runtime.join("composition"))
        .expect("composition directory should be created");
    fs::create_dir_all(runtime.join("remotion")).expect("compositor directory should be created");
    fs::create_dir_all(executable_directory).expect("executable directory should be created");
    fs::write(
        executable_directory.join("render-worker-bun.exe"),
        b"fake bun",
    )
    .expect("fake Bun should be written");
    fs::write(runtime.join("worker/index.js"), b"fake worker")
        .expect("fake worker should be written");
    fs::write(
        runtime.join("chrome-headless-shell/chrome-headless-shell.exe"),
        b"fake chrome",
    )
    .expect("fake Chrome should be written");
    fs::write(runtime.join("composition/index.html"), b"fake composition")
        .expect("fake composition should be written");
    fs::write(runtime.join("remotion/remotion.exe"), b"fake compositor")
        .expect("fake compositor should be written");
    fs::write(runtime.join("remotion/ffmpeg.exe"), b"fake ffmpeg")
        .expect("fake ffmpeg should be written");
    fs::write(runtime.join("remotion/ffprobe.exe"), b"fake ffprobe")
        .expect("fake ffprobe should be written");
    fs::write(
        runtime.join("runtime-layout.json"),
        br#"{"schemaVersion":1,"platform":"windows-x64","versions":{"bun":"1.4.0","chromeHeadlessShell":"149.0.7790.0","remotion":"4.0.516"},"paths":{"bunExecutable":"bun/bun.exe","browserExecutable":"chrome-headless-shell/chrome-headless-shell.exe","binariesDirectory":"remotion","composition":"composition"},"archiveSha256":{"bun":"e6f093d39da486b20262ca8cdd5ed6a9e8bc9c2f275b78e6d3a0c5b28cc95901","chromeHeadlessShell":"8a112c0e768907ff382ff994dcde35cdbd0e1f5a3a475da93d3a83e3260e78c2","remotionCompositor":"946cdc35b2d08ca83e2d4bf66f1c23656cd321ce90d8bdfee72d2ff10d8c502e"}}"#,
    )
    .expect("runtime receipt should be written");
    fs::write(
        runtime.join("worker-layout.json"),
        br#"{"schemaVersion":1,"targetTriple":"x86_64-pc-windows-msvc","worker":{"path":"worker/index.js","sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"bunSidecar":"render-worker-bun.exe"}"#,
    )
    .expect("worker receipt should be written");
}

fn powershell_spec(script: &Path) -> LaunchSpec {
    let windows = std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("WINDIR"))
        .expect("Windows directory should be defined");
    LaunchSpec {
        program: PathBuf::from(windows).join("System32/WindowsPowerShell/v1.0/powershell.exe"),
        arguments: vec![
            OsString::from("-NoLogo"),
            OsString::from("-NoProfile"),
            OsString::from("-NonInteractive"),
            OsString::from("-ExecutionPolicy"),
            OsString::from("Bypass"),
            OsString::from("-File"),
            script.as_os_str().to_owned(),
        ],
    }
}

fn process_has_exited(process_id: u32) -> bool {
    // SAFETY: OpenProcess returns either a null handle or a uniquely closed synchronization handle.
    let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, process_id) };
    if handle.is_null() {
        return true;
    }
    // SAFETY: the handle is valid and remains open for the zero-timeout status check.
    let exited = unsafe { WaitForSingleObject(handle, 0) } == WAIT_OBJECT_0;
    // SAFETY: this test owns the handle returned by OpenProcess.
    unsafe {
        CloseHandle(handle);
    }
    exited
}
