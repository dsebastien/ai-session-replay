use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::indexed_library::{
    INDEXED_LIBRARY_SCHEMA_VERSION, JetBrainsCopilotCliStatusV1, JetBrainsCopilotStatusV1,
    JetBrainsNativeTranscriptStatusV1, JetBrainsPluginStatusV1,
};
use crate::io::{LocalRoot, SnapshotError, capture_file_prefix};

const MAX_PRODUCT_DIRECTORIES: usize = 128;
const MAX_CLI_SESSIONS: usize = 512;
const MAX_ATTRIBUTION_PREFIX_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct JetBrainsDetectionRoots {
    jetbrains_config_root: Option<PathBuf>,
    copilot_data_root: Option<PathBuf>,
}

impl JetBrainsDetectionRoots {
    pub(crate) fn from_environment() -> Self {
        let user_profile = std::env::var_os("USERPROFILE").filter(|value| !value.is_empty());
        Self {
            // JetBrains documents Windows configuration roots beneath
            // %APPDATA%\JetBrains\<product><version>.
            jetbrains_config_root: std::env::var_os("APPDATA")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .map(|root| root.join("JetBrains")),
            copilot_data_root: std::env::var_os("COPILOT_HOME")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .or_else(|| {
                    user_profile
                        .map(PathBuf::from)
                        .map(|root| root.join(".copilot"))
                }),
        }
    }

    #[cfg(test)]
    fn synthetic(jetbrains_config_root: PathBuf, copilot_data_root: PathBuf) -> Self {
        Self {
            jetbrains_config_root: Some(jetbrains_config_root),
            copilot_data_root: Some(copilot_data_root),
        }
    }
}

pub(crate) fn detect_jetbrains_copilot(
    roots: &JetBrainsDetectionRoots,
) -> JetBrainsCopilotStatusV1 {
    JetBrainsCopilotStatusV1 {
        schema_version: INDEXED_LIBRARY_SCHEMA_VERSION,
        plugin_status: detect_plugin(roots.jetbrains_config_root.as_deref()),
        native_transcript_status: JetBrainsNativeTranscriptStatusV1::Unsupported,
        copilot_cli_status: detect_copilot_cli(roots.copilot_data_root.as_deref()),
    }
}

fn detect_plugin(root_path: Option<&Path>) -> JetBrainsPluginStatusV1 {
    let Some(root_path) = root_path else {
        return JetBrainsPluginStatusV1::Unavailable;
    };
    let Some(root) = optional_root(root_path) else {
        return JetBrainsPluginStatusV1::NotDetected;
    };
    let Ok(root) = root else {
        return JetBrainsPluginStatusV1::Unavailable;
    };
    let entries = match bounded_directory_entries(root_path, MAX_PRODUCT_DIRECTORIES) {
        Ok(entries) => entries,
        Err(()) => return JetBrainsPluginStatusV1::Unavailable,
    };
    let mut uncertain = false;
    for entry in entries {
        let Ok(file_type) = entry.file_type() else {
            uncertain = true;
            continue;
        };
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }
        let settings = entry.path().join("options").join("github-copilot.xml");
        match root.open_file(&settings) {
            Ok(_) => return JetBrainsPluginStatusV1::Detected,
            Err(SnapshotError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => uncertain = true,
        }
    }
    if uncertain {
        JetBrainsPluginStatusV1::Unavailable
    } else {
        JetBrainsPluginStatusV1::NotDetected
    }
}

fn detect_copilot_cli(root_path: Option<&Path>) -> JetBrainsCopilotCliStatusV1 {
    let Some(root_path) = root_path else {
        return JetBrainsCopilotCliStatusV1::Unavailable;
    };
    let Some(root) = optional_root(root_path) else {
        return JetBrainsCopilotCliStatusV1::NotDetected;
    };
    if root.is_err() {
        return JetBrainsCopilotCliStatusV1::Unavailable;
    }
    let session_state_path = root_path.join("session-state");
    let Some(session_root) = optional_root(&session_state_path) else {
        return JetBrainsCopilotCliStatusV1::NotDetected;
    };
    let Ok(session_root) = session_root else {
        return JetBrainsCopilotCliStatusV1::Unavailable;
    };
    let candidates = match copilot_event_candidates(&session_state_path) {
        Ok(candidates) => candidates,
        Err(()) => return JetBrainsCopilotCliStatusV1::Unavailable,
    };
    if candidates.is_empty() {
        return JetBrainsCopilotCliStatusV1::NotDetected;
    }
    let mut uncertain = false;
    for candidate in &candidates {
        match capture_file_prefix(&session_root, candidate, MAX_ATTRIBUTION_PREFIX_BYTES) {
            Ok(snapshot) if explicitly_names_jetbrains(snapshot.bytes()) => {
                return JetBrainsCopilotCliStatusV1::JetbrainsAttributed;
            }
            Ok(_) => {}
            Err(_) => uncertain = true,
        }
    }
    if uncertain {
        JetBrainsCopilotCliStatusV1::Unavailable
    } else {
        JetBrainsCopilotCliStatusV1::AvailableSeparately
    }
}

fn optional_root(path: &Path) -> Option<Result<LocalRoot, SnapshotError>> {
    match path.try_exists() {
        Ok(false) => None,
        Ok(true) => Some(LocalRoot::new(path)),
        Err(error) => Some(Err(SnapshotError::Io(error))),
    }
}

fn bounded_directory_entries(path: &Path, maximum: usize) -> Result<Vec<fs::DirEntry>, ()> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(path).map_err(|_| ())? {
        if entries.len() >= maximum {
            return Err(());
        }
        entries.push(entry.map_err(|_| ())?);
    }
    Ok(entries)
}

fn copilot_event_candidates(session_state_path: &Path) -> Result<Vec<PathBuf>, ()> {
    let entries = bounded_directory_entries(session_state_path, MAX_CLI_SESSIONS)?;
    let mut candidates = Vec::new();
    for entry in entries {
        let file_type = entry.file_type().map_err(|_| ())?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let candidate = entry.path().join("events.jsonl");
            match candidate.try_exists() {
                Ok(true) => candidates.push(candidate),
                Ok(false) => {}
                Err(_) => return Err(()),
            }
        } else if file_type.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("jsonl"))
        {
            candidates.push(entry.path());
        }
    }
    candidates.sort_by(|left, right| {
        left.to_string_lossy()
            .to_lowercase()
            .cmp(&right.to_string_lossy().to_lowercase())
    });
    Ok(candidates)
}

fn explicitly_names_jetbrains(bytes: &[u8]) -> bool {
    let complete_length = bytes.iter().rposition(|byte| *byte == b'\n').unwrap_or(0);
    bytes[..complete_length]
        .split(|byte| *byte == b'\n')
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .filter(is_attribution_record)
        .any(|record| explicit_client_values(&record).any(is_jetbrains_name))
}

fn is_attribution_record(value: &Value) -> bool {
    matches!(
        value.get("type").and_then(Value::as_str),
        Some("session.start" | "session.resume" | "user.message")
    )
}

fn explicit_client_values(value: &Value) -> impl Iterator<Item = &str> {
    let data = value.get("data");
    let context = data.and_then(|data| data.get("context"));
    [
        data.and_then(|data| data.get("source")),
        data.and_then(|data| data.get("client")),
        data.and_then(|data| data.get("clientName")),
        data.and_then(|data| data.get("ide")),
        data.and_then(|data| data.get("editor")),
        data.and_then(|data| data.get("clientInfo"))
            .and_then(|client| client.get("name")),
        context.and_then(|context| context.get("client")),
        context.and_then(|context| context.get("clientName")),
        context.and_then(|context| context.get("ide")),
        context.and_then(|context| context.get("editor")),
    ]
    .into_iter()
    .filter_map(|candidate| candidate.and_then(Value::as_str))
}

fn is_jetbrains_name(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    normalized.contains("jetbrains")
        || [
            "intellij",
            "intellij idea",
            "pycharm",
            "webstorm",
            "rider",
            "clion",
            "goland",
            "phpstorm",
            "rubymine",
            "datagrip",
            "dataspell",
            "rustrover",
            "aqua",
        ]
        .contains(&normalized.as_str())
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        JetBrainsDetectionRoots, bounded_directory_entries, detect_jetbrains_copilot,
        explicitly_names_jetbrains,
    };
    use crate::indexed_library::{JetBrainsCopilotCliStatusV1, JetBrainsPluginStatusV1};

    const ATTRIBUTED: &str = include_str!("../../../tests/fixtures/copilot-cli-jetbrains.jsonl");

    struct TempTree(PathBuf);

    impl TempTree {
        fn new(label: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "ai-session-replay-jetbrains-{label}-{}-{nonce}",
                std::process::id()
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn write(&self, relative: &str, contents: &[u8]) {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }

        fn create_dir(&self, relative: &str) {
            std::fs::create_dir_all(self.0.join(relative)).unwrap();
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn status(tree: &TempTree) -> crate::indexed_library::JetBrainsCopilotStatusV1 {
        detect_jetbrains_copilot(&JetBrainsDetectionRoots::synthetic(
            tree.0.join("JetBrains"),
            tree.0.join("copilot"),
        ))
    }

    #[test]
    fn distinguishes_absent_plugin_separate_cli_and_explicit_attribution() {
        let absent = TempTree::new("absent");
        let absent_status = status(&absent);
        assert_eq!(
            absent_status.plugin_status,
            JetBrainsPluginStatusV1::NotDetected
        );
        assert_eq!(
            absent_status.copilot_cli_status,
            JetBrainsCopilotCliStatusV1::NotDetected
        );

        let plugin = TempTree::new("plugin");
        plugin.write(
            r"JetBrains\IntelliJIdea2026.2\options\github-copilot.xml",
            b"<application />",
        );
        assert_eq!(
            status(&plugin).plugin_status,
            JetBrainsPluginStatusV1::Detected
        );

        let separate = TempTree::new("separate");
        separate.write(
            r"copilot\session-state\session-one\events.jsonl",
            br#"{"type":"user.message","data":{"content":"mentions JetBrains only in transcript text"}}
"#,
        );
        assert_eq!(
            status(&separate).copilot_cli_status,
            JetBrainsCopilotCliStatusV1::AvailableSeparately
        );

        let attributed = TempTree::new("attributed");
        attributed.write(
            r"copilot\session-state\session-two\events.jsonl",
            ATTRIBUTED.as_bytes(),
        );
        assert_eq!(
            status(&attributed).copilot_cli_status,
            JetBrainsCopilotCliStatusV1::JetbrainsAttributed
        );
    }

    #[test]
    fn ignores_cache_logs_projects_and_transcript_mentions() {
        let tree = TempTree::new("scope");
        tree.write(r"JetBrains\Product\log\github-copilot.xml", b"ignored");
        tree.write(r"JetBrains\Product\system\github-copilot.xml", b"ignored");
        tree.write(r"project\.idea\options\github-copilot.xml", b"ignored");
        tree.write(
            r"copilot\session-state\session\events.jsonl",
            br#"{"type":"user.message","data":{"content":"JetBrains IntelliJ"}}
"#,
        );

        let detected = status(&tree);
        assert_eq!(detected.plugin_status, JetBrainsPluginStatusV1::NotDetected);
        assert_eq!(
            detected.copilot_cli_status,
            JetBrainsCopilotCliStatusV1::AvailableSeparately
        );
    }

    #[test]
    fn reads_only_explicit_metadata_fields_from_complete_jsonl_records() {
        assert!(explicitly_names_jetbrains(ATTRIBUTED.as_bytes()));
        assert!(explicitly_names_jetbrains(
            br#"{"type":"session.start","data":{"clientInfo":{"name":"IntelliJ"}}}
"#,
        ));
        assert!(!explicitly_names_jetbrains(
            br#"{"type":"user.message","data":{"content":"JetBrains","source":"terminal"}}
{"type":"tool.execution_complete","data":{"source":"jetbrains"}}
{"type":"user.message","data":{"source":"jetbrains"}}"#,
        ));
    }

    #[test]
    fn distinguishes_unconfigured_inaccessible_and_empty_roots() {
        let unavailable = detect_jetbrains_copilot(&JetBrainsDetectionRoots {
            jetbrains_config_root: None,
            copilot_data_root: None,
        });
        assert_eq!(
            unavailable.plugin_status,
            JetBrainsPluginStatusV1::Unavailable
        );
        assert_eq!(
            unavailable.copilot_cli_status,
            JetBrainsCopilotCliStatusV1::Unavailable
        );

        let invalid = TempTree::new("invalid-roots");
        invalid.write("JetBrains", b"not a directory");
        invalid.write("copilot", b"not a directory");
        let invalid_status = status(&invalid);
        assert_eq!(
            invalid_status.plugin_status,
            JetBrainsPluginStatusV1::Unavailable
        );
        assert_eq!(
            invalid_status.copilot_cli_status,
            JetBrainsCopilotCliStatusV1::Unavailable
        );

        let empty = TempTree::new("empty-roots");
        empty.create_dir("JetBrains");
        empty.create_dir("copilot");
        let empty_status = status(&empty);
        assert_eq!(
            empty_status.plugin_status,
            JetBrainsPluginStatusV1::NotDetected
        );
        assert_eq!(
            empty_status.copilot_cli_status,
            JetBrainsCopilotCliStatusV1::NotDetected
        );

        empty.create_dir(r"copilot\session-state");
        assert_eq!(
            status(&empty).copilot_cli_status,
            JetBrainsCopilotCliStatusV1::NotDetected
        );

        let invalid_session_state = TempTree::new("invalid-session-state");
        invalid_session_state.create_dir("copilot");
        invalid_session_state.write(r"copilot\session-state", b"not a directory");
        assert_eq!(
            status(&invalid_session_state).copilot_cli_status,
            JetBrainsCopilotCliStatusV1::Unavailable
        );
    }

    #[test]
    fn reports_uncertain_bounded_scans_without_guessing() {
        let plugin = TempTree::new("uncertain-plugin");
        let plugin_settings = plugin
            .0
            .join(r"JetBrains\Product\options\github-copilot.xml");
        plugin.write(
            r"JetBrains\Product\options\github-copilot.xml",
            b"<application />",
        );
        std::fs::hard_link(
            &plugin_settings,
            plugin
                .0
                .join(r"JetBrains\Product\options\copilot-linked.xml"),
        )
        .unwrap();
        assert_eq!(
            status(&plugin).plugin_status,
            JetBrainsPluginStatusV1::Unavailable
        );
        assert!(bounded_directory_entries(&plugin.0.join("JetBrains"), 0).is_err());

        let cli = TempTree::new("uncertain-cli");
        let event_path = cli.0.join(r"copilot\session-state\session\events.jsonl");
        cli.write(
            r"copilot\session-state\session\events.jsonl",
            b"{\"type\":\"session.start\",\"data\":{}}\n",
        );
        std::fs::hard_link(
            &event_path,
            cli.0.join(r"copilot\session-state\linked.jsonl"),
        )
        .unwrap();
        assert_eq!(
            status(&cli).copilot_cli_status,
            JetBrainsCopilotCliStatusV1::Unavailable
        );
    }

    #[test]
    fn accepts_direct_cli_logs_and_ignores_non_session_artifacts() {
        let tree = TempTree::new("direct-cli");
        tree.write(r"copilot\session-state\notes.txt", b"jetbrains");
        tree.create_dir(r"copilot\session-state\empty-session");
        tree.write(r"copilot\session-state\direct.jsonl", ATTRIBUTED.as_bytes());

        assert_eq!(
            status(&tree).copilot_cli_status,
            JetBrainsCopilotCliStatusV1::JetbrainsAttributed
        );
    }

    #[test]
    fn locked_cli_metadata_is_reported_as_unavailable() {
        let tree = TempTree::new("locked-cli");
        let event_path = tree.0.join(r"copilot\session-state\session\events.jsonl");
        tree.write(
            r"copilot\session-state\session\events.jsonl",
            b"{\"type\":\"session.start\",\"data\":{}}\n",
        );
        let _guard = OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(event_path)
            .unwrap();

        assert_eq!(
            status(&tree).copilot_cli_status,
            JetBrainsCopilotCliStatusV1::Unavailable
        );
    }
}
