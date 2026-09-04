use super::claude::ClaudeAdapter;
use super::codex::CodexAdapter;
use super::copilot_cli::CopilotCliAdapter;
use super::vscode::VscodeCopilotAdapter;
use crate::indexed_library::IndexedSessionSourceV1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeletionArtifactDeclaration {
    PrimaryFile,
    SiblingFiles {
        extensions: &'static [&'static str],
    },
    SessionDirectory {
        primary_file_name: &'static str,
        max_artifacts: usize,
        max_depth: usize,
    },
}

pub(crate) trait SourceDeletionAdapter {
    fn deletion_artifacts(&self) -> DeletionArtifactDeclaration;
}

pub(crate) fn artifact_declaration(source: &IndexedSessionSourceV1) -> DeletionArtifactDeclaration {
    match source {
        IndexedSessionSourceV1::ClaudeCode => ClaudeAdapter.deletion_artifacts(),
        IndexedSessionSourceV1::Codex => CodexAdapter.deletion_artifacts(),
        IndexedSessionSourceV1::CopilotCli => CopilotCliAdapter.deletion_artifacts(),
        IndexedSessionSourceV1::VscodeCopilot => VscodeCopilotAdapter.deletion_artifacts(),
    }
}
