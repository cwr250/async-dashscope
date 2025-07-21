use crate::{
    client::Client,
    error::{DashScopeError, Result},
};
use futures_util::{Stream, StreamExt};
use reqwest::header::{AUTHORIZATION, USER_AGENT};
use tokio_tungstenite::{connect_async, tungstenite::client::IntoClientRequest};

pub mod connection;
pub mod output;
pub mod param;

pub use connection::AsrConnection;
pub use output::AsrResponse;
pub use param::{
    AsrParaformerParams, AsrParaformerParamsBuilder, AsrParameters, AsrParametersBuilder,
};

/// 提供对 Paraformer ASR (语音识别) 功能的访问。
pub struct Asr<'a> {
    client: &'a Client,
}

impl<'a> Asr<'a> {
    pub fn new(client: &'a Client) -> Self {
        Self { client }
    }

    /// 建立一个 ASR WebSocket 连接并开始一个识别任务。
    ///
    /// 这个方法会处理 WebSocket握手，并发送初始的 `run-task` 消息。
    /// 成功后，它返回一个 `AsrConnection` 对象，你可以用它来发送音频流并接收识别结果。
    ///
    /// # Arguments
    /// * `params` - ASR 任务的配置参数。
    ///
    /// # Returns
    /// 一个可用于流式识别的 `AsrConnection`。
    pub async fn connect(&self, params: AsrParaformerParams) -> Result<AsrConnection> {
        let config = self.client.config();

        // 正确做法：用 IntoClientRequest，把握手头交给库来生成
        let mut request = config
            .asr_ws_url()
            .into_client_request()
            .map_err(|e| DashScopeError::WebSocketError(format!("Invalid URL: {}", e)))?;

        // 再补我们自己的认证头
        let headers = request.headers_mut();
        if let Some(auth_value) = config.headers().get(AUTHORIZATION) {
            headers.insert(AUTHORIZATION, auth_value.clone());
        }

        // 添加 User-Agent 头部
        if let Some(ua_value) = config.headers().get(USER_AGENT) {
            headers.insert(USER_AGENT, ua_value.clone());
        }

        // 这样 connect_async 会自动加上 Upgrade/Connection/Sec-WebSocket-Key 等所有必需头
        let (ws_stream, _) = connect_async(request)
            .await
            .map_err(|e| DashScopeError::WebSocketError(e.to_string()))?;

        AsrConnection::new(ws_stream, params).await
    }

    /// 识别一个完整的音频流并返回最终的文本结果。
    ///
    /// 这是一个便捷方法，它封装了 `connect`, 发送所有音频块, `finish`,
    /// 以及收集最终识别结果的全过程。
    ///
    /// # Arguments
    /// * `params` - ASR 任务的配置参数。
    /// * `audio_stream` - 一个包含音频块 (如 Ogg 页) 的流。
    ///
    /// # Returns
    /// 最终的识别文本字符串。
    pub async fn recognize(
        &self,
        params: AsrParaformerParams,
        mut audio_stream: impl Stream<Item = Result<Vec<u8>>> + Unpin + Send,
    ) -> Result<String> {
        let mut connection = self.connect(params).await?;

        // 1. 发送所有音频数据
        while let Some(chunk_result) = audio_stream.next().await {
            connection.send_audio(chunk_result?).await?;
        }

        // 2. 发送结束信号
        connection.finish().await?;

        // 3. 收集结果
        let mut final_text = String::new();
        while let Some(response_result) = connection.next().await {
            match response_result? {
                AsrResponse::ResultGenerated(output) => {
                    // 对于 paraformer, task_finished 前的最后一个 result-generated 就是最终结果
                    final_text = output.sentence.text;
                }
                AsrResponse::TaskFinished => {
                    break; // 任务结束，可以停止接收
                }
                AsrResponse::TaskFailed {
                    error_code,
                    error_message,
                } => {
                    return Err(DashScopeError::ApiError(crate::error::ApiError {
                        message: error_message.unwrap_or_else(|| "ASR task failed".to_string()),
                        request_id: Some(connection.task_id().to_string()),
                        code: error_code,
                    }));
                }
                _ => {} // 忽略 TaskStarted 等其他消息
            }
        }

        Ok(final_text)
    }
}
