use std::env;
use std::path::{Path, PathBuf};

#[cfg(test)]
use crate::io::{LocalRoot, SnapshotError};
#[cfg(test)]
use std::fs;
#[cfg(test)]
use std::os::windows::fs::MetadataExt;
#[cfg(test)]
use std::path::Component;
#[cfg(test)]
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

use super::catalog::{AvailabilityStatus, DiscoverySource};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CandidateMatcher {
    ClaudeProject,
    Codex,
    CopilotCli,
    VsCodeWorkspace,
    VsCodeSessionDirectory,
}

#[derive(Clone, Debug)]
pub(crate) enum RootSpec {
    Local {
        source: DiscoverySource,
        collection: &'static str,
        path: PathBuf,
        read_root_path: PathBuf,
        matcher: CandidateMatcher,
    },
    Unavailable {
        source: DiscoverySource,
        collection: &'static str,
        status: AvailabilityStatus,
    },
}

pub(crate) trait RootProvider: Send + Sync {
    fn roots(&self) -> Vec<RootSpec>;
}

#[derive(Debug)]
pub(crate) struct ProductionRootProvider {
    claude_data_root: Option<PathBuf>,
    codex_data_root: Option<PathBuf>,
    copilot_data_root: Option<PathBuf>,
    vscode_stable_user_root: Option<PathBuf>,
    vscode_insiders_user_root: Option<PathBuf>,
}

impl ProductionRootProvider {
    pub(crate) fn from_environment() -> Self {
        let user_profile = env_path("USERPROFILE");
        let roaming_app_data = env_path("APPDATA");
        Self {
            claude_data_root: env_path("CLAUDE_CONFIG_DIR")
                .or_else(|| user_profile.as_ref().map(|root| root.join(".claude"))),
            codex_data_root: env_path("CODEX_HOME")
                .or_else(|| user_profile.as_ref().map(|root| root.join(".codex"))),
            copilot_data_root: env_path("COPILOT_HOME")
                .or_else(|| user_profile.as_ref().map(|root| root.join(".copilot"))),
            vscode_stable_user_root: roaming_app_data
                .as_ref()
                .map(|root| root.join("Code").join("User")),
            vscode_insiders_user_root: roaming_app_data
                .as_ref()
                .map(|root| root.join("Code - Insiders").join("User")),
        }
    }

    pub(crate) fn configured_data_roots(&self) -> Vec<PathBuf> {
        [
            self.claude_data_root.as_ref(),
            self.codex_data_root.as_ref(),
            self.copilot_data_root.as_ref(),
            self.vscode_stable_user_root.as_ref(),
            self.vscode_insiders_user_root.as_ref(),
        ]
        .into_iter()
        .flatten()
        .cloned()
        .collect()
    }

    #[cfg(test)]
    pub(super) fn with_environment_paths(user_profile: impl Into<PathBuf>) -> Self {
        let user_profile = user_profile.into();
        let roaming_app_data = user_profile.join("AppData").join("Roaming");
        Self {
            claude_data_root: Some(user_profile.join(".claude")),
            codex_data_root: Some(user_profile.join(".codex")),
            copilot_data_root: Some(user_profile.join(".copilot")),
            vscode_stable_user_root: Some(roaming_app_data.join("Code").join("User")),
            vscode_insiders_user_root: Some(roaming_app_data.join("Code - Insiders").join("User")),
        }
    }

    #[cfg(test)]
    pub(super) fn with_vendor_overrides(
        mut self,
        claude_data_root: impl Into<PathBuf>,
        codex_data_root: impl Into<PathBuf>,
        copilot_data_root: impl Into<PathBuf>,
    ) -> Self {
        self.claude_data_root = Some(claude_data_root.into());
        self.codex_data_root = Some(codex_data_root.into());
        self.copilot_data_root = Some(copilot_data_root.into());
        self
    }

    fn configured_root(
        &self,
        source: DiscoverySource,
        collection: &'static str,
        configured_root: Option<&Path>,
        relative: &Path,
        read_root_relative: Option<&Path>,
        matcher: CandidateMatcher,
    ) -> RootSpec {
        match configured_root {
            Some(root) => {
                let path = root.join(relative);
                let read_root_path = match read_root_relative {
                    Some(read_root_relative) if read_root_relative.as_os_str().is_empty() => {
                        root.to_path_buf()
                    }
                    Some(read_root_relative) => root.join(read_root_relative),
                    None => path.clone(),
                };
                RootSpec::Local {
                    source,
                    collection,
                    path,
                    read_root_path,
                    matcher,
                }
            }
            None => RootSpec::Unavailable {
                source,
                collection,
                status: AvailabilityStatus::Disabled,
            },
        }
    }
}

impl Default for ProductionRootProvider {
    fn default() -> Self {
        Self::from_environment()
    }
}

impl RootProvider for ProductionRootProvider {
    fn roots(&self) -> Vec<RootSpec> {
        vec![
            self.configured_root(
                DiscoverySource::ClaudeCode,
                "projects",
                self.claude_data_root.as_deref(),
                Path::new("projects"),
                None,
                CandidateMatcher::ClaudeProject,
            ),
            self.configured_root(
                DiscoverySource::Codex,
                "active",
                self.codex_data_root.as_deref(),
                Path::new("sessions"),
                None,
                CandidateMatcher::Codex,
            ),
            self.configured_root(
                DiscoverySource::Codex,
                "archived",
                self.codex_data_root.as_deref(),
                Path::new("archived_sessions"),
                None,
                CandidateMatcher::Codex,
            ),
            self.configured_root(
                DiscoverySource::CopilotCli,
                "sessions",
                self.copilot_data_root.as_deref(),
                Path::new("session-state"),
                Some(Path::new("")),
                CandidateMatcher::CopilotCli,
            ),
            self.configured_root(
                DiscoverySource::VscodeCopilot,
                "stable-workspaces",
                self.vscode_stable_user_root.as_deref(),
                Path::new("workspaceStorage"),
                None,
                CandidateMatcher::VsCodeWorkspace,
            ),
            self.configured_root(
                DiscoverySource::VscodeCopilot,
                "stable-empty-window",
                self.vscode_stable_user_root.as_deref(),
                Path::new(r"globalStorage\emptyWindowChatSessions"),
                None,
                CandidateMatcher::VsCodeSessionDirectory,
            ),
            self.configured_root(
                DiscoverySource::VscodeCopilot,
                "insiders-workspaces",
                self.vscode_insiders_user_root.as_deref(),
                Path::new("workspaceStorage"),
                None,
                CandidateMatcher::VsCodeWorkspace,
            ),
            self.configured_root(
                DiscoverySource::VscodeCopilot,
                "insiders-empty-window",
                self.vscode_insiders_user_root.as_deref(),
                Path::new(r"globalStorage\emptyWindowChatSessions"),
                None,
                CandidateMatcher::VsCodeSessionDirectory,
            ),
        ]
    }
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FixtureRootError {
    OutsideFixture,
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct FixtureRootProvider {
    base: PathBuf,
    roots: Vec<RootSpec>,
}

#[cfg(test)]
impl FixtureRootProvider {
    pub(crate) fn new(base: impl AsRef<Path>) -> Result<Self, SnapshotError> {
        let base = base.as_ref();
        LocalRoot::new(base)?;
        Ok(Self {
            base: base.to_path_buf(),
            roots: Vec::new(),
        })
    }

    pub(crate) fn add_root(
        &mut self,
        source: DiscoverySource,
        collection: &'static str,
        relative: &Path,
        matcher: CandidateMatcher,
    ) -> Result<(), FixtureRootError> {
        self.add_root_with_read_scope(source, collection, relative, relative, matcher)
    }

    pub(crate) fn add_root_with_read_scope(
        &mut self,
        source: DiscoverySource,
        collection: &'static str,
        relative: &Path,
        read_root_relative: &Path,
        matcher: CandidateMatcher,
    ) -> Result<(), FixtureRootError> {
        if !is_strict_relative(relative)
            || !is_strict_relative(read_root_relative)
            || !relative.starts_with(read_root_relative)
        {
            return Err(FixtureRootError::OutsideFixture);
        }
        reject_fixture_reparse_components(&self.base, relative)?;
        reject_fixture_reparse_components(&self.base, read_root_relative)?;
        self.roots.push(RootSpec::Local {
            source,
            collection,
            path: self.base.join(relative),
            read_root_path: self.base.join(read_root_relative),
            matcher,
        });
        Ok(())
    }
}

#[cfg(test)]
impl RootProvider for FixtureRootProvider {
    fn roots(&self) -> Vec<RootSpec> {
        self.roots.clone()
    }
}

#[cfg(test)]
fn is_strict_relative(path: &Path) -> bool {
    path.components()
        .all(|component| matches!(component, Component::Normal(_)))
}

#[cfg(test)]
fn reject_fixture_reparse_components(base: &Path, relative: &Path) -> Result<(), FixtureRootError> {
    let mut current = base.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata)
                if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
                    || !metadata.is_dir() =>
            {
                return Err(FixtureRootError::OutsideFixture);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) => return Err(FixtureRootError::OutsideFixture),
        }
    }
    Ok(())
}
