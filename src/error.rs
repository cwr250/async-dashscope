use std::fmt::Display;

use bytes::Bytes;
use reqwest_eventsource::CannotCloneRequestError;
use serde::Deserialize;

use crate::config::ConfigError;

#[derive(Debug, thiserror::Error)]
pub enum DashScopeError {
    #[error("http error: {0}")]
    Reqwest(#[from] reqwest::Error),

    #[error("event source error: {0}")]
    EventSource(#[from] CannotCloneRequestError),

    #[error("failed to deserialize api response: {source}. Raw response: {}", String::from_utf8_lossy(&raw_response).chars().take(200).collect::<String>())]
    JSONDeserialize {
        source: serde_json::Error,
        raw_response: Bytes,
    },
    // 新增
    #[error("failed to serialize json for websocket: {0}")]
    JSONSerialize(String),

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

    #[error("configuration error: {0}")]
    ConfigError(#[from] ConfigError),

    // 新增
    #[error("websocket error: {0}")]
    WebSocketError(String),
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

// 新增 From impl，方便转换
impl From<tokio_tungstenite::tungstenite::Error> for DashScopeError {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        DashScopeError::WebSocketError(e.to_string())
    }
}

pub(crate) fn map_deserialization_error(e: serde_json::Error, bytes: Bytes) -> DashScopeError {
    tracing::error!(
        "failed deserialization of: {}",
        String::from_utf8_lossy(&bytes)
    );
    DashScopeError::JSONDeserialize {
        source: e,
        raw_response: bytes,
    }
}

pub type Result<T> = std::result::Result<T, DashScopeError>;
