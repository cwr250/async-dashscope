use super::{output, param, util};
use crate::{
    Client,
    error::{DashScopeError, Result},
    ws::pool::{WsLease, WsPool, WsPoolBuilder},
};

use futures_util::Stream;
use reqwest::header::{AUTHORIZATION, USER_AGENT};
use secrecy::ExposeSecret as _;

use std::{
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio_stream::wrappers::BroadcastStream;
use tokio_tungstenite::tungstenite::{Message as WsMessage, client::IntoClientRequest};
use uuid::Uuid;

#[derive(Clone)]
pub struct AsrPool {
    pool: WsPool,
}

pub struct AsrPoolBuilder<'a> {
    client: &'a Client,
    size: usize,
    write_cap: usize,
    read_broadcast_cap: usize,
    initial_size: usize,
    ping_interval: Option<Duration>,
    connect_timeout: Duration,
    message_timeout: Duration,
}

impl<'a> AsrPoolBuilder<'a> {
    pub fn new(client: &'a Client) -> Self {
        Self {
            client,
            size: 4,
            write_cap: 64,
            read_broadcast_cap: 1024,
            initial_size: 0,
            ping_interval: Some(Duration::from_secs(30)),
            connect_timeout: Duration::from_secs(15),
            message_timeout: Duration::from_secs(5),
        }
    }

    pub fn pool_size(mut self, n: usize) -> Self {
        self.size = n.max(1);
        self
    }

    /// Alias for pool_size for naming consistency with WS pool
    pub fn max_size(mut self, n: usize) -> Self {
        self.size = n.max(1);
        self
    }
    pub fn write_capacity(mut self, n: usize) -> Self {
        self.write_cap = n.max(16);
        self
    }
    pub fn read_broadcast_capacity(mut self, n: usize) -> Self {
        self.read_broadcast_cap = n.max(64);
        self
    }

    pub fn initial_size(mut self, n: usize) -> Self {
        self.initial_size = n;
        self
    }

    pub fn ping_interval(mut self, dur: Option<Duration>) -> Self {
        self.ping_interval = dur;
        self
    }

    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    pub fn message_timeout(mut self, timeout: Duration) -> Self {
        self.message_timeout = timeout;
        self
    }

    pub async fn build(self) -> Result<AsrPool> {
        let cfg = self.client.config();
        let mut request = cfg
            .asr_ws_url()
            .into_client_request()
            .map_err(|e| DashScopeError::WebSocketError(format!("Invalid ASR WS URL: {e}")))?;

        // 仅注入必要头
        let h = request.headers_mut();

        // Add Authorization header if API key is not empty
        if !cfg.api_key().expose_secret().is_empty() {
            h.insert(
                AUTHORIZATION,
                format!("Bearer {}", cfg.api_key().expose_secret())
                    .parse()
                    .expect("authorization header should parse successfully"),
            );
        }

        // Add User-Agent header
        h.insert(
            USER_AGENT,
            crate::config::USER_AGENT_VALUE
                .parse()
                .expect("static User-Agent header should parse successfully"),
        );

        let pool = WsPoolBuilder::new(request, self.size)
            .write_capacity(self.write_cap)
            .read_broadcast_capacity(self.read_broadcast_cap)
            .ping_interval(self.ping_interval)
            .initial_size(self.initial_size)
            .connect_timeout(self.connect_timeout)
            .message_timeout(self.message_timeout)
            .build()
            .await?;
        Ok(AsrPool { pool })
    }
}

pub struct AsrSession {
    task_id: String,
    lease: WsLease,
    finished: AtomicBool,
}

/// 统一的ASR事件类型，用于全双工会话
///
/// 这个枚举提供了标准化的ASR事件接口，抽象了底层厂商的具体实现细节。
/// 支持实时获取识别过程中的各种状态和结果，适用于需要低延迟反馈的应用场景。
#[derive(Debug, Clone, PartialEq)]
pub enum AsrEvent {
    /// 任务已启动
    ///
    /// 当ASR会话成功建立并开始处理时触发
    Started,

    /// 部分识别结果（增量）
    ///
    /// 包含当前识别到的增量文本，通常在语音识别过程中实时产生。
    /// 这些结果可能会在后续识别中被修正或替换。
    Partial(String),

    /// 最终识别结果
    ///
    /// 包含确定的识别文本，通常在语音段落结束或达到某种确定性阈值时产生。
    /// 这是比 Partial 更可靠的识别结果。
    Final(String),

    /// 任务已完成
    ///
    /// 当整个ASR会话正常结束时触发，表示所有音频都已处理完毕
    Finished,

    /// 任务失败
    ///
    /// 当ASR会话因错误而终止时触发
    Failed {
        /// 错误代码，由ASR服务提供商定义
        error_code: Option<String>,
        /// 错误消息的详细描述
        error_message: Option<String>,
    },
}

/// 全双工ASR会话，支持并发收发
///
/// 这个结构提供了真正的全双工ASR通信能力：
/// - 上行：通过 `send_audio` 和 `finish` 方法发送音频数据
/// - 下行：通过 `resp_stream` 实时接收识别事件
///
/// 与传统的"先发完再收"模式不同，此会话在创建时就立即订阅了响应流，
/// 确保不会错过任何早期的识别结果，实现最低延迟的流式语音识别。
///
/// # 使用示例
///
/// ```rust,no_run
/// # use async_dashscope::operation::audio::asr::{AsrPool, AsrParaformerParamsBuilder, AsrEvent};
/// # use futures_util::StreamExt;
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let pool = AsrPool::builder().build().await?;
/// let params = AsrParaformerParamsBuilder::default().build()?;
/// let mut duplex = pool.recognize_duplex(params).await?;
///
/// // 并发发送音频和接收结果
/// tokio::select! {
///     // 发送音频
///     result = async {
///         duplex.send_audio(audio_chunk1).await?;
///         duplex.send_audio(audio_chunk2).await?;
///         duplex.finish().await
///     } => result?,
///
///     // 接收识别结果
///     _ = async {
///         while let Some(event) = duplex.resp_stream.next().await {
///             match event? {
///                 AsrEvent::Partial(text) => println!("部分结果: {}", text),
///                 AsrEvent::Final(text) => println!("最终结果: {}", text),
///                 AsrEvent::Finished => break,
///                 _ => {}
///             }
///         }
///     } => {}
/// }
/// # Ok(())
/// # }
/// ```
pub struct AsrDuplexSession {
    /// 底层的ASR会话，用于发送音频数据
    pub session: AsrSession,
    /// 响应事件流，实时接收识别结果
    pub resp_stream: Pin<Box<dyn Stream<Item = Result<AsrEvent>> + Send>>,
}

impl AsrDuplexSession {
    /// 发送音频数据块
    ///
    /// 此方法将音频数据异步发送到ASR服务。支持背压控制，
    /// 如果发送缓冲区满，会自动等待直到有空间可用。
    ///
    /// # 参数
    /// - `data`: 音频数据块，应符合会话建立时指定的音频格式
    ///
    /// # 错误
    /// 如果底层WebSocket连接断开或发送失败，将返回错误
    pub async fn send_audio(&self, data: Vec<u8>) -> Result<()> {
        self.session.send_audio(data).await
    }

    /// Send audio data as bytes (avoids intermediate Vec allocation)
    ///
    /// # 错误
    /// 如果底层WebSocket连接断开或发送失败，将返回错误
    pub async fn send_audio_bytes(&self, data: bytes::Bytes) -> Result<()> {
        self.session.send_audio_bytes(data).await
    }

    /// 完成音频发送，表示没有更多音频数据
    ///
    /// 调用此方法后，ASR服务将处理剩余的音频并生成最终结果。
    /// 响应流会继续活跃直到收到 `AsrEvent::Finished` 事件。
    ///
    /// # 错误
    /// 如果无法向服务发送完成信号，将返回错误
    pub async fn finish(&self) -> Result<()> {
        self.session.finish().await
    }

    /// 获取会话的任务ID
    ///
    /// 任务ID用于标识和跟踪特定的ASR会话，在日志记录和错误排查中很有用。
    pub fn task_id(&self) -> &str {
        self.session.task_id()
    }
}

impl AsrPool {
    /// Test connection acquisition for health checks
    pub async fn test_acquire(&self) -> Result<()> {
        let lease = self.pool.acquire().await?;
        drop(lease); // Immediately return to pool
        Ok(())
    }

    /// Acquire a connection with timeout for better resilience during transient failures
    pub async fn acquire_with_timeout(&self, timeout: Duration) -> Result<WsLease> {
        self.pool.acquire_with_timeout(timeout).await
    }

    pub async fn start_session(&self, params: param::AsrParaformerParams) -> Result<AsrSession> {
        let lease = self
            .pool
            .acquire_with_timeout(Duration::from_millis(500))
            .await?;
        let task_id = Uuid::new_v4().to_string();

        let run = param::RunTaskPayload::new(&params.model, &params.parameters);
        let header = param::MessageHeader::run_task(task_id.clone());
        let start_msg = param::WebSocketMessage {
            header,
            payload: run,
        };

        let bytes = serde_json::to_vec(&start_msg)
            .map_err(|e| DashScopeError::JSONSerialize(e.to_string()))?;
        lease.send_text(bytes).await?;

        Ok(AsrSession {
            task_id,
            lease,
            finished: AtomicBool::new(false),
        })
    }

    /// 创建ASR会话并预订阅响应流，消除早期事件丢失风险
    ///
    /// 此方法在发送run-task之前先订阅broadcast receiver，
    /// 确保不会丢失任何早期事件（如TaskStarted或首个增量结果）。
    ///
    /// # Returns
    /// 返回元组: (AsrSession, broadcast::Receiver)
    /// 调用者应该使用预订阅的receiver来构造响应流
    pub async fn start_session_with_subscription(
        &self,
        params: param::AsrParaformerParams,
    ) -> Result<(
        AsrSession,
        tokio::sync::broadcast::Receiver<crate::ws::pool::WsResult>,
    )> {
        let lease = self
            .pool
            .acquire_with_timeout(Duration::from_millis(500))
            .await?;
        let task_id = Uuid::new_v4().to_string();

        // 关键：先订阅，再发送run-task，消除竞态窗口
        let rx = lease.subscribe();

        let run = param::RunTaskPayload::new(&params.model, &params.parameters);
        let header = param::MessageHeader::run_task(task_id.clone());
        let start_msg = param::WebSocketMessage {
            header,
            payload: run,
        };

        let bytes = serde_json::to_vec(&start_msg)
            .map_err(|e| DashScopeError::JSONSerialize(e.to_string()))?;
        lease.send_text(bytes).await?;

        Ok((
            AsrSession {
                task_id,
                lease,
                finished: AtomicBool::new(false),
            },
            rx,
        ))
    }

    /// 语音识别方法（重构为真正的全双工流式处理）
    ///
    /// 此方法已重构以实现真正的全双工通信：
    /// 1. 先订阅响应流，立即开始接收识别结果
    /// 2. 并发发送音频数据流
    /// 3. 完成音频发送后等待最终识别结果
    ///
    /// 这样可以避免丢失早期的 result-generated 事件，显著降低端到端延迟。
    ///
    /// # 参数
    /// - `params`: ASR识别参数
    /// - `audio_stream`: 音频数据流，每个chunk应为有效的音频格式数据
    ///
    /// # 返回
    /// 返回最终的识别文本结果
    ///
    /// # 错误
    /// 如果连接失败、音频发送失败或识别任务失败，将返回相应错误
    pub async fn recognize(
        &self,
        params: param::AsrParaformerParams,
        mut audio_stream: impl Stream<Item = Result<Vec<u8>>> + Unpin + Send + 'static,
    ) -> Result<String> {
        let (session, rx) = self.start_session_with_subscription(params).await?;
        let session_task_id = session.task_id().to_string();

        // 关键：使用预订阅的receiver构造响应流，消除早期事件丢失风险
        let task_id_for_stream = session_task_id.clone();
        let mut resp_stream = {
            use tokio_stream::wrappers::BroadcastStream;

            tokio_stream::StreamExt::filter_map(BroadcastStream::new(rx), move |ev| {
                let task_id = task_id_for_stream.clone();
                match ev {
                    Ok(Ok(WsMessage::Text(txt))) => {
                        util::parse_asr_ws_text_for_task(&task_id, &txt)
                    }
                    Ok(Ok(WsMessage::Close(_))) => Some(Ok(output::AsrResponse::TaskFinished)),
                    Ok(Err(e)) => Some(Err(DashScopeError::WebSocketError(e.to_string()))),
                    Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                        Some(Err(DashScopeError::WebSocketError(format!(
                            "ASR stream lagged by {n}"
                        ))))
                    }
                    _ => None,
                }
            })
        };

        // 并发接收任务
        let task_id_clone = session_task_id.clone();
        let recv = tokio::spawn(async move {
            let mut text = String::new();
            while let Some(item) = tokio_stream::StreamExt::next(&mut resp_stream).await {
                match item? {
                    output::AsrResponse::ResultGenerated(o) => text = o.sentence.text,
                    output::AsrResponse::TaskFailed {
                        error_code,
                        error_message,
                    } => {
                        return Err(DashScopeError::ApiError(crate::error::ApiError {
                            message: error_message.unwrap_or_else(|| "ASR task failed".to_string()),
                            request_id: Some(task_id_clone),
                            code: error_code,
                        }));
                    }
                    output::AsrResponse::TaskFinished => break,
                    _ => {}
                }
            }
            Ok::<String, DashScopeError>(text)
        });

        // 并发发送音频，错误要优雅收尾
        let send_result = async {
            while let Some(chunk) = tokio_stream::StreamExt::next(&mut audio_stream).await {
                session.send_audio(chunk?).await?;
            }
            session.finish().await
        }
        .await;

        // 统一收尾
        match send_result {
            Ok(()) => {
                // 等接收完成
                let final_text = recv.await.map_err(|e| {
                    DashScopeError::WebSocketError(format!("Task join error: {:?}", e))
                })??;
                Ok(final_text)
            }
            Err(e) => {
                // 发送失败，尝试finish并中止接收任务
                let _ = session.finish().await;
                recv.abort();
                Err(e)
            }
        }
    }

    /// 创建一个真正全双工的ASR会话，返回发送句柄和响应流
    ///
    /// 与 `recognize` 方法不同，此方法立即订阅响应流，支持并发收发，
    /// 适用于需要实时获取增量识别结果的场景。
    ///
    /// # 全双工优势
    ///
    /// - **零延迟订阅**: 会话创建时立即订阅响应流，不会错过早期识别结果
    /// - **并发处理**: 音频发送和结果接收完全并发，最大化吞吐量
    /// - **实时反馈**: 支持获取增量识别结果，实现边说边显示的用户体验
    /// - **资源高效**: 避免缓冲整段音频，降低内存占用
    ///
    /// # 使用场景
    ///
    /// - 实时语音转文字应用
    /// - 需要增量结果反馈的语音助手
    /// - 低延迟要求的语音交互系统
    /// - 长时间音频流的实时处理
    ///
    /// # 参数
    /// - `params`: ASR识别参数配置
    ///
    /// # 返回
    /// 返回 `AsrDuplexSession`，包含：
    /// - `session`: 用于发送音频数据的会话句柄
    /// - `resp_stream`: 实时接收识别事件的响应流
    ///
    /// # 错误
    /// 如果无法建立WebSocket连接或创建ASR会话失败，将返回错误
    ///
    /// # 示例
    ///
    /// ```rust,no_run
    /// use futures_util::StreamExt;
    /// use tokio::select;
    ///
    /// let mut duplex = pool.recognize_duplex(params).await?;
    ///
    /// select! {
    ///     // 发送音频流
    ///     send_result = async {
    ///         for chunk in audio_chunks {
    ///             duplex.send_audio(chunk).await?;
    ///         }
    ///         duplex.finish().await
    ///     } => send_result?,
    ///
    ///     // 实时处理识别结果
    ///     _ = async {
    ///         while let Some(event) = duplex.resp_stream.next().await {
    ///             match event? {
    ///                 AsrEvent::Partial(text) => {
    ///                     println!("增量结果: {}", text);
    ///                 }
    ///                 AsrEvent::Final(text) => {
    ///                     println!("最终结果: {}", text);
    ///                 }
    ///                 AsrEvent::Finished => break,
    ///                 AsrEvent::Failed { error_message, .. } => {
    ///                     eprintln!("识别失败: {:?}", error_message);
    ///                     break;
    ///                 }
    ///                 _ => {}
    ///             }
    ///         }
    ///     } => {}
    /// }
    /// ```
    pub async fn recognize_duplex(
        &self,
        params: param::AsrParaformerParams,
    ) -> Result<AsrDuplexSession> {
        let (session, rx) = self.start_session_with_subscription(params).await?;

        // 使用预订阅的receiver构造响应流，消除早期事件丢失风险
        let task_id = session.task_id().to_string();
        let resp_stream = {
            use tokio_stream::wrappers::BroadcastStream;

            tokio_stream::StreamExt::filter_map(BroadcastStream::new(rx), move |ev| {
                let task_id = task_id.clone();
                match ev {
                    Ok(Ok(WsMessage::Text(txt))) => {
                        util::parse_asr_ws_text_for_task(&task_id, &txt)
                    }
                    Ok(Ok(WsMessage::Close(_))) => Some(Ok(output::AsrResponse::TaskFinished)),
                    Ok(Err(e)) => Some(Err(DashScopeError::WebSocketError(e.to_string()))),
                    Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                        Some(Err(DashScopeError::WebSocketError(format!(
                            "ASR stream lagged by {n}"
                        ))))
                    }
                    _ => None,
                }
            })
        };

        // 转换 AsrResponse 到 AsrEvent，简化映射逻辑
        let event_stream = {
            // 直接映射AsrResponse到AsrEvent，不维护状态
            tokio_stream::StreamExt::map(resp_stream, |result| {
                result.map(|asr_resp| match asr_resp {
                    output::AsrResponse::TaskStarted => AsrEvent::Started,
                    output::AsrResponse::ResultGenerated(o) => {
                        // 智能区分增量和最终结果
                        let text = o.sentence.text;
                        let has_end_time = o.sentence.end_time.is_some();
                        let has_sentence_ending = text.ends_with('.')
                            || text.ends_with('?')
                            || text.ends_with('!')
                            || text.ends_with('。')
                            || text.ends_with('？')
                            || text.ends_with('！');

                        if text.trim().is_empty() || text.len() < 2 {
                            // 过短的文本认为是增量
                            AsrEvent::Partial(text)
                        } else if has_end_time || has_sentence_ending {
                            // 有结束时间或句子结尾标点，认为是最终结果
                            AsrEvent::Final(text)
                        } else {
                            // 其他情况认为是增量结果
                            AsrEvent::Partial(text)
                        }
                    }
                    output::AsrResponse::TaskFinished => {
                        // 简化：一律返回Finished，让上层pipeline处理Final补发
                        AsrEvent::Finished
                    }
                    output::AsrResponse::TaskFailed {
                        error_code,
                        error_message,
                    } => AsrEvent::Failed {
                        error_code,
                        error_message,
                    },
                })
            })
        };

        Ok(AsrDuplexSession {
            session,
            resp_stream: Box::pin(event_stream),
        })
    }

    /// Close all connections in the pool
    pub async fn close_all(&self) {
        self.pool.close_all().await;
    }

    /// Get current pool status for observability
    pub async fn status(&self) -> crate::ws::pool::PoolStatus {
        self.pool.status().await
    }
}

impl AsrSession {
    pub async fn send_audio(&self, chunk: Vec<u8>) -> Result<()> {
        if self.finished.load(Ordering::Acquire) {
            return Err(DashScopeError::InvalidArgument(
                "send_audio after finish".into(),
            ));
        }
        self.lease.send_binary(chunk).await
    }

    /// Send audio data as bytes (avoids intermediate Vec allocation)
    pub async fn send_audio_bytes(&self, chunk: bytes::Bytes) -> Result<()> {
        if self.finished.load(Ordering::Acquire) {
            return Err(DashScopeError::InvalidArgument(
                "send_audio after finish".into(),
            ));
        }
        self.lease.send_binary(chunk.to_vec()).await
    }

    pub async fn finish(&self) -> Result<()> {
        if self.finished.load(Ordering::Acquire) {
            return Ok(());
        }
        self.finished.store(true, Ordering::Release);
        let header = param::MessageHeader::finish_task(self.task_id.clone());
        let finish = param::FinishTaskPayload::default();
        let msg = param::WebSocketMessage {
            header,
            payload: finish,
        };
        let bytes =
            serde_json::to_vec(&msg).map_err(|e| DashScopeError::JSONSerialize(e.to_string()))?;
        self.lease.send_text(bytes).await
    }

    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    /// 非消费会话的订阅接口：返回该会话的事件流
    ///
    /// 此方法创建一个新的广播接收端，不消费 self，允许在发送音频的同时
    /// 并发地接收 ASR 响应事件，实现真正的双工通信。
    pub fn stream_by_ref(&self) -> impl Stream<Item = Result<output::AsrResponse>> + use<> {
        let task_id = self.task_id.clone();
        let rx = self.lease.subscribe(); // 拿到一个新的广播接收端（独立于 self）
        tokio_stream::StreamExt::filter_map(BroadcastStream::new(rx), move |ev| {
            let task_id = task_id.clone();
            match ev {
                Ok(Ok(WsMessage::Text(txt))) => {
                    util::parse_asr_ws_text_for_task(&task_id, &txt)
                }
                Ok(Ok(WsMessage::Close(_))) => Some(Ok(output::AsrResponse::TaskFinished)),
                Ok(Err(e)) => Some(Err(DashScopeError::WebSocketError(e.to_string()))),
                Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                    Some(Err(DashScopeError::WebSocketError(format!(
                        "ASR stream lagged by {n}"
                    ))))
                }
                _ => None,
            }
        })
    }

    pub fn stream(self) -> impl Stream<Item = Result<output::AsrResponse>> {
        let task_id = self.task_id.clone();
        tokio_stream::StreamExt::filter_map(
            BroadcastStream::new(self.lease.subscribe()),
            move |ev| {
                let task_id = task_id.clone();
                match ev {
                    Ok(Ok(WsMessage::Text(txt))) => {
                        util::parse_asr_ws_text_for_task(&task_id, &txt)
                    }
                    Ok(Ok(WsMessage::Close(_))) => Some(Ok(output::AsrResponse::TaskFinished)),
                    Ok(Err(e)) => Some(Err(DashScopeError::WebSocketError(e.to_string()))),
                    Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                        Some(Err(DashScopeError::WebSocketError(format!(
                            "ASR stream lagged by {n}"
                        ))))
                    }
                    _ => None,
                }
            },
        )
    }
}

impl Drop for AsrSession {
    fn drop(&mut self) {
        if !self.finished.load(Ordering::Acquire) {
            // best-effort finish-task（非阻塞）
            let header = param::MessageHeader::finish_task(self.task_id.clone());
            let msg = param::WebSocketMessage {
                header,
                payload: param::FinishTaskPayload::default(),
            };
            if let Ok(bytes) = serde_json::to_vec(&msg) {
                let _ = self.lease.try_send_text(bytes);
            }
        }
    }
}
