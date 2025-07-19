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

pub const DASHSCOPE_API_BASE: &str = "https://dashscope.aliyuncs.com/api/v1";
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
pub struct Config {
    #[builder(setter(into, strip_option))]
    #[builder(default = "self.default_base_url()")]
    api_base: Option<String>,
    api_key: SecretString,
}

impl ConfigBuilder {
    fn default_base_url(&self) -> Option<String> {
        Some(DASHSCOPE_API_BASE.to_string())
    }
}

impl Config {
    pub fn url(&self, path: &str) -> String {
        let base = self
            .api_base
            .clone()
            .unwrap_or(DASHSCOPE_API_BASE.to_string());

        // Validate that base URL is properly formed
        let base_url = Url::parse(&base).expect("base URL should be valid");

        // Handle empty path case
        if path.is_empty() {
            return base_url.to_string().trim_end_matches('/').to_string();
        }

        // For proper relative URL resolution, ensure base ends with '/' for joining
        let base_str = if base_url.path().ends_with('/') {
            base_url.to_string()
        } else {
            format!("{}/", base_url.to_string())
        };

        let base_with_slash =
            Url::parse(&base_str).expect("base URL with trailing slash should be valid");

        // Clean the path and join using url crate for robust handling
        let clean_path = path.trim_start_matches('/');

        let joined_url = base_with_slash
            .join(clean_path)
            .expect("path should be valid for URL joining");

        // Trim trailing slash to match original behavior
        joined_url.to_string().trim_end_matches('/').to_string()
    }
    pub fn headers(&self) -> reqwest::header::HeaderMap {
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
            format!("Bearer {}", self.api_key.expose_secret())
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

    pub fn set_api_key(&mut self, api_key: SecretString) {
        self.api_key = api_key;
    }

    pub fn api_key(&self) -> &SecretString {
        &self.api_key
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_base: Some(DASHSCOPE_API_BASE.to_string()),
            api_key: std::env::var("DASHSCOPE_API_KEY")
                .unwrap_or_else(|_| "".to_string())
                .into(),
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
        assert_eq!(instance.url("/v1"), "https://example.com/v1");
    }

    #[test]
    fn test_url_empty_path() {
        let instance = ConfigBuilder::default()
            .api_base("http://localhost:8080")
            .api_key("test")
            .build()
            .unwrap();
        assert_eq!(instance.url(""), "http://localhost:8080");
    }

    #[test]
    fn test_url_empty_api_base() {
        let instance = ConfigBuilder::default().api_key("test").build().unwrap();
        assert_eq!(
            instance.url("/test"),
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
        assert_eq!(instance.url("/b"), "https://a.com/b");
    }

    #[test]
    fn test_url_no_slash_in_path() {
        let instance = ConfigBuilder::default()
            .api_base("https://a.com")
            .api_key("test")
            .build()
            .unwrap();
        assert_eq!(instance.url("b"), "https://a.com/b");
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
}
