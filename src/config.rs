// https://dashscope.aliyuncs.com/api/v1/services/aigc/text-generation/generation - text-generation
// https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation - image-generation
// https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation - 音频理解、视觉理解
// https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation  - 录音文件识别
// https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation  - 语音合成
// https://dashscope.aliyuncs.com/api/v1/services/aigc/text2image/image-synthesis - 创意海报生成API参考

use derive_builder::Builder;
use reqwest::header::{AUTHORIZATION, USER_AGENT};
use secrecy::{ExposeSecret as _, SecretString};
use url::Url;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Invalid base URL: {0}")]
    InvalidBaseUrl(#[from] url::ParseError),
    #[error("Invalid path: {0}")]
    InvalidPath(String),
    #[error("Missing API key")]
    MissingApiKey,
}

pub const DASHSCOPE_API_BASE: &str = "https://dashscope.aliyuncs.com/api/v1";
pub const DASHSCOPE_ASR_WS_URL: &str = "wss://dashscope.aliyuncs.com/api-ws/v1/inference";
pub const USER_AGENT_VALUE: &str = concat!("async-dashscope/", env!("CARGO_PKG_VERSION"));

/// # Config
///
/// ```rust
/// let conf = ConfigBuilder::default()
///         // optional, default is: https://dashscope.aliyuncs.com/api/v1
///         .api_base("http://localhost:8080")
///         .api_key("test")
///         .build()
///         .unwrap();
/// let  client = Client::with_config(conf);
/// ```
#[derive(Debug, Builder, Clone)]
#[builder(setter(into))]
#[builder(build_fn(skip))]
pub struct Config {
    #[builder(setter(into, strip_option))]
    #[builder(default = "self.default_base_url()")]
    api_base: Option<String>,
    #[builder(setter(into, strip_option))]
    #[builder(default = "self.default_asr_ws_url()")]
    asr_ws_url: Option<String>,
    api_key: SecretString,
    headers: reqwest::header::HeaderMap,
}

impl ConfigBuilder {
    fn default_base_url(&self) -> Option<String> {
        Some(DASHSCOPE_API_BASE.to_string())
    }

    fn default_asr_ws_url(&self) -> Option<String> {
        Some(DASHSCOPE_ASR_WS_URL.to_string())
    }

    pub fn build(&self) -> Result<Config, ConfigBuilderError> {
        let api_key = self.api_key.clone().ok_or_else(|| {
            ConfigBuilderError::ValidationError("api_key is required".to_string())
        })?;

        // Validate that API key is not empty
        if api_key.expose_secret().is_empty() {
            return Err(ConfigBuilderError::ValidationError(
                "api_key cannot be empty".to_string(),
            ));
        }

        let api_base = self
            .api_base
            .clone()
            .unwrap_or_else(|| self.default_base_url());

        let asr_ws_url = self
            .asr_ws_url
            .clone()
            .unwrap_or_else(|| self.default_asr_ws_url());

        // Validate base URL if provided
        if let Some(ref base) = api_base {
            Url::parse(base).map_err(|e| {
                ConfigBuilderError::ValidationError(format!("Invalid base URL: {}", e))
            })?;
        }

        // Build headers once during construction
        let headers = Self::build_headers(&api_key);

        Ok(Config {
            api_base,
            asr_ws_url,
            api_key,
            headers,
        })
    }

    fn build_headers(api_key: &SecretString) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "Content-Type",
            "application/json"
                .parse()
                .expect("static header value 'application/json' should parse successfully"),
        );
        headers.insert(
            "X-DashScope-OssResourceResolve",
            "enable"
                .parse()
                .expect("static header value 'enable' should parse successfully"),
        );
        headers.insert(
            AUTHORIZATION,
            format!("Bearer {}", api_key.expose_secret())
                .parse()
                .expect("authorization header should parse successfully"),
        );
        headers.insert(
            USER_AGENT,
            USER_AGENT_VALUE
                .parse()
                .expect("static User-Agent header should parse successfully"),
        );
        headers
    }
}

impl Config {
    pub fn url(&self, path: &str) -> Result<String, ConfigError> {
        let base = self.api_base.as_deref().unwrap_or(DASHSCOPE_API_BASE);

        // Validate that base URL is properly formed
        let base_url = Url::parse(base)?;

        // Handle empty path case
        if path.is_empty() {
            return Ok(base_url.to_string().trim_end_matches('/').to_string());
        }

        // For proper relative URL resolution, ensure base ends with '/' for joining
        let base_str = if base_url.path().ends_with('/') {
            base_url.to_string()
        } else {
            format!("{}/", base_url.to_string())
        };

        let base_with_slash = Url::parse(&base_str)?;

        // Clean the path and join using url crate for robust handling
        let clean_path = path.trim_start_matches('/');

        let joined_url = base_with_slash.join(clean_path).map_err(|e| {
            ConfigError::InvalidPath(format!("Failed to join path '{}': {}", path, e))
        })?;

        // Trim trailing slash to match original behavior
        Ok(joined_url.to_string().trim_end_matches('/').to_string())
    }
    pub fn headers(&self) -> &reqwest::header::HeaderMap {
        &self.headers
    }

    pub fn set_api_key(&mut self, api_key: SecretString) {
        self.api_key = api_key.clone();
        // Rebuild headers when API key changes
        self.headers = ConfigBuilder::build_headers(&api_key);
    }

    pub fn api_key(&self) -> &SecretString {
        &self.api_key
    }

    /// Get the ASR WebSocket URL
    pub fn asr_ws_url(&self) -> &str {
        self.asr_ws_url.as_deref().unwrap_or(DASHSCOPE_ASR_WS_URL)
    }
}

impl Default for Config {
    fn default() -> Self {
        let api_key: SecretString = std::env::var("DASHSCOPE_API_KEY")
            .unwrap_or_else(|_| "".to_string())
            .into();
        let headers = ConfigBuilder::build_headers(&api_key);

        Self {
            api_base: Some(DASHSCOPE_API_BASE.to_string()),
            asr_ws_url: Some(DASHSCOPE_ASR_WS_URL.to_string()),
            api_key,
            headers,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_normal_case() {
        let instance = ConfigBuilder::default()
            .api_base("https://example.com")
            .api_key("test")
            .build()
            .unwrap();
        assert_eq!(instance.url("/v1").unwrap(), "https://example.com/v1");
    }

    #[test]
    fn test_url_empty_path() {
        let instance = ConfigBuilder::default()
            .api_base("http://localhost:8080")
            .api_key("test")
            .build()
            .unwrap();
        assert_eq!(instance.url("").unwrap(), "http://localhost:8080");
    }

    #[test]
    fn test_url_empty_api_base() {
        let instance = ConfigBuilder::default().api_key("test").build().unwrap();
        assert_eq!(
            instance.url("/test").unwrap(),
            format!("{DASHSCOPE_API_BASE}/test").as_str()
        );
    }

    #[test]
    fn test_url_slash_in_both_parts() {
        let instance = ConfigBuilder::default()
            .api_base("https://a.com/")
            .api_key("test")
            .build()
            .unwrap(); //Config {
        assert_eq!(instance.url("/b").unwrap(), "https://a.com/b");
    }

    #[test]
    fn test_url_no_slash_in_path() {
        let instance = ConfigBuilder::default()
            .api_base("https://a.com")
            .api_key("test")
            .build()
            .unwrap();
        assert_eq!(instance.url("b").unwrap(), "https://a.com/b");
    }

    #[test]
    fn test_api_key() {
        let instance = ConfigBuilder::default()
            .api_base("https://example.com")
            .api_key("test")
            .build()
            .unwrap();
        assert_eq!(
            instance.headers().get("Authorization").unwrap(),
            "Bearer test"
        );
    }

    #[test]
    fn test_empty_api_key_validation() {
        let result = ConfigBuilder::default()
            .api_base("https://example.com")
            .api_key("")
            .build();

        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("api_key cannot be empty"));
    }

    #[test]
    fn test_missing_api_key_validation() {
        let result = ConfigBuilder::default()
            .api_base("https://example.com")
            .build();

        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("api_key is required"));
    }

    #[test]
    fn test_invalid_base_url_validation() {
        let result = ConfigBuilder::default()
            .api_base("not-a-valid-url")
            .api_key("test")
            .build();

        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("Invalid base URL"));
    }

    #[test]
    fn test_url_with_invalid_base() {
        let config = Config {
            api_base: Some("invalid-url".to_string()),
            asr_ws_url: Some(DASHSCOPE_ASR_WS_URL.to_string()),
            api_key: "test".into(),
            headers: ConfigBuilder::build_headers(&"test".into()),
        };

        let result = config.url("/test");
        assert!(result.is_err());

        match result.unwrap_err() {
            ConfigError::InvalidBaseUrl(_) => {} // Expected
            _ => panic!("Expected InvalidBaseUrl error"),
        }
    }

    #[test]
    fn test_url_with_invalid_path() {
        let instance = ConfigBuilder::default()
            .api_base("https://example.com")
            .api_key("test")
            .build()
            .unwrap();

        // Test with a path that would cause URL joining to fail
        let result = instance.url("://invalid");
        assert!(result.is_err());

        match result.unwrap_err() {
            ConfigError::InvalidPath(_) => {} // Expected
            _ => panic!("Expected InvalidPath error"),
        }
    }

    #[test]
    fn test_asr_ws_url_default() {
        let config = ConfigBuilder::default().api_key("test").build().unwrap();

        assert_eq!(config.asr_ws_url(), DASHSCOPE_ASR_WS_URL);
    }

    #[test]
    fn test_asr_ws_url_custom() {
        let custom_url = "wss://custom-dashscope.example.com/api-ws/v1/inference";
        let config = ConfigBuilder::default()
            .api_key("test")
            .asr_ws_url(custom_url)
            .build()
            .unwrap();

        assert_eq!(config.asr_ws_url(), custom_url);
    }

    #[test]
    fn test_asr_ws_url_vs_rest_api_base() {
        let config = ConfigBuilder::default()
            .api_key("test")
            .api_base("https://dashscope.aliyuncs.com/api/v1")
            .build()
            .unwrap();

        // Verify that ASR WebSocket URL is independent of REST API base
        assert_eq!(
            config.asr_ws_url(),
            "wss://dashscope.aliyuncs.com/api-ws/v1/inference"
        );
        assert_eq!(
            config.url("").unwrap(),
            "https://dashscope.aliyuncs.com/api/v1"
        );

        // They should be different - REST uses https/api/v1, WebSocket uses wss/api-ws/v1
        assert_ne!(config.asr_ws_url(), config.url("").unwrap());
    }
}
