use std::collections::HashSet;
use std::fmt;
use std::io::{Cursor, Read};
use std::ops::Range;
use std::path::PathBuf;

use ruzstd::decoding::StreamingDecoder;

use super::snapshot::{prepare_file, read_prepared};
use super::{LocalRoot, SnapshotError, SnapshotLimits};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberCompression {
    None,
    Zstandard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentKind {
    Bytes,
    JsonLines,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberRole {
    Primary,
    Metadata,
    Artifact,
}

#[derive(Debug, Clone)]
pub struct BundleMemberSpec {
    pub id: String,
    pub logical_name: String,
    pub path: PathBuf,
    pub role: MemberRole,
    pub compression: MemberCompression,
    pub content_kind: ContentKind,
    /// If set, the captured file's identity must match this discovery-time
    /// identity. A mismatch means the file was replaced between discovery
    /// and load.
    pub expected_identity: Option<(u64, [u8; 16])>,
}

struct SnapshotMember {
    id: String,
    logical_name: String,
    role: MemberRole,
    bytes: Vec<u8>,
    record_ranges: Vec<Range<usize>>,
    partial_final_record_ignored: bool,
}

impl fmt::Debug for SnapshotMember {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SnapshotMember")
            .field("id", &self.id)
            .field("logical_name", &self.logical_name)
            .field("role", &self.role)
            .field("decoded_byte_count", &self.bytes.len())
            .field("record_count", &self.record_ranges.len())
            .field(
                "partial_final_record_ignored",
                &self.partial_final_record_ignored,
            )
            .finish()
    }
}

#[derive(Debug)]
pub struct SessionSnapshotBundle {
    members: Vec<SnapshotMember>,
    record_count: usize,
}

impl SessionSnapshotBundle {
    pub fn members(&self) -> impl ExactSizeIterator<Item = SnapshotMemberView<'_>> {
        self.members.iter().map(SnapshotMemberView::from)
    }

    pub fn member(&self, id: &str) -> Option<SnapshotMemberView<'_>> {
        self.members
            .iter()
            .find(|member| member.id == id)
            .map(SnapshotMemberView::from)
    }

    pub fn record_count(&self) -> usize {
        self.record_count
    }

    /// Build a synthetic bundle for testing without filesystem access.
    #[cfg(test)]
    pub fn synthetic(members: Vec<SyntheticMember>) -> Self {
        let mut record_count = 0;
        let built: Vec<SnapshotMember> = members
            .into_iter()
            .map(|m| {
                let bytes: Vec<u8> = m
                    .records
                    .iter()
                    .flat_map(|r| r.iter().chain(std::iter::once(&b'\n')))
                    .copied()
                    .collect();
                let mut offset = 0;
                let mut record_ranges = Vec::new();
                for r in &m.records {
                    record_ranges.push(offset..offset + r.len());
                    offset += r.len() + 1; // +1 for newline
                    record_count += 1;
                }
                SnapshotMember {
                    id: m.id,
                    logical_name: m.logical_name,
                    role: MemberRole::Primary,
                    bytes,
                    record_ranges,
                    partial_final_record_ignored: m.partial_final,
                }
            })
            .collect();
        Self {
            members: built,
            record_count,
        }
    }
}

/// Synthetic member specification for testing.
#[cfg(test)]
pub struct SyntheticMember {
    pub id: String,
    pub logical_name: String,
    pub records: Vec<Vec<u8>>,
    pub partial_final: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct SnapshotMemberView<'a> {
    member: &'a SnapshotMember,
}

impl<'a> SnapshotMemberView<'a> {
    pub fn id(self) -> &'a str {
        &self.member.id
    }

    pub fn logical_name(self) -> &'a str {
        &self.member.logical_name
    }

    pub fn role(self) -> MemberRole {
        self.member.role
    }

    #[cfg(test)]
    fn bytes(self) -> &'a [u8] {
        &self.member.bytes
    }

    pub fn records(self) -> impl ExactSizeIterator<Item = &'a [u8]> {
        self.member
            .record_ranges
            .iter()
            .map(|range| &self.member.bytes[range.clone()])
    }

    pub fn partial_final_record_ignored(self) -> bool {
        self.member.partial_final_record_ignored
    }
}

impl<'a> From<&'a SnapshotMember> for SnapshotMemberView<'a> {
    fn from(member: &'a SnapshotMember) -> Self {
        Self { member }
    }
}

pub fn capture_bundle(
    root: &LocalRoot,
    specs: &[BundleMemberSpec],
    limits: SnapshotLimits,
) -> Result<SessionSnapshotBundle, SnapshotError> {
    capture_bundle_impl(root, specs, limits, || {})
}

#[cfg(test)]
pub(crate) fn capture_bundle_with_hook(
    root: &LocalRoot,
    specs: &[BundleMemberSpec],
    limits: SnapshotLimits,
    after_prepare: impl FnOnce(),
) -> Result<SessionSnapshotBundle, SnapshotError> {
    capture_bundle_impl(root, specs, limits, after_prepare)
}

fn capture_bundle_impl(
    root: &LocalRoot,
    specs: &[BundleMemberSpec],
    limits: SnapshotLimits,
    after_prepare: impl FnOnce(),
) -> Result<SessionSnapshotBundle, SnapshotError> {
    validate_specs(specs, limits)?;
    let mut prepared = Vec::new();
    prepared
        .try_reserve_exact(specs.len())
        .map_err(|_| SnapshotError::AllocationFailed)?;
    let mut raw_total = 0_u64;
    let mut identities = HashSet::new();
    for spec in specs {
        let file = prepare_file(root, &spec.path, limits)?;
        raw_total = raw_total
            .checked_add(file.raw_length())
            .ok_or(SnapshotError::AggregateLimitExceeded)?;
        if raw_total > limits.max_raw_total_bytes {
            return Err(SnapshotError::AggregateLimitExceeded);
        }
        if !identities.insert(file.identity()) {
            return Err(SnapshotError::InvalidPath);
        }
        if let Some(expected) = spec.expected_identity
            && file.identity() != expected
        {
            return Err(SnapshotError::ConcurrentRotate);
        }
        prepared.push(file);
    }
    after_prepare();

    let mut members = Vec::new();
    members
        .try_reserve_exact(specs.len())
        .map_err(|_| SnapshotError::AllocationFailed)?;
    let mut decoded_total = 0_u64;
    let mut record_total = 0_usize;
    for (spec, file) in specs.iter().zip(prepared) {
        let raw = read_prepared(root, &spec.path, file)?.into_bytes();
        let bytes = match spec.compression {
            MemberCompression::None => raw,
            MemberCompression::Zstandard => decode_zstandard(&raw, limits)?,
        };
        let decoded_length =
            u64::try_from(bytes.len()).map_err(|_| SnapshotError::DecodedLimitExceeded)?;
        if decoded_length > limits.max_decoded_file_bytes {
            return Err(SnapshotError::DecodedLimitExceeded);
        }
        decoded_total = decoded_total
            .checked_add(decoded_length)
            .ok_or(SnapshotError::DecodedLimitExceeded)?;
        if decoded_total > limits.max_decoded_total_bytes {
            return Err(SnapshotError::DecodedLimitExceeded);
        }

        let (record_ranges, partial_final_record_ignored) = match spec.content_kind {
            ContentKind::Bytes => (index_bytes(&bytes, limits, &mut record_total)?, false),
            ContentKind::JsonLines => index_json_lines(&bytes, limits, &mut record_total)?,
        };
        members.push(SnapshotMember {
            id: spec.id.clone(),
            logical_name: spec.logical_name.clone(),
            role: spec.role,
            bytes,
            record_ranges,
            partial_final_record_ignored,
        });
    }

    Ok(SessionSnapshotBundle {
        members,
        record_count: record_total,
    })
}

fn validate_specs(specs: &[BundleMemberSpec], limits: SnapshotLimits) -> Result<(), SnapshotError> {
    if specs.is_empty() || specs.len() > limits.max_members {
        return Err(SnapshotError::MemberLimitExceeded);
    }
    if specs
        .iter()
        .filter(|spec| spec.role == MemberRole::Artifact)
        .count()
        > limits.max_artifacts
    {
        return Err(SnapshotError::ArtifactLimitExceeded);
    }
    let mut ids = HashSet::new();
    for spec in specs {
        if !ids.insert(spec.id.as_str())
            || !is_logical_name(&spec.id)
            || !is_logical_name(&spec.logical_name)
        {
            return Err(SnapshotError::InvalidPath);
        }
    }
    Ok(())
}

fn is_logical_name(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= 256
        && !value
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
}

fn decode_zstandard(raw: &[u8], limits: SnapshotLimits) -> Result<Vec<u8>, SnapshotError> {
    let mut decoder =
        StreamingDecoder::new_with_max_window_size(Cursor::new(raw), limits.max_zstd_window_bytes)
            .map_err(|_| SnapshotError::InvalidZstd)?;
    let mut decoded = Vec::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = decoder
            .read(&mut buffer)
            .map_err(|_| SnapshotError::InvalidZstd)?;
        if count == 0 {
            break;
        }
        let new_length = decoded
            .len()
            .checked_add(count)
            .ok_or(SnapshotError::DecodedLimitExceeded)?;
        if new_length as u64 > limits.max_decoded_file_bytes {
            return Err(SnapshotError::DecodedLimitExceeded);
        }
        decoded
            .try_reserve(count)
            .map_err(|_| SnapshotError::AllocationFailed)?;
        decoded.extend_from_slice(&buffer[..count]);
    }
    if decoder.decoder.get_checksum_from_data().is_some()
        && decoder.decoder.get_checksum_from_data() != decoder.decoder.get_calculated_checksum()
    {
        return Err(SnapshotError::InvalidZstd);
    }
    let (source, _) = decoder.into_parts();
    if source.position() != raw.len() as u64 {
        return Err(SnapshotError::InvalidZstd);
    }
    Ok(decoded)
}

fn index_bytes(
    bytes: &[u8],
    limits: SnapshotLimits,
    record_total: &mut usize,
) -> Result<Vec<Range<usize>>, SnapshotError> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }

    *record_total = record_total
        .checked_add(1)
        .ok_or(SnapshotError::RecordLimitExceeded)?;
    if limits.max_records_per_file == 0 || *record_total > limits.max_records_total {
        return Err(SnapshotError::RecordLimitExceeded);
    }

    let mut ranges = Vec::new();
    ranges
        .try_reserve_exact(1)
        .map_err(|_| SnapshotError::AllocationFailed)?;
    ranges.push(0..bytes.len());
    Ok(ranges)
}

fn index_json_lines(
    bytes: &[u8],
    limits: SnapshotLimits,
    record_total: &mut usize,
) -> Result<(Vec<Range<usize>>, bool), SnapshotError> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        let end = index - usize::from(index > start && bytes[index - 1] == b'\r');
        validate_record(&bytes[start..end], limits)?;
        ranges
            .try_reserve(1)
            .map_err(|_| SnapshotError::AllocationFailed)?;
        ranges.push(start..end);
        *record_total = record_total
            .checked_add(1)
            .ok_or(SnapshotError::RecordLimitExceeded)?;
        if ranges.len() > limits.max_records_per_file || *record_total > limits.max_records_total {
            return Err(SnapshotError::RecordLimitExceeded);
        }
        start = index + 1;
    }
    let has_partial_tail = start < bytes.len();
    if has_partial_tail {
        validate_record(&bytes[start..], limits)?;
    }
    Ok((ranges, has_partial_tail))
}

fn validate_record(record: &[u8], limits: SnapshotLimits) -> Result<(), SnapshotError> {
    if record.len() > limits.max_line_bytes {
        return Err(SnapshotError::LineLimitExceeded);
    }
    let mut depth = 0_usize;
    let mut in_string = false;
    let mut escaped = false;
    for byte in record {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match *byte {
            b'"' => in_string = true,
            b'{' | b'[' => {
                depth = depth
                    .checked_add(1)
                    .ok_or(SnapshotError::NestingLimitExceeded)?;
                if depth > limits.max_json_depth {
                    return Err(SnapshotError::NestingLimitExceeded);
                }
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use ruzstd::encoding::{CompressionLevel, compress_to_vec};

    use super::{BundleMemberSpec, ContentKind, MemberCompression, MemberRole, capture_bundle};
    use crate::io::{LocalRoot, SnapshotError, SnapshotLimits};

    struct TempTree(PathBuf);

    impl TempTree {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should follow the epoch")
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("ai-session-bundle-{}-{nonce}", std::process::id()));
            fs::create_dir(&path).expect("temporary root should be created");
            Self(path)
        }

        fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, bytes).expect("fixture should be written");
            path
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn spec(id: &str, path: &Path) -> BundleMemberSpec {
        BundleMemberSpec {
            id: id.to_owned(),
            logical_name: format!("{id}.jsonl"),
            path: path.to_path_buf(),
            role: MemberRole::Primary,
            compression: MemberCompression::None,
            content_kind: ContentKind::JsonLines,
            expected_identity: None,
        }
    }

    #[test]
    fn returns_path_free_record_views_and_ignores_a_partial_tail() {
        let tree = TempTree::new();
        let path = tree.write("session.jsonl", b"{\"text\":\"[ignored]\"}\r\n\npartial");
        let root = LocalRoot::new(&tree.0).expect("root should be valid");

        let bundle = capture_bundle(&root, &[spec("primary", &path)], SnapshotLimits::default())
            .expect("bundle should be captured");
        let member = bundle.member("primary").expect("member should exist");

        assert_eq!(member.logical_name(), "primary.jsonl");
        assert_eq!(member.role(), MemberRole::Primary);
        assert_eq!(
            member.records().collect::<Vec<_>>(),
            vec![b"{\"text\":\"[ignored]\"}".as_slice(), b"".as_slice()]
        );
        assert!(member.partial_final_record_ignored());
        assert_eq!(bundle.record_count(), 2);
    }

    #[test]
    fn enforces_member_raw_decoded_and_artifact_aggregate_limits() {
        let tree = TempTree::new();
        let first = tree.write("first.jsonl", b"12345678");
        let second = tree.write("second.jsonl", b"12345678");
        let root = LocalRoot::new(&tree.0).expect("root should be valid");
        let specs = [spec("first", &first), spec("second", &second)];

        let raw_result = capture_bundle(
            &root,
            &specs,
            SnapshotLimits {
                max_raw_total_bytes: 15,
                ..SnapshotLimits::default()
            },
        );
        assert!(matches!(
            raw_result,
            Err(SnapshotError::AggregateLimitExceeded)
        ));

        let decoded_result = capture_bundle(
            &root,
            &specs,
            SnapshotLimits {
                max_decoded_total_bytes: 15,
                ..SnapshotLimits::default()
            },
        );
        assert!(matches!(
            decoded_result,
            Err(SnapshotError::DecodedLimitExceeded)
        ));

        let mut artifacts = specs;
        artifacts[0].role = MemberRole::Artifact;
        artifacts[1].role = MemberRole::Artifact;
        let artifact_result = capture_bundle(
            &root,
            &artifacts,
            SnapshotLimits {
                max_artifacts: 1,
                ..SnapshotLimits::default()
            },
        );
        assert!(matches!(
            artifact_result,
            Err(SnapshotError::ArtifactLimitExceeded)
        ));
    }

    #[test]
    fn enforces_jsonl_line_record_and_nesting_limits() {
        let tree = TempTree::new();
        let line_path = tree.write("line.jsonl", b"12345\n");
        let record_path = tree.write("records.jsonl", b"{}\n{}\n");
        let depth_path = tree.write("depth.jsonl", b"[[[]]]\n");
        let root = LocalRoot::new(&tree.0).expect("root should be valid");

        let line_result = capture_bundle(
            &root,
            &[spec("line", &line_path)],
            SnapshotLimits {
                max_line_bytes: 4,
                ..SnapshotLimits::default()
            },
        );
        assert!(matches!(line_result, Err(SnapshotError::LineLimitExceeded)));

        let record_result = capture_bundle(
            &root,
            &[spec("records", &record_path)],
            SnapshotLimits {
                max_records_per_file: 1,
                ..SnapshotLimits::default()
            },
        );
        assert!(matches!(
            record_result,
            Err(SnapshotError::RecordLimitExceeded)
        ));

        let depth_result = capture_bundle(
            &root,
            &[spec("depth", &depth_path)],
            SnapshotLimits {
                max_json_depth: 2,
                ..SnapshotLimits::default()
            },
        );
        assert!(matches!(
            depth_result,
            Err(SnapshotError::NestingLimitExceeded)
        ));
    }

    #[test]
    fn decodes_zstandard_with_output_and_window_limits() {
        let tree = TempTree::new();
        let content = b"{\"kind\":\"event\"}\n";
        let compressed = compress_to_vec(content.as_slice(), CompressionLevel::Uncompressed);
        let path = tree.write("session.jsonl.zst", &compressed);
        let root = LocalRoot::new(&tree.0).expect("root should be valid");
        let mut compressed_spec = spec("compressed", &path);
        compressed_spec.compression = MemberCompression::Zstandard;

        let bundle = capture_bundle(&root, &[compressed_spec.clone()], SnapshotLimits::default())
            .expect("compressed bundle should decode");
        assert_eq!(
            bundle
                .member("compressed")
                .expect("member should exist")
                .bytes(),
            content
        );

        let bomb = compress_to_vec(vec![b'x'; 1_024].as_slice(), CompressionLevel::Uncompressed);
        fs::write(&path, bomb).expect("bomb fixture should be written");
        let result = capture_bundle(
            &root,
            &[compressed_spec],
            SnapshotLimits {
                max_decoded_file_bytes: 128,
                ..SnapshotLimits::default()
            },
        );
        assert!(matches!(result, Err(SnapshotError::DecodedLimitExceeded)));
    }

    #[test]
    fn rejects_expected_identity_mismatch() {
        let tree = TempTree::new();
        let path = tree.write("session.jsonl", b"{}\n");
        let root = LocalRoot::new(&tree.0).expect("root should be valid");

        let wrong_identity: (u64, [u8; 16]) = (999_999, [0xFF; 16]);
        let mut s = spec("primary", &path);
        s.expected_identity = Some(wrong_identity);

        let result = capture_bundle(&root, &[s], SnapshotLimits::default());
        assert!(matches!(result, Err(SnapshotError::ConcurrentRotate)));
    }

    #[test]
    fn accepts_matching_expected_identity() {
        let tree = TempTree::new();
        let path = tree.write("session.jsonl", b"{}\n");
        let root = LocalRoot::new(&tree.0).expect("root should be valid");

        let bundle = capture_bundle(&root, &[spec("primary", &path)], SnapshotLimits::default())
            .expect("bundle should be captured");
        drop(bundle);

        let opened = root.open_file(&path).expect("file should open");
        let identity = (opened.stamp.volume_serial, opened.stamp.file_id);

        let mut s = spec("verified", &path);
        s.expected_identity = Some(identity);
        let result = capture_bundle(&root, &[s], SnapshotLimits::default());
        assert!(result.is_ok());
    }
}
