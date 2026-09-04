use std::collections::HashSet;

use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", try_from = "RawReplayProjectV1")]
pub struct ReplayProjectV1 {
    pub schema_version: u8,
    pub session: NormalizedSessionV1,
    pub trim: TrimRange,
    pub segments: Vec<SpeedSegment>,
    pub theme: ThemeSpec,
    pub font: FontSpec,
    pub terminal_hold_ms: u64,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawReplayProjectV1 {
    schema_version: u8,
    session: NormalizedSessionV1,
    trim: TrimRange,
    segments: Vec<SpeedSegment>,
    theme: ThemeSpec,
    font: FontSpec,
    terminal_hold_ms: u64,
    fps: u32,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedSessionV1 {
    pub schema_version: u8,
    pub id: String,
    pub source: SessionSource,
    pub source_version: Option<String>,
    pub title: String,
    pub created_at: Option<String>,
    pub cwd: Option<String>,
    pub relationships: Vec<SessionRelationship>,
    pub events: Vec<ReplayEvent>,
    pub duration_ms: u64,
    pub diagnostics: Vec<SourceDiagnostic>,
    pub unknown_record_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(
    rename_all = "camelCase",
    try_from = "RawNormalizedSessionV2",
    deny_unknown_fields
)]
pub struct NormalizedSessionV2 {
    pub schema_version: u8,
    pub id: String,
    pub source: SessionSource,
    pub source_version: Option<String>,
    pub title: String,
    pub created_at: Option<String>,
    pub cwd: Option<String>,
    pub relationships: Vec<SessionRelationship>,
    pub entries: Vec<NormalizedEntryV2>,
    pub duration_ms: u64,
    pub diagnostics: Vec<SourceDiagnostic>,
    pub unknown_record_count: u64,
    pub content_availability: NormalizedContentAvailabilityV2,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawNormalizedSessionV2 {
    schema_version: u8,
    id: String,
    source: SessionSource,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    source_version: Option<String>,
    title: String,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    created_at: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    cwd: Option<String>,
    relationships: Vec<SessionRelationship>,
    entries: Vec<NormalizedEntryV2>,
    duration_ms: u64,
    diagnostics: Vec<SourceDiagnostic>,
    unknown_record_count: u64,
    content_availability: NormalizedContentAvailabilityV2,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NormalizedReasoningAvailabilityV2 {
    Available,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NormalizedContentAvailabilityValueV2 {
    Available,
    Partial,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormalizedContentAvailabilityV2 {
    pub reasoning: NormalizedReasoningAvailabilityV2,
    pub tool_details: NormalizedContentAvailabilityValueV2,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "availability", rename_all = "lowercase", deny_unknown_fields)]
pub enum NormalizedToolDetailV2 {
    Unavailable,
    Available {
        #[serde(deserialize_with = "deserialize_required_nullable")]
        arguments: Option<String>,
        #[serde(deserialize_with = "deserialize_required_nullable")]
        result: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NormalizedToolStatusV2 {
    Pending,
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum NormalizedEntryV2 {
    User {
        entry_key: String,
        at_ms: u64,
        text: String,
    },
    Assistant {
        entry_key: String,
        at_ms: u64,
        markdown: String,
    },
    Reasoning {
        entry_key: String,
        at_ms: u64,
        text: String,
    },
    ToolCall {
        entry_key: String,
        at_ms: u64,
        name: String,
        status: NormalizedToolStatusV2,
        summary: String,
        detail: NormalizedToolDetailV2,
    },
    FileChange {
        entry_key: String,
        at_ms: u64,
        display_path: String,
        summary: String,
    },
    Unknown {
        entry_key: String,
        at_ms: u64,
        source_type: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SessionSource {
    ClaudeCode,
    Codex,
    CopilotCli,
    VscodeCopilot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionRelationship {
    pub kind: RelationshipKind,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RelationshipKind {
    Parent,
    Fork,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ReplayEvent {
    User {
        id: String,
        #[serde(rename = "atMs")]
        at_ms: u64,
        text: String,
    },
    Assistant {
        id: String,
        #[serde(rename = "atMs")]
        at_ms: u64,
        markdown: String,
    },
    Tool {
        id: String,
        #[serde(rename = "atMs")]
        at_ms: u64,
        name: String,
        status: ToolStatus,
        summary: String,
    },
    FileChange {
        id: String,
        #[serde(rename = "atMs")]
        at_ms: u64,
        path: String,
        summary: String,
    },
    Unknown {
        id: String,
        #[serde(rename = "atMs")]
        at_ms: u64,
        #[serde(rename = "sourceType")]
        source_type: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ToolStatus {
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceDiagnostic {
    pub code: String,
    pub severity: DiagnosticSeverity,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TrimRange {
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SpeedSegment {
    pub id: String,
    pub source_start_ms: u64,
    pub source_end_ms: u64,
    pub speed: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThemeSpec {
    pub background: String,
    pub surface: String,
    pub text: String,
    pub muted: String,
    pub accent: String,
    pub success: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FontSpec {
    pub family: FontFamily,
    pub size_px: u32,
    pub line_height: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FontFamily {
    #[serde(rename = "JetBrains Mono")]
    JetBrainsMono,
}

const MAX_EVENT_COUNT: usize = 100_000;
const MAX_SEGMENT_COUNT: usize = 10_000;
const MAX_SOURCE_DURATION_MS: u64 = 7 * 24 * 60 * 60 * 1_000;
const MAX_OUTPUT_DURATION_MS: f64 = 2.0 * 60.0 * 60.0 * 1_000.0;
const MAX_FRAME_COUNT: f64 = 216_000.0;
const MIN_SPEED: f64 = 0.25;
const MAX_SPEED: f64 = 8.0;
const MAX_TEXT_LENGTH: usize = 2_000_000;
const NORMALIZED_SESSION_SCHEMA_VERSION: u8 = 2;
const FNV_OFFSET_BASIS_32: u32 = 0x811c9dc5;
const FNV_ALTERNATE_BASIS_32: u32 = 0x9e3779b9;
const FNV_PRIME_32: u32 = 0x01000193;

impl TryFrom<RawNormalizedSessionV2> for NormalizedSessionV2 {
    type Error = String;

    fn try_from(raw: RawNormalizedSessionV2) -> Result<Self, Self::Error> {
        let session = Self {
            schema_version: raw.schema_version,
            id: raw.id,
            source: raw.source,
            source_version: raw.source_version,
            title: raw.title,
            created_at: raw.created_at,
            cwd: raw.cwd,
            relationships: raw.relationships,
            entries: raw.entries,
            duration_ms: raw.duration_ms,
            diagnostics: raw.diagnostics,
            unknown_record_count: raw.unknown_record_count,
            content_availability: raw.content_availability,
        };
        session.validate()?;
        Ok(session)
    }
}

impl NormalizedSessionV2 {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.schema_version != NORMALIZED_SESSION_SCHEMA_VERSION {
            return Err("unsupported normalized session version".into());
        }
        validate_string(&self.id, 1, 256, "session id")?;
        validate_string(&self.title, 1, 512, "session title")?;
        validate_optional_string(&self.source_version, 128, "source version")?;
        if self
            .created_at
            .as_deref()
            .is_some_and(|value| !is_canonical_timestamp(value))
        {
            return Err("invalid creation timestamp".into());
        }
        validate_optional_string(&self.cwd, 32_768, "working directory")?;
        if self.entries.is_empty() || self.entries.len() > MAX_EVENT_COUNT {
            return Err("invalid normalized entry count".into());
        }
        if self.relationships.len() > 100 || self.diagnostics.len() > 1_000 {
            return Err("session metadata exceeds supported limits".into());
        }

        let mut entry_keys = HashSet::new();
        let mut previous_at_ms = 0;
        let mut unknown_record_count = 0_u64;
        let mut has_reasoning = false;
        let mut has_available_tool_detail = false;
        let mut has_unavailable_tool_detail = false;
        for (index, entry) in self.entries.iter().enumerate() {
            entry.validate()?;
            let at_ms = entry.at_ms();
            if (index > 0 && at_ms < previous_at_ms) || !entry_keys.insert(entry.entry_key()) {
                return Err("entry timeline is not monotonic or has duplicate keys".into());
            }
            previous_at_ms = at_ms;
            match entry {
                NormalizedEntryV2::Reasoning { .. } => has_reasoning = true,
                NormalizedEntryV2::ToolCall { detail, .. } => match detail {
                    NormalizedToolDetailV2::Available { .. } => {
                        has_available_tool_detail = true;
                    }
                    NormalizedToolDetailV2::Unavailable => {
                        has_unavailable_tool_detail = true;
                    }
                },
                NormalizedEntryV2::Unknown { .. } => unknown_record_count += 1,
                _ => {}
            }
        }
        if self.duration_ms > MAX_SOURCE_DURATION_MS || self.duration_ms < previous_at_ms {
            return Err("invalid normalized session duration".into());
        }
        if self.unknown_record_count != unknown_record_count {
            return Err("unknown record count does not match normalized entries".into());
        }
        if has_reasoning
            && self.content_availability.reasoning == NormalizedReasoningAvailabilityV2::Unavailable
        {
            return Err("reasoning availability contradicts normalized entries".into());
        }
        match self.content_availability.tool_details {
            NormalizedContentAvailabilityValueV2::Available if has_unavailable_tool_detail => {
                return Err("tool-detail availability contradicts normalized entries".into());
            }
            NormalizedContentAvailabilityValueV2::Unavailable if has_available_tool_detail => {
                return Err("tool-detail availability contradicts normalized entries".into());
            }
            NormalizedContentAvailabilityValueV2::Partial
                if !has_available_tool_detail || !has_unavailable_tool_detail =>
            {
                return Err("partial tool-detail availability requires mixed entries".into());
            }
            _ => {}
        }
        for relationship in &self.relationships {
            validate_string(&relationship.session_id, 1, 256, "related session id")?;
        }
        for diagnostic in &self.diagnostics {
            validate_string(&diagnostic.code, 1, 128, "diagnostic code")?;
            validate_string(&diagnostic.message, 1, 4_096, "diagnostic message")?;
        }
        Ok(())
    }
}

impl NormalizedEntryV2 {
    pub(crate) fn entry_key(&self) -> &str {
        match self {
            Self::User { entry_key, .. }
            | Self::Assistant { entry_key, .. }
            | Self::Reasoning { entry_key, .. }
            | Self::ToolCall { entry_key, .. }
            | Self::FileChange { entry_key, .. }
            | Self::Unknown { entry_key, .. } => entry_key,
        }
    }

    pub(crate) fn at_ms(&self) -> u64 {
        match self {
            Self::User { at_ms, .. }
            | Self::Assistant { at_ms, .. }
            | Self::Reasoning { at_ms, .. }
            | Self::ToolCall { at_ms, .. }
            | Self::FileChange { at_ms, .. }
            | Self::Unknown { at_ms, .. } => *at_ms,
        }
    }

    fn validate(&self) -> Result<(), String> {
        validate_entry_key(self.entry_key())?;
        if self.at_ms() > MAX_SOURCE_DURATION_MS {
            return Err("normalized entry timestamp exceeds the supported range".into());
        }
        match self {
            Self::User { text, .. } | Self::Reasoning { text, .. } => {
                validate_string(text, 0, MAX_TEXT_LENGTH, "entry text")
            }
            Self::Assistant { markdown, .. } => {
                validate_string(markdown, 0, MAX_TEXT_LENGTH, "assistant text")
            }
            Self::ToolCall {
                name,
                summary,
                detail,
                ..
            } => {
                validate_string(name, 1, 256, "tool name")?;
                validate_string(summary, 0, MAX_TEXT_LENGTH, "tool summary")?;
                match detail {
                    NormalizedToolDetailV2::Unavailable => Ok(()),
                    NormalizedToolDetailV2::Available { arguments, result } => {
                        validate_optional_string(arguments, MAX_TEXT_LENGTH, "tool arguments")?;
                        validate_optional_string(result, MAX_TEXT_LENGTH, "tool result")?;
                        if arguments.is_none() && result.is_none() {
                            return Err("available tool detail is empty".into());
                        }
                        Ok(())
                    }
                }
            }
            Self::FileChange {
                display_path,
                summary,
                ..
            } => {
                validate_string(display_path, 1, 32_768, "display path")?;
                validate_string(summary, 0, MAX_TEXT_LENGTH, "file summary")
            }
            Self::Unknown { source_type, .. } => {
                validate_string(source_type, 1, 256, "unknown source type")
            }
        }
    }
}

pub(crate) fn migrate_normalized_session_v1(
    legacy: NormalizedSessionV1,
    terminal_hold_ms: u64,
) -> Result<NormalizedSessionV2, String> {
    legacy.validate(terminal_hold_ms)?;
    let entries = legacy
        .events
        .into_iter()
        .map(|event| match event {
            ReplayEvent::User { id, at_ms, text } => NormalizedEntryV2::User {
                entry_key: migrate_legacy_entry_key(&id),
                at_ms,
                text,
            },
            ReplayEvent::Assistant {
                id,
                at_ms,
                markdown,
            } => NormalizedEntryV2::Assistant {
                entry_key: migrate_legacy_entry_key(&id),
                at_ms,
                markdown,
            },
            ReplayEvent::Tool {
                id,
                at_ms,
                name,
                status,
                summary,
            } => NormalizedEntryV2::ToolCall {
                entry_key: migrate_legacy_entry_key(&id),
                at_ms,
                name,
                status: match status {
                    ToolStatus::Running => NormalizedToolStatusV2::Running,
                    ToolStatus::Succeeded => NormalizedToolStatusV2::Succeeded,
                    ToolStatus::Failed => NormalizedToolStatusV2::Failed,
                },
                summary,
                detail: NormalizedToolDetailV2::Unavailable,
            },
            ReplayEvent::FileChange {
                id,
                at_ms,
                path,
                summary,
            } => NormalizedEntryV2::FileChange {
                entry_key: migrate_legacy_entry_key(&id),
                at_ms,
                display_path: path,
                summary,
            },
            ReplayEvent::Unknown {
                id,
                at_ms,
                source_type,
            } => NormalizedEntryV2::Unknown {
                entry_key: migrate_legacy_entry_key(&id),
                at_ms,
                source_type,
            },
        })
        .collect();
    let session = NormalizedSessionV2 {
        schema_version: NORMALIZED_SESSION_SCHEMA_VERSION,
        id: legacy.id,
        source: legacy.source,
        source_version: legacy.source_version,
        title: legacy.title,
        created_at: legacy.created_at,
        cwd: legacy.cwd,
        relationships: legacy.relationships,
        entries,
        duration_ms: legacy.duration_ms,
        diagnostics: legacy.diagnostics,
        unknown_record_count: legacy.unknown_record_count,
        content_availability: NormalizedContentAvailabilityV2 {
            reasoning: NormalizedReasoningAvailabilityV2::Unavailable,
            tool_details: NormalizedContentAvailabilityValueV2::Unavailable,
        },
    };
    session.validate()?;
    Ok(session)
}

fn migrate_legacy_entry_key(id: &str) -> String {
    if validate_entry_key(id).is_ok() {
        return id.to_owned();
    }
    let first = fnv1a32(id.as_bytes(), FNV_OFFSET_BASIS_32);
    let second = fnv1a32(id.as_bytes(), FNV_ALTERNATE_BASIS_32);
    format!("legacy-{first:08x}{second:08x}")
}

fn fnv1a32(bytes: &[u8], seed: u32) -> u32 {
    bytes.iter().fold(seed, |hash, byte| {
        (hash ^ u32::from(*byte)).wrapping_mul(FNV_PRIME_32)
    })
}

fn validate_entry_key(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 256
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err("invalid normalized entry key".into());
    }
    Ok(())
}

fn deserialize_required_nullable<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)
}

impl TryFrom<RawReplayProjectV1> for ReplayProjectV1 {
    type Error = String;

    fn try_from(raw: RawReplayProjectV1) -> Result<Self, Self::Error> {
        let project = Self {
            schema_version: raw.schema_version,
            session: raw.session,
            trim: raw.trim,
            segments: raw.segments,
            theme: raw.theme,
            font: raw.font,
            terminal_hold_ms: raw.terminal_hold_ms,
            fps: raw.fps,
            width: raw.width,
            height: raw.height,
        };
        project.validate()?;
        Ok(project)
    }
}

impl ReplayProjectV1 {
    fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 || self.session.schema_version != 1 {
            return Err("unsupported project or session version".into());
        }
        if self.fps != 30
            || self.width != 1920
            || self.height != 1080
            || !(250..=10_000).contains(&self.terminal_hold_ms)
        {
            return Err("invalid render settings".into());
        }
        self.session.validate(self.terminal_hold_ms)?;
        self.trim.validate(self.session.duration_ms)?;
        validate_segments(&self.segments, &self.trim)?;
        validate_theme(&self.theme)?;
        if !(12..=96).contains(&self.font.size_px)
            || !self.font.line_height.is_finite()
            || !(1.0..=2.5).contains(&self.font.line_height)
        {
            return Err("invalid font settings".into());
        }

        let output_duration_ms = self.segments.iter().fold(0.0, |total, segment| {
            total + (segment.source_end_ms - segment.source_start_ms) as f64 / segment.speed
        });
        let frame_count = (output_duration_ms / 1_000.0 * f64::from(self.fps)).ceil();
        if !output_duration_ms.is_finite()
            || output_duration_ms > MAX_OUTPUT_DURATION_MS
            || !(1.0..=MAX_FRAME_COUNT).contains(&frame_count)
        {
            return Err("output duration or frame count is outside the supported range".into());
        }

        let final_event_ms = self
            .session
            .events
            .iter()
            .rev()
            .map(ReplayEvent::at_ms)
            .find(|at_ms| *at_ms < self.trim.end_ms)
            .ok_or_else(|| "trim range contains no replayable events".to_owned())?;
        let final_frame_output_ms = (frame_count - 1.0) / f64::from(self.fps) * 1_000.0;
        let final_frame_source_ms = source_time_at_output_ms(&self.segments, final_frame_output_ms);
        if final_frame_source_ms < final_event_ms.max(self.trim.start_ms) as f64 {
            return Err("final event is not renderable".into());
        }
        Ok(())
    }
}

impl NormalizedSessionV1 {
    pub(crate) fn validate(&self, terminal_hold_ms: u64) -> Result<(), String> {
        validate_string(&self.id, 1, 256, "session id")?;
        validate_string(&self.title, 1, 512, "session title")?;
        validate_optional_string(&self.source_version, 128, "source version")?;
        if self
            .created_at
            .as_deref()
            .is_some_and(|value| !is_canonical_timestamp(value))
        {
            return Err("invalid creation timestamp".into());
        }
        validate_optional_string(&self.cwd, 32_768, "working directory")?;
        if self.events.is_empty() || self.events.len() > MAX_EVENT_COUNT {
            return Err("invalid event count".into());
        }
        if self.relationships.len() > 100 || self.diagnostics.len() > 1_000 {
            return Err("session metadata exceeds supported limits".into());
        }
        if self.unknown_record_count > self.events.len() as u64 {
            return Err("invalid unknown record count".into());
        }

        let mut event_ids = HashSet::new();
        let mut previous_at_ms = 0;
        for (index, event) in self.events.iter().enumerate() {
            event.validate()?;
            let at_ms = event.at_ms();
            if (index > 0 && at_ms < previous_at_ms) || !event_ids.insert(event.id()) {
                return Err("event timeline is not monotonic or has duplicate ids".into());
            }
            previous_at_ms = at_ms;
        }
        if self.duration_ms > MAX_SOURCE_DURATION_MS || self.duration_ms < previous_at_ms {
            return Err("invalid session duration".into());
        }
        let required_duration = previous_at_ms
            .checked_add(terminal_hold_ms)
            .ok_or_else(|| "session duration overflow".to_owned())?;
        if self.duration_ms != required_duration {
            return Err("session duration does not include the terminal hold".into());
        }

        for relationship in &self.relationships {
            validate_string(&relationship.session_id, 1, 256, "related session id")?;
        }
        for diagnostic in &self.diagnostics {
            validate_string(&diagnostic.code, 1, 128, "diagnostic code")?;
            validate_string(&diagnostic.message, 1, 4_096, "diagnostic message")?;
        }
        Ok(())
    }
}

impl ReplayEvent {
    fn id(&self) -> &str {
        match self {
            Self::User { id, .. }
            | Self::Assistant { id, .. }
            | Self::Tool { id, .. }
            | Self::FileChange { id, .. }
            | Self::Unknown { id, .. } => id,
        }
    }

    pub(crate) fn at_ms(&self) -> u64 {
        match self {
            Self::User { at_ms, .. }
            | Self::Assistant { at_ms, .. }
            | Self::Tool { at_ms, .. }
            | Self::FileChange { at_ms, .. }
            | Self::Unknown { at_ms, .. } => *at_ms,
        }
    }

    fn validate(&self) -> Result<(), String> {
        validate_string(self.id(), 1, 256, "event id")?;
        if self.at_ms() > MAX_SOURCE_DURATION_MS {
            return Err("event timestamp exceeds the supported range".into());
        }
        match self {
            Self::User { text, .. } => validate_string(text, 0, MAX_TEXT_LENGTH, "user text"),
            Self::Assistant { markdown, .. } => {
                validate_string(markdown, 0, MAX_TEXT_LENGTH, "assistant text")
            }
            Self::Tool { name, summary, .. } => {
                validate_string(name, 1, 256, "tool name")?;
                validate_string(summary, 0, MAX_TEXT_LENGTH, "tool summary")
            }
            Self::FileChange { path, summary, .. } => {
                validate_string(path, 1, 32_768, "file path")?;
                validate_string(summary, 0, MAX_TEXT_LENGTH, "file summary")
            }
            Self::Unknown { source_type, .. } => {
                validate_string(source_type, 1, 256, "unknown source type")
            }
        }
    }
}

impl TrimRange {
    fn validate(&self, session_duration_ms: u64) -> Result<(), String> {
        if self.start_ms >= self.end_ms
            || self.end_ms > session_duration_ms
            || self.end_ms > MAX_SOURCE_DURATION_MS
        {
            return Err("invalid trim range".into());
        }
        Ok(())
    }
}

fn validate_segments(segments: &[SpeedSegment], trim: &TrimRange) -> Result<(), String> {
    if segments.is_empty() || segments.len() > MAX_SEGMENT_COUNT {
        return Err("invalid speed segment count".into());
    }
    let mut ids = HashSet::new();
    let mut expected_start = trim.start_ms;
    for segment in segments {
        if !ids.insert(segment.id.as_str())
            || validate_string(&segment.id, 1, 256, "speed segment id").is_err()
            || segment.source_start_ms != expected_start
            || segment.source_start_ms >= segment.source_end_ms
            || segment.source_end_ms > MAX_SOURCE_DURATION_MS
            || !segment.speed.is_finite()
            || !(MIN_SPEED..=MAX_SPEED).contains(&segment.speed)
        {
            return Err("speed segments do not exactly partition the trim range".into());
        }
        expected_start = segment.source_end_ms;
    }
    if expected_start != trim.end_ms {
        return Err("speed segments do not exactly partition the trim range".into());
    }
    Ok(())
}

fn validate_theme(theme: &ThemeSpec) -> Result<(), String> {
    for color in [
        &theme.background,
        &theme.surface,
        &theme.text,
        &theme.muted,
        &theme.accent,
        &theme.success,
        &theme.error,
    ] {
        if color.len() != 7
            || !color.starts_with('#')
            || !color[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("invalid theme color".into());
        }
    }
    Ok(())
}

fn validate_optional_string(
    value: &Option<String>,
    maximum: usize,
    field: &str,
) -> Result<(), String> {
    if let Some(value) = value {
        validate_string(value, 0, maximum, field)?;
    }
    Ok(())
}

fn is_canonical_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 24
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'.'
        || bytes[23] != b'Z'
        || bytes.iter().enumerate().any(|(index, byte)| {
            !matches!(index, 4 | 7 | 10 | 13 | 16 | 19 | 23) && !byte.is_ascii_digit()
        })
    {
        return false;
    }

    let parse = |start, end| value[start..end].parse::<u32>().ok();
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        parse(0, 4),
        parse(5, 7),
        parse(8, 10),
        parse(11, 13),
        parse(14, 16),
        parse(17, 19),
    ) else {
        return false;
    };
    let leap_year =
        year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap_year => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days_in_month).contains(&day) && hour < 24 && minute < 60 && second < 60
}

fn validate_string(value: &str, minimum: usize, maximum: usize, field: &str) -> Result<(), String> {
    let length = value.chars().count();
    if length < minimum || length > maximum || value.contains('\0') {
        return Err(format!("invalid {field}"));
    }
    Ok(())
}

fn source_time_at_output_ms(segments: &[SpeedSegment], output_ms: f64) -> f64 {
    let mut output_cursor = 0.0;
    for segment in segments {
        let segment_output_duration =
            (segment.source_end_ms - segment.source_start_ms) as f64 / segment.speed;
        let output_end = output_cursor + segment_output_duration;
        if output_ms <= output_end {
            return (segment.source_start_ms as f64 + (output_ms - output_cursor) * segment.speed)
                .min(segment.source_end_ms as f64);
        }
        output_cursor = output_end;
    }
    segments
        .last()
        .map_or(0.0, |segment| segment.source_end_ms as f64)
}

#[cfg(test)]
mod tests {
    use super::{
        NormalizedContentAvailabilityV2, NormalizedEntryV2, NormalizedSessionV2,
        NormalizedToolDetailV2, ReplayProjectV1, migrate_normalized_session_v1,
    };

    #[test]
    fn round_trips_the_typescript_contract_fixture() {
        let fixture = include_str!("../../tests/fixtures/contracts-v1.json");
        let expected: serde_json::Value =
            serde_json::from_str(fixture).expect("fixture should contain valid JSON");

        let project: ReplayProjectV1 =
            serde_json::from_str(fixture).expect("fixture should match the Rust contract");
        let actual = serde_json::to_value(project).expect("project should serialize");

        assert_eq!(actual, expected);
    }

    #[test]
    fn round_trips_the_rich_normalized_session_fixture() {
        let fixture = include_str!("../../tests/fixtures/normalized-session-v2.json");
        let expected: serde_json::Value =
            serde_json::from_str(fixture).expect("fixture should contain valid JSON");

        let session: NormalizedSessionV2 =
            serde_json::from_str(fixture).expect("fixture should match the Rust contract");
        let actual = serde_json::to_value(session).expect("session should serialize");

        assert_eq!(actual, expected);
    }

    #[test]
    fn migrates_v1_without_inventing_rich_content() {
        let project: ReplayProjectV1 =
            serde_json::from_str(include_str!("../../tests/fixtures/contracts-v1.json"))
                .expect("fixture should match the legacy contract");

        let session = migrate_normalized_session_v1(project.session, project.terminal_hold_ms)
            .expect("legacy session should migrate");

        assert_eq!(session.schema_version, 2);
        assert_eq!(session.entries.len(), 7);
        assert_eq!(
            session.content_availability,
            NormalizedContentAvailabilityV2 {
                reasoning: super::NormalizedReasoningAvailabilityV2::Unavailable,
                tool_details: super::NormalizedContentAvailabilityValueV2::Unavailable,
            }
        );
        assert!(
            session
                .entries
                .iter()
                .all(|entry| !matches!(entry, NormalizedEntryV2::Reasoning { .. }))
        );
        assert!(
            session
                .entries
                .iter()
                .filter_map(|entry| match entry {
                    NormalizedEntryV2::ToolCall { detail, .. } => Some(detail),
                    _ => None,
                })
                .all(|detail| matches!(detail, NormalizedToolDetailV2::Unavailable))
        );
    }

    #[test]
    fn legacy_entry_key_migration_is_safe_and_deterministic() {
        let project: ReplayProjectV1 =
            serde_json::from_str(include_str!("../../tests/fixtures/contracts-v1.json"))
                .expect("fixture should match the legacy contract");
        let terminal_hold_ms = project.terminal_hold_ms;
        let mut first = project.session.clone();
        first.events.truncate(2);
        match &mut first.events[0] {
            super::ReplayEvent::User { id, .. } => *id = "safe_entry-1".to_owned(),
            _ => panic!("fixture entry should be a user event"),
        }
        match &mut first.events[1] {
            super::ReplayEvent::Assistant { id, .. } => *id = "unsafe:entry:2".to_owned(),
            _ => panic!("fixture entry should be an assistant event"),
        }
        first.duration_ms = first.events[1].at_ms() + terminal_hold_ms;
        first.unknown_record_count = 0;

        let migrated_first = migrate_normalized_session_v1(first.clone(), terminal_hold_ms)
            .expect("legacy session should migrate");
        let migrated_second = migrate_normalized_session_v1(first, terminal_hold_ms)
            .expect("the same legacy session should migrate");

        assert_eq!(migrated_first, migrated_second);
        assert_eq!(migrated_first.entries[0].entry_key(), "safe_entry-1");
        assert_eq!(
            migrated_first.entries[1].entry_key(),
            "legacy-288ce059a7cfea15"
        );
    }

    #[test]
    fn rejects_malformed_rich_normalized_sessions() {
        for (pointer, value) in [
            ("/schemaVersion", serde_json::json!(1)),
            ("/entries/0/entryKey", serde_json::json!("../entry")),
            ("/entries/1/entryKey", serde_json::json!("entry-user")),
            ("/entries/2/atMs", serde_json::json!(1)),
            (
                "/entries/3/detail",
                serde_json::json!({
                    "availability": "available",
                    "arguments": null,
                    "result": null
                }),
            ),
            ("/unknownRecordCount", serde_json::json!(0)),
        ] {
            let mut fixture: serde_json::Value = serde_json::from_str(include_str!(
                "../../tests/fixtures/normalized-session-v2.json"
            ))
            .expect("fixture should contain valid JSON");
            *fixture
                .pointer_mut(pointer)
                .expect("test pointer should exist in fixture") = value;

            assert!(
                serde_json::from_value::<NormalizedSessionV2>(fixture).is_err(),
                "{pointer} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_unknown_rich_session_fields_and_availability_mismatches() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/normalized-session-v2.json"
        ))
        .expect("fixture should contain valid JSON");
        let mutations: [fn(&mut serde_json::Value); 4] = [
            |value: &mut serde_json::Value| {
                value["rawVendorRecord"] = serde_json::json!({"secret": true});
            },
            |value: &mut serde_json::Value| {
                value["contentAvailability"]["reasoning"] = serde_json::json!("unavailable");
            },
            |value: &mut serde_json::Value| {
                value["contentAvailability"]["toolDetails"] = serde_json::json!("unavailable");
            },
            |value: &mut serde_json::Value| {
                value["entries"][3]["detail"]
                    .as_object_mut()
                    .expect("tool detail should be an object")
                    .remove("result");
            },
        ];
        for mutate in mutations {
            let mut candidate = fixture.clone();
            mutate(&mut candidate);
            assert!(serde_json::from_value::<NormalizedSessionV2>(candidate).is_err());
        }
    }

    #[test]
    fn rejects_unsupported_versions_and_render_settings() {
        for (pointer, value) in [
            ("/schemaVersion", serde_json::json!(2)),
            ("/session/schemaVersion", serde_json::json!(2)),
            ("/fps", serde_json::json!(60)),
            ("/width", serde_json::json!(3840)),
            ("/terminalHoldMs", serde_json::json!(0)),
            ("/font/lineHeight", serde_json::json!(0.5)),
            ("/segments/0/speed", serde_json::json!(9)),
            ("/session/createdAt", serde_json::json!("August 24, 2026")),
        ] {
            let mut fixture: serde_json::Value =
                serde_json::from_str(include_str!("../../tests/fixtures/contracts-v1.json"))
                    .expect("fixture should contain valid JSON");
            *fixture
                .pointer_mut(pointer)
                .expect("test pointer should exist in fixture") = value;

            assert!(
                serde_json::from_value::<ReplayProjectV1>(fixture).is_err(),
                "{pointer} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_invalid_numeric_timelines() {
        for (pointer, value) in [
            ("/session/events/0/atMs", serde_json::json!(604_800_001_u64)),
            ("/session/durationMs", serde_json::json!(604_800_001_u64)),
            ("/trim/endMs", serde_json::json!(604_800_001_u64)),
            ("/segments/0/sourceStartMs", serde_json::json!(1)),
        ] {
            let mut fixture: serde_json::Value =
                serde_json::from_str(include_str!("../../tests/fixtures/contracts-v1.json"))
                    .expect("fixture should contain valid JSON");
            *fixture
                .pointer_mut(pointer)
                .expect("test pointer should exist in fixture") = value;

            assert!(
                serde_json::from_value::<ReplayProjectV1>(fixture).is_err(),
                "{pointer} should be rejected"
            );
        }
    }

    #[test]
    fn accepts_a_one_frame_replay_when_frame_zero_contains_the_event() {
        let mut fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/contracts-v1.json"))
                .expect("fixture should contain valid JSON");
        fixture["session"]["events"] = serde_json::json!([{
            "id": "event-1",
            "atMs": 0,
            "kind": "user",
            "text": "Start"
        }]);
        fixture["session"]["relationships"] = serde_json::json!([]);
        fixture["session"]["diagnostics"] = serde_json::json!([]);
        fixture["session"]["unknownRecordCount"] = serde_json::json!(0);
        fixture["session"]["durationMs"] = serde_json::json!(250);
        fixture["terminalHoldMs"] = serde_json::json!(250);
        fixture["trim"]["endMs"] = serde_json::json!(250);
        fixture["segments"][0]["sourceEndMs"] = serde_json::json!(250);
        fixture["segments"][0]["speed"] = serde_json::json!(8);

        serde_json::from_value::<ReplayProjectV1>(fixture)
            .expect("frame zero should make the event renderable");
    }

    #[test]
    fn accepts_a_trim_contained_in_the_final_event_hold() {
        let mut fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/contracts-v1.json"))
                .expect("fixture should contain valid JSON");
        fixture["trim"]["startMs"] = serde_json::json!(6_500);
        fixture["segments"][0]["sourceStartMs"] = serde_json::json!(6_500);

        serde_json::from_value::<ReplayProjectV1>(fixture)
            .expect("the prior display state should remain visible from frame zero");
    }

    #[test]
    fn counts_unicode_scalar_values_for_string_limits() {
        let mut fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/contracts-v1.json"))
                .expect("fixture should contain valid JSON");
        fixture["session"]["title"] = serde_json::json!("😀".repeat(300));

        serde_json::from_value::<ReplayProjectV1>(fixture)
            .expect("Unicode scalar counts should match TypeScript");
    }

    #[test]
    fn rejects_a_final_event_that_inverse_frame_mapping_does_not_reach() {
        let mut fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/contracts-v1.json"))
                .expect("fixture should contain valid JSON");
        fixture["session"]["events"] = serde_json::json!([{
            "id": "event-1",
            "atMs": 687,
            "kind": "assistant",
            "markdown": "Finish"
        }]);
        fixture["session"]["relationships"] = serde_json::json!([]);
        fixture["session"]["diagnostics"] = serde_json::json!([]);
        fixture["session"]["unknownRecordCount"] = serde_json::json!(0);
        fixture["session"]["durationMs"] = serde_json::json!(937);
        fixture["terminalHoldMs"] = serde_json::json!(250);
        fixture["trim"]["endMs"] = serde_json::json!(688);
        fixture["segments"][0]["sourceEndMs"] = serde_json::json!(688);
        fixture["segments"][0]["speed"] = serde_json::json!(4.122);

        serde_json::from_value::<ReplayProjectV1>(fixture)
            .expect_err("the final sampled source time is before the event");
    }
}
