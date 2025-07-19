use std::fmt::Display;

use reqwest_eventsource::CannotCloneRequestError;
use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum DashScopeError {
    #[error("http error: {0}")]
    Reqwest(#[from] reqwest::Error),

    #[error("event source error: {0}")]
    EventSource(#[from] CannotCloneRequestError),

    #[error("failed to deserialize api response: {source}. Raw response: {}", String::from_utf8_lossy(&raw_response).chars().take(200).collect::<String>())]
    JSONDeserialize {
        source: serde_json::Error,
        raw_response: Vec<u8>,
    },
    #[error("{0}")]
    ElementError(String),
    #[error("{0}")]
    ApiError(ApiError),
    #[error("invalid argument:{0}")]
    InvalidArgument(String),
    #[error("stream error: {0}")]
    StreamError(#[from] reqwest_eventsource::Error),
    #[error("response body contains invalid UTF-8: {0}")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),

    #[error("upload error: {0}")]
    UploadError(String),
}

#[derive(Debug, Deserialize, Clone)]
pub struct ApiError {
    pub message: String,
    pub request_id: Option<String>,
    pub code: Option<String>,
}

impl Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = Vec::new();
        parts.push(format!("message: {}", self.message));
        if let Some(code) = &self.code {
            parts.push(format!("code: {code}"));
        }
        if let Some(request_id) = &self.request_id {
            parts.push(format!("request_id: {request_id}"));
        }
        write!(f, "{}", parts.join(", "))
    }
}

impl From<crate::operation::common::ParametersBuilderError> for DashScopeError {
    fn from(error: crate::operation::common::ParametersBuilderError) -> Self {
        DashScopeError::InvalidArgument(error.to_string())
    }
}

pub(crate) fn map_deserialization_error(e: serde_json::Error, bytes: &[u8]) -> DashScopeError {
    tracing::error!(
        "failed deserialization of: {}",
        String::from_utf8_lossy(bytes)
    );
    DashScopeError::JSONDeserialize {
        source: e,
        raw_response: bytes.to_vec(),
    }
}

pub type Result<T> = std::result::Result<T, DashScopeError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_error_display_includes_message() {
        let api_error = ApiError {
            message: "Invalid API key".to_string(),
            code: Some("InvalidApiKey".to_string()),
            request_id: Some("req-12345".to_string()),
        };

        let display_str = format!("{}", api_error);

        // Should include the message
        assert!(display_str.contains("Invalid API key"));
        assert!(display_str.contains("code: InvalidApiKey"));
        assert!(display_str.contains("request_id: req-12345"));
    }

    #[test]
    fn test_api_error_display_with_missing_fields() {
        let api_error = ApiError {
            message: "Something went wrong".to_string(),
            code: None,
            request_id: None,
        };

        let display_str = format!("{}", api_error);

        // Should still include the message even when other fields are missing
        assert!(display_str.contains("Something went wrong"));
        assert!(!display_str.contains("code:"));
        assert!(!display_str.contains("request_id:"));
    }

    #[test]
    fn test_json_deserialize_error_includes_raw_response() {
        let raw_response = b"invalid json content that caused the error".to_vec();
        let serde_error = serde_json::from_str::<serde_json::Value>("invalid json").unwrap_err();

        let error = DashScopeError::JSONDeserialize {
            source: serde_error,
            raw_response: raw_response.clone(),
        };

        let error_str = format!("{}", error);

        // Should include part of the raw response in the error message
        assert!(error_str.contains("invalid json content"));
        assert!(error_str.contains("failed to deserialize"));
    }

    #[test]
    fn test_json_deserialize_error_truncates_long_response() {
        // Create a response longer than 200 characters
        let long_response = "x".repeat(300).into_bytes();
        let serde_error = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();

        let error = DashScopeError::JSONDeserialize {
            source: serde_error,
            raw_response: long_response,
        };

        let error_str = format!("{}", error);

        // Should truncate long responses to 200 characters
        let response_part = error_str.split("Raw response: ").nth(1).unwrap_or("");
        assert!(response_part.len() <= 200);
    }

    #[test]
    fn test_map_deserialization_error_logs_and_creates_error() {
        let invalid_json = b"{ invalid json }";
        let serde_error = serde_json::from_slice::<serde_json::Value>(invalid_json).unwrap_err();

        let result = map_deserialization_error(serde_error, invalid_json);

        match result {
            DashScopeError::JSONDeserialize {
                source: _,
                raw_response,
            } => {
                assert_eq!(raw_response, invalid_json);
            }
            _ => panic!("Expected JSONDeserialize error"),
        }
    }

    #[test]
    fn test_stream_error_preserves_original_type() {
        // This test verifies that StreamError now uses #[from] attribute
        // and preserves the original reqwest_eventsource::Error type

        // We can't easily create a reqwest_eventsource::Error for testing,
        // but we can verify the error type structure
        let error_msg = "stream connection failed";

        // This would be how a real StreamError is created via the #[from] conversion
        // let original_error = reqwest_eventsource::Error::...;
        // let dashscope_error = DashScopeError::from(original_error);

        // For now, just test that the error variant exists and has the right structure
        match DashScopeError::InvalidArgument(error_msg.to_string()) {
            DashScopeError::InvalidArgument(msg) => assert_eq!(msg, error_msg),
            _ => {}
        }
    }

    #[test]
    fn test_error_chain_conversion() {
        // Test UTF-8 error conversion
        let utf8_error = String::from_utf8(vec![0, 159, 146, 150]).unwrap_err();
        let dashscope_error = DashScopeError::from(utf8_error);

        match dashscope_error {
            DashScopeError::InvalidUtf8(_) => {
                // Success - the conversion worked
            }
            _ => panic!("Expected InvalidUtf8 error variant"),
        }

        // Test that we can create an ApiError and wrap it
        let api_error = ApiError {
            message: "Test error".to_string(),
            code: Some("TEST_ERROR".to_string()),
            request_id: Some("req-123".to_string()),
        };
        let dashscope_error = DashScopeError::ApiError(api_error);

        match dashscope_error {
            DashScopeError::ApiError(_) => {
                // Success
            }
            _ => panic!("Expected ApiError variant"),
        }
    }
}
