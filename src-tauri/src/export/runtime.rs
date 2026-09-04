use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString, c_void};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::FromRawHandle;
use std::path::{Component, Path, PathBuf, Prefix};
use std::ptr::{null, null_mut};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
    GetExitCodeProcess, PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOW,
    TerminateProcess, WaitForSingleObject,
};

const MAX_LAYOUT_BYTES: u64 = 16 * 1_024;
const MAX_PROTOCOL_BYTES: usize = 128 * 1_024;
const MAX_PROTOCOL_LINE_BYTES: usize = 512;
const MAX_PROTOCOL_EVENTS: usize = 256;
const MAX_REQUEST_BYTES: usize = 64 * 1_024 * 1_024 + 2;
const MAX_STDERR_BYTES: usize = 16 * 1_024;
const SHA256_HEX_LENGTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimePaths {
    pub(crate) bun_executable: PathBuf,
    pub(crate) worker_entrypoint: PathBuf,
    pub(crate) runtime_directory: PathBuf,
    pub(crate) browser_executable: PathBuf,
    pub(crate) composition_directory: PathBuf,
    pub(crate) binaries_directory: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaSummary {
    pub(crate) codec: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) pixel_format: String,
    pub(crate) duration_ms: u64,
    pub(crate) frame_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifiedMediaSummary {
    pub(crate) media: MediaSummary,
    pub(crate) staging_sha256: [u8; 32],
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum WorkerFailureCode {
    InvalidRequest,
    RequestLimitExceeded,
    InvalidRuntime,
    OutputNotAvailable,
    RenderFailed,
    MediaAssertionFailed,
    ProtocolLimitExceeded,
    WorkerInternal,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RuntimeError {
    InvalidRuntime,
    Protocol,
    Process,
    OutputLimit,
    Worker(WorkerFailureCode),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaunchSpec {
    pub(crate) program: PathBuf,
    pub(crate) arguments: Vec<OsString>,
}

pub(crate) fn run_worker_process(
    spec: &LaunchSpec,
    request: &[u8],
    expected_job_id: &str,
) -> Result<VerifiedMediaSummary, RuntimeError> {
    run_worker_process_cancellable(spec, request, expected_job_id, &AtomicBool::new(false))
}

pub(crate) fn run_worker_process_cancellable(
    spec: &LaunchSpec,
    request: &[u8],
    expected_job_id: &str,
    cancelled: &AtomicBool,
) -> Result<VerifiedMediaSummary, RuntimeError> {
    run_worker_process_cancellable_with_progress(
        spec,
        request,
        expected_job_id,
        cancelled,
        Arc::new(|_, _| {}),
    )
}

pub(crate) fn run_worker_process_cancellable_with_progress(
    spec: &LaunchSpec,
    request: &[u8],
    expected_job_id: &str,
    cancelled: &AtomicBool,
    on_progress: Arc<dyn Fn(u32, u32) + Send + Sync>,
) -> Result<VerifiedMediaSummary, RuntimeError> {
    if request.is_empty()
        || request.len() > MAX_REQUEST_BYTES
        || !request.ends_with(b"\n")
        || !valid_job_id(expected_job_id)
    {
        return Err(RuntimeError::Protocol);
    }

    let mut child = SuspendedChild::spawn(spec)?;
    let stdout = child.stdout.take().ok_or(RuntimeError::Process)?;
    let stderr = child.stderr.take().ok_or(RuntimeError::Process)?;
    let output_limit_hit = Arc::new(AtomicBool::new(false));
    let stdout_limit = Arc::clone(&output_limit_hit);
    let stderr_limit = Arc::clone(&output_limit_hit);
    let stdout_job_id = expected_job_id.to_owned();
    let stdout_thread = thread::spawn(move || {
        read_protocol_bounded(stdout, &stdout_limit, &stdout_job_id, on_progress)
    });
    let stderr_thread =
        thread::spawn(move || read_bounded(stderr, MAX_STDERR_BYTES, false, &stderr_limit));

    child.resume()?;
    let write_result = child
        .stdin
        .take()
        .ok_or(RuntimeError::Process)
        .and_then(|mut stdin| {
            stdin
                .write_all(request)
                .map_err(|_| RuntimeError::Process)?;
            stdin.flush().map_err(|_| RuntimeError::Process)
        });
    if write_result.is_err() {
        child.terminate();
    }

    let wait_result = child.wait(&output_limit_hit, cancelled);
    let stdout = stdout_thread.join().map_err(|_| RuntimeError::Process)??;
    stderr_thread.join().map_err(|_| RuntimeError::Process)??;
    write_result?;
    let exit_code = wait_result?;
    if output_limit_hit.load(Ordering::Acquire) {
        return Err(RuntimeError::OutputLimit);
    }

    let result = parse_worker_stdout(&stdout, expected_job_id);
    match result {
        Ok(media) if exit_code == 0 => Ok(media),
        Err(RuntimeError::Worker(code)) => Err(RuntimeError::Worker(code)),
        Err(error) => Err(error),
        Ok(_) => Err(RuntimeError::Process),
    }
}

fn read_protocol_bounded(
    stream: File,
    output_limit_hit: &AtomicBool,
    expected_job_id: &str,
    on_progress: Arc<dyn Fn(u32, u32) + Send + Sync>,
) -> Result<Vec<u8>, RuntimeError> {
    let mut reader = BufReader::new(stream).take((MAX_PROTOCOL_BYTES + 1) as u64);
    let mut output = Vec::new();
    let mut state = ProgressStreamState::default();
    loop {
        let mut line = Vec::new();
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(|_| RuntimeError::Process)?;
        if read == 0 {
            return Ok(output);
        }
        if output.len().saturating_add(read) > MAX_PROTOCOL_BYTES {
            output_limit_hit.store(true, Ordering::Release);
            return Err(RuntimeError::OutputLimit);
        }
        output.extend_from_slice(&line);
        if line.ends_with(b"\n") {
            state.observe(&line, expected_job_id, &on_progress)?;
        }
    }
}

#[derive(Default)]
struct ProgressStreamState {
    event_count: usize,
    started_frames: Option<u32>,
    last_rendered_frames: Option<u32>,
    terminal: bool,
}

impl ProgressStreamState {
    fn observe(
        &mut self,
        line_with_newline: &[u8],
        expected_job_id: &str,
        on_progress: &Arc<dyn Fn(u32, u32) + Send + Sync>,
    ) -> Result<(), RuntimeError> {
        self.event_count += 1;
        if self.event_count > MAX_PROTOCOL_EVENTS
            || line_with_newline.len() > MAX_PROTOCOL_LINE_BYTES
            || line_with_newline.len() < 2
            || line_with_newline.contains(&b'\r')
            || self.terminal
        {
            return Err(RuntimeError::Protocol);
        }
        let event: WorkerEvent =
            serde_json::from_slice(&line_with_newline[..line_with_newline.len() - 1])
                .map_err(|_| RuntimeError::Protocol)?;
        match event {
            WorkerEvent::Started {
                schema_version,
                job_id,
                total_frames,
            } => {
                if schema_version != 1
                    || job_id != expected_job_id
                    || self.started_frames.is_some()
                    || total_frames == 0
                    || total_frames > 216_000
                    || self.event_count != 1
                {
                    return Err(RuntimeError::Protocol);
                }
                self.started_frames = Some(total_frames);
            }
            WorkerEvent::Progress {
                schema_version,
                job_id,
                rendered_frames,
                total_frames,
            } => {
                if schema_version != 1
                    || job_id != expected_job_id
                    || self.started_frames != Some(total_frames)
                    || self
                        .last_rendered_frames
                        .is_some_and(|last| rendered_frames <= last)
                    || rendered_frames > total_frames
                {
                    return Err(RuntimeError::Protocol);
                }
                self.last_rendered_frames = Some(rendered_frames);
                on_progress(rendered_frames, total_frames);
            }
            WorkerEvent::Succeeded { .. } | WorkerEvent::Failed { .. } => {
                self.terminal = true;
            }
        }
        Ok(())
    }
}

struct OwnedHandle(HANDLE);

impl OwnedHandle {
    fn new(handle: HANDLE) -> Result<Self, RuntimeError> {
        if handle.is_null() {
            Err(RuntimeError::Process)
        } else {
            Ok(Self(handle))
        }
    }

    fn raw(&self) -> HANDLE {
        self.0
    }

    fn into_file(mut self) -> File {
        let handle = self.0;
        self.0 = null_mut();
        // SAFETY: ownership of a valid Windows handle is transferred to File exactly once.
        unsafe { File::from_raw_handle(handle) }
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: this wrapper uniquely owns the non-null handle.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

struct SuspendedChild {
    job: OwnedHandle,
    process: OwnedHandle,
    thread: Option<OwnedHandle>,
    stdin: Option<File>,
    stdout: Option<File>,
    stderr: Option<File>,
}

impl SuspendedChild {
    fn spawn(spec: &LaunchSpec) -> Result<Self, RuntimeError> {
        let program = canonicalize_local(&spec.program).map_err(|_| RuntimeError::Process)?;
        let metadata = fs::metadata(&program).map_err(|_| RuntimeError::Process)?;
        if !metadata.is_file() {
            return Err(RuntimeError::Process);
        }

        // SAFETY: null security/name pointers request a private job with default security.
        let job = OwnedHandle::new(unsafe { CreateJobObjectW(null(), null()) })?;
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: the buffer matches the requested information class and is valid for the call.
        if unsafe {
            SetInformationJobObject(
                job.raw(),
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast::<c_void>(),
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(RuntimeError::Process);
        }

        let (parent_stdin, child_stdin) = create_pipe(false)?;
        let (parent_stdout, child_stdout) = create_pipe(true)?;
        let (parent_stderr, child_stderr) = create_pipe(true)?;
        let mut command_line = windows_command_line(&program, &spec.arguments)?;
        let application = nul_terminated(program.as_os_str())?;
        let environment = scrubbed_environment_block()?;
        let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
        startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        startup.dwFlags = STARTF_USESTDHANDLES;
        startup.hStdInput = child_stdin.raw();
        startup.hStdOutput = child_stdout.raw();
        startup.hStdError = child_stderr.raw();
        let mut process_information: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: all buffers remain live and writable as required for the duration of the call.
        let created = unsafe {
            CreateProcessW(
                application.as_ptr(),
                command_line.as_mut_ptr(),
                null(),
                null(),
                1,
                CREATE_NO_WINDOW | CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT,
                environment.as_ptr().cast::<c_void>(),
                null(),
                &startup,
                &mut process_information,
            )
        };
        drop(child_stdin);
        drop(child_stdout);
        drop(child_stderr);
        if created == 0 {
            return Err(RuntimeError::Process);
        }

        let process = OwnedHandle::new(process_information.hProcess)?;
        let thread = OwnedHandle::new(process_information.hThread)?;
        // Assigning while suspended prevents descendants from escaping the kill-on-close job.
        // SAFETY: both handles are valid and the process has not started executing.
        if unsafe { AssignProcessToJobObject(job.raw(), process.raw()) } == 0 {
            // SAFETY: the process handle is valid and uniquely controlled here.
            unsafe {
                TerminateProcess(process.raw(), 1);
            }
            return Err(RuntimeError::Process);
        }

        Ok(Self {
            job,
            process,
            thread: Some(thread),
            stdin: Some(parent_stdin.into_file()),
            stdout: Some(parent_stdout.into_file()),
            stderr: Some(parent_stderr.into_file()),
        })
    }

    fn resume(&mut self) -> Result<(), RuntimeError> {
        let thread = self.thread.take().ok_or(RuntimeError::Process)?;
        // SAFETY: the primary thread is valid and was created suspended.
        if unsafe { ResumeThread(thread.raw()) } == u32::MAX {
            self.terminate();
            return Err(RuntimeError::Process);
        }
        Ok(())
    }

    fn terminate(&self) {
        // SAFETY: terminating the private job only affects this child process tree.
        unsafe {
            TerminateJobObject(self.job.raw(), 1);
        }
    }

    fn wait(
        &self,
        output_limit_hit: &AtomicBool,
        cancelled: &AtomicBool,
    ) -> Result<u32, RuntimeError> {
        let mut terminated = false;
        loop {
            if output_limit_hit.load(Ordering::Acquire) && !terminated {
                self.terminate();
                terminated = true;
            }
            if cancelled.load(Ordering::Acquire) {
                self.terminate();
                return Err(RuntimeError::Cancelled);
            }
            // SAFETY: the process handle remains valid for this object's lifetime.
            match unsafe { WaitForSingleObject(self.process.raw(), 50) } {
                WAIT_OBJECT_0 => {
                    let mut exit_code = 0;
                    // SAFETY: the process is signaled and the output pointer is valid.
                    if unsafe { GetExitCodeProcess(self.process.raw(), &mut exit_code) } == 0 {
                        return Err(RuntimeError::Process);
                    }
                    return Ok(exit_code);
                }
                WAIT_TIMEOUT => {}
                _ => {
                    self.terminate();
                    return Err(RuntimeError::Process);
                }
            }
        }
    }
}

fn create_pipe(parent_reads: bool) -> Result<(OwnedHandle, OwnedHandle), RuntimeError> {
    let mut read_handle = null_mut();
    let mut write_handle = null_mut();
    let security = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    // SAFETY: output pointers and the security descriptor are valid for this call.
    if unsafe { CreatePipe(&mut read_handle, &mut write_handle, &security, 0) } == 0 {
        return Err(RuntimeError::Process);
    }
    let read_handle = OwnedHandle::new(read_handle)?;
    let write_handle = OwnedHandle::new(write_handle)?;
    let parent_handle = if parent_reads {
        read_handle.raw()
    } else {
        write_handle.raw()
    };
    // Only the worker ends of the pipes may cross the process boundary.
    // SAFETY: the parent handle is valid and owned by this function.
    if unsafe { SetHandleInformation(parent_handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
        return Err(RuntimeError::Process);
    }
    if parent_reads {
        Ok((read_handle, write_handle))
    } else {
        Ok((write_handle, read_handle))
    }
}

fn read_bounded(
    mut stream: File,
    limit: usize,
    capture: bool,
    output_limit_hit: &AtomicBool,
) -> Result<Vec<u8>, RuntimeError> {
    let mut total = 0usize;
    let mut output = Vec::new();
    let mut buffer = [0u8; 8 * 1_024];
    loop {
        let read = stream
            .read(&mut buffer)
            .map_err(|_| RuntimeError::Process)?;
        if read == 0 {
            return Ok(output);
        }
        total = total.checked_add(read).ok_or(RuntimeError::OutputLimit)?;
        if total > limit {
            output_limit_hit.store(true, Ordering::Release);
            return Err(RuntimeError::OutputLimit);
        }
        if capture {
            output.extend_from_slice(&buffer[..read]);
        }
    }
}

fn nul_terminated(value: &OsStr) -> Result<Vec<u16>, RuntimeError> {
    let mut wide: Vec<u16> = value.encode_wide().collect();
    if wide.contains(&0) {
        return Err(RuntimeError::Process);
    }
    wide.push(0);
    Ok(wide)
}

fn windows_command_line(program: &Path, arguments: &[OsString]) -> Result<Vec<u16>, RuntimeError> {
    let mut command = Vec::new();
    append_quoted_argument(&mut command, program.as_os_str())?;
    for argument in arguments {
        command.push(u16::from(b' '));
        append_quoted_argument(&mut command, argument)?;
    }
    command.push(0);
    if command.len() > 32_767 {
        return Err(RuntimeError::Process);
    }
    Ok(command)
}

fn append_quoted_argument(command: &mut Vec<u16>, argument: &OsStr) -> Result<(), RuntimeError> {
    let units: Vec<u16> = argument.encode_wide().collect();
    if units.contains(&0) {
        return Err(RuntimeError::Process);
    }
    command.push(u16::from(b'"'));
    let mut backslashes = 0usize;
    for unit in units {
        if unit == u16::from(b'\\') {
            backslashes += 1;
        } else if unit == u16::from(b'"') {
            command.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes * 2 + 1));
            command.push(unit);
            backslashes = 0;
        } else {
            command.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes));
            command.push(unit);
            backslashes = 0;
        }
    }
    command.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes * 2));
    command.push(u16::from(b'"'));
    Ok(())
}

fn scrubbed_environment_block() -> Result<Vec<u16>, RuntimeError> {
    let mut variables: BTreeMap<String, (OsString, OsString)> = BTreeMap::new();
    for allowed in [
        "SYSTEMROOT",
        "WINDIR",
        "SYSTEMDRIVE",
        "PROGRAMDATA",
        "LOCALAPPDATA",
        "APPDATA",
        "USERPROFILE",
        "TEMP",
        "TMP",
    ] {
        if let Some((key, value)) =
            std::env::vars_os().find(|(key, _)| key.to_string_lossy().eq_ignore_ascii_case(allowed))
        {
            variables.insert(allowed.to_owned(), (key, value));
        }
    }
    variables.insert("PATH".to_owned(), (OsString::from("PATH"), OsString::new()));

    let mut block = Vec::new();
    for (_, (key, value)) in variables {
        let key: Vec<u16> = key.encode_wide().collect();
        let value: Vec<u16> = value.encode_wide().collect();
        if key.is_empty()
            || key.contains(&0)
            || key.contains(&u16::from(b'='))
            || value.contains(&0)
        {
            return Err(RuntimeError::Process);
        }
        block.extend(key);
        block.push(u16::from(b'='));
        block.extend(value);
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

impl RuntimePaths {
    pub(crate) fn resolve(
        resource_directory: &Path,
        executable_directory: &Path,
    ) -> Result<Self, RuntimeError> {
        let resource_directory = canonical_directory(resource_directory)?;
        let executable_directory = canonical_directory(executable_directory)?;
        let runtime_directory = canonical_directory_within(
            &resource_directory,
            &resource_directory.join("render-runtime"),
        )?;
        let bun_executable = canonical_file_within(
            &executable_directory,
            &executable_directory.join("render-worker-bun.exe"),
        )?;

        let runtime_receipt: RuntimeLayout =
            read_layout(&runtime_directory.join("runtime-layout.json"))?;
        if runtime_receipt != RuntimeLayout::expected() {
            return Err(RuntimeError::InvalidRuntime);
        }
        let worker_receipt: WorkerLayout =
            read_layout(&runtime_directory.join("worker-layout.json"))?;
        if !worker_receipt.is_expected() {
            return Err(RuntimeError::InvalidRuntime);
        }

        let worker_entrypoint = canonical_file_within(
            &runtime_directory,
            &runtime_directory.join("worker/index.js"),
        )?;
        let browser_executable = canonical_file_within(
            &runtime_directory,
            &runtime_directory.join("chrome-headless-shell/chrome-headless-shell.exe"),
        )?;
        let composition_directory =
            canonical_directory_within(&runtime_directory, &runtime_directory.join("composition"))?;
        canonical_file_within(
            &composition_directory,
            &composition_directory.join("index.html"),
        )?;
        let binaries_directory =
            canonical_directory_within(&runtime_directory, &runtime_directory.join("remotion"))?;
        for name in ["remotion.exe", "ffmpeg.exe", "ffprobe.exe"] {
            canonical_file_within(&binaries_directory, &binaries_directory.join(name))?;
        }

        Ok(Self {
            bun_executable,
            worker_entrypoint,
            runtime_directory,
            browser_executable,
            composition_directory,
            binaries_directory,
        })
    }

    pub(crate) fn worker_launch_spec(&self) -> LaunchSpec {
        LaunchSpec {
            program: self.bun_executable.clone(),
            arguments: vec![
                self.worker_entrypoint.as_os_str().to_owned(),
                OsString::from("--runtime-directory"),
                self.runtime_directory.as_os_str().to_owned(),
            ],
        }
    }
}

pub(crate) fn parse_worker_stdout(
    stdout: &[u8],
    expected_job_id: &str,
) -> Result<VerifiedMediaSummary, RuntimeError> {
    if stdout.is_empty()
        || stdout.len() > MAX_PROTOCOL_BYTES
        || !stdout.ends_with(b"\n")
        || !valid_job_id(expected_job_id)
    {
        return Err(RuntimeError::Protocol);
    }

    let mut started_frames = None;
    let mut last_rendered_frames = None;
    let mut terminal = None;
    let mut event_count = 0;
    for line_with_newline in stdout.split_inclusive(|byte| *byte == b'\n') {
        event_count += 1;
        if event_count > MAX_PROTOCOL_EVENTS
            || line_with_newline.len() > MAX_PROTOCOL_LINE_BYTES
            || line_with_newline.len() < 2
            || line_with_newline.contains(&b'\r')
            || terminal.is_some()
        {
            return Err(RuntimeError::Protocol);
        }
        let line = &line_with_newline[..line_with_newline.len() - 1];
        let event: WorkerEvent =
            serde_json::from_slice(line).map_err(|_| RuntimeError::Protocol)?;
        match event {
            WorkerEvent::Started {
                schema_version,
                job_id,
                total_frames,
            } => {
                if schema_version != 1
                    || job_id != expected_job_id
                    || started_frames.is_some()
                    || total_frames == 0
                    || total_frames > 216_000
                    || event_count != 1
                {
                    return Err(RuntimeError::Protocol);
                }
                started_frames = Some(total_frames);
            }
            WorkerEvent::Progress {
                schema_version,
                job_id,
                rendered_frames,
                total_frames,
            } => {
                if schema_version != 1
                    || job_id != expected_job_id
                    || started_frames != Some(total_frames)
                    || last_rendered_frames.is_some_and(|last| rendered_frames <= last)
                    || rendered_frames > total_frames
                {
                    return Err(RuntimeError::Protocol);
                }
                last_rendered_frames = Some(rendered_frames);
            }
            WorkerEvent::Succeeded {
                schema_version,
                job_id,
                media,
                staging_sha256,
            } => {
                if schema_version != 1
                    || job_id != expected_job_id
                    || started_frames != Some(media.frame_count)
                    || !media.is_expected()
                    || !valid_sha256(&staging_sha256)
                {
                    return Err(RuntimeError::Protocol);
                }
                terminal = Some(Ok((media, staging_sha256)));
            }
            WorkerEvent::Failed {
                schema_version,
                job_id,
                code,
            } => {
                if schema_version != 1 || job_id != expected_job_id {
                    return Err(RuntimeError::Protocol);
                }
                terminal = Some(Err(code));
            }
        }
    }

    match terminal {
        Some(Ok((media, staging_sha256))) => Ok(VerifiedMediaSummary {
            media: media.into(),
            staging_sha256: decode_sha256(&staging_sha256).ok_or(RuntimeError::Protocol)?,
        }),
        Some(Err(code)) => Err(RuntimeError::Worker(code)),
        None => Err(RuntimeError::Protocol),
    }
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeLayout {
    schema_version: u8,
    platform: String,
    versions: RuntimeVersions,
    paths: RuntimeLayoutPaths,
    archive_sha256: RuntimeArchiveHashes,
}

impl RuntimeLayout {
    fn expected() -> Self {
        Self {
            schema_version: 1,
            platform: "windows-x64".to_owned(),
            versions: RuntimeVersions {
                bun: "1.4.0".to_owned(),
                chrome_headless_shell: "149.0.7790.0".to_owned(),
                remotion: "4.0.516".to_owned(),
            },
            paths: RuntimeLayoutPaths {
                bun_executable: "bun/bun.exe".to_owned(),
                browser_executable: "chrome-headless-shell/chrome-headless-shell.exe".to_owned(),
                binaries_directory: "remotion".to_owned(),
                composition: "composition".to_owned(),
            },
            archive_sha256: RuntimeArchiveHashes {
                bun: "e6f093d39da486b20262ca8cdd5ed6a9e8bc9c2f275b78e6d3a0c5b28cc95901".to_owned(),
                chrome_headless_shell:
                    "8a112c0e768907ff382ff994dcde35cdbd0e1f5a3a475da93d3a83e3260e78c2".to_owned(),
                remotion_compositor:
                    "946cdc35b2d08ca83e2d4bf66f1c23656cd321ce90d8bdfee72d2ff10d8c502e".to_owned(),
            },
        }
    }
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeVersions {
    bun: String,
    chrome_headless_shell: String,
    remotion: String,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeLayoutPaths {
    bun_executable: String,
    browser_executable: String,
    binaries_directory: String,
    composition: String,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeArchiveHashes {
    bun: String,
    chrome_headless_shell: String,
    remotion_compositor: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkerLayout {
    schema_version: u8,
    target_triple: String,
    worker: WorkerLayoutEntry,
    bun_sidecar: String,
}

impl WorkerLayout {
    fn is_expected(&self) -> bool {
        self.schema_version == 1
            && self.target_triple == "x86_64-pc-windows-msvc"
            && self.worker.path == "worker/index.js"
            && self.worker.sha256.len() == SHA256_HEX_LENGTH
            && self
                .worker
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && self.bun_sidecar == "render-worker-bun.exe"
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerLayoutEntry {
    path: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
enum WorkerEvent {
    Started {
        #[serde(rename = "schemaVersion")]
        schema_version: u8,
        #[serde(rename = "jobId")]
        job_id: String,
        #[serde(rename = "totalFrames")]
        total_frames: u32,
    },
    Progress {
        #[serde(rename = "schemaVersion")]
        schema_version: u8,
        #[serde(rename = "jobId")]
        job_id: String,
        #[serde(rename = "renderedFrames")]
        rendered_frames: u32,
        #[serde(rename = "totalFrames")]
        total_frames: u32,
    },
    Succeeded {
        #[serde(rename = "schemaVersion")]
        schema_version: u8,
        #[serde(rename = "jobId")]
        job_id: String,
        media: WorkerMediaSummary,
        #[serde(rename = "stagingSha256")]
        staging_sha256: String,
    },
    Failed {
        #[serde(rename = "schemaVersion")]
        schema_version: u8,
        #[serde(rename = "jobId")]
        job_id: String,
        code: WorkerFailureCode,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkerMediaSummary {
    codec: String,
    width: u32,
    height: u32,
    pixel_format: String,
    duration_ms: u64,
    frame_count: u32,
}

impl WorkerMediaSummary {
    fn is_expected(&self) -> bool {
        self.codec == "h264"
            && self.width == 1_920
            && self.height == 1_080
            && self.pixel_format == "yuv420p"
            && self.duration_ms > 0
            && self.frame_count > 0
    }
}

impl From<WorkerMediaSummary> for MediaSummary {
    fn from(value: WorkerMediaSummary) -> Self {
        Self {
            codec: value.codec,
            width: value.width,
            height: value.height,
            pixel_format: value.pixel_format,
            duration_ms: value.duration_ms,
            frame_count: value.frame_count,
        }
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == SHA256_HEX_LENGTH
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn decode_sha256(value: &str) -> Option<[u8; 32]> {
    if !valid_sha256(value) {
        return None;
    }
    let mut decoded = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = hex_nibble(pair[0])?;
        let low = hex_nibble(pair[1])?;
        decoded[index] = (high << 4) | low;
    }
    Some(decoded)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn canonical_directory(path: &Path) -> Result<PathBuf, RuntimeError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| RuntimeError::InvalidRuntime)?;
    if !metadata.is_dir() || metadata.is_symlink() {
        return Err(RuntimeError::InvalidRuntime);
    }
    canonicalize_local(path)
}

fn canonical_directory_within(root: &Path, path: &Path) -> Result<PathBuf, RuntimeError> {
    let canonical = canonical_directory(path)?;
    canonical
        .starts_with(root)
        .then_some(canonical)
        .ok_or(RuntimeError::InvalidRuntime)
}

fn canonical_file_within(root: &Path, path: &Path) -> Result<PathBuf, RuntimeError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| RuntimeError::InvalidRuntime)?;
    if !metadata.is_file() || metadata.is_symlink() {
        return Err(RuntimeError::InvalidRuntime);
    }
    let canonical = canonicalize_local(path)?;
    canonical
        .starts_with(root)
        .then_some(canonical)
        .ok_or(RuntimeError::InvalidRuntime)
}

fn read_layout<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, RuntimeError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| RuntimeError::InvalidRuntime)?;
    if !metadata.is_file() || metadata.is_symlink() || metadata.len() > MAX_LAYOUT_BYTES {
        return Err(RuntimeError::InvalidRuntime);
    }
    let bytes = fs::read(path).map_err(|_| RuntimeError::InvalidRuntime)?;
    serde_json::from_slice(&bytes).map_err(|_| RuntimeError::InvalidRuntime)
}

fn canonicalize_local(path: &Path) -> Result<PathBuf, RuntimeError> {
    let canonical = fs::canonicalize(path).map_err(|_| RuntimeError::InvalidRuntime)?;
    let units: Vec<u16> = canonical.as_os_str().encode_wide().collect();
    let verbatim_prefix = [
        u16::from(b'\\'),
        u16::from(b'\\'),
        u16::from(b'?'),
        u16::from(b'\\'),
    ];
    let normalized = if units.starts_with(&verbatim_prefix) {
        PathBuf::from(std::ffi::OsString::from_wide(
            &units[verbatim_prefix.len()..],
        ))
    } else {
        canonical
    };
    if !matches!(
        normalized.components().next(),
        Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_))
    ) {
        return Err(RuntimeError::InvalidRuntime);
    }
    Ok(normalized)
}

pub(crate) fn valid_job_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphanumeric() || (index > 0 && matches!(byte, b'_' | b'-'))
        })
}
