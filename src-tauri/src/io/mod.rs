mod bundle;
mod local_path;
mod snapshot;

pub use bundle::{
    BundleMemberSpec, ContentKind, MemberCompression, MemberRole, SessionSnapshotBundle,
    SnapshotMemberView, capture_bundle,
};
#[cfg(test)]
pub(crate) use bundle::{SyntheticMember, capture_bundle_with_hook};
pub use local_path::LocalRoot;
pub(crate) use local_path::OpenedDeletionTarget;
#[cfg(test)]
pub(crate) use local_path::validate_new_local_file;
pub(crate) use local_path::{lock_new_local_file_destination, volume_supports_atomic_links};
pub(crate) use snapshot::capture_file_prefix;
pub use snapshot::{FileSnapshot, SnapshotLimits, capture_file};

use std::io;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("The path is not a supported local Windows path")]
    InvalidPath,
    #[error("The path is outside its approved session root")]
    OutsideRoot,
    #[error("The path is not on a local Windows volume")]
    NonLocalVolume,
    #[error("The session file is not a regular file")]
    NotFile,
    #[error("The session file exceeds the configured byte limit")]
    RawLimitExceeded,
    #[error("The session bundle exceeds the configured byte limit")]
    AggregateLimitExceeded,
    #[error("The session bundle contains too many members")]
    MemberLimitExceeded,
    #[error("The session bundle contains too many artifacts")]
    ArtifactLimitExceeded,
    #[error("Decoded session data exceeds the configured byte limit")]
    DecodedLimitExceeded,
    #[error("A session record exceeds the configured line limit")]
    LineLimitExceeded,
    #[error("The session contains too many records")]
    RecordLimitExceeded,
    #[error("A session record exceeds the configured nesting limit")]
    NestingLimitExceeded,
    #[error("The compressed session data is invalid or unsupported")]
    InvalidZstd,
    #[error("The session file was truncated while it was being read")]
    ConcurrentTruncate,
    #[error("The session file changed while it was being read")]
    ConcurrentChange,
    #[error("The session file was replaced while it was being read")]
    ConcurrentRotate,
    #[error("Memory could not be reserved for the session file")]
    AllocationFailed,
    #[error("The session file could not be read")]
    Io(#[source] io::Error),
}
