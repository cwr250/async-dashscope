use super::{output, param};
use crate::{
    Client,
    error::{DashScopeError, Result},
    ws::pool::{WsLease, WsPool, WsPoolBuilder},
};
use bytes::Bytes;
use futures_util::Stream;
use reqwest::header::{AUTHORIZATION, USER_AGENT};
use serde_json::Value;
use std::time::Duration;
use tokio_stream::{StreamExt, wrappers::BroadcastStream};
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
        if let Some(v) = cfg.headers().get(AUTHORIZATION) {
            h.insert(AUTHORIZATION, v.clone());
        }
        if let Some(v) = cfg.headers().get(USER_AGENT) {
            h.insert(USER_AGENT, v.clone());
        }

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
    finished: bool,
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
            finished: false,
        })
    }

    pub async fn recognize(
        &self,
        params: param::AsrParaformerParams,
        mut audio_stream: impl Stream<Item = Result<Vec<u8>>> + Unpin + Send,
    ) -> Result<String> {
        let mut session = self.start_session(params).await?;
        let session_task_id = session.task_id().to_string();

        // 发送音频
        while let Some(chunk) = audio_stream.next().await {
            session.send_audio(chunk?).await?;
        }
        session.finish().await?;

        // 收集结果
        let mut final_text = String::new();
        let mut resp_stream = session.stream();
        while let Some(item) = resp_stream.next().await {
            match item? {
                output::AsrResponse::ResultGenerated(o) => final_text = o.sentence.text,
                output::AsrResponse::TaskFailed {
                    error_code,
                    error_message,
                } => {
                    return Err(DashScopeError::ApiError(crate::error::ApiError {
                        message: error_message.unwrap_or_else(|| "ASR task failed".to_string()),
                        request_id: Some(session_task_id),
                        code: error_code,
                    }));
                }
                output::AsrResponse::TaskFinished => break,
                _ => {}
            }
        }
        Ok(final_text)
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
        if self.finished {
            return Err(DashScopeError::InvalidArgument(
                "send_audio after finish".into(),
            ));
        }
        self.lease.send_binary(chunk).await
    }

    pub async fn finish(&mut self) -> Result<()> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
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
        BroadcastStream::new(rx).filter_map(move |ev| {
            let task_id = task_id.clone();
            match ev {
                Ok(Ok(WsMessage::Text(txt))) => {
                    // 轻量判断目标 task，再解析
                    let v: Value = match serde_json::from_str(&txt) {
                        Ok(v) => v,
                        Err(e) => {
                            return Some(Err(DashScopeError::JSONDeserialize {
                                source: e,
                                raw_response: Bytes::from(txt),
                            }));
                        }
                    };
                    let hid = v
                        .get("header")
                        .and_then(|h| h.get("task_id"))
                        .and_then(|s| s.as_str());
                    if hid != Some(task_id.as_str()) {
                        return None;
                    }

                    let header: output::IncomingHeader =
                        match serde_json::from_value(v["header"].clone()) {
                            Ok(h) => h,
                            Err(e) => {
                                return Some(Err(DashScopeError::JSONDeserialize {
                                    source: e,
                                    raw_response: Bytes::from(txt),
                                }));
                            }
                        };

                    let resp = match header.event_type() {
                        output::AsrEventType::ResultGenerated => {
                            if let Some(out_val) = v.get("payload").and_then(|p| p.get("output")) {
                                match serde_json::from_value::<output::AsrOutput>(out_val.clone()) {
                                    Ok(o) => output::AsrResponse::ResultGenerated(o),
                                    Err(e) => {
                                        return Some(Err(DashScopeError::JSONDeserialize {
                                            source: e,
                                            raw_response: Bytes::from(txt),
                                        }));
                                    }
                                }
                            } else {
                                return None;
                            }
                        }
                        output::AsrEventType::TaskStarted => output::AsrResponse::TaskStarted,
                        output::AsrEventType::TaskFinished => output::AsrResponse::TaskFinished,
                        output::AsrEventType::TaskFailed => output::AsrResponse::TaskFailed {
                            error_code: header.error_code,
                            error_message: header.error_message,
                        },
                        output::AsrEventType::Unknown(_) => return None,
                    };
                    Some(Ok(resp))
                }
                Ok(Ok(WsMessage::Close(_))) => Some(Ok(output::AsrResponse::TaskFinished)),
                Ok(Err(e)) => Some(Err(DashScopeError::WebSocketError(format!("{:?}", e)))),
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
        BroadcastStream::new(self.lease.subscribe()).filter_map(move |ev| {
            let task_id = task_id.clone();
            match ev {
                Ok(Ok(WsMessage::Text(txt))) => {
                    // 轻量判断目标 task，再解析
                    let v: Value = match serde_json::from_str(&txt) {
                        Ok(v) => v,
                        Err(e) => {
                            return Some(Err(DashScopeError::JSONDeserialize {
                                source: e,
                                raw_response: Bytes::from(txt),
                            }));
                        }
                    };
                    let hid = v
                        .get("header")
                        .and_then(|h| h.get("task_id"))
                        .and_then(|s| s.as_str());
                    if hid != Some(task_id.as_str()) {
                        return None;
                    }

                    let header: output::IncomingHeader =
                        match serde_json::from_value(v["header"].clone()) {
                            Ok(h) => h,
                            Err(e) => {
                                return Some(Err(DashScopeError::JSONDeserialize {
                                    source: e,
                                    raw_response: Bytes::from(txt),
                                }));
                            }
                        };

                    let resp = match header.event_type() {
                        output::AsrEventType::ResultGenerated => {
                            if let Some(out_val) = v.get("payload").and_then(|p| p.get("output")) {
                                match serde_json::from_value::<output::AsrOutput>(out_val.clone()) {
                                    Ok(o) => output::AsrResponse::ResultGenerated(o),
                                    Err(e) => {
                                        return Some(Err(DashScopeError::JSONDeserialize {
                                            source: e,
                                            raw_response: Bytes::from(txt),
                                        }));
                                    }
                                }
                            } else {
                                return None;
                            }
                        }
                        output::AsrEventType::TaskStarted => output::AsrResponse::TaskStarted,
                        output::AsrEventType::TaskFinished => output::AsrResponse::TaskFinished,
                        output::AsrEventType::TaskFailed => output::AsrResponse::TaskFailed {
                            error_code: header.error_code,
                            error_message: header.error_message,
                        },
                        output::AsrEventType::Unknown(_) => return None,
                    };
                    Some(Ok(resp))
                }
                Ok(Ok(WsMessage::Close(_))) => Some(Ok(output::AsrResponse::TaskFinished)),
                Ok(Err(e)) => Some(Err(DashScopeError::WebSocketError(format!("{:?}", e)))),
                Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                    Some(Err(DashScopeError::WebSocketError(format!(
                        "ASR stream lagged by {n}"
                    ))))
                }
                _ => None,
            }
        })
    }
}

impl Drop for AsrSession {
    fn drop(&mut self) {
        if !self.finished {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Client, ClientBuilder};

    #[tokio::test]
    async fn test_asr_pool_builder_creation() {
        let client = ClientBuilder::new().api_key("test-key").build();

        let builder = AsrPoolBuilder::new(&client)
            .pool_size(2)
            .write_capacity(32)
            .read_broadcast_capacity(512);

        // Just verify the builder was created successfully
        assert_eq!(builder.size, 2);
        assert_eq!(builder.write_cap, 32);
        assert_eq!(builder.read_broadcast_cap, 512);
    }

    #[test]
    fn test_asr_session_task_id() {
        // We can't easily create a real AsrSession without a WebSocket connection,
        // so we'll test that the task_id generation works by checking UUID format
        let task_id = Uuid::new_v4().to_string();
        assert!(task_id.len() == 36); // Standard UUID length
        assert!(task_id.contains('-')); // UUIDs contain hyphens
    }

    #[test]
    fn test_pool_builder_configuration() {
        let client = ClientBuilder::new().api_key("test-key").build();

        // Test default values
        let builder = AsrPoolBuilder::new(&client);
        assert_eq!(builder.size, 4);
        assert_eq!(builder.write_cap, 64);
        assert_eq!(builder.read_broadcast_cap, 1024);

        // Test configuration chain
        let configured_builder = AsrPoolBuilder::new(&client)
            .pool_size(8)
            .write_capacity(128)
            .read_broadcast_capacity(2048);

        assert_eq!(configured_builder.size, 8);
        assert_eq!(configured_builder.write_cap, 128);
        assert_eq!(configured_builder.read_broadcast_cap, 2048);

        // Test minimum bounds
        let min_builder = AsrPoolBuilder::new(&client)
            .pool_size(0) // Should be clamped to 1
            .write_capacity(0) // Should be clamped to 16
            .read_broadcast_capacity(0); // Should be clamped to 64

        assert_eq!(min_builder.size, 1);
        assert_eq!(min_builder.write_cap, 16);
        assert_eq!(min_builder.read_broadcast_cap, 64);
    }
}
