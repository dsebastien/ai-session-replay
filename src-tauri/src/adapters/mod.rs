pub mod claude;
pub mod codex;
pub mod conformance;
pub mod contract;
pub mod copilot_cli;
pub(crate) mod deletion;
pub(crate) mod jetbrains;
pub mod normalize;
pub mod vscode;
pub(crate) mod vscode_mutation;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
