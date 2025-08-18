// https://dashscope.aliyuncs.com/api/v1/services/aigc/text-generation/generation - text-generation
// https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation - image-generation
// https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation - 音频理解、视觉理解
// https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation  - 录音文件识别
// https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation  - 语音合成
// https://dashscope.aliyuncs.com/api/v1/services/aigc/text2image/image-synthesis - 创意海报生成API参考



use secrecy::SecretString;
use url::Url;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("URL join error: {0}")]
    UrlJoin(#[from] url::ParseError),
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
#[derive(Debug, Clone)]
pub struct Config {
    api_base: Url,
    asr_ws_url: String,
    api_key: SecretString,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigBuilderError {
    #[error("Validation error: {0}")]
    ValidationError(String),
}

#[derive(Debug, Default)]
pub struct ConfigBuilder {
    api_base: Option<String>,
    asr_ws_url: Option<String>,
    api_key: Option<SecretString>,
}

impl ConfigBuilder {
    pub fn api_base<VALUE: Into<String>>(mut self, value: VALUE) -> Self {
        self.api_base = Some(value.into());
        self
    }

    pub fn asr_ws_url<VALUE: Into<String>>(mut self, value: VALUE) -> Self {
        self.asr_ws_url = Some(value.into());
        self
    }

    pub fn api_key<VALUE: Into<SecretString>>(mut self, value: VALUE) -> Self {
        self.api_key = Some(value.into());
        self
    }

    pub fn build(self) -> Result<Config, ConfigBuilderError> {
        let api_key = self.api_key.unwrap_or_else(|| "".to_string().into());

        let mut api_base_str = self
            .api_base
            .unwrap_or_else(|| DASHSCOPE_API_BASE.to_owned());

        let asr_ws_url = self
            .asr_ws_url
            .unwrap_or_else(|| DASHSCOPE_ASR_WS_URL.to_owned());

        // Ensure base URL ends with slash for proper directory semantics
        if !api_base_str.ends_with('/') {
            api_base_str.push('/');
        }

        // Parse and validate base URL
        let api_base_url = Url::parse(&api_base_str).map_err(|e| {
            ConfigBuilderError::ValidationError(format!("Invalid base URL: {}", e))
        })?;

        Ok(Config {
            api_base: api_base_url,
            asr_ws_url,
            api_key,
        })
    }


}

impl Config {
    pub fn url(&self, path: &str) -> Result<String, ConfigError> {
        // Handle empty path case
        if path.is_empty() {
            let base_str = self.api_base.as_str();
            return Ok(base_str.trim_end_matches('/').to_owned());
        }

        // Clean the path and join using url crate for robust handling
        let path = path.trim_start_matches('/');
        let joined_url = self.api_base.join(path)?;

        Ok(joined_url.to_string())
    }
    pub fn set_api_key(&mut self, api_key: SecretString) {
        self.api_key = api_key;
    }

    pub fn api_key(&self) -> &SecretString {
        &self.api_key
    }

    /// Get the ASR WebSocket URL
    pub fn asr_ws_url(&self) -> &str {
        &self.asr_ws_url
    }
}

impl Default for Config {
    fn default() -> Self {
        let api_key: SecretString = std::env::var("DASHSCOPE_API_KEY")
            .unwrap_or_else(|_| "".to_string())
            .into();

        let mut base = DASHSCOPE_API_BASE.to_owned();
        if !base.ends_with('/') {
            base.push('/');
        }

        Self {
            api_base: Url::parse(&base).expect("Default DASHSCOPE_API_BASE should be valid URL"),
            asr_ws_url: DASHSCOPE_ASR_WS_URL.to_owned(),
            api_key,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

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
            "https://dashscope.aliyuncs.com/api/v1/test"
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
        assert_eq!(instance.api_key().expose_secret(), "test");
    }

    #[test]
    fn test_empty_api_key_allowed() {
        let result = ConfigBuilder::default()
            .api_base("https://example.com")
            .api_key("")
            .build();

        assert!(result.is_ok());
        let config = result.unwrap();
        assert_eq!(config.api_key().expose_secret(), "");
    }

    #[test]
    fn test_missing_api_key_uses_empty() {
        let result = ConfigBuilder::default()
            .api_base("https://example.com")
            .build();

        assert!(result.is_ok());
        let config = result.unwrap();
        assert_eq!(config.api_key().expose_secret(), "");
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
    fn test_url_with_invalid_join() {
        let instance = ConfigBuilder::default()
            .api_base("https://example.com")
            .api_key("test")
            .build()
            .unwrap();

        // Test with a path that would cause URL joining to fail
        // Use a truly invalid relative URL reference
        let result = instance.url("http://");
        assert!(result.is_err());

        match result.unwrap_err() {
            ConfigError::UrlJoin(_) => {} // Expected
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
