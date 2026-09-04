use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("{0}")]
    InvalidRequest(String),
    #[error("The operation could not be completed")]
    Internal,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: &'static str,
    pub message: String,
}

impl From<AppError> for CommandError {
    fn from(error: AppError) -> Self {
        match error {
            AppError::InvalidRequest(message) => Self {
                code: "INVALID_REQUEST",
                message,
            },
            AppError::Internal => Self {
                code: "INTERNAL_ERROR",
                message: "The operation could not be completed".to_owned(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AppError, CommandError};

    #[test]
    fn serializes_command_errors_with_stable_camel_case_fields() {
        let error = CommandError::from(AppError::InvalidRequest(
            "A session must be selected".to_owned(),
        ));

        let json = serde_json::to_value(error).expect("command error should serialize");

        assert_eq!(
            json,
            serde_json::json!({
                "code": "INVALID_REQUEST",
                "message": "A session must be selected"
            })
        );
    }

    #[test]
    fn internal_errors_do_not_expose_details() {
        let error = CommandError::from(AppError::Internal);

        assert_eq!(error.code, "INTERNAL_ERROR");
        assert_eq!(error.message, "The operation could not be completed");
    }
}
