use std::{fmt::Debug, pin::Pin, sync::Arc, time::Duration};

use async_stream::try_stream;
use bytes::Bytes;
use reqwest_eventsource::{Event, EventSource, RequestBuilderExt as _};

use serde::{Serialize, de::DeserializeOwned};
use tokio_stream::{Stream, StreamExt as _};

use crate::{
    config::Config,
    error::{ApiError, DashScopeError, map_deserialization_error},
};

#[derive(Debug, Clone)]
pub struct Client {
    http_client: Arc<reqwest::Client>,
    config: Arc<Config>,
    backoff: Arc<backoff::ExponentialBackoff>,
}

/// Builder for constructing a Client with custom configuration
#[derive(Debug, Clone)]
pub struct ClientBuilder {
    http_client: Option<reqwest::Client>,
    config: Option<Config>,
    backoff: Option<backoff::ExponentialBackoff>,
    timeout: Option<Duration>,
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientBuilder {
    /// Create a new ClientBuilder with default settings
    pub fn new() -> Self {
        Self {
            http_client: None,
            config: None,
            backoff: None,
            timeout: None,
        }
    }

    /// Set a custom reqwest::Client (for timeouts, proxy, TLS config, etc.)
    pub fn http_client(mut self, client: reqwest::Client) -> Self {
        self.http_client = Some(client);
        self
    }

    /// Set the configuration (API key, base URL, etc.)
    pub fn config(mut self, config: Config) -> Self {
        self.config = Some(config);
        self
    }

    /// Set a custom backoff strategy for retries
    pub fn backoff(mut self, backoff: backoff::ExponentialBackoff) -> Self {
        self.backoff = Some(backoff);
        self
    }

    /// Convenience method to set API key
    pub fn api_key<S: Into<String>>(mut self, api_key: S) -> Self {
        let mut config = self.config.take().unwrap_or_default();
        config.set_api_key(api_key.into().into());
        self.config = Some(config);
        self
    }

    /// Set maximum retry time (default: 2 minutes)
    pub fn max_retry_duration(mut self, duration: Duration) -> Self {
        let mut backoff = self.backoff.take().unwrap_or_default();
        backoff.max_elapsed_time = Some(duration);
        self.backoff = Some(backoff);
        self
    }

    /// Set maximum number of retries (default: unlimited within time bound)
    /// Note: backoff crate doesn't directly support max count, so we set a reasonable max time
    pub fn max_retry_count(mut self, _count: u64) -> Self {
        let mut backoff = self.backoff.take().unwrap_or_default();
        backoff.max_elapsed_time = Some(Duration::from_secs(300)); // 5 minutes default
        self.backoff = Some(backoff);
        self
    }

    /// Set request timeout (default: 30 seconds)
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Build the final Client
    pub fn build(self) -> Client {
        let mut backoff = self.backoff.unwrap_or_default();
        // Set sensible defaults for production use
        if backoff.max_elapsed_time.is_none() {
            backoff.max_elapsed_time = Some(Duration::from_secs(120)); // 2 minutes
        }

        // Build HTTP client with timeout if not provided
        let http_client = self.http_client.unwrap_or_else(|| {
            let mut client_builder = reqwest::Client::builder();

            // Set timeout (default: 30 seconds)
            let timeout = self.timeout.unwrap_or_else(|| Duration::from_secs(30));
            client_builder = client_builder.timeout(timeout);

            client_builder.build().expect("Failed to build HTTP client")
        });

        Client {
            http_client: Arc::new(http_client),
            config: Arc::new(self.config.unwrap_or_default()),
            backoff: Arc::new(backoff),
        }
    }
}

impl Default for Client {
    fn default() -> Self {
        ClientBuilder::default().build()
    }
}

impl Client {
    /// Create a new Client with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a ClientBuilder for custom configuration
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    /// Create a Client with custom configuration (legacy method, prefer builder())
    pub fn with_config(config: Config) -> Self {
        ClientBuilder::new().config(config).build()
    }

    /// Set API key (legacy method, prefer builder())
    pub fn with_api_key(self, api_key: String) -> Self {
        let mut config = (*self.config).clone();
        config.set_api_key(api_key.into());
        Client {
            config: Arc::new(config),
            ..self
        }
    }

    /// Build client with all components (legacy method, prefer builder())
    pub fn build(
        http_client: reqwest::Client,
        config: Config,
        backoff: backoff::ExponentialBackoff,
    ) -> Self {
        ClientBuilder::new()
            .http_client(http_client)
            .config(config)
            .backoff(backoff)
            .build()
    }

    /// 获取当前实例的生成（Generation）信息
    ///
    /// 此方法属于操作级别，用于创建一个`Generation`对象，
    /// 该对象表示当前实例的某一特定生成（代）信息
    ///
    /// # Returns
    ///
    /// 返回一个`Generation`对象，用于表示当前实例的生成信息
    pub fn generation(&self) -> crate::operation::generation::Generation<'_> {
        crate::operation::generation::Generation::new(self)
    }

    /// 启发多模态对话的功能
    ///
    /// 该函数提供了与多模态对话相关的操作入口
    /// 它创建并返回一个MultiModalConversation实例，用于执行多模态对话操作
    ///
    /// 返回一个`MultiModalConversation`实例，用于进行多模态对话操作
    pub fn multi_modal_conversation(
        &self,
    ) -> crate::operation::multi_modal_conversation::MultiModalConversation<'_> {
        crate::operation::multi_modal_conversation::MultiModalConversation::new(self)
    }

    /// 获取音频处理功能
    pub fn audio(&self) -> crate::operation::audio::Audio<'_> {
        crate::operation::audio::Audio::new(self)
    }

    /// 获取文本嵌入表示
    ///
    /// 此函数提供了一个接口，用于将文本转换为嵌入表示
    /// 它利用当前实例的上下文来生成文本的嵌入表示
    ///
    /// 返回一个`Embeddings`实例，该实例封装了文本嵌入相关的操作和数据
    /// `Embeddings`类型提供了进一步处理文本数据的能力，如计算文本相似度或进行文本分类等
    pub fn text_embeddings(&self) -> crate::operation::embeddings::Embeddings<'_> {
        crate::operation::embeddings::Embeddings::new(self)
    }

    #[must_use]
    pub(crate) async fn post_stream<I, O>(
        &self,
        path: &str,
        request: I,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<O, DashScopeError>> + Send>>, DashScopeError>
    where
        I: Serialize,
        O: DeserializeOwned + std::marker::Send + 'static,
    {
        let url = self
            .config
            .url(path)
            .map_err(|e| DashScopeError::InvalidArgument(e.to_string()))?;
        let event_source = self
            .http_client
            .post(url)
            .headers(self.config.headers().clone())
            .json(&request)
            .eventsource()?;

        Ok(Box::pin(stream(event_source)))
    }

    pub(crate) async fn post<I, O>(&self, path: &str, request: I) -> Result<O, DashScopeError>
    where
        I: Serialize + Debug,
        O: DeserializeOwned,
    {
        let request_maker = || async {
            let url = self
                .config
                .url(path)
                .map_err(|e| DashScopeError::InvalidArgument(e.to_string()))?;
            Ok(self
                .http_client
                .post(url)
                .headers(self.config.headers().clone())
                .json(&request)
                .build()?)
        };

        self.execute(request_maker).await
    }

    async fn execute<O, M, Fut>(&self, request_maker: M) -> Result<O, DashScopeError>
    where
        O: DeserializeOwned,
        M: Fn() -> Fut,
        Fut: core::future::Future<Output = Result<reqwest::Request, DashScopeError>>,
    {
        let bytes = self.execute_raw(request_maker).await?;

        let response: O = serde_json::from_slice(bytes.as_ref())
            .map_err(|e| map_deserialization_error(e, bytes.clone()))?;

        Ok(response)
    }

    async fn execute_raw<M, Fut>(&self, request_maker: M) -> Result<Bytes, DashScopeError>
    where
        M: Fn() -> Fut,
        Fut: core::future::Future<Output = Result<reqwest::Request, DashScopeError>>,
    {
        let client = Arc::clone(&self.http_client);

        backoff::future::retry((*self.backoff).clone(), || async {
            let request = request_maker().await.map_err(backoff::Error::Permanent)?;
            let response = client
                .execute(request)
                .await
                .map_err(DashScopeError::Reqwest)
                .map_err(backoff::Error::Permanent)?;

            let status = response.status();

            // Extract Retry-After header before consuming response
            let retry_after = if status.as_u16() == 429 {
                response
                    .headers()
                    .get("retry-after")
                    .and_then(|h| h.to_str().ok())
                    .and_then(|s| {
                        // Try parsing as seconds first, then as HTTP date
                        s.parse::<u64>().ok().map(Duration::from_secs).or_else(|| {
                            // If it's a date string, calculate duration from now
                            // For simplicity, we'll just use a default duration
                            Some(Duration::from_secs(60))
                        })
                    })
            } else {
                None
            };

            let bytes = response
                .bytes()
                .await
                .map_err(DashScopeError::Reqwest)
                .map_err(backoff::Error::Permanent)?;

            // Deserialize response body from either error object or actual response object
            if !status.is_success() {
                let api_error: ApiError = serde_json::from_slice(bytes.as_ref())
                    .map_err(|e| map_deserialization_error(e, bytes.clone()))
                    .map_err(backoff::Error::Permanent)?;

                if status.as_u16() == 429 {
                    // Rate limited - honor Retry-After header if present
                    tracing::warn!(
                        "Rate limited: {} (retry after: {:?})",
                        api_error.message,
                        retry_after
                    );
                    return Err(backoff::Error::Transient {
                        err: DashScopeError::ApiError(api_error),
                        retry_after,
                    });
                } else if status.is_server_error() && status != reqwest::StatusCode::NOT_IMPLEMENTED
                {
                    // Server errors (5xx) are generally transient, except 501 Not Implemented
                    tracing::warn!(
                        "Server error ({}): {} - retrying",
                        status.as_u16(),
                        api_error.message
                    );
                    return Err(backoff::Error::Transient {
                        err: DashScopeError::ApiError(api_error),
                        retry_after: None,
                    });
                } else {
                    return Err(backoff::Error::Permanent(DashScopeError::ApiError(
                        api_error,
                    )));
                }
            }

            Ok(bytes)
        })
        .await
    }

    pub fn config(&self) -> &Config {
        &self.config
    }
}

pub(crate) fn stream<O>(
    mut event_source: EventSource,
) -> Pin<Box<dyn Stream<Item = Result<O, DashScopeError>> + Send>>
where
    O: DeserializeOwned + std::marker::Send + 'static,
{
    let stream = try_stream! {
        while let Some(ev) = event_source.next().await {
            match ev {
                Err(e) => {
                    Err(DashScopeError::StreamError(e))?;
                }
                Ok(Event::Open) => continue,
                Ok(Event::Message(message)) => {
                    // Direct deserialization to target type O for better performance
                    let response: O = serde_json::from_str(&message.data)
                        .map_err(|e| map_deserialization_error(e, Bytes::from(message.data.clone())))?;

                    // Yield the successful message
                    yield response;

                    // Check for finish reason after sending the message.
                    // This ensures the final message with "stop" is delivered.
                    // Parse JSON only for finish reason check to avoid double parsing
                    if message.data.contains("\"finish_reason\":\"stop\"") {
                        break;
                    }
                }
            }
        }
        event_source.close();
    };

    Box::pin(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    #[test]
    pub fn test_config() {
        let client = Client::builder().api_key("test key").build();

        for header in client.config.headers().iter() {
            if header.0 == "authorization" {
                assert_eq!(header.1, "Bearer test key");
            }
        }
    }

    #[test]
    pub fn test_client_builder() {
        let client = Client::builder()
            .api_key("test-key")
            .max_retry_duration(std::time::Duration::from_secs(60))
            .build();

        // Verify the client was built successfully
        assert_eq!(client.config().api_key().expose_secret(), "test-key");

        // Verify User-Agent header is set
        let headers = client.config().headers();
        assert!(headers.contains_key("user-agent"));
        let user_agent = headers.get("user-agent").unwrap().to_str().unwrap();
        assert!(user_agent.starts_with("async-dashscope/"));
    }

    #[test]
    pub fn test_default_client() {
        let client = Client::new();

        // Should have default configuration
        let headers = client.config().headers();
        assert!(headers.contains_key("user-agent"));
        assert!(headers.contains_key("content-type"));
    }
}
