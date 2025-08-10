#![doc = include_str!("../docs/base.md")]

mod client;
pub mod config;
pub mod error;
pub mod operation;
mod ws; // 新增（crate 内部使用）

pub use client::{Client, ClientBuilder};
pub(crate) mod oss_util;

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;
    use std::time::Duration;

    #[test]
    fn test_client_builder_basic() {
        let client = ClientBuilder::new()
            .api_key("test-key")
            .timeout(Duration::from_secs(10))
            .build();

        // Verify the client was built successfully
        assert!(client.config().api_key().expose_secret() == "test-key");
    }

    #[test]
    fn test_config_headers_cached() {
        let config = config::ConfigBuilder::default()
            .api_key("test-key")
            .build()
            .unwrap();

        // Headers should be pre-built and cached
        let headers1 = config.headers();
        let headers2 = config.headers();

        // Should return the same reference (cached)
        assert!(std::ptr::eq(headers1, headers2));
    }

    #[test]
    fn test_url_safety() {
        let config = config::ConfigBuilder::default()
            .api_key("test-key")
            .build()
            .unwrap();

        // Test that URL building returns Result
        let result = config.url("/test/path");
        assert!(result.is_ok());
        assert!(result.unwrap().contains("/test/path"));
    }
}
