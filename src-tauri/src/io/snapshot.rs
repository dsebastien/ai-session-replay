use std::io::{self, Read};
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::{
    LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx, UnlockFileEx,
};
use windows_sys::Win32::System::IO::OVERLAPPED;

use super::local_path::{FileStamp, OpenedLocalFile, file_stamp};
use super::{LocalRoot, SnapshotError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotLimits {
    pub max_raw_file_bytes: u64,
    pub max_raw_total_bytes: u64,
    pub max_decoded_file_bytes: u64,
    pub max_decoded_total_bytes: u64,
    pub max_line_bytes: usize,
    pub max_records_per_file: usize,
    pub max_records_total: usize,
    pub max_members: usize,
    pub max_artifacts: usize,
    pub max_json_depth: usize,
    pub max_zstd_window_bytes: u64,
}

impl Default for SnapshotLimits {
    fn default() -> Self {
        Self {
            max_raw_file_bytes: 64 * 1024 * 1024,
            max_raw_total_bytes: 256 * 1024 * 1024,
            max_decoded_file_bytes: 128 * 1024 * 1024,
            max_decoded_total_bytes: 256 * 1024 * 1024,
            max_line_bytes: 16 * 1024 * 1024,
            max_records_per_file: 125_000,
            max_records_total: 200_000,
            max_members: 520,
            max_artifacts: 512,
            max_json_depth: 64,
            max_zstd_window_bytes: 32 * 1024 * 1024,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct FileSnapshot {
    bytes: Vec<u8>,
}

impl FileSnapshot {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(super) fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

pub fn capture_file(
    root: &LocalRoot,
    candidate: &Path,
    limits: SnapshotLimits,
) -> Result<FileSnapshot, SnapshotError> {
    capture_file_with_hooks(root, candidate, limits, || {}, || {})
}

pub(crate) fn capture_file_prefix(
    root: &LocalRoot,
    candidate: &Path,
    maximum_bytes: usize,
) -> Result<FileSnapshot, SnapshotError> {
    if maximum_bytes == 0 {
        return Err(SnapshotError::RawLimitExceeded);
    }
    let mut opened = root.open_file(candidate)?;
    let initial = opened.stamp;
    let length = initial.length.min(maximum_bytes as u64);
    let _prefix_lock = PrefixLock::acquire(&opened.file, length)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length as usize)
        .map_err(|_| SnapshotError::AllocationFailed)?;
    (&mut opened.file)
        .take(length)
        .read_to_end(&mut bytes)
        .map_err(SnapshotError::Io)?;
    if bytes.len() != length as usize {
        return Err(SnapshotError::ConcurrentTruncate);
    }
    let current = file_stamp(&opened.file)?;
    let reopened = root.open_file(candidate)?;
    if !current.same_identity(initial) || !reopened.stamp.same_identity(initial) {
        return Err(SnapshotError::ConcurrentRotate);
    }
    Ok(FileSnapshot { bytes })
}

pub(super) struct PreparedFile {
    _prefix_lock: Option<PrefixLock>,
    opened: OpenedLocalFile,
    initial: FileStamp,
}

impl PreparedFile {
    pub(super) fn raw_length(&self) -> u64 {
        self.initial.length
    }

    pub(super) fn identity(&self) -> (u64, [u8; 16]) {
        (self.initial.volume_serial, self.initial.file_id)
    }
}

pub(super) fn prepare_file(
    root: &LocalRoot,
    candidate: &Path,
    limits: SnapshotLimits,
) -> Result<PreparedFile, SnapshotError> {
    prepare_file_after_open(root, candidate, limits, || {})
}

fn prepare_file_after_open(
    root: &LocalRoot,
    candidate: &Path,
    limits: SnapshotLimits,
    after_open: impl FnOnce(),
) -> Result<PreparedFile, SnapshotError> {
    let opened = root.open_file(candidate)?;
    let initial = opened.stamp;
    if initial.length > limits.max_raw_file_bytes {
        return Err(SnapshotError::RawLimitExceeded);
    }
    usize::try_from(initial.length).map_err(|_| SnapshotError::RawLimitExceeded)?;

    after_open();
    let prefix_lock = PrefixLock::acquire(&opened.file, initial.length)?;
    let locked = file_stamp(&opened.file)?;
    if !locked.same_identity(initial) {
        return Err(SnapshotError::ConcurrentRotate);
    }
    if locked.length < initial.length {
        return Err(SnapshotError::ConcurrentTruncate);
    }
    if locked.last_write_time != initial.last_write_time {
        return Err(SnapshotError::ConcurrentChange);
    }
    Ok(PreparedFile {
        _prefix_lock: prefix_lock,
        opened,
        initial,
    })
}

pub(super) fn read_prepared(
    root: &LocalRoot,
    candidate: &Path,
    mut prepared: PreparedFile,
) -> Result<FileSnapshot, SnapshotError> {
    let capacity =
        usize::try_from(prepared.initial.length).map_err(|_| SnapshotError::RawLimitExceeded)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(capacity)
        .map_err(|_| SnapshotError::AllocationFailed)?;
    (&mut prepared.opened.file)
        .take(prepared.initial.length)
        .read_to_end(&mut bytes)
        .map_err(SnapshotError::Io)?;
    if bytes.len() != capacity {
        return Err(SnapshotError::ConcurrentTruncate);
    }

    let current = file_stamp(&prepared.opened.file)?;
    if !current.same_identity(prepared.initial) {
        return Err(SnapshotError::ConcurrentRotate);
    }
    if current.length < prepared.initial.length {
        return Err(SnapshotError::ConcurrentTruncate);
    }
    let reopened = root.open_file(candidate)?;
    if !reopened.stamp.same_identity(prepared.initial) {
        return Err(SnapshotError::ConcurrentRotate);
    }
    Ok(FileSnapshot { bytes })
}

fn capture_file_with_hooks(
    root: &LocalRoot,
    candidate: &Path,
    limits: SnapshotLimits,
    after_open: impl FnOnce(),
    after_lock: impl FnOnce(),
) -> Result<FileSnapshot, SnapshotError> {
    let prepared = prepare_file_after_open(root, candidate, limits, after_open)?;
    after_lock();
    read_prepared(root, candidate, prepared)
}

struct PrefixLock {
    handle: HANDLE,
    length: u64,
}

impl PrefixLock {
    fn acquire(file: &std::fs::File, length: u64) -> Result<Option<Self>, SnapshotError> {
        if length == 0 {
            return Ok(None);
        }
        let handle = file.as_raw_handle() as HANDLE;
        let mut overlapped = OVERLAPPED::default();
        // SAFETY: `handle` is live and `overlapped` is writable for this synchronous lock.
        if unsafe {
            LockFileEx(
                handle,
                LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                0,
                length as u32,
                (length >> 32) as u32,
                &mut overlapped,
            )
        } == 0
        {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(33) {
                Err(SnapshotError::ConcurrentChange)
            } else {
                Err(SnapshotError::Io(error))
            };
        }
        Ok(Some(Self { handle, length }))
    }
}

impl Drop for PrefixLock {
    fn drop(&mut self) {
        let mut overlapped = OVERLAPPED::default();
        // SAFETY: this releases the exact range acquired on the still-live file handle.
        unsafe {
            UnlockFileEx(
                self.handle,
                0,
                self.length as u32,
                (self.length >> 32) as u32,
                &mut overlapped,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, OpenOptions};
    use std::io::{Seek, SeekFrom, Write};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{SnapshotLimits, capture_file, capture_file_prefix, capture_file_with_hooks};
    use crate::io::{LocalRoot, SnapshotError};

    struct TempTree(PathBuf);

    impl TempTree {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should follow the epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "ai-session-snapshot-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("temporary root should be created");
            Self(path)
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fixture() -> (TempTree, PathBuf) {
        let tree = TempTree::new();
        let path = tree.0.join("session.jsonl");
        fs::write(&path, b"original").expect("fixture should be written");
        (tree, path)
    }

    #[test]
    fn enforces_the_raw_file_limit_at_the_exact_boundary() {
        let (tree, path) = fixture();
        let root = LocalRoot::new(&tree.0).expect("root should be valid");

        assert_eq!(
            capture_file(
                &root,
                &path,
                SnapshotLimits {
                    max_raw_file_bytes: 8,
                    ..SnapshotLimits::default()
                }
            )
            .expect("boundary-sized file should be accepted")
            .bytes(),
            b"original"
        );
        assert!(matches!(
            capture_file(
                &root,
                &path,
                SnapshotLimits {
                    max_raw_file_bytes: 7,
                    ..SnapshotLimits::default()
                }
            ),
            Err(SnapshotError::RawLimitExceeded)
        ));
    }

    #[test]
    fn captures_only_the_requested_prefix_of_a_larger_file() {
        let (tree, path) = fixture();
        let root = LocalRoot::new(&tree.0).expect("root should be valid");

        assert_eq!(
            capture_file_prefix(&root, &path, 4)
                .expect("bounded prefix should be captured")
                .bytes(),
            b"orig"
        );
        assert!(matches!(
            capture_file_prefix(&root, &path, 0),
            Err(SnapshotError::RawLimitExceeded)
        ));
    }

    #[test]
    fn ignores_bytes_appended_after_the_initial_length() {
        let (tree, path) = fixture();
        let root = LocalRoot::new(&tree.0).expect("root should be valid");

        let snapshot = capture_file_with_hooks(
            &root,
            &path,
            SnapshotLimits::default(),
            || {},
            || {
                let mut writer = OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .expect("fixture should open for append");
                writer
                    .write_all(b"-appended")
                    .expect("append should succeed");
            },
        )
        .expect("append-only change should be accepted");

        assert_eq!(snapshot.bytes(), b"original");
    }

    #[test]
    fn rejects_truncation_after_the_initial_length() {
        let (tree, path) = fixture();
        let root = LocalRoot::new(&tree.0).expect("root should be valid");

        let result = capture_file_with_hooks(
            &root,
            &path,
            SnapshotLimits::default(),
            || {
                OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(&path)
                    .expect("fixture should be truncated");
            },
            || {},
        );

        assert!(matches!(result, Err(SnapshotError::ConcurrentTruncate)));
    }

    #[test]
    fn rejects_same_length_overwrites() {
        let (tree, path) = fixture();
        let root = LocalRoot::new(&tree.0).expect("root should be valid");

        let result = capture_file_with_hooks(
            &root,
            &path,
            SnapshotLimits::default(),
            || {
                let mut writer = OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .expect("fixture should open for overwrite");
                writer
                    .seek(SeekFrom::Start(0))
                    .expect("seek should succeed");
                writer
                    .write_all(b"replaced")
                    .expect("overwrite should succeed");
                writer.flush().expect("overwrite should flush");
            },
            || {},
        );

        assert!(matches!(result, Err(SnapshotError::ConcurrentChange)));
    }

    #[test]
    fn rejects_rotation_and_replacement() {
        let (tree, path) = fixture();
        let root = LocalRoot::new(&tree.0).expect("root should be valid");
        let rotated = tree.0.join("rotated.jsonl");

        let result = capture_file_with_hooks(
            &root,
            &path,
            SnapshotLimits::default(),
            || {},
            || {
                fs::rename(&path, rotated).expect("fixture should rotate");
                fs::write(&path, b"new-file").expect("replacement should be written");
            },
        );

        assert!(matches!(result, Err(SnapshotError::ConcurrentRotate)));
    }

    #[test]
    fn locks_the_snapshot_prefix_against_overwrite() {
        let (tree, path) = fixture();
        let root = LocalRoot::new(&tree.0).expect("root should be valid");

        let snapshot = capture_file_with_hooks(
            &root,
            &path,
            SnapshotLimits::default(),
            || {},
            || {
                let mut writer = OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .expect("fixture should open for overwrite");
                let error = writer
                    .write_all(b"replaced")
                    .expect_err("the locked prefix must reject overwrite");
                assert_eq!(error.raw_os_error(), Some(33));
            },
        )
        .expect("locked snapshot should remain stable");

        assert_eq!(snapshot.bytes(), b"original");
    }
}
