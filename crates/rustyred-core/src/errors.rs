use serde::{Deserialize, Serialize};

pub type RustyredResult<T> = Result<T, RustyredError>;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RustyredError {
    pub code: String,
    pub message: String,
}

impl RustyredError {
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
            format!("Unsupported RustyRed command: {}", command.into()),
        )
    }
}
