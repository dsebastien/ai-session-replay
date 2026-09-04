use std::collections::HashSet;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Deserializer, Serialize};

use crate::model::FontFamily;

pub const INDEXED_LIBRARY_SCHEMA_VERSION: u8 = 1;
pub const MAX_INDEXED_PAGE_SIZE: usize = 200;
pub const MAX_SELECTION_CHANGE_COUNT: usize = 500;
pub const MAX_PRESENTATION_ENTRY_COUNT: usize = 100_000;
pub const DEFAULT_PRESENTATION_DELAY_MS: u64 = 5_000;
pub const MIN_PRESENTATION_DELAY_MS: u64 = 250;
pub const MAX_PRESENTATION_DELAY_MS: u64 = 10_000;
pub const MIN_PRESENTATION_SPEED: f64 = 0.25;
pub const MAX_PRESENTATION_SPEED: f64 = 4.0;

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const MAX_OPAQUE_ID_LENGTH: usize = 256;
const MAX_QUERY_LENGTH: usize = 256;
const MAX_TITLE_LENGTH: usize = 512;
const MAX_ENTRY_TEXT_LENGTH: usize = 2_000_000;
const MAX_TOOL_NAME_LENGTH: usize = 256;
const MAX_DISPLAY_PATH_LENGTH: usize = 32_768;
const MAX_SOURCE_TYPE_LENGTH: usize = 256;
const MAX_DIAGNOSTIC_COUNT: u64 = 1_000;
const MAX_SOURCE_DURATION_MS: u64 = 7 * 24 * 60 * 60 * 1_000;
const MAX_PRESENTATION_DURATION_MS: u64 = 2 * 60 * 60 * 1_000;
const MAX_DELETION_ARTIFACT_COUNT: u64 = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractError {
    pub code: &'static str,
    pub message: &'static str,
}

impl ContractError {
    fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }
}

impl Display for ContractError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ContractError {}

pub trait ContractValidate {
    fn validate(&self) -> Result<(), ContractError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Validated<T>(T);

impl<T: ContractValidate> Validated<T> {
    pub fn new(value: T) -> Result<Self, ContractError> {
        value.validate()?;
        Ok(Self(value))
    }

    pub fn into_inner(self) -> T {
        self.0
    }

    pub fn get(&self) -> &T {
        &self.0
    }
}

impl<'de, T> Deserialize<'de> for Validated<T>
where
    T: serde::de::DeserializeOwned + ContractValidate,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = serde_json::Value::deserialize(deserializer)?;
        let value: T = serde_json::from_value(raw)
            .map_err(|_| serde::de::Error::custom("INVALID_CONTRACT: Contract shape is invalid"))?;
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(Self(value))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ContentAvailability {
    Available,
    Partial,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningAvailability {
    Available,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum IndexedSessionSourceV1 {
    ClaudeCode,
    Codex,
    CopilotCli,
    VscodeCopilot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum JetBrainsPluginStatusV1 {
    NotDetected,
    Detected,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum JetBrainsCopilotCliStatusV1 {
    NotDetected,
    AvailableSeparately,
    JetbrainsAttributed,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JetBrainsCopilotStatusV1 {
    pub schema_version: u8,
    pub plugin_status: JetBrainsPluginStatusV1,
    pub native_transcript_status: JetBrainsNativeTranscriptStatusV1,
    pub copilot_cli_status: JetBrainsCopilotCliStatusV1,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum JetBrainsNativeTranscriptStatusV1 {
    Unsupported,
}

impl ContractValidate for JetBrainsCopilotStatusV1 {
    fn validate(&self) -> Result<(), ContractError> {
        validate_schema(self.schema_version)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContentAvailabilityV1 {
    pub reasoning: ReasoningAvailability,
    pub tool_details: ContentAvailability,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "availability", rename_all = "lowercase", deny_unknown_fields)]
pub enum ToolDetailV1 {
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
pub enum IndexedToolStatus {
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
pub enum SessionEntryV1 {
    User {
        entry_key: String,
        ordinal: u64,
        at_ms: u64,
        text: String,
    },
    Assistant {
        entry_key: String,
        ordinal: u64,
        at_ms: u64,
        markdown: String,
    },
    Reasoning {
        entry_key: String,
        ordinal: u64,
        at_ms: u64,
        text: String,
    },
    ToolCall {
        entry_key: String,
        ordinal: u64,
        at_ms: u64,
        name: String,
        status: IndexedToolStatus,
        summary: String,
        detail: ToolDetailV1,
    },
    FileChange {
        entry_key: String,
        ordinal: u64,
        at_ms: u64,
        display_path: String,
        summary: String,
    },
    Unknown {
        entry_key: String,
        ordinal: u64,
        at_ms: u64,
        source_type: String,
    },
}

impl SessionEntryV1 {
    fn entry_key(&self) -> &str {
        match self {
            Self::User { entry_key, .. }
            | Self::Assistant { entry_key, .. }
            | Self::Reasoning { entry_key, .. }
            | Self::ToolCall { entry_key, .. }
            | Self::FileChange { entry_key, .. }
            | Self::Unknown { entry_key, .. } => entry_key,
        }
    }

    fn ordinal(&self) -> u64 {
        match self {
            Self::User { ordinal, .. }
            | Self::Assistant { ordinal, .. }
            | Self::Reasoning { ordinal, .. }
            | Self::ToolCall { ordinal, .. }
            | Self::FileChange { ordinal, .. }
            | Self::Unknown { ordinal, .. } => *ordinal,
        }
    }

    fn at_ms(&self) -> u64 {
        match self {
            Self::User { at_ms, .. }
            | Self::Assistant { at_ms, .. }
            | Self::Reasoning { at_ms, .. }
            | Self::ToolCall { at_ms, .. }
            | Self::FileChange { at_ms, .. }
            | Self::Unknown { at_ms, .. } => *at_ms,
        }
    }

    fn validate(&self) -> Result<(), ContractError> {
        validate_opaque_id(self.entry_key())?;
        if self.ordinal() >= MAX_PRESENTATION_ENTRY_COUNT as u64
            || self.at_ms() > MAX_SOURCE_DURATION_MS
        {
            return Err(error(
                "INVALID_ENTRY",
                "Entry order or timestamp is invalid",
            ));
        }
        match self {
            Self::User { text, .. } | Self::Reasoning { text, .. } => {
                validate_string(text, 0, MAX_ENTRY_TEXT_LENGTH, "INVALID_ENTRY")
            }
            Self::Assistant { markdown, .. } => {
                validate_string(markdown, 0, MAX_ENTRY_TEXT_LENGTH, "INVALID_ENTRY")
            }
            Self::ToolCall {
                name,
                summary,
                detail,
                ..
            } => {
                validate_string(name, 1, MAX_TOOL_NAME_LENGTH, "INVALID_ENTRY")?;
                validate_string(summary, 0, MAX_ENTRY_TEXT_LENGTH, "INVALID_ENTRY")?;
                validate_tool_detail(detail)
            }
            Self::FileChange {
                display_path,
                summary,
                ..
            } => {
                validate_string(display_path, 1, MAX_DISPLAY_PATH_LENGTH, "INVALID_ENTRY")?;
                validate_string(summary, 0, MAX_ENTRY_TEXT_LENGTH, "INVALID_ENTRY")
            }
            Self::Unknown { source_type, .. } => {
                validate_string(source_type, 1, MAX_SOURCE_TYPE_LENGTH, "INVALID_ENTRY")
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectableEntryV1 {
    pub entry: SessionEntryV1,
    pub selected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IndexedSessionSummaryV1 {
    pub schema_version: u8,
    pub session_id: String,
    pub source: IndexedSessionSourceV1,
    pub title: String,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub created_at_ms: Option<u64>,
    pub last_indexed_at_ms: u64,
    pub source_present: bool,
    pub entry_count: u64,
    pub selected_entry_count: u64,
    pub duration_ms: u64,
    pub diagnostic_count: u64,
    pub content_availability: ContentAvailabilityV1,
}

impl ContractValidate for IndexedSessionSummaryV1 {
    fn validate(&self) -> Result<(), ContractError> {
        validate_schema(self.schema_version)?;
        validate_opaque_id(&self.session_id)?;
        validate_string(&self.title, 1, MAX_TITLE_LENGTH, "INVALID_TITLE")?;
        if self
            .created_at_ms
            .is_some_and(|value| value > MAX_SAFE_INTEGER)
            || self.last_indexed_at_ms > MAX_SAFE_INTEGER
        {
            return Err(error("INVALID_TIMESTAMP", "Session timestamp is invalid"));
        }
        if self.entry_count == 0
            || self.entry_count > MAX_PRESENTATION_ENTRY_COUNT as u64
            || self.selected_entry_count > self.entry_count
        {
            return Err(error(
                "INVALID_ENTRY_COUNT",
                "Session entry counts are invalid",
            ));
        }
        if self.duration_ms > MAX_SOURCE_DURATION_MS {
            return Err(error(
                "INVALID_SESSION_DURATION",
                "Session duration is invalid",
            ));
        }
        if self.diagnostic_count > MAX_DIAGNOSTIC_COUNT {
            return Err(error(
                "INVALID_DIAGNOSTIC_COUNT",
                "Diagnostic count is invalid",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IndexedSessionListRequestV1 {
    pub schema_version: u8,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub source: Option<IndexedSessionSourceV1>,
    pub query: String,
    pub sort_order: IndexedSessionSortOrderV1,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub cursor: Option<String>,
    pub page_size: usize,
}

impl ContractValidate for IndexedSessionListRequestV1 {
    fn validate(&self) -> Result<(), ContractError> {
        validate_schema(self.schema_version)?;
        validate_string(&self.query, 0, MAX_QUERY_LENGTH, "INVALID_QUERY")?;
        if let Some(cursor) = &self.cursor
            && self.sort_order.session_id_from_cursor(cursor).is_none()
        {
            return Err(error("INVALID_CURSOR", "Page cursor is invalid"));
        }
        if !(1..=MAX_INDEXED_PAGE_SIZE).contains(&self.page_size) {
            return Err(error("INVALID_PAGE_SIZE", "Page size is invalid"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum IndexedSessionSortOrderV1 {
    Newest,
    Oldest,
}

impl IndexedSessionSortOrderV1 {
    pub(crate) fn cursor_for(self, session_id: &str) -> String {
        format!("{}__{session_id}", self.cursor_prefix())
    }

    pub(crate) fn session_id_from_cursor(self, cursor: &str) -> Option<&str> {
        cursor
            .strip_prefix(self.cursor_prefix())
            .and_then(|value| value.strip_prefix("__"))
            .filter(|session_id| validate_opaque_id(session_id).is_ok())
    }

    fn cursor_prefix(self) -> &'static str {
        match self {
            Self::Newest => "newest",
            Self::Oldest => "oldest",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IndexedSessionPageV1 {
    pub schema_version: u8,
    pub sort_order: IndexedSessionSortOrderV1,
    pub items: Vec<IndexedSessionSummaryV1>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub next_cursor: Option<String>,
}

impl ContractValidate for IndexedSessionPageV1 {
    fn validate(&self) -> Result<(), ContractError> {
        validate_schema(self.schema_version)?;
        if self.items.len() > MAX_INDEXED_PAGE_SIZE {
            return Err(error(
                "PAGE_LIMIT_EXCEEDED",
                "Session page exceeds the supported limit",
            ));
        }
        let mut ids = HashSet::new();
        for item in &self.items {
            item.validate()?;
            if !ids.insert(item.session_id.as_str()) {
                return Err(error(
                    "DUPLICATE_SESSION_ID",
                    "Session page contains duplicate IDs",
                ));
            }
        }
        if let Some(cursor) = &self.next_cursor
            && self.sort_order.session_id_from_cursor(cursor).is_none()
        {
            return Err(error("INVALID_CURSOR", "Page cursor is invalid"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionRevisionV1 {
    pub schema_version: u8,
    pub revision_id: String,
    pub session_id: String,
    pub indexed_at_ms: u64,
    pub entry_count: u64,
    pub duration_ms: u64,
    pub diagnostic_count: u64,
}

impl ContractValidate for SessionRevisionV1 {
    fn validate(&self) -> Result<(), ContractError> {
        validate_schema(self.schema_version)?;
        validate_opaque_id(&self.revision_id)?;
        validate_opaque_id(&self.session_id)?;
        if self.indexed_at_ms > MAX_SAFE_INTEGER
            || self.entry_count == 0
            || self.entry_count > MAX_PRESENTATION_ENTRY_COUNT as u64
            || self.duration_ms > MAX_SOURCE_DURATION_MS
            || self.diagnostic_count > MAX_DIAGNOSTIC_COUNT
        {
            return Err(error("INVALID_REVISION", "Revision metadata is invalid"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IndexedEntryPageV1 {
    pub schema_version: u8,
    pub session_id: String,
    pub revision_id: String,
    pub entries: Vec<SelectableEntryV1>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub next_cursor: Option<String>,
    pub total_entry_count: u64,
}

impl ContractValidate for IndexedEntryPageV1 {
    fn validate(&self) -> Result<(), ContractError> {
        validate_schema(self.schema_version)?;
        validate_opaque_id(&self.session_id)?;
        validate_opaque_id(&self.revision_id)?;
        if self.entries.len() > MAX_INDEXED_PAGE_SIZE {
            return Err(error(
                "PAGE_LIMIT_EXCEEDED",
                "Entry page exceeds the supported limit",
            ));
        }
        if self.total_entry_count == 0
            || self.total_entry_count > MAX_PRESENTATION_ENTRY_COUNT as u64
            || self.entries.len() as u64 > self.total_entry_count
        {
            return Err(error("INVALID_ENTRY_COUNT", "Entry count is invalid"));
        }
        let mut keys = HashSet::new();
        let mut previous_ordinal = None;
        let mut previous_at_ms = None;
        for selectable in &self.entries {
            selectable.entry.validate()?;
            if !keys.insert(selectable.entry.entry_key())
                || previous_ordinal.is_some_and(|value| selectable.entry.ordinal() <= value)
                || previous_at_ms.is_some_and(|value| selectable.entry.at_ms() < value)
            {
                return Err(error(
                    "INVALID_ENTRY_ORDER",
                    "Entries are not in deterministic order",
                ));
            }
            previous_ordinal = Some(selectable.entry.ordinal());
            previous_at_ms = Some(selectable.entry.at_ms());
        }
        if let Some(cursor) = &self.next_cursor {
            validate_named_opaque_id(cursor, "INVALID_CURSOR")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VisibilityPreferencesV1 {
    pub show_tool_calls: bool,
    pub show_tool_details: bool,
    pub show_reasoning: bool,
}

impl ContractValidate for VisibilityPreferencesV1 {
    fn validate(&self) -> Result<(), ContractError> {
        if self.show_tool_details && !self.show_tool_calls {
            return Err(error(
                "INVALID_VISIBILITY",
                "Visibility preferences are invalid",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PresentationTimingV1 {
    pub entry_delay_ms: u64,
    pub playback_speed: f64,
}

impl ContractValidate for PresentationTimingV1 {
    fn validate(&self) -> Result<(), ContractError> {
        if !(MIN_PRESENTATION_DELAY_MS..=MAX_PRESENTATION_DELAY_MS).contains(&self.entry_delay_ms)
            || !self.playback_speed.is_finite()
            || !(MIN_PRESENTATION_SPEED..=MAX_PRESENTATION_SPEED).contains(&self.playback_speed)
        {
            return Err(error(
                "INVALID_PRESENTATION_TIMING",
                "Presentation timing is invalid",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LibraryThemeV1 {
    pub background: String,
    pub surface: String,
    pub text: String,
    pub muted: String,
    pub accent: String,
    pub success: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LibraryFontV1 {
    pub family: FontFamily,
    pub size_px: u32,
    pub line_height: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppearancePreferencesV1 {
    pub theme: LibraryThemeV1,
    pub font: LibraryFontV1,
}

impl ContractValidate for AppearancePreferencesV1 {
    fn validate(&self) -> Result<(), ContractError> {
        for color in [
            &self.theme.background,
            &self.theme.surface,
            &self.theme.text,
            &self.theme.muted,
            &self.theme.accent,
            &self.theme.success,
            &self.theme.error,
        ] {
            if color.len() != 7
                || !color.starts_with('#')
                || !color[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(error("INVALID_APPEARANCE", "Theme colors are invalid"));
            }
        }
        if !(12..=96).contains(&self.font.size_px)
            || !self.font.line_height.is_finite()
            || !(1.0..=2.5).contains(&self.font.line_height)
        {
            return Err(error("INVALID_APPEARANCE", "Font settings are invalid"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionPreferencesV1 {
    pub schema_version: u8,
    pub visibility: VisibilityPreferencesV1,
    pub timing: PresentationTimingV1,
    pub appearance: AppearancePreferencesV1,
}

impl ContractValidate for SessionPreferencesV1 {
    fn validate(&self) -> Result<(), ContractError> {
        validate_schema(self.schema_version)?;
        self.visibility.validate()?;
        self.timing.validate()?;
        self.appearance.validate()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IndexedSessionDetailV1 {
    pub schema_version: u8,
    pub summary: IndexedSessionSummaryV1,
    pub revision: SessionRevisionV1,
    pub entry_page: IndexedEntryPageV1,
    pub preferences: SessionPreferencesV1,
}

impl ContractValidate for IndexedSessionDetailV1 {
    fn validate(&self) -> Result<(), ContractError> {
        validate_schema(self.schema_version)?;
        self.summary.validate()?;
        self.revision.validate()?;
        self.entry_page.validate()?;
        self.preferences.validate()?;
        if self.summary.session_id != self.revision.session_id
            || self.summary.session_id != self.entry_page.session_id
        {
            return Err(error("SESSION_MISMATCH", "Session identities do not match"));
        }
        if self.revision.revision_id != self.entry_page.revision_id {
            return Err(error(
                "REVISION_MISMATCH",
                "Revision identities do not match",
            ));
        }
        if self.summary.entry_count != self.revision.entry_count
            || self.summary.entry_count != self.entry_page.total_entry_count
            || self.summary.duration_ms != self.revision.duration_ms
            || self.summary.diagnostic_count != self.revision.diagnostic_count
        {
            return Err(error(
                "SESSION_METADATA_MISMATCH",
                "Session metadata does not match the current revision",
            ));
        }
        for selectable in &self.entry_page.entries {
            match &selectable.entry {
                SessionEntryV1::Reasoning { .. }
                    if self.summary.content_availability.reasoning
                        == ReasoningAvailability::Unavailable =>
                {
                    return Err(error(
                        "CONTENT_AVAILABILITY_MISMATCH",
                        "Reasoning availability contradicts the entries",
                    ));
                }
                SessionEntryV1::ToolCall { detail, .. } => {
                    let detail_is_available = matches!(detail, ToolDetailV1::Available { .. });
                    if (detail_is_available
                        && self.summary.content_availability.tool_details
                            == ContentAvailability::Unavailable)
                        || (!detail_is_available
                            && self.summary.content_availability.tool_details
                                == ContentAvailability::Available)
                    {
                        return Err(error(
                            "CONTENT_AVAILABILITY_MISMATCH",
                            "Tool-detail availability contradicts the entries",
                        ));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntrySelectionChangeV1 {
    pub entry_key: String,
    pub selected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetEntrySelectionsRequestV1 {
    pub schema_version: u8,
    pub session_id: String,
    pub revision_id: String,
    pub changes: Vec<EntrySelectionChangeV1>,
}

impl ContractValidate for SetEntrySelectionsRequestV1 {
    fn validate(&self) -> Result<(), ContractError> {
        validate_schema(self.schema_version)?;
        validate_opaque_id(&self.session_id)?;
        validate_opaque_id(&self.revision_id)?;
        if self.changes.len() > MAX_SELECTION_CHANGE_COUNT {
            return Err(error(
                "SELECTION_CHANGE_LIMIT_EXCEEDED",
                "Selection change batch exceeds the supported limit",
            ));
        }
        let mut keys = HashSet::new();
        for change in &self.changes {
            validate_opaque_id(&change.entry_key)?;
            if !keys.insert(change.entry_key.as_str()) {
                return Err(error(
                    "DUPLICATE_ENTRY_KEY",
                    "Selection changes contain duplicate entry keys",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RenameIndexedSessionRequestV1 {
    pub schema_version: u8,
    pub session_id: String,
    pub title: String,
}

impl ContractValidate for RenameIndexedSessionRequestV1 {
    fn validate(&self) -> Result<(), ContractError> {
        validate_schema(self.schema_version)?;
        validate_opaque_id(&self.session_id)?;
        validate_string(&self.title, 1, MAX_TITLE_LENGTH, "INVALID_SESSION_TITLE")?;
        if self.title.trim() != self.title {
            return Err(error("INVALID_SESSION_TITLE", "Session title is invalid"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetSessionPreferencesRequestV1 {
    pub schema_version: u8,
    pub session_id: String,
    pub revision_id: String,
    pub preferences: SessionPreferencesV1,
}

impl ContractValidate for SetSessionPreferencesRequestV1 {
    fn validate(&self) -> Result<(), ContractError> {
        validate_schema(self.schema_version)?;
        validate_opaque_id(&self.session_id)?;
        validate_opaque_id(&self.revision_id)?;
        self.preferences.validate()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "status",
    rename_all = "lowercase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum IndexRefreshStateV1 {
    Idle {
        schema_version: u8,
        #[serde(deserialize_with = "deserialize_required_nullable")]
        last_completed_at_ms: Option<u64>,
    },
    Running {
        schema_version: u8,
        generation: u64,
        started_at_ms: u64,
        discovered_count: u64,
        processed_count: u64,
        indexed_count: u64,
        unchanged_count: u64,
        failed_count: u64,
        skipped_count: u64,
        warning_count: u64,
    },
    Completed {
        schema_version: u8,
        generation: u64,
        started_at_ms: u64,
        completed_at_ms: u64,
        discovered_count: u64,
        processed_count: u64,
        indexed_count: u64,
        unchanged_count: u64,
        failed_count: u64,
        skipped_count: u64,
        warning_count: u64,
    },
    Failed {
        schema_version: u8,
        generation: u64,
        started_at_ms: u64,
        completed_at_ms: u64,
        error_code: String,
        discovered_count: u64,
        processed_count: u64,
        indexed_count: u64,
        unchanged_count: u64,
        failed_count: u64,
        skipped_count: u64,
        warning_count: u64,
    },
}

impl ContractValidate for IndexRefreshStateV1 {
    fn validate(&self) -> Result<(), ContractError> {
        match self {
            Self::Idle {
                schema_version,
                last_completed_at_ms,
            } => {
                validate_schema(*schema_version)?;
                if last_completed_at_ms.is_some_and(|value| value > MAX_SAFE_INTEGER) {
                    return Err(error("INVALID_TIMESTAMP", "Refresh timestamp is invalid"));
                }
            }
            Self::Running {
                schema_version,
                generation,
                started_at_ms,
                discovered_count,
                processed_count,
                indexed_count,
                unchanged_count,
                failed_count,
                skipped_count,
                warning_count,
            } => {
                validate_refresh_header(*schema_version, *generation, *started_at_ms)?;
                validate_refresh_counts(
                    *discovered_count,
                    *processed_count,
                    *indexed_count,
                    *unchanged_count,
                    *failed_count,
                    *skipped_count,
                    *warning_count,
                )?;
            }
            Self::Completed {
                schema_version,
                generation,
                started_at_ms,
                completed_at_ms,
                discovered_count,
                processed_count,
                indexed_count,
                unchanged_count,
                failed_count,
                skipped_count,
                warning_count,
            } => {
                validate_refresh_header(*schema_version, *generation, *started_at_ms)?;
                validate_completed_at(*started_at_ms, *completed_at_ms)?;
                validate_refresh_counts(
                    *discovered_count,
                    *processed_count,
                    *indexed_count,
                    *unchanged_count,
                    *failed_count,
                    *skipped_count,
                    *warning_count,
                )?;
            }
            Self::Failed {
                schema_version,
                generation,
                started_at_ms,
                completed_at_ms,
                error_code,
                discovered_count,
                processed_count,
                indexed_count,
                unchanged_count,
                failed_count,
                skipped_count,
                warning_count,
            } => {
                validate_refresh_header(*schema_version, *generation, *started_at_ms)?;
                validate_completed_at(*started_at_ms, *completed_at_ms)?;
                validate_safe_code(error_code)?;
                validate_refresh_counts(
                    *discovered_count,
                    *processed_count,
                    *indexed_count,
                    *unchanged_count,
                    *failed_count,
                    *skipped_count,
                    *warning_count,
                )?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum LibraryOnlyDeletionMode {
    LibraryOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PrepareSourceDeletionMode {
    PrepareSourceDeletion,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SourceDeletionMode {
    LibraryAndSource,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeleteIndexedSessionRequestV1 {
    pub schema_version: u8,
    pub mode: LibraryOnlyDeletionMode,
    pub session_id: String,
}

impl ContractValidate for DeleteIndexedSessionRequestV1 {
    fn validate(&self) -> Result<(), ContractError> {
        validate_schema(self.schema_version)?;
        validate_opaque_id(&self.session_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrepareSourceDeletionRequestV1 {
    pub schema_version: u8,
    pub mode: PrepareSourceDeletionMode,
    pub session_id: String,
}

impl ContractValidate for PrepareSourceDeletionRequestV1 {
    fn validate(&self) -> Result<(), ContractError> {
        validate_schema(self.schema_version)?;
        validate_opaque_id(&self.session_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceDeletionConfirmationV1 {
    pub schema_version: u8,
    pub session_id: String,
    pub confirmation_token: String,
    pub artifact_count: u64,
    pub expires_at_ms: u64,
}

impl ContractValidate for SourceDeletionConfirmationV1 {
    fn validate(&self) -> Result<(), ContractError> {
        validate_schema(self.schema_version)?;
        validate_opaque_id(&self.session_id)?;
        validate_named_opaque_id(&self.confirmation_token, "INVALID_CONFIRMATION_TOKEN")?;
        if !(1..=MAX_DELETION_ARTIFACT_COUNT).contains(&self.artifact_count) {
            return Err(error(
                "INVALID_ARTIFACT_COUNT",
                "Deletion artifact count is invalid",
            ));
        }
        validate_timestamp(self.expires_at_ms)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeleteIndexedSessionWithSourceRequestV1 {
    pub schema_version: u8,
    pub mode: SourceDeletionMode,
    pub session_id: String,
    pub confirmation_token: String,
}

impl ContractValidate for DeleteIndexedSessionWithSourceRequestV1 {
    fn validate(&self) -> Result<(), ContractError> {
        validate_schema(self.schema_version)?;
        validate_opaque_id(&self.session_id)?;
        validate_named_opaque_id(&self.confirmation_token, "INVALID_CONFIRMATION_TOKEN")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestoreSuppressedSourceRequestV1 {
    pub schema_version: u8,
    pub suppression_id: String,
}

impl ContractValidate for RestoreSuppressedSourceRequestV1 {
    fn validate(&self) -> Result<(), ContractError> {
        validate_schema(self.schema_version)?;
        validate_opaque_id(&self.suppression_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SuppressedSourceV1 {
    pub schema_version: u8,
    pub suppression_id: String,
    pub source: IndexedSessionSourceV1,
    pub source_deleted: bool,
    pub suppressed_at_ms: u64,
}

impl ContractValidate for SuppressedSourceV1 {
    fn validate(&self) -> Result<(), ContractError> {
        validate_schema(self.schema_version)?;
        validate_opaque_id(&self.suppression_id)?;
        validate_timestamp(self.suppressed_at_ms)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SuppressedSourceListRequestV1 {
    pub schema_version: u8,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub cursor: Option<String>,
    pub page_size: usize,
}

impl ContractValidate for SuppressedSourceListRequestV1 {
    fn validate(&self) -> Result<(), ContractError> {
        validate_schema(self.schema_version)?;
        if let Some(cursor) = &self.cursor {
            validate_opaque_id(cursor)?;
        }
        if !(1..=MAX_INDEXED_PAGE_SIZE).contains(&self.page_size) {
            return Err(error("INVALID_PAGE_SIZE", "Page size is invalid"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SuppressedSourcePageV1 {
    pub schema_version: u8,
    pub items: Vec<SuppressedSourceV1>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub next_cursor: Option<String>,
}

impl ContractValidate for SuppressedSourcePageV1 {
    fn validate(&self) -> Result<(), ContractError> {
        validate_schema(self.schema_version)?;
        if self.items.len() > MAX_INDEXED_PAGE_SIZE {
            return Err(error(
                "PAGE_LIMIT_EXCEEDED",
                "Suppressed-source page exceeds the supported limit",
            ));
        }
        let mut ids = HashSet::with_capacity(self.items.len());
        for item in &self.items {
            item.validate()?;
            if !ids.insert(item.suppression_id.as_str()) {
                return Err(error(
                    "DUPLICATE_SUPPRESSION_ID",
                    "Suppressed-source page contains duplicate IDs",
                ));
            }
        }
        if let Some(cursor) = &self.next_cursor {
            validate_opaque_id(cursor)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestoreSuppressedSourceResultV1 {
    pub schema_version: u8,
    pub suppression_id: String,
}

impl ContractValidate for RestoreSuppressedSourceResultV1 {
    fn validate(&self) -> Result<(), ContractError> {
        validate_schema(self.schema_version)?;
        validate_opaque_id(&self.suppression_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ResetLocalDatabaseMode {
    ResetLocalDatabase,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResetLocalDatabaseRequestV1 {
    pub schema_version: u8,
    pub mode: ResetLocalDatabaseMode,
}

impl ContractValidate for ResetLocalDatabaseRequestV1 {
    fn validate(&self) -> Result<(), ContractError> {
        validate_schema(self.schema_version)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResetLocalDatabaseResultV1 {
    pub schema_version: u8,
    pub reset_at_ms: u64,
}

impl ContractValidate for ResetLocalDatabaseResultV1 {
    fn validate(&self) -> Result<(), ContractError> {
        validate_schema(self.schema_version)?;
        if self.reset_at_ms > MAX_SAFE_INTEGER {
            return Err(error("INVALID_TIMESTAMP", "Reset timestamp is invalid"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PresentedToolDetailV1 {
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub arguments: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub result: Option<String>,
}

impl PresentedToolDetailV1 {
    fn validate(&self) -> Result<(), ContractError> {
        validate_optional_string(
            &self.arguments,
            MAX_ENTRY_TEXT_LENGTH,
            "INVALID_TOOL_DETAIL",
        )?;
        validate_optional_string(&self.result, MAX_ENTRY_TEXT_LENGTH, "INVALID_TOOL_DETAIL")?;
        if self.arguments.is_none() && self.result.is_none() {
            return Err(error(
                "INVALID_TOOL_DETAIL",
                "Presented tool detail is invalid",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PresentationEntryContentV1 {
    User {
        entry_key: String,
        ordinal: u64,
        at_ms: u64,
        text: String,
    },
    Assistant {
        entry_key: String,
        ordinal: u64,
        at_ms: u64,
        markdown: String,
    },
    Reasoning {
        entry_key: String,
        ordinal: u64,
        at_ms: u64,
        text: String,
    },
    ToolCall {
        entry_key: String,
        ordinal: u64,
        at_ms: u64,
        name: String,
        status: IndexedToolStatus,
        summary: String,
        #[serde(deserialize_with = "deserialize_required_nullable")]
        detail: Option<PresentedToolDetailV1>,
    },
    FileChange {
        entry_key: String,
        ordinal: u64,
        at_ms: u64,
        display_path: String,
        summary: String,
    },
    Unknown {
        entry_key: String,
        ordinal: u64,
        at_ms: u64,
        source_type: String,
    },
}

impl PresentationEntryContentV1 {
    fn entry_key(&self) -> &str {
        match self {
            Self::User { entry_key, .. }
            | Self::Assistant { entry_key, .. }
            | Self::Reasoning { entry_key, .. }
            | Self::ToolCall { entry_key, .. }
            | Self::FileChange { entry_key, .. }
            | Self::Unknown { entry_key, .. } => entry_key,
        }
    }

    fn ordinal(&self) -> u64 {
        match self {
            Self::User { ordinal, .. }
            | Self::Assistant { ordinal, .. }
            | Self::Reasoning { ordinal, .. }
            | Self::ToolCall { ordinal, .. }
            | Self::FileChange { ordinal, .. }
            | Self::Unknown { ordinal, .. } => *ordinal,
        }
    }

    fn at_ms(&self) -> u64 {
        match self {
            Self::User { at_ms, .. }
            | Self::Assistant { at_ms, .. }
            | Self::Reasoning { at_ms, .. }
            | Self::ToolCall { at_ms, .. }
            | Self::FileChange { at_ms, .. }
            | Self::Unknown { at_ms, .. } => *at_ms,
        }
    }

    fn validate(&self) -> Result<(), ContractError> {
        validate_opaque_id(self.entry_key())?;
        if self.ordinal() >= MAX_PRESENTATION_ENTRY_COUNT as u64
            || self.at_ms() > MAX_SOURCE_DURATION_MS
        {
            return Err(error("INVALID_ENTRY", "Presentation entry is invalid"));
        }
        match self {
            Self::User { text, .. } | Self::Reasoning { text, .. } => {
                validate_string(text, 0, MAX_ENTRY_TEXT_LENGTH, "INVALID_PRESENTATION_ENTRY")
            }
            Self::Assistant { markdown, .. } => validate_string(
                markdown,
                0,
                MAX_ENTRY_TEXT_LENGTH,
                "INVALID_PRESENTATION_ENTRY",
            ),
            Self::ToolCall {
                name,
                summary,
                detail,
                ..
            } => {
                validate_string(name, 1, MAX_TOOL_NAME_LENGTH, "INVALID_PRESENTATION_ENTRY")?;
                validate_string(
                    summary,
                    0,
                    MAX_ENTRY_TEXT_LENGTH,
                    "INVALID_PRESENTATION_ENTRY",
                )?;
                if let Some(detail) = detail {
                    detail.validate()?;
                }
                Ok(())
            }
            Self::FileChange {
                display_path,
                summary,
                ..
            } => {
                validate_string(
                    display_path,
                    1,
                    MAX_DISPLAY_PATH_LENGTH,
                    "INVALID_PRESENTATION_ENTRY",
                )?;
                validate_string(
                    summary,
                    0,
                    MAX_ENTRY_TEXT_LENGTH,
                    "INVALID_PRESENTATION_ENTRY",
                )
            }
            Self::Unknown { source_type, .. } => validate_string(
                source_type,
                1,
                MAX_SOURCE_TYPE_LENGTH,
                "INVALID_PRESENTATION_ENTRY",
            ),
        }
    }

    fn is_reasoning(&self) -> bool {
        matches!(self, Self::Reasoning { .. })
    }

    fn is_tool_call(&self) -> bool {
        matches!(self, Self::ToolCall { .. })
    }

    fn has_tool_detail(&self) -> bool {
        matches!(
            self,
            Self::ToolCall {
                detail: Some(_),
                ..
            }
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PresentationPlanEntryV1 {
    pub entry: PresentationEntryContentV1,
    pub reveal_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PresentationPlanV1 {
    pub schema_version: u8,
    pub plan_id: String,
    pub session_id: String,
    pub revision_id: String,
    pub session_title: String,
    pub created_at_ms: u64,
    pub entries: Vec<PresentationPlanEntryV1>,
    pub preferences: SessionPreferencesV1,
    pub duration_ms: u64,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
}

impl ContractValidate for PresentationPlanV1 {
    fn validate(&self) -> Result<(), ContractError> {
        validate_schema(self.schema_version)?;
        validate_opaque_id(&self.plan_id)?;
        validate_opaque_id(&self.session_id)?;
        validate_opaque_id(&self.revision_id)?;
        validate_string(
            &self.session_title,
            1,
            MAX_TITLE_LENGTH,
            "INVALID_PRESENTATION_PLAN",
        )?;
        validate_timestamp(self.created_at_ms)?;
        self.preferences.validate()?;
        if self.entries.is_empty() {
            return Err(error(
                "EMPTY_PRESENTATION_PLAN",
                "Presentation plan has no entries",
            ));
        }
        if self.entries.len() > MAX_PRESENTATION_ENTRY_COUNT {
            return Err(error(
                "PRESENTATION_ENTRY_LIMIT_EXCEEDED",
                "Presentation plan exceeds the supported entry limit",
            ));
        }
        let mut keys = HashSet::new();
        let mut previous_ordinal = None;
        let mut previous_at_ms = None;
        let mut previous_reveal_at_ms = None;
        for item in &self.entries {
            item.entry.validate()?;
            if item.reveal_at_ms > MAX_PRESENTATION_DURATION_MS
                || !keys.insert(item.entry.entry_key())
                || previous_ordinal.is_some_and(|value| item.entry.ordinal() <= value)
                || previous_at_ms.is_some_and(|value| item.entry.at_ms() < value)
                || previous_reveal_at_ms.is_some_and(|value| item.reveal_at_ms < value)
            {
                return Err(error(
                    "INVALID_PRESENTATION_ORDER",
                    "Presentation entries are not in deterministic order",
                ));
            }
            previous_ordinal = Some(item.entry.ordinal());
            previous_at_ms = Some(item.entry.at_ms());
            previous_reveal_at_ms = Some(item.reveal_at_ms);
        }
        if (!self.preferences.visibility.show_reasoning
            && self.entries.iter().any(|item| item.entry.is_reasoning()))
            || (!self.preferences.visibility.show_tool_calls
                && self.entries.iter().any(|item| item.entry.is_tool_call()))
            || (!self.preferences.visibility.show_tool_details
                && self.entries.iter().any(|item| item.entry.has_tool_detail()))
        {
            return Err(error(
                "PRESENTATION_VISIBILITY_MISMATCH",
                "Presentation entries contradict visibility preferences",
            ));
        }
        if self.duration_ms == 0
            || self.duration_ms > MAX_PRESENTATION_DURATION_MS
            || previous_reveal_at_ms.is_some_and(|value| value >= self.duration_ms)
        {
            return Err(error(
                "INVALID_PRESENTATION_DURATION",
                "Presentation duration is invalid",
            ));
        }
        if self.fps != 30 || self.width != 1920 || self.height != 1080 {
            return Err(error(
                "INVALID_RENDER_SETTINGS",
                "Presentation render settings are invalid",
            ));
        }
        Ok(())
    }
}

fn validate_tool_detail(detail: &ToolDetailV1) -> Result<(), ContractError> {
    match detail {
        ToolDetailV1::Unavailable => Ok(()),
        ToolDetailV1::Available { arguments, result } => {
            validate_optional_string(arguments, MAX_ENTRY_TEXT_LENGTH, "INVALID_TOOL_DETAIL")?;
            validate_optional_string(result, MAX_ENTRY_TEXT_LENGTH, "INVALID_TOOL_DETAIL")?;
            if arguments.is_none() && result.is_none() {
                return Err(error(
                    "INVALID_TOOL_DETAIL",
                    "Available tool detail must contain arguments or a result",
                ));
            }
            Ok(())
        }
    }
}

fn validate_refresh_header(
    schema_version: u8,
    generation: u64,
    started_at_ms: u64,
) -> Result<(), ContractError> {
    validate_schema(schema_version)?;
    if generation == 0 || generation > MAX_SAFE_INTEGER {
        return Err(error(
            "INVALID_REFRESH_STATE",
            "Refresh generation is invalid",
        ));
    }
    validate_timestamp(started_at_ms)
}

fn validate_completed_at(started_at_ms: u64, completed_at_ms: u64) -> Result<(), ContractError> {
    validate_timestamp(completed_at_ms)?;
    if completed_at_ms < started_at_ms {
        return Err(error("INVALID_TIMESTAMP", "Refresh timestamp is invalid"));
    }
    Ok(())
}

fn validate_refresh_counts(
    discovered: u64,
    processed: u64,
    indexed: u64,
    unchanged: u64,
    failed: u64,
    skipped: u64,
    warnings: u64,
) -> Result<(), ContractError> {
    if discovered > MAX_PRESENTATION_ENTRY_COUNT as u64
        || processed > discovered
        || indexed > processed
        || unchanged > processed
        || failed > processed
        || skipped > processed
        || warnings > MAX_PRESENTATION_ENTRY_COUNT as u64
        || indexed
            .checked_add(unchanged)
            .and_then(|value| value.checked_add(failed))
            .and_then(|value| value.checked_add(skipped))
            != Some(processed)
    {
        return Err(error(
            "INVALID_REFRESH_COUNTS",
            "Refresh counts are inconsistent",
        ));
    }
    Ok(())
}

fn validate_schema(schema_version: u8) -> Result<(), ContractError> {
    if schema_version != INDEXED_LIBRARY_SCHEMA_VERSION {
        return Err(error(
            "UNSUPPORTED_SCHEMA_VERSION",
            "Schema version is not supported",
        ));
    }
    Ok(())
}

fn deserialize_required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

fn validate_timestamp(value: u64) -> Result<(), ContractError> {
    if value > MAX_SAFE_INTEGER {
        return Err(error("INVALID_TIMESTAMP", "Timestamp is invalid"));
    }
    Ok(())
}

fn validate_opaque_id(value: &str) -> Result<(), ContractError> {
    validate_named_opaque_id(value, "INVALID_OPAQUE_ID")
}

fn validate_named_opaque_id(value: &str, code: &'static str) -> Result<(), ContractError> {
    if value.is_empty()
        || value.len() > MAX_OPAQUE_ID_LENGTH
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(error(code, "Opaque identifier is invalid"));
    }
    Ok(())
}

fn validate_safe_code(value: &str) -> Result<(), ContractError> {
    if value.is_empty()
        || value.len() > 128
        || !value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_uppercase()
                || byte.is_ascii_digit() && index > 0
                || byte == b'_' && index > 0
        })
    {
        return Err(error("INVALID_ERROR_CODE", "Refresh error code is invalid"));
    }
    Ok(())
}

fn validate_optional_string(
    value: &Option<String>,
    maximum: usize,
    code: &'static str,
) -> Result<(), ContractError> {
    if let Some(value) = value {
        validate_string(value, 0, maximum, code)?;
    }
    Ok(())
}

fn validate_string(
    value: &str,
    minimum: usize,
    maximum: usize,
    code: &'static str,
) -> Result<(), ContractError> {
    let length = value.chars().count();
    if length < minimum || length > maximum || value.contains('\0') {
        return Err(error(code, "String value is invalid"));
    }
    Ok(())
}

fn error(code: &'static str, message: &'static str) -> ContractError {
    ContractError::new(code, message)
}

#[cfg(test)]
mod tests {
    use serde::Serialize;
    use serde::de::DeserializeOwned;
    use serde_json::{Value, json};

    use super::{
        DeleteIndexedSessionRequestV1, DeleteIndexedSessionWithSourceRequestV1,
        IndexRefreshStateV1, IndexedSessionDetailV1, IndexedSessionListRequestV1,
        IndexedSessionPageV1, JetBrainsCopilotStatusV1, MAX_DELETION_ARTIFACT_COUNT,
        MAX_DIAGNOSTIC_COUNT, MAX_INDEXED_PAGE_SIZE, MAX_OPAQUE_ID_LENGTH,
        MAX_PRESENTATION_DURATION_MS, MAX_PRESENTATION_ENTRY_COUNT, MAX_SELECTION_CHANGE_COUNT,
        MAX_SOURCE_DURATION_MS, PrepareSourceDeletionRequestV1, PresentationPlanV1,
        RenameIndexedSessionRequestV1, ResetLocalDatabaseRequestV1, ResetLocalDatabaseResultV1,
        RestoreSuppressedSourceRequestV1, RestoreSuppressedSourceResultV1,
        SetEntrySelectionsRequestV1, SetSessionPreferencesRequestV1, SourceDeletionConfirmationV1,
        SuppressedSourceListRequestV1, SuppressedSourcePageV1, SuppressedSourceV1, Validated,
    };

    fn fixture() -> Value {
        serde_json::from_str(include_str!(
            "../../tests/fixtures/indexed-library-contracts-v1.json"
        ))
        .expect("indexed-library fixture should be valid JSON")
    }

    fn assert_round_trip<T>(value: &Value)
    where
        T: DeserializeOwned + Serialize + super::ContractValidate,
    {
        let validated: Validated<T> = serde_json::from_value(value.clone())
            .expect("fixture member should satisfy the Rust contract");
        let actual = serde_json::to_value(validated.into_inner())
            .expect("validated contract should serialize");
        assert_eq!(actual, *value);
    }

    fn assert_rejected<T>(value: Value, code: &str)
    where
        T: DeserializeOwned + super::ContractValidate + std::fmt::Debug,
    {
        let error = serde_json::from_value::<Validated<T>>(value)
            .expect_err("hostile contract payload should be rejected")
            .to_string();
        assert!(error.contains(code), "expected {code}, got {error}");
    }

    fn assert_fixture_mutation_rejected<T>(
        member: &str,
        code: &str,
        mutate: impl FnOnce(&mut Value),
    ) where
        T: DeserializeOwned + super::ContractValidate + std::fmt::Debug,
    {
        let mut value = fixture()[member].clone();
        mutate(&mut value);
        assert_rejected::<T>(value, code);
    }

    #[test]
    fn round_trips_every_typescript_contract_fixture_member() {
        let fixture = fixture();
        assert_round_trip::<IndexedSessionListRequestV1>(&fixture["listRequest"]);
        assert_round_trip::<IndexedSessionPageV1>(&fixture["sessionPage"]);
        assert_round_trip::<IndexedSessionDetailV1>(&fixture["sessionDetail"]);
        assert_round_trip::<SetEntrySelectionsRequestV1>(&fixture["selectionRequest"]);
        assert_round_trip::<RenameIndexedSessionRequestV1>(&fixture["renameRequest"]);
        assert_round_trip::<SetSessionPreferencesRequestV1>(&fixture["preferencesRequest"]);
        assert_round_trip::<IndexRefreshStateV1>(&fixture["refreshState"]);
        assert_round_trip::<DeleteIndexedSessionRequestV1>(&fixture["libraryDeletionRequest"]);
        assert_round_trip::<PrepareSourceDeletionRequestV1>(
            &fixture["prepareSourceDeletionRequest"],
        );
        assert_round_trip::<SourceDeletionConfirmationV1>(&fixture["sourceDeletionConfirmation"]);
        assert_round_trip::<DeleteIndexedSessionWithSourceRequestV1>(
            &fixture["sourceDeletionRequest"],
        );
        assert_round_trip::<RestoreSuppressedSourceRequestV1>(&fixture["restoreRequest"]);
        assert_round_trip::<SuppressedSourceV1>(&fixture["suppressedSource"]);
        assert_round_trip::<SuppressedSourceListRequestV1>(&fixture["suppressedSourceListRequest"]);
        assert_round_trip::<SuppressedSourcePageV1>(&fixture["suppressedSourcePage"]);
        assert_round_trip::<RestoreSuppressedSourceResultV1>(&fixture["restoreResult"]);
        assert_round_trip::<ResetLocalDatabaseRequestV1>(&fixture["resetRequest"]);
        assert_round_trip::<ResetLocalDatabaseResultV1>(&fixture["resetResult"]);
        assert_round_trip::<PresentationPlanV1>(&fixture["presentationPlan"]);
    }

    #[test]
    fn round_trips_every_jetbrains_status_variant() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/jetbrains-status-v1.json"
        ))
        .expect("JetBrains status fixture should be valid JSON");
        for status in fixture["valid"].as_array().unwrap() {
            assert_round_trip::<JetBrainsCopilotStatusV1>(status);
        }

        let mut expanded = fixture["valid"][0].clone();
        expanded["path"] = json!(r"C:\private");
        assert_rejected::<JetBrainsCopilotStatusV1>(expanded, "INVALID_CONTRACT");
    }

    #[test]
    fn rejects_unknown_fields_that_could_smuggle_private_paths() {
        let mut page = fixture()["sessionPage"].clone();
        page["sourcePath"] = json!("C:\\private\\session.jsonl");
        assert_rejected::<IndexedSessionPageV1>(page, "INVALID_CONTRACT");

        let mut deletion = fixture()["sourceDeletionRequest"].clone();
        deletion["path"] = json!("C:\\private");
        assert_rejected::<DeleteIndexedSessionWithSourceRequestV1>(deletion, "INVALID_CONTRACT");

        let mut reset = fixture()["resetRequest"].clone();
        reset["databasePath"] = json!("C:\\private\\session-library.sqlite3");
        assert_rejected::<ResetLocalDatabaseRequestV1>(reset, "INVALID_CONTRACT");
    }

    #[test]
    fn accepts_vscode_copilot_and_rejects_unknown_variants() {
        let mut list = fixture()["listRequest"].clone();
        list["source"] = json!("vscode-copilot");
        serde_json::from_value::<IndexedSessionListRequestV1>(list)
            .expect("VS Code Copilot should be an indexed source");

        let mut page = fixture()["sessionPage"].clone();
        page["items"][0]["source"] = json!("vscode-copilot");
        serde_json::from_value::<IndexedSessionPageV1>(page)
            .expect("VS Code Copilot summaries should deserialize");

        let mut unknown_source = fixture()["listRequest"].clone();
        unknown_source["source"] = json!("jetbrains");
        assert_rejected::<IndexedSessionListRequestV1>(unknown_source, "INVALID_CONTRACT");

        let mut refresh = fixture()["refreshState"].clone();
        refresh["status"] = json!("paused");
        assert_rejected::<IndexRefreshStateV1>(refresh, "INVALID_CONTRACT");

        let mut reset = fixture()["resetRequest"].clone();
        reset["mode"] = json!("reset-all-files");
        assert_rejected::<ResetLocalDatabaseRequestV1>(reset, "INVALID_CONTRACT");

        let mut detail = fixture()["sessionDetail"].clone();
        detail["entryPage"]["entries"][0]["entry"]["kind"] = json!("system");
        assert_rejected::<IndexedSessionDetailV1>(detail, "INVALID_CONTRACT");

        let mut plan = fixture()["presentationPlan"].clone();
        plan["entries"][0]["entry"]["kind"] = json!("system");
        assert_rejected::<PresentationPlanV1>(plan, "INVALID_CONTRACT");
    }

    #[test]
    fn requires_explicit_nullable_fields() {
        let mut list = fixture()["listRequest"].clone();
        list.as_object_mut()
            .expect("list request should be an object")
            .remove("source");
        assert_rejected::<IndexedSessionListRequestV1>(list, "INVALID_CONTRACT");

        let mut list = fixture()["listRequest"].clone();
        list.as_object_mut()
            .expect("list request should be an object")
            .remove("cursor");
        assert_rejected::<IndexedSessionListRequestV1>(list, "INVALID_CONTRACT");

        let mut summary = fixture()["sessionPage"].clone();
        summary["items"][0]
            .as_object_mut()
            .expect("summary should be an object")
            .remove("createdAtMs");
        assert_rejected::<IndexedSessionPageV1>(summary, "INVALID_CONTRACT");

        let mut page = fixture()["sessionPage"].clone();
        page.as_object_mut()
            .expect("session page should be an object")
            .remove("nextCursor");
        assert_rejected::<IndexedSessionPageV1>(page, "INVALID_CONTRACT");

        let mut detail = fixture()["sessionDetail"].clone();
        detail["entryPage"]
            .as_object_mut()
            .expect("entry page should be an object")
            .remove("nextCursor");
        assert_rejected::<IndexedSessionDetailV1>(detail, "INVALID_CONTRACT");

        assert_rejected::<IndexRefreshStateV1>(
            json!({"schemaVersion": 1, "status": "idle"}),
            "INVALID_CONTRACT",
        );

        let mut detail = fixture()["sessionDetail"].clone();
        detail["entryPage"]["entries"][3]["entry"]["detail"]
            .as_object_mut()
            .expect("tool detail should be an object")
            .remove("arguments");
        assert_rejected::<IndexedSessionDetailV1>(detail, "INVALID_CONTRACT");

        let mut detail = fixture()["sessionDetail"].clone();
        detail["entryPage"]["entries"][3]["entry"]["detail"]
            .as_object_mut()
            .expect("tool detail should be an object")
            .remove("result");
        assert_rejected::<IndexedSessionDetailV1>(detail, "INVALID_CONTRACT");

        let mut plan = fixture()["presentationPlan"].clone();
        plan["entries"][3]["entry"]
            .as_object_mut()
            .expect("presentation tool entry should be an object")
            .remove("detail");
        assert_rejected::<PresentationPlanV1>(plan, "INVALID_CONTRACT");

        let mut plan = fixture()["presentationPlan"].clone();
        plan["entries"][3]["entry"]["detail"] = json!({"arguments": "fixture.json"});
        assert_rejected::<PresentationPlanV1>(plan, "INVALID_CONTRACT");
    }

    #[test]
    fn rejects_oversized_ids_tokens_and_unsupported_versions() {
        let mut request = fixture()["libraryDeletionRequest"].clone();
        request["sessionId"] = json!("a".repeat(MAX_OPAQUE_ID_LENGTH + 1));
        assert_rejected::<DeleteIndexedSessionRequestV1>(request, "INVALID_OPAQUE_ID");

        let mut request = fixture()["sourceDeletionRequest"].clone();
        request["confirmationToken"] = json!("a".repeat(MAX_OPAQUE_ID_LENGTH + 1));
        assert_rejected::<DeleteIndexedSessionWithSourceRequestV1>(
            request,
            "INVALID_CONFIRMATION_TOKEN",
        );

        let mut request = fixture()["listRequest"].clone();
        request["schemaVersion"] = json!(2);
        assert_rejected::<IndexedSessionListRequestV1>(request, "UNSUPPORTED_SCHEMA_VERSION");
    }

    #[test]
    fn validates_every_summary_revision_and_page_boundary() {
        for (field, value, code) in [
            ("title", json!(""), "INVALID_TITLE"),
            ("title", json!("x".repeat(513)), "INVALID_TITLE"),
            ("title", json!("bad\0title"), "INVALID_TITLE"),
            (
                "createdAtMs",
                json!(super::MAX_SAFE_INTEGER + 1),
                "INVALID_TIMESTAMP",
            ),
            (
                "lastIndexedAtMs",
                json!(super::MAX_SAFE_INTEGER + 1),
                "INVALID_TIMESTAMP",
            ),
            ("entryCount", json!(0), "INVALID_ENTRY_COUNT"),
            (
                "entryCount",
                json!(MAX_PRESENTATION_ENTRY_COUNT + 1),
                "INVALID_ENTRY_COUNT",
            ),
            ("selectedEntryCount", json!(8), "INVALID_ENTRY_COUNT"),
            (
                "durationMs",
                json!(MAX_SOURCE_DURATION_MS + 1),
                "INVALID_SESSION_DURATION",
            ),
            (
                "diagnosticCount",
                json!(MAX_DIAGNOSTIC_COUNT + 1),
                "INVALID_DIAGNOSTIC_COUNT",
            ),
        ] {
            assert_fixture_mutation_rejected::<IndexedSessionPageV1>("sessionPage", code, |page| {
                page["items"][0][field] = value
            });
        }

        assert_fixture_mutation_rejected::<IndexedSessionPageV1>(
            "sessionPage",
            "DUPLICATE_SESSION_ID",
            |page| {
                let item = page["items"][0].clone();
                page["items"] = json!([item.clone(), item]);
            },
        );
        assert_fixture_mutation_rejected::<IndexedSessionPageV1>(
            "sessionPage",
            "INVALID_CURSOR",
            |page| page["nextCursor"] = json!("../cursor"),
        );

        for (field, value) in [
            ("indexedAtMs", json!(super::MAX_SAFE_INTEGER + 1)),
            ("entryCount", json!(0)),
            ("entryCount", json!(MAX_PRESENTATION_ENTRY_COUNT + 1)),
            ("durationMs", json!(MAX_SOURCE_DURATION_MS + 1)),
            ("diagnosticCount", json!(MAX_DIAGNOSTIC_COUNT + 1)),
        ] {
            assert_fixture_mutation_rejected::<IndexedSessionDetailV1>(
                "sessionDetail",
                "INVALID_REVISION",
                |detail| detail["revision"][field] = value,
            );
        }

        assert_fixture_mutation_rejected::<IndexedSessionDetailV1>(
            "sessionDetail",
            "PAGE_LIMIT_EXCEEDED",
            |detail| {
                let entry = detail["entryPage"]["entries"][0].clone();
                detail["entryPage"]["entries"] =
                    Value::Array(vec![entry; MAX_INDEXED_PAGE_SIZE + 1]);
            },
        );
        for value in [json!(0), json!(MAX_PRESENTATION_ENTRY_COUNT + 1), json!(1)] {
            assert_fixture_mutation_rejected::<IndexedSessionDetailV1>(
                "sessionDetail",
                "INVALID_ENTRY_COUNT",
                |detail| detail["entryPage"]["totalEntryCount"] = value,
            );
        }
        assert_fixture_mutation_rejected::<IndexedSessionDetailV1>(
            "sessionDetail",
            "INVALID_ENTRY_ORDER",
            |detail| detail["entryPage"]["entries"][2]["entry"]["atMs"] = json!(500),
        );
        assert_fixture_mutation_rejected::<IndexedSessionDetailV1>(
            "sessionDetail",
            "INVALID_CURSOR",
            |detail| detail["entryPage"]["nextCursor"] = json!("../cursor"),
        );
    }

    #[test]
    fn validates_cross_object_identity_and_metadata_invariants() {
        for (path, value, code) in [
            (
                ["revision", "sessionId"],
                json!("session_other"),
                "SESSION_MISMATCH",
            ),
            (
                ["entryPage", "sessionId"],
                json!("session_other"),
                "SESSION_MISMATCH",
            ),
            (
                ["revision", "entryCount"],
                json!(6),
                "SESSION_METADATA_MISMATCH",
            ),
            (
                ["entryPage", "totalEntryCount"],
                json!(8),
                "SESSION_METADATA_MISMATCH",
            ),
            (
                ["revision", "durationMs"],
                json!(7_001),
                "SESSION_METADATA_MISMATCH",
            ),
            (
                ["revision", "diagnosticCount"],
                json!(2),
                "SESSION_METADATA_MISMATCH",
            ),
        ] {
            assert_fixture_mutation_rejected::<IndexedSessionDetailV1>(
                "sessionDetail",
                code,
                |detail| detail[path[0]][path[1]] = value,
            );
        }
    }

    #[test]
    fn rejects_path_like_opaque_ids() {
        for session_id in ["../session", "C:\\private", "token with spaces", ""] {
            let mut request = fixture()["libraryDeletionRequest"].clone();
            request["sessionId"] = json!(session_id);
            assert_rejected::<DeleteIndexedSessionRequestV1>(request, "INVALID_OPAQUE_ID");
        }
    }

    #[test]
    fn rejects_invalid_local_session_titles() {
        for title in ["", " leading", "trailing "] {
            assert_fixture_mutation_rejected::<RenameIndexedSessionRequestV1>(
                "renameRequest",
                "INVALID_SESSION_TITLE",
                |request| request["title"] = json!(title),
            );
        }
        assert_fixture_mutation_rejected::<RenameIndexedSessionRequestV1>(
            "renameRequest",
            "INVALID_SESSION_TITLE",
            |request| request["title"] = json!("x".repeat(super::MAX_TITLE_LENGTH + 1)),
        );
        assert_fixture_mutation_rejected::<RenameIndexedSessionRequestV1>(
            "renameRequest",
            "INVALID_OPAQUE_ID",
            |request| request["sessionId"] = json!("../session"),
        );
    }

    #[test]
    fn bounds_pages_and_selection_batches() {
        let mut list = fixture()["listRequest"].clone();
        list["pageSize"] = json!(MAX_INDEXED_PAGE_SIZE + 1);
        assert_rejected::<IndexedSessionListRequestV1>(list, "INVALID_PAGE_SIZE");

        let summary = fixture()["sessionPage"]["items"][0].clone();
        let mut page = fixture()["sessionPage"].clone();
        page["items"] = Value::Array(vec![summary; MAX_INDEXED_PAGE_SIZE + 1]);
        assert_rejected::<IndexedSessionPageV1>(page, "PAGE_LIMIT_EXCEEDED");

        let mut selections = fixture()["selectionRequest"].clone();
        selections["changes"] = Value::Array(
            (0..=MAX_SELECTION_CHANGE_COUNT)
                .map(|index| json!({"entryKey": format!("entry_{index}"), "selected": true}))
                .collect(),
        );
        assert_rejected::<SetEntrySelectionsRequestV1>(
            selections,
            "SELECTION_CHANGE_LIMIT_EXCEEDED",
        );
    }

    #[test]
    fn rejects_duplicate_selection_keys_and_revision_mismatches() {
        let mut selections = fixture()["selectionRequest"].clone();
        let change = selections["changes"][0].clone();
        selections["changes"] = json!([change.clone(), change]);
        assert_rejected::<SetEntrySelectionsRequestV1>(selections, "DUPLICATE_ENTRY_KEY");

        let mut detail = fixture()["sessionDetail"].clone();
        detail["entryPage"]["revisionId"] = json!("revision_other");
        assert_rejected::<IndexedSessionDetailV1>(detail, "REVISION_MISMATCH");
    }

    #[test]
    fn enforces_content_availability_and_entry_order() {
        let mut reasoning = fixture()["sessionDetail"].clone();
        reasoning["summary"]["contentAvailability"]["reasoning"] = json!("unavailable");
        assert_rejected::<IndexedSessionDetailV1>(reasoning, "CONTENT_AVAILABILITY_MISMATCH");

        let mut details = fixture()["sessionDetail"].clone();
        details["summary"]["contentAvailability"]["toolDetails"] = json!("unavailable");
        assert_rejected::<IndexedSessionDetailV1>(details, "CONTENT_AVAILABILITY_MISMATCH");

        let mut out_of_order = fixture()["sessionDetail"].clone();
        out_of_order["entryPage"]["entries"]
            .as_array_mut()
            .expect("fixture entries should be an array")
            .reverse();
        assert_rejected::<IndexedSessionDetailV1>(out_of_order, "INVALID_ENTRY_ORDER");
    }

    #[test]
    fn validates_rich_entry_payload_boundaries() {
        for (entry_index, field, value, code) in [
            (0, "entryKey", json!("../entry"), "INVALID_OPAQUE_ID"),
            (
                0,
                "ordinal",
                json!(MAX_PRESENTATION_ENTRY_COUNT),
                "INVALID_ENTRY",
            ),
            (
                0,
                "atMs",
                json!(MAX_SOURCE_DURATION_MS + 1),
                "INVALID_ENTRY",
            ),
            (0, "text", json!("bad\0text"), "INVALID_ENTRY"),
            (1, "markdown", json!("bad\0markdown"), "INVALID_ENTRY"),
            (2, "text", json!("bad\0reasoning"), "INVALID_ENTRY"),
            (3, "name", json!(""), "INVALID_ENTRY"),
            (3, "summary", json!("bad\0summary"), "INVALID_ENTRY"),
            (5, "displayPath", json!(""), "INVALID_ENTRY"),
            (5, "summary", json!("bad\0summary"), "INVALID_ENTRY"),
            (6, "sourceType", json!(""), "INVALID_ENTRY"),
        ] {
            assert_fixture_mutation_rejected::<IndexedSessionDetailV1>(
                "sessionDetail",
                code,
                |detail| detail["entryPage"]["entries"][entry_index]["entry"][field] = value,
            );
        }

        assert_fixture_mutation_rejected::<IndexedSessionDetailV1>(
            "sessionDetail",
            "INVALID_TOOL_DETAIL",
            |detail| {
                detail["entryPage"]["entries"][3]["entry"]["detail"] = json!({
                    "availability": "available",
                    "arguments": null,
                    "result": null
                })
            },
        );
        assert_fixture_mutation_rejected::<IndexedSessionDetailV1>(
            "sessionDetail",
            "INVALID_CONTRACT",
            |detail| detail["entryPage"]["entries"][0]["selected"] = json!("yes"),
        );
    }

    #[test]
    fn validates_preferences_and_refresh_counter_invariants() {
        let mut preferences = fixture()["preferencesRequest"].clone();
        preferences["preferences"]["timing"]["entryDelayMs"] = json!(249);
        assert_rejected::<SetSessionPreferencesRequestV1>(
            preferences,
            "INVALID_PRESENTATION_TIMING",
        );

        let mut refresh = fixture()["refreshState"].clone();
        refresh["processedCount"] = json!(13);
        assert_rejected::<IndexRefreshStateV1>(refresh, "INVALID_REFRESH_COUNTS");

        for state in [
            json!({"schemaVersion": 1, "status": "idle", "lastCompletedAtMs": null}),
            json!({
                "schemaVersion": 1,
                "status": "completed",
                "generation": 4,
                "startedAtMs": 1788163260000_u64,
                "completedAtMs": 1788163270000_u64,
                "discoveredCount": 12,
                "processedCount": 7,
                "indexedCount": 2,
                "unchangedCount": 4,
                "failedCount": 1,
                "skippedCount": 0,
                "warningCount": 2
            }),
            json!({
                "schemaVersion": 1,
                "status": "failed",
                "generation": 4,
                "startedAtMs": 1788163260000_u64,
                "completedAtMs": 1788163270000_u64,
                "errorCode": "SOURCE_READ_FAILED",
                "discoveredCount": 12,
                "processedCount": 7,
                "indexedCount": 2,
                "unchangedCount": 4,
                "failedCount": 1,
                "skippedCount": 0,
                "warningCount": 2
            }),
        ] {
            serde_json::from_value::<Validated<IndexRefreshStateV1>>(state)
                .expect("every refresh-state variant should validate");
        }
    }

    #[test]
    fn validates_every_preference_boundary() {
        for (field, value) in [
            ("entryDelayMs", json!(super::MAX_PRESENTATION_DELAY_MS + 1)),
            ("playbackSpeed", json!(super::MAX_PRESENTATION_SPEED + 0.01)),
        ] {
            assert_fixture_mutation_rejected::<SetSessionPreferencesRequestV1>(
                "preferencesRequest",
                "INVALID_PRESENTATION_TIMING",
                |request| request["preferences"]["timing"][field] = value,
            );
        }

        assert_fixture_mutation_rejected::<SetSessionPreferencesRequestV1>(
            "preferencesRequest",
            "INVALID_VISIBILITY",
            |request| {
                request["preferences"]["visibility"]["showToolCalls"] = json!(false);
                request["preferences"]["visibility"]["showToolDetails"] = json!(true);
            },
        );

        for color in ["1234567", "#gggggg", "#12345"] {
            assert_fixture_mutation_rejected::<SetSessionPreferencesRequestV1>(
                "preferencesRequest",
                "INVALID_APPEARANCE",
                |request| request["preferences"]["appearance"]["theme"]["accent"] = json!(color),
            );
        }
        for (field, value) in [("sizePx", json!(11)), ("lineHeight", json!(3.0))] {
            assert_fixture_mutation_rejected::<SetSessionPreferencesRequestV1>(
                "preferencesRequest",
                "INVALID_APPEARANCE",
                |request| request["preferences"]["appearance"]["font"][field] = value,
            );
        }

        let mut request: SetSessionPreferencesRequestV1 =
            serde_json::from_value(fixture()["preferencesRequest"].clone())
                .expect("fixture preferences should deserialize");
        request.preferences.timing.playback_speed = f64::NAN;
        assert_eq!(
            Validated::new(request)
                .expect_err("non-finite speed should fail")
                .code,
            "INVALID_PRESENTATION_TIMING"
        );

        let request: IndexedSessionListRequestV1 =
            serde_json::from_value(fixture()["listRequest"].clone())
                .expect("fixture request should deserialize");
        let validated = Validated::new(request).expect("fixture request should validate");
        assert_eq!(validated.get().page_size, 50);
    }

    #[test]
    fn validates_refresh_and_deletion_boundaries() {
        for (field, value, code) in [
            ("generation", json!(0), "INVALID_REFRESH_STATE"),
            (
                "generation",
                json!(super::MAX_SAFE_INTEGER + 1),
                "INVALID_REFRESH_STATE",
            ),
            (
                "startedAtMs",
                json!(super::MAX_SAFE_INTEGER + 1),
                "INVALID_TIMESTAMP",
            ),
            (
                "discoveredCount",
                json!(MAX_PRESENTATION_ENTRY_COUNT + 1),
                "INVALID_REFRESH_COUNTS",
            ),
            ("indexedCount", json!(8), "INVALID_REFRESH_COUNTS"),
            ("unchangedCount", json!(8), "INVALID_REFRESH_COUNTS"),
            ("failedCount", json!(8), "INVALID_REFRESH_COUNTS"),
            ("skippedCount", json!(8), "INVALID_REFRESH_COUNTS"),
            (
                "warningCount",
                json!(MAX_PRESENTATION_ENTRY_COUNT + 1),
                "INVALID_REFRESH_COUNTS",
            ),
        ] {
            assert_fixture_mutation_rejected::<IndexRefreshStateV1>(
                "refreshState",
                code,
                |state| state[field] = value,
            );
        }

        for error_code in [
            String::new(),
            "lowercase".to_owned(),
            "1_STARTS_WITH_DIGIT".to_owned(),
            "A".repeat(129),
        ] {
            let state = json!({
                "schemaVersion": 1,
                "status": "failed",
                "generation": 4,
                "startedAtMs": 100,
                "completedAtMs": 101,
                "errorCode": error_code,
                "discoveredCount": 1,
                "processedCount": 1,
                "indexedCount": 0,
                "unchangedCount": 0,
                "failedCount": 1,
                "skippedCount": 0,
                "warningCount": 0
            });
            assert_rejected::<IndexRefreshStateV1>(state, "INVALID_ERROR_CODE");
        }

        let mut idle = json!({
            "schemaVersion": 1,
            "status": "idle",
            "lastCompletedAtMs": super::MAX_SAFE_INTEGER + 1
        });
        assert_rejected::<IndexRefreshStateV1>(idle.take(), "INVALID_TIMESTAMP");

        let mut completed = json!({
            "schemaVersion": 1,
            "status": "completed",
            "generation": 4,
            "startedAtMs": 200,
            "completedAtMs": 100,
            "discoveredCount": 1,
            "processedCount": 1,
            "indexedCount": 1,
            "unchangedCount": 0,
            "failedCount": 0,
            "skippedCount": 0,
            "warningCount": 0
        });
        assert_rejected::<IndexRefreshStateV1>(completed.take(), "INVALID_TIMESTAMP");

        for artifact_count in [0, MAX_DELETION_ARTIFACT_COUNT + 1] {
            assert_fixture_mutation_rejected::<SourceDeletionConfirmationV1>(
                "sourceDeletionConfirmation",
                "INVALID_ARTIFACT_COUNT",
                |confirmation| confirmation["artifactCount"] = json!(artifact_count),
            );
        }
        assert_fixture_mutation_rejected::<SourceDeletionConfirmationV1>(
            "sourceDeletionConfirmation",
            "INVALID_TIMESTAMP",
            |confirmation| confirmation["expiresAtMs"] = json!(super::MAX_SAFE_INTEGER + 1),
        );
    }

    #[test]
    fn keeps_library_and_source_deletion_structurally_distinct() {
        assert_rejected::<DeleteIndexedSessionRequestV1>(
            fixture()["sourceDeletionRequest"].clone(),
            "INVALID_CONTRACT",
        );
        assert_rejected::<DeleteIndexedSessionWithSourceRequestV1>(
            fixture()["libraryDeletionRequest"].clone(),
            "INVALID_CONTRACT",
        );

        let mut request = fixture()["sourceDeletionRequest"].clone();
        request["confirmationToken"] = json!("");
        assert_rejected::<DeleteIndexedSessionWithSourceRequestV1>(
            request,
            "INVALID_CONFIRMATION_TOKEN",
        );
    }

    #[test]
    fn validates_path_free_suppression_pages_and_restore_results() {
        let mut tombstone = fixture()["suppressedSource"].clone();
        tombstone["vendorSessionId"] = json!("private");
        assert_rejected::<SuppressedSourceV1>(tombstone, "INVALID_CONTRACT");

        assert_fixture_mutation_rejected::<SuppressedSourceV1>(
            "suppressedSource",
            "INVALID_CONTRACT",
            |value| value["sourceDeleted"] = json!("yes"),
        );
        assert_fixture_mutation_rejected::<SuppressedSourceListRequestV1>(
            "suppressedSourceListRequest",
            "INVALID_PAGE_SIZE",
            |value| value["pageSize"] = json!(0),
        );
        assert_fixture_mutation_rejected::<SuppressedSourcePageV1>(
            "suppressedSourcePage",
            "INVALID_OPAQUE_ID",
            |value| value["nextCursor"] = json!("../private"),
        );

        let mut restored = fixture()["restoreResult"].clone();
        restored["sourcePath"] = json!("C:\\private");
        assert_rejected::<RestoreSuppressedSourceResultV1>(restored, "INVALID_CONTRACT");

        let mut reset = fixture()["resetResult"].clone();
        reset["resetAtMs"] = json!(super::MAX_SAFE_INTEGER + 1);
        assert_rejected::<ResetLocalDatabaseResultV1>(reset, "INVALID_TIMESTAMP");
    }

    #[test]
    fn bounds_and_orders_presentation_plans() {
        let mut empty = fixture()["presentationPlan"].clone();
        empty["entries"] = json!([]);
        assert_rejected::<PresentationPlanV1>(empty, "EMPTY_PRESENTATION_PLAN");

        let entry = fixture()["presentationPlan"]["entries"][0].clone();
        let mut oversized = fixture()["presentationPlan"].clone();
        oversized["entries"] = Value::Array(vec![entry; MAX_PRESENTATION_ENTRY_COUNT + 1]);
        assert_rejected::<PresentationPlanV1>(oversized, "PRESENTATION_ENTRY_LIMIT_EXCEEDED");

        let mut reversed = fixture()["presentationPlan"].clone();
        reversed["entries"]
            .as_array_mut()
            .expect("fixture entries should be an array")
            .reverse();
        assert_rejected::<PresentationPlanV1>(reversed, "INVALID_PRESENTATION_ORDER");
    }

    #[test]
    fn validates_presentation_metadata_content_and_timing_boundaries() {
        for (field, value, code) in [
            ("planId", json!("../plan"), "INVALID_OPAQUE_ID"),
            ("sessionId", json!("../session"), "INVALID_OPAQUE_ID"),
            ("revisionId", json!("../revision"), "INVALID_OPAQUE_ID"),
            ("sessionTitle", json!(""), "INVALID_PRESENTATION_PLAN"),
            (
                "createdAtMs",
                json!(super::MAX_SAFE_INTEGER + 1),
                "INVALID_TIMESTAMP",
            ),
            ("durationMs", json!(0), "INVALID_PRESENTATION_DURATION"),
            (
                "durationMs",
                json!(MAX_PRESENTATION_DURATION_MS + 1),
                "INVALID_PRESENTATION_DURATION",
            ),
            ("fps", json!(60), "INVALID_RENDER_SETTINGS"),
            ("width", json!(1280), "INVALID_RENDER_SETTINGS"),
            ("height", json!(720), "INVALID_RENDER_SETTINGS"),
        ] {
            assert_fixture_mutation_rejected::<PresentationPlanV1>(
                "presentationPlan",
                code,
                |plan| plan[field] = value,
            );
        }

        for (entry_index, field, value, code) in [
            (
                0,
                "ordinal",
                json!(MAX_PRESENTATION_ENTRY_COUNT),
                "INVALID_ENTRY",
            ),
            (
                0,
                "atMs",
                json!(MAX_SOURCE_DURATION_MS + 1),
                "INVALID_ENTRY",
            ),
            (0, "text", json!("bad\0text"), "INVALID_PRESENTATION_ENTRY"),
            (
                1,
                "markdown",
                json!("bad\0markdown"),
                "INVALID_PRESENTATION_ENTRY",
            ),
            (
                2,
                "text",
                json!("bad\0reasoning"),
                "INVALID_PRESENTATION_ENTRY",
            ),
            (3, "name", json!(""), "INVALID_PRESENTATION_ENTRY"),
            (
                3,
                "summary",
                json!("bad\0summary"),
                "INVALID_PRESENTATION_ENTRY",
            ),
            (4, "displayPath", json!(""), "INVALID_PRESENTATION_ENTRY"),
        ] {
            assert_fixture_mutation_rejected::<PresentationPlanV1>(
                "presentationPlan",
                code,
                |plan| plan["entries"][entry_index]["entry"][field] = value,
            );
        }

        assert_fixture_mutation_rejected::<PresentationPlanV1>(
            "presentationPlan",
            "INVALID_PRESENTATION_ORDER",
            |plan| plan["entries"][0]["revealAtMs"] = json!(MAX_PRESENTATION_DURATION_MS + 1),
        );
        assert_fixture_mutation_rejected::<PresentationPlanV1>(
            "presentationPlan",
            "INVALID_PRESENTATION_ORDER",
            |plan| plan["entries"][1]["entry"]["entryKey"] = json!("entry_01"),
        );
        assert_fixture_mutation_rejected::<PresentationPlanV1>(
            "presentationPlan",
            "INVALID_PRESENTATION_ORDER",
            |plan| plan["entries"][2]["entry"]["atMs"] = json!(500),
        );
        assert_fixture_mutation_rejected::<PresentationPlanV1>(
            "presentationPlan",
            "INVALID_PRESENTATION_ORDER",
            |plan| plan["entries"][2]["revealAtMs"] = json!(500),
        );
        assert_fixture_mutation_rejected::<PresentationPlanV1>(
            "presentationPlan",
            "PRESENTATION_VISIBILITY_MISMATCH",
            |plan| plan["preferences"]["visibility"]["showToolCalls"] = json!(false),
        );
        assert_fixture_mutation_rejected::<PresentationPlanV1>(
            "presentationPlan",
            "INVALID_PRESENTATION_DURATION",
            |plan| {
                let final_reveal = plan["entries"][4]["revealAtMs"].clone();
                plan["durationMs"] = final_reveal;
            },
        );

        let mut with_detail = fixture()["presentationPlan"].clone();
        with_detail["entries"][3]["entry"]["detail"] = json!({"arguments": null, "result": null});
        assert_rejected::<PresentationPlanV1>(with_detail, "INVALID_TOOL_DETAIL");

        let mut with_unknown = fixture()["presentationPlan"].clone();
        with_unknown["entries"][4]["entry"] = json!({
            "entryKey": "entry_07",
            "ordinal": 6,
            "atMs": 6_000,
            "kind": "unknown",
            "sourceType": "future-record"
        });
        serde_json::from_value::<Validated<PresentationPlanV1>>(with_unknown.clone())
            .expect("bounded unknown presentation entries should remain representable");
        with_unknown["entries"][4]["entry"]["sourceType"] = json!("");
        assert_rejected::<PresentationPlanV1>(with_unknown, "INVALID_PRESENTATION_ENTRY");
    }

    #[test]
    fn presentation_entries_honor_frozen_visibility() {
        let mut hidden_reasoning = fixture()["presentationPlan"].clone();
        hidden_reasoning["preferences"]["visibility"]["showReasoning"] = json!(false);
        assert_rejected::<PresentationPlanV1>(hidden_reasoning, "PRESENTATION_VISIBILITY_MISMATCH");

        let mut hidden_detail = fixture()["presentationPlan"].clone();
        hidden_detail["entries"][3]["entry"]["detail"] =
            json!({"arguments": "fixture.json", "result": "7 entries"});
        assert_rejected::<PresentationPlanV1>(hidden_detail, "PRESENTATION_VISIBILITY_MISMATCH");
    }
}
