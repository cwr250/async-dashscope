use std::{fmt::Debug, pin::Pin, sync::Arc, time::Duration};

use async_stream::try_stream;
use bytes::Bytes;
use httpdate::parse_http_date;
use reqwest_eventsource::{Event, EventSource, RequestBuilderExt as _};
use secrecy::ExposeSecret as _;

use serde::{Serialize, de::DeserializeOwned};
use tokio_stream::{Stream, StreamExt as _};

use crate::{
    config::Config,
    error::{ApiError, DashScopeError, map_deserialization_error},
};

#[derive(Debug, thiserror::Error)]
pub enum ClientBuildError {
    #[error("failed to build HTTP client: {0}")]
    Reqwest(#[from] reqwest::Error),
}

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
    #[deprecated(
        since = "0.5.0",
        note = "Use max_retry_duration() or a custom backoff instead; count-based retries are not implemented"
    )]
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
    pub fn build(self) -> Result<Client, ClientBuildError> {
        let mut backoff = self.backoff.unwrap_or_default();
        // Set sensible defaults for production use
        if backoff.max_elapsed_time.is_none() {
            backoff.max_elapsed_time = Some(Duration::from_secs(120)); // 2 minutes
        }

        // Build HTTP client with timeout if not provided
        let http_client = match self.http_client {
            Some(client) => client,
            None => {
                let mut client_builder = reqwest::Client::builder();

                // Set timeout (default: 30 seconds)
                let timeout = self.timeout.unwrap_or_else(|| Duration::from_secs(30));
                client_builder = client_builder.timeout(timeout);

                // Set static default headers (non-auth headers)
                let mut default_headers = reqwest::header::HeaderMap::new();
                default_headers.insert(
                    "Content-Type",
                    reqwest::header::HeaderValue::from_static("application/json"),
                );
                default_headers.insert(
                    "X-DashScope-OssResourceResolve",
                    reqwest::header::HeaderValue::from_static("enable"),
                );
                default_headers.insert(
                    reqwest::header::USER_AGENT,
                    crate::config::USER_AGENT_VALUE
                        .parse()
                        .expect("static User-Agent header should parse successfully"),
                );
                client_builder = client_builder.default_headers(default_headers);

                client_builder.build()?
            }
        };

        Ok(Client {
            http_client: Arc::new(http_client),
            config: Arc::new(self.config.unwrap_or_default()),
            backoff: Arc::new(backoff),
        })
    }
}

impl Default for Client {
    fn default() -> Self {
        ClientBuilder::default().build().unwrap()
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
            .url(path)?;
        tracing::debug!(target = "dashscope.http", %url, "opening SSE stream");
        let mut request_builder = self
            .http_client
            .post(&url);

        // Add Authorization header if API key is present
        if let Some(auth_value) = self.auth_header() {
            request_builder = request_builder.header(reqwest::header::AUTHORIZATION, auth_value);
        }

        let event_source = request_builder
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
                .url(path)?;
            tracing::debug!(target = "dashscope.http", %url, "http post");
            let mut request_builder = self
                .http_client
                .post(&url);

            // Add Authorization header if API key is present
            if let Some(auth_value) = self.auth_header() {
                request_builder = request_builder.header(reqwest::header::AUTHORIZATION, auth_value);
            }

            Ok(request_builder
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
                .map_err(|e| {
                    // 仅对网络层可恢复错误启用退避重试
                    if e.is_connect() || e.is_timeout() {
                        backoff::Error::Transient {
                            err: DashScopeError::Reqwest(e),
                            retry_after: None
                        }
                    } else {
                        backoff::Error::Permanent(DashScopeError::Reqwest(e))
                    }
                })?;

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
                            parse_http_date(s).ok().and_then(|t| {
                                let now = std::time::SystemTime::now();
                                t.duration_since(now).ok()
                            })
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
                let api_error = match serde_json::from_slice::<ApiError>(bytes.as_ref()) {
                    Ok(e) => e,
                    Err(_) => {
                        // Non-JSON error body, construct generic error
                        let snippet = String::from_utf8_lossy(&bytes).chars().take(200).collect::<String>();
                        let generic = ApiError {
                            message: format!("HTTP {} error: {}", status, snippet),
                            code: Some(format!("HTTP_{}", status.as_u16())),
                            request_id: None,
                        };
                        // 429/5xx → transient, others → permanent
                        if status.as_u16() == 429 || (status.is_server_error() && status != reqwest::StatusCode::NOT_IMPLEMENTED) {
                            return Err(backoff::Error::Transient {
                                err: DashScopeError::ApiError(generic),
                                retry_after
                            });
                        } else {
                            return Err(backoff::Error::Permanent(DashScopeError::ApiError(generic)));
                        }
                    }
                };

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

    fn auth_header(&self) -> Option<reqwest::header::HeaderValue> {
        let api_key = self.config.api_key().expose_secret();
        if api_key.is_empty() {
            None
        } else {
            Some(format!("Bearer {}", api_key).parse().expect("Bearer token should be valid header value"))
        }
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
                    // This ensures the final message with finish_reason is delivered.
                    // Try JSON parsing first for robust detection, fall back to string contains
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&message.data) {
                        if let Some(fr) = v.pointer("/output/finish_reason").and_then(|x| x.as_str()) {
                            if fr != "null" {
                                break;
                            }
                        }
                    } else if message.data.contains("\"finish_reason\":") {
                        break;
                    }
                }
            }
        }
        event_source.close();
    };

    Box::pin(stream)
}
