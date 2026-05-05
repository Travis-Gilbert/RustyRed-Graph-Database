use serde::{Deserialize, Serialize};

pub type ThgResult<T> = Result<T, ThgError>;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ThgError {
    pub code: String,
    pub message: String,
}

impl ThgError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn invalid_json(message: impl Into<String>) -> Self {
        Self::new("invalid_json", message)
    }

    pub fn unsupported_command(command: impl Into<String>) -> Self {
        Self::new(
            "unsupported_command",
            format!("Unsupported THG command: {}", command.into()),
        )
    }
}
