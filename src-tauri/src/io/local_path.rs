use std::ffi::c_void;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Component, Path, PathBuf, Prefix};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_INFO,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_SEQUENTIAL_SCAN,
    FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FileDispositionInfo, FileIdInfo, GetDriveTypeW, GetFileInformationByHandle,
    GetFileInformationByHandleEx, GetFinalPathNameByHandleW, GetVolumeInformationW,
    SECURITY_IDENTIFICATION, SetFileInformationByHandle,
};
use windows_sys::Win32::System::WindowsProgramming::{DRIVE_FIXED, DRIVE_RAMDISK, DRIVE_REMOVABLE};

use super::SnapshotError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStamp {
    pub volume_serial: u64,
    pub file_id: [u8; 16],
    pub length: u64,
    pub last_write_time: u64,
    pub link_count: u32,
}

impl FileStamp {
    pub fn same_identity(self, other: Self) -> bool {
        self.volume_serial == other.volume_serial && self.file_id == other.file_id
    }
}

#[derive(Debug)]
pub struct OpenedLocalFile {
    pub file: File,
    pub canonical_path: PathBuf,
    pub stamp: FileStamp,
}

#[derive(Debug)]
pub(crate) struct OpenedDeletionTarget {
    file: File,
    canonical_path: PathBuf,
    stamp: FileStamp,
}

#[derive(Debug)]
pub(crate) struct OpenedPublicationTarget {
    file: File,
    canonical_path: PathBuf,
    stamp: FileStamp,
}

impl OpenedPublicationTarget {
    pub(crate) fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    pub(crate) fn canonical_path(&self) -> &Path {
        &self.canonical_path
    }

    pub(crate) fn revalidate(&self, expected_link_count: u32) -> Result<(), SnapshotError> {
        let current = file_stamp(&self.file)?;
        if final_path(&self.file)? != self.canonical_path
            || !current.same_identity(self.stamp)
            || current.length != self.stamp.length
            || current.last_write_time != self.stamp.last_write_time
            || current.link_count != expected_link_count
        {
            return Err(SnapshotError::OutsideRoot);
        }
        Ok(())
    }

    pub(crate) fn delete_link(self) -> Result<(), SnapshotError> {
        self.revalidate(2)?;
        let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        // SAFETY: the live handle has DELETE access and holds the exact staging
        // directory entry without delete or write sharing.
        if unsafe {
            SetFileInformationByHandle(
                self.file.as_raw_handle() as HANDLE,
                FileDispositionInfo,
                (&raw const disposition).cast::<c_void>(),
                size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        } == 0
        {
            return Err(SnapshotError::Io(io::Error::last_os_error()));
        }
        drop(self);
        Ok(())
    }
}

impl OpenedDeletionTarget {
    pub(crate) fn canonical_path(&self) -> &Path {
        &self.canonical_path
    }

    pub(crate) fn stamp(&self) -> FileStamp {
        self.stamp
    }

    pub(crate) fn delete(self) -> Result<(), SnapshotError> {
        if final_path(&self.file)? != self.canonical_path
            || !file_stamp(&self.file)?.same_identity(self.stamp)
        {
            return Err(SnapshotError::OutsideRoot);
        }
        let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        // The handle was opened with DELETE access and without delete sharing,
        // so the validated identity cannot be renamed or replaced before this
        // handle-based deletion. Source: https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-setfileinformationbyhandle
        if unsafe {
            SetFileInformationByHandle(
                self.file.as_raw_handle() as HANDLE,
                FileDispositionInfo,
                (&raw const disposition).cast::<c_void>(),
                size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        } == 0
        {
            return Err(SnapshotError::Io(io::Error::last_os_error()));
        }
        drop(self);
        Ok(())
    }
}

#[derive(Debug)]
pub struct LocalRoot {
    handle: File,
    canonical_path: PathBuf,
    identity: FileStamp,
}

impl LocalRoot {
    pub fn new(path: impl AsRef<Path>) -> Result<Self, SnapshotError> {
        Self::open(path.as_ref(), false, true)
    }

    pub(crate) fn new_locked(path: impl AsRef<Path>) -> Result<Self, SnapshotError> {
        Self::open(path.as_ref(), true, true)
    }

    pub(crate) fn new_resolved(path: impl AsRef<Path>) -> Result<Self, SnapshotError> {
        Self::open(path.as_ref(), false, false)
    }

    fn new_locked_resolved(path: impl AsRef<Path>) -> Result<Self, SnapshotError> {
        Self::open(path.as_ref(), true, false)
    }

    fn open(
        path: &Path,
        lock_against_rename: bool,
        reject_redirects: bool,
    ) -> Result<Self, SnapshotError> {
        validate_input_path(path)?;
        validate_local_volume(path)?;
        if reject_redirects {
            reject_reparse_components(path)?;
        }
        let handle = if lock_against_rename {
            open_directory_locked(path)?
        } else {
            open_directory(path)?
        };
        if !handle.metadata().map_err(SnapshotError::Io)?.is_dir() {
            return Err(SnapshotError::InvalidPath);
        }
        let canonical_path = final_path(&handle)?;
        validate_local_volume(&canonical_path)?;
        let identity = file_stamp(&handle)?;
        Ok(Self {
            handle,
            canonical_path,
            identity,
        })
    }

    pub fn open_file(&self, candidate: impl AsRef<Path>) -> Result<OpenedLocalFile, SnapshotError> {
        validate_input_path(candidate.as_ref())?;
        validate_local_volume(candidate.as_ref())?;
        reject_reparse_components(candidate.as_ref())?;
        self.revalidate()?;
        if !fs::metadata(candidate.as_ref())
            .map_err(SnapshotError::Io)?
            .is_file()
        {
            return Err(SnapshotError::NotFile);
        }
        let file = open_regular_file(candidate.as_ref())?;
        let canonical_path = final_path(&file)?;
        if !path_starts_with(&canonical_path, &self.canonical_path) {
            return Err(SnapshotError::OutsideRoot);
        }
        validate_local_volume(&canonical_path)?;
        if !file.metadata().map_err(SnapshotError::Io)?.is_file() {
            return Err(SnapshotError::NotFile);
        }
        let stamp = file_stamp(&file)?;
        if stamp.link_count != 1 {
            return Err(SnapshotError::InvalidPath);
        }
        self.revalidate()?;
        if final_path(&file)? != canonical_path {
            return Err(SnapshotError::OutsideRoot);
        }
        let reopened = open_regular_file(candidate.as_ref())?;
        if final_path(&reopened)? != canonical_path || !file_stamp(&reopened)?.same_identity(stamp)
        {
            return Err(SnapshotError::OutsideRoot);
        }
        Ok(OpenedLocalFile {
            file,
            canonical_path,
            stamp,
        })
    }

    pub(crate) fn open_file_for_deletion(
        &self,
        candidate: impl AsRef<Path>,
    ) -> Result<OpenedDeletionTarget, SnapshotError> {
        self.open_deletion_target(candidate.as_ref(), false, false)
    }

    pub(crate) fn open_staging_link_for_deletion(
        &self,
        candidate: impl AsRef<Path>,
    ) -> Result<OpenedDeletionTarget, SnapshotError> {
        self.open_deletion_target(candidate.as_ref(), false, true)
    }

    pub(crate) fn open_directory_for_deletion(
        &self,
        candidate: impl AsRef<Path>,
    ) -> Result<OpenedDeletionTarget, SnapshotError> {
        self.open_deletion_target(candidate.as_ref(), true, false)
    }

    pub(crate) fn open_file_for_publication(
        &self,
        candidate: impl AsRef<Path>,
    ) -> Result<OpenedPublicationTarget, SnapshotError> {
        let candidate = candidate.as_ref();
        validate_input_path(candidate)?;
        validate_local_volume(candidate)?;
        reject_reparse_components(candidate)?;
        self.revalidate()?;
        let file = open_for_publication(candidate)?;
        let metadata = file.metadata().map_err(SnapshotError::Io)?;
        if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(SnapshotError::NotFile);
        }
        let canonical_path = final_path(&file)?;
        if !path_starts_with(&canonical_path, &self.canonical_path) {
            return Err(SnapshotError::OutsideRoot);
        }
        let stamp = file_stamp(&file)?;
        if stamp.link_count != 1 || stamp.length == 0 {
            return Err(SnapshotError::InvalidPath);
        }
        self.revalidate()?;
        Ok(OpenedPublicationTarget {
            file,
            canonical_path,
            stamp,
        })
    }

    pub(crate) fn canonical_path(&self) -> &Path {
        &self.canonical_path
    }

    pub(crate) fn contains_canonical_path(&self, path: &Path) -> bool {
        path_starts_with(path, &self.canonical_path)
    }

    fn open_deletion_target(
        &self,
        candidate: &Path,
        directory: bool,
        allow_additional_links: bool,
    ) -> Result<OpenedDeletionTarget, SnapshotError> {
        validate_input_path(candidate)?;
        validate_local_volume(candidate)?;
        reject_reparse_components(candidate)?;
        self.revalidate()?;
        let file = open_for_deletion(candidate, directory)?;
        let metadata = file.metadata().map_err(SnapshotError::Io)?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || (directory && !metadata.is_dir())
            || (!directory && !metadata.is_file())
        {
            return Err(if directory {
                SnapshotError::InvalidPath
            } else {
                SnapshotError::NotFile
            });
        }
        let canonical_path = final_path(&file)?;
        if canonical_path == self.canonical_path
            || !path_starts_with(&canonical_path, &self.canonical_path)
        {
            return Err(SnapshotError::OutsideRoot);
        }
        let stamp = file_stamp(&file)?;
        if !directory && !allow_additional_links && stamp.link_count != 1 {
            return Err(SnapshotError::InvalidPath);
        }
        self.revalidate()?;
        Ok(OpenedDeletionTarget {
            file,
            canonical_path,
            stamp,
        })
    }

    pub(crate) fn identity(&self) -> (u64, [u8; 16]) {
        (self.identity.volume_serial, self.identity.file_id)
    }

    pub(crate) fn revalidate(&self) -> Result<(), SnapshotError> {
        if final_path(&self.handle)? != self.canonical_path
            || !file_stamp(&self.handle)?.same_identity(self.identity)
        {
            return Err(SnapshotError::OutsideRoot);
        }
        let reopened = open_directory(&self.canonical_path)?;
        if final_path(&reopened)? != self.canonical_path
            || !file_stamp(&reopened)?.same_identity(self.identity)
        {
            return Err(SnapshotError::OutsideRoot);
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn validate_new_local_file(path: &Path) -> Result<PathBuf, SnapshotError> {
    validate_input_path(path)?;
    validate_local_volume(path)?;
    let parent = path.parent().ok_or(SnapshotError::InvalidPath)?;
    let file_name = path.file_name().ok_or(SnapshotError::InvalidPath)?;
    validate_component(file_name)?;
    let root = LocalRoot::new(parent)?;
    let normalized = root.canonical_path().join(file_name);
    if normalized.exists() {
        return Err(SnapshotError::InvalidPath);
    }
    Ok(normalized)
}

pub(crate) fn lock_new_local_file_destination(
    path: &Path,
) -> Result<(LocalRoot, PathBuf), SnapshotError> {
    validate_input_path(path)?;
    validate_local_volume(path)?;
    let parent = path.parent().ok_or(SnapshotError::InvalidPath)?;
    let file_name = path.file_name().ok_or(SnapshotError::InvalidPath)?;
    validate_component(file_name)?;
    // Save dialogs commonly return Desktop/Documents paths redirected through
    // OneDrive reparse points. Follow that user-selected redirect once, then
    // pin the resolved local directory by handle for the whole publication.
    let root = LocalRoot::new_locked_resolved(parent)?;
    let normalized = root.canonical_path().join(file_name);
    if normalized.try_exists().map_err(SnapshotError::Io)? {
        return Err(SnapshotError::InvalidPath);
    }
    Ok((root, normalized))
}

pub(crate) fn volume_supports_atomic_links(path: &Path) -> Result<bool, SnapshotError> {
    let Some(Component::Prefix(prefix)) = path.components().next() else {
        return Err(SnapshotError::InvalidPath);
    };
    let Prefix::Disk(letter) = prefix.kind() else {
        return Err(SnapshotError::InvalidPath);
    };
    let root = [u16::from(letter), u16::from(b':'), u16::from(b'\\'), 0];
    let mut flags = 0_u32;
    // SAFETY: `root` is terminated and all optional output buffers are null.
    if unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &raw mut flags,
            std::ptr::null_mut(),
            0,
        )
    } == 0
    {
        return Err(SnapshotError::Io(io::Error::last_os_error()));
    }
    Ok(flags & 0x0040_0000 != 0)
}

pub fn file_stamp(file: &File) -> Result<FileStamp, SnapshotError> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    let mut id = FILE_ID_INFO::default();
    let handle = file.as_raw_handle() as HANDLE;
    // SAFETY: `handle` belongs to the live `File` and `info` is writable.
    if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
        return Err(SnapshotError::Io(io::Error::last_os_error()));
    }
    // SAFETY: `handle` is live and `id` is writable for its declared size.
    if unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            (&raw mut id).cast::<c_void>(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
    {
        return Err(SnapshotError::Io(io::Error::last_os_error()));
    }
    Ok(FileStamp {
        volume_serial: id.VolumeSerialNumber,
        file_id: id.FileId.Identifier,
        length: (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow),
        last_write_time: (u64::from(info.ftLastWriteTime.dwHighDateTime) << 32)
            | u64::from(info.ftLastWriteTime.dwLowDateTime),
        link_count: info.nNumberOfLinks,
    })
}

fn open_directory(path: &Path) -> Result<File, SnapshotError> {
    let mut options = OpenOptions::new();
    options
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .security_qos_flags(SECURITY_IDENTIFICATION);
    options.open(path).map_err(SnapshotError::Io)
}

fn open_directory_locked(path: &Path) -> Result<File, SnapshotError> {
    let mut options = OpenOptions::new();
    options
        .access_mode(DELETE | FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .security_qos_flags(SECURITY_IDENTIFICATION);
    options.open(path).map_err(SnapshotError::Io)
}

fn open_regular_file(path: &Path) -> Result<File, SnapshotError> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_SEQUENTIAL_SCAN)
        .security_qos_flags(SECURITY_IDENTIFICATION);
    options.open(path).map_err(SnapshotError::Io)
}

fn open_for_deletion(path: &Path, directory: bool) -> Result<File, SnapshotError> {
    let mut options = OpenOptions::new();
    options
        .access_mode(DELETE | FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(
            FILE_FLAG_OPEN_REPARSE_POINT
                | if directory {
                    FILE_FLAG_BACKUP_SEMANTICS
                } else {
                    FILE_FLAG_SEQUENTIAL_SCAN
                },
        )
        .security_qos_flags(SECURITY_IDENTIFICATION);
    options.open(path).map_err(SnapshotError::Io)
}

fn open_for_publication(path: &Path) -> Result<File, SnapshotError> {
    let mut options = OpenOptions::new();
    options
        .access_mode(DELETE | FILE_READ_ATTRIBUTES | FILE_READ_DATA)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_SEQUENTIAL_SCAN)
        .security_qos_flags(SECURITY_IDENTIFICATION);
    options.open(path).map_err(SnapshotError::Io)
}

fn final_path(file: &File) -> Result<PathBuf, SnapshotError> {
    let handle = file.as_raw_handle() as HANDLE;
    let mut buffer = vec![0_u16; 512];
    loop {
        // SAFETY: `handle` is live and the buffer is valid for the supplied length.
        let length = unsafe {
            GetFinalPathNameByHandleW(handle, buffer.as_mut_ptr(), buffer.len() as u32, 0)
        };
        if length == 0 {
            return Err(SnapshotError::Io(io::Error::last_os_error()));
        }
        if length as usize >= buffer.len() {
            buffer.resize(length as usize + 1, 0);
            continue;
        }
        let path = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length as usize]));
        return normalize_canonical_path(&path);
    }
}

fn validate_input_path(path: &Path) -> Result<(), SnapshotError> {
    let units: Vec<u16> = path.as_os_str().encode_wide().collect();
    if !path.is_absolute() || units.len() > 32_767 {
        return Err(SnapshotError::InvalidPath);
    }
    let raw = String::from_utf16(&units).map_err(|_| SnapshotError::InvalidPath)?;
    let normalized = raw.replace('/', "\\");
    if normalized.get(3..).is_none_or(|tail| {
        tail.split('\\')
            .any(|component| matches!(component, "" | "." | ".."))
    }) {
        return Err(SnapshotError::InvalidPath);
    }
    let mut components = path.components();
    if !matches!(
        components.next(),
        Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_))
    ) || !matches!(components.next(), Some(Component::RootDir))
    {
        return Err(SnapshotError::InvalidPath);
    }
    let mut normal_count = 0;
    for component in components {
        let Component::Normal(name) = component else {
            return Err(SnapshotError::InvalidPath);
        };
        validate_component(name)?;
        normal_count += 1;
        if normal_count > 128 {
            return Err(SnapshotError::InvalidPath);
        }
    }
    (normal_count > 0)
        .then_some(())
        .ok_or(SnapshotError::InvalidPath)
}

fn validate_component(name: &std::ffi::OsStr) -> Result<(), SnapshotError> {
    let units: Vec<u16> = name.encode_wide().collect();
    if units.is_empty() || units.len() > 255 {
        return Err(SnapshotError::InvalidPath);
    }
    let value = String::from_utf16(&units).map_err(|_| SnapshotError::InvalidPath)?;
    if value.ends_with([' ', '.'])
        || value
            .chars()
            .any(|character| character <= '\u{1f}' || r#"<>:"|?*"#.contains(character))
    {
        return Err(SnapshotError::InvalidPath);
    }
    let upper = value
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || upper
            .strip_prefix("COM")
            .or_else(|| upper.strip_prefix("LPT"))
            .is_some_and(|digit| {
                matches!(
                    digit,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
    {
        return Err(SnapshotError::InvalidPath);
    }
    Ok(())
}

fn normalize_canonical_path(path: &Path) -> Result<PathBuf, SnapshotError> {
    let units: Vec<u16> = path.as_os_str().encode_wide().collect();
    let prefix = [
        u16::from(b'\\'),
        u16::from(b'\\'),
        u16::from(b'?'),
        u16::from(b'\\'),
    ];
    let normalized = if units.starts_with(&prefix) {
        PathBuf::from(std::ffi::OsString::from_wide(&units[prefix.len()..]))
    } else {
        path.to_path_buf()
    };
    validate_input_path(&normalized)?;
    if !matches!(
        normalized.components().next(),
        Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_))
    ) {
        return Err(SnapshotError::InvalidPath);
    }
    Ok(normalized)
}

fn validate_local_volume(path: &Path) -> Result<(), SnapshotError> {
    let Some(Component::Prefix(prefix)) = path.components().next() else {
        return Err(SnapshotError::InvalidPath);
    };
    let Prefix::Disk(letter) = prefix.kind() else {
        return Err(SnapshotError::InvalidPath);
    };
    let root = [u16::from(letter), u16::from(b':'), u16::from(b'\\'), 0];
    // SAFETY: `root` is a terminated UTF-16 drive-root string.
    let drive_type = unsafe { GetDriveTypeW(root.as_ptr()) };
    if is_allowed_drive_type(drive_type) {
        Ok(())
    } else {
        Err(SnapshotError::NonLocalVolume)
    }
}

fn reject_reparse_components(path: &Path) -> Result<(), SnapshotError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        if matches!(component, Component::Normal(_)) {
            let metadata = fs::symlink_metadata(&current).map_err(SnapshotError::Io)?;
            if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(SnapshotError::InvalidPath);
            }
        }
    }
    Ok(())
}

fn is_allowed_drive_type(drive_type: u32) -> bool {
    matches!(drive_type, DRIVE_FIXED | DRIVE_REMOVABLE | DRIVE_RAMDISK)
}

fn path_starts_with(path: &Path, root: &Path) -> bool {
    let mut path_components = path.components();
    root.components().enumerate().all(|(index, expected)| {
        path_components.next().is_some_and(|actual| {
            if index == 0 {
                actual
                    .as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&expected.as_os_str().to_string_lossy())
            } else {
                actual == expected
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use windows_sys::Win32::System::WindowsProgramming::DRIVE_REMOTE;

    use super::{
        LocalRoot, SnapshotError, is_allowed_drive_type, lock_new_local_file_destination,
        normalize_canonical_path, validate_new_local_file, volume_supports_atomic_links,
    };

    struct TempTree(PathBuf);

    impl TempTree {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should follow the epoch")
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("ai-session-replay-{}-{nonce}", std::process::id()));
            fs::create_dir(&path).expect("temporary root should be created");
            Self(path)
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn opens_only_regular_files_inside_the_root() {
        let tree = TempTree::new();
        let file_path = tree.0.join("session.jsonl");
        fs::write(&file_path, b"event").expect("fixture should be written");
        let root = LocalRoot::new(&tree.0).expect("root should be valid");

        let opened = root.open_file(&file_path).expect("file should be accepted");

        assert_eq!(opened.stamp.length, 5);
        assert_eq!(opened.canonical_path, file_path);
        assert!(matches!(
            root.open_file(&tree.0),
            Err(SnapshotError::NotFile)
        ));
    }

    #[test]
    fn rejects_paths_outside_the_approved_root() {
        let root_tree = TempTree::new();
        let outside_tree = TempTree::new();
        let outside_file = outside_tree.0.join("session.jsonl");
        fs::write(&outside_file, b"event").expect("fixture should be written");
        let root = LocalRoot::new(&root_tree.0).expect("root should be valid");

        assert!(matches!(
            root.open_file(outside_file),
            Err(SnapshotError::OutsideRoot)
        ));
    }

    #[test]
    fn rejects_relative_unc_device_and_mapped_network_paths() {
        for path in [
            PathBuf::from("relative\\session.jsonl"),
            PathBuf::from(r"C:relative\session.jsonl"),
            PathBuf::from(r"\\server\share\session.jsonl"),
            PathBuf::from(r"\\?\C:\session.jsonl"),
            PathBuf::from(r"\\.\C:\session.jsonl"),
            PathBuf::from(r"C:\safe\.\session.jsonl"),
            PathBuf::from(r"C:\safe\..\session.jsonl"),
            PathBuf::from(r"C:\safe\session.jsonl:stream"),
            PathBuf::from(r"C:\safe\NUL.txt"),
            PathBuf::from(r"C:\safe\COM¹.txt"),
            PathBuf::from(r"C:\safe\LPT³.log"),
            PathBuf::from(r"C:\safe\trailing."),
        ] {
            assert!(matches!(
                LocalRoot::new(path),
                Err(SnapshotError::InvalidPath)
            ));
        }
        assert!(!is_allowed_drive_type(DRIVE_REMOTE));
    }

    #[test]
    fn normalizes_a_new_file_beneath_an_existing_local_directory() {
        let tree = TempTree::new();
        let candidate = tree.0.join("new-output.mp4");
        let normalized = validate_new_local_file(&candidate).unwrap();
        assert!(normalized.is_absolute());
        assert!(!normalized.to_string_lossy().starts_with(r"\\?\"));
        assert!(normalized.ends_with("new-output.mp4"));
        assert!(volume_supports_atomic_links(&normalized).unwrap());

        fs::write(&candidate, b"existing").unwrap();
        assert!(validate_new_local_file(&candidate).is_err());
    }

    #[test]
    fn locks_a_resolved_export_parent_until_publication_finishes() {
        let tree = TempTree::new();
        let candidate = tree.0.join("safe-output.mp4");
        let (root, normalized) = lock_new_local_file_destination(&candidate).unwrap();

        assert_eq!(normalized.parent(), Some(root.canonical_path()));
        assert!(normalized.ends_with("safe-output.mp4"));
        root.revalidate().unwrap();
    }

    #[test]
    fn follows_a_user_selected_local_directory_redirect_for_export() {
        use std::os::windows::fs::symlink_dir;

        let tree = TempTree::new();
        let target = tree.0.join("redirect-target");
        let redirect = tree.0.join("redirect");
        fs::create_dir(&target).unwrap();
        if symlink_dir(&target, &redirect).is_err() {
            return;
        }

        let (root, normalized) =
            lock_new_local_file_destination(&redirect.join("safe-output.mp4")).unwrap();
        assert_eq!(normalized.parent(), Some(root.canonical_path()));
        assert_eq!(root.canonical_path(), fs::canonicalize(&target).unwrap());
    }

    #[test]
    fn validates_authoritative_handle_path_names() {
        for path in [
            PathBuf::from(r"\\?\UNC\server\share\session.jsonl"),
            PathBuf::from(r"\\?\C:\safe\NUL.txt"),
            PathBuf::from(r"\\?\C:\safe\trailing."),
        ] {
            assert!(matches!(
                normalize_canonical_path(&path),
                Err(SnapshotError::InvalidPath)
            ));
        }
    }

    #[test]
    fn rejects_files_with_additional_hard_links() {
        let tree = TempTree::new();
        let file_path = tree.0.join("session.jsonl");
        let second_path = tree.0.join("session-copy.jsonl");
        fs::write(&file_path, b"event").expect("fixture should be written");
        fs::hard_link(&file_path, second_path).expect("hard link should be created");
        let root = LocalRoot::new(&tree.0).expect("root should be valid");

        assert!(matches!(
            root.open_file(file_path),
            Err(SnapshotError::InvalidPath)
        ));
    }

    #[test]
    fn rejects_reparse_links_that_escape_the_root() {
        let root_tree = TempTree::new();
        let outside_tree = TempTree::new();
        let outside_file = outside_tree.0.join("session.jsonl");
        fs::write(&outside_file, b"event").expect("fixture should be written");
        let link = root_tree.0.join("escape");
        if let Err(error) = std::os::windows::fs::symlink_dir(&outside_tree.0, &link) {
            if error.kind() == std::io::ErrorKind::PermissionDenied
                || error.raw_os_error() == Some(1_314)
            {
                return;
            }
            panic!("directory link should be created: {error}");
        }
        let root = LocalRoot::new(&root_tree.0).expect("root should be valid");

        assert!(matches!(
            root.open_file(link.join("session.jsonl")),
            Err(SnapshotError::InvalidPath)
        ));
    }

    #[test]
    fn deletes_the_opened_file_identity_without_touching_siblings() {
        let tree = TempTree::new();
        let target = tree.0.join("session.jsonl");
        let sibling = tree.0.join("keep.jsonl");
        fs::write(&target, b"delete me").expect("target should be written");
        fs::write(&sibling, b"keep me").expect("sibling should be written");
        let root = LocalRoot::new(&tree.0).expect("root should be valid");
        assert_eq!(root.canonical_path(), tree.0.as_path());

        let opened = root
            .open_file_for_deletion(&target)
            .expect("target should open for deletion");
        assert_eq!(opened.canonical_path(), target.as_path());
        assert_eq!(opened.stamp().length, 9);
        assert!(fs::rename(&target, tree.0.join("swapped.jsonl")).is_err());
        opened.delete().expect("opened identity should be deleted");

        assert!(!target.exists());
        assert!(sibling.exists());
    }

    #[test]
    fn deletes_only_an_empty_opened_directory_and_rejects_the_root() {
        let tree = TempTree::new();
        let session = tree.0.join("session");
        fs::create_dir(&session).expect("session directory should be created");
        let root = LocalRoot::new(&tree.0).expect("root should be valid");

        assert!(root.open_directory_for_deletion(&tree.0).is_err());
        let opened = root
            .open_directory_for_deletion(&session)
            .expect("child directory should open for deletion");
        opened.delete().expect("empty directory should be deleted");

        assert!(!session.exists());
        assert!(tree.0.exists());
    }

    #[test]
    fn refuses_to_mark_a_nonempty_directory_for_deletion() {
        let tree = TempTree::new();
        let session = tree.0.join("session");
        fs::create_dir(&session).expect("session directory should be created");
        fs::write(session.join("events.jsonl"), b"event").expect("event should be written");
        let root = LocalRoot::new(&tree.0).expect("root should be valid");

        let opened = root
            .open_directory_for_deletion(&session)
            .expect("session directory should open");
        assert!(opened.delete().is_err());
        assert!(session.exists());
    }
}
