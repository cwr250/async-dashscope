use crate::{
    error::{DashScopeError, Result},
    operation::audio::asr::{
        output::{AsrEventType, AsrOutput, AsrResponse, IncomingHeader},
        param::{
            AsrParaformerParams, FinishTaskPayload, MessageHeader, RunTaskPayload, WebSocketMessage,
        },
    },
};
use bytes::Bytes;
use futures_util::{
    SinkExt, Stream, StreamExt,
    stream::{SplitSink, SplitStream},
};
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message as WsMessage};
use uuid::Uuid;

type WsSink = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, WsMessage>;
type WsStream = SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;

/// 一个可用于连接池的 ASR WebSocket 连接。
///
/// 此对象代表一个与 Dashscope ASR 服务建立的、活跃的 WebSocket 连接，
/// 并管理一个单一的识别任务。它的设计目标是能被 `deadpool` 等连接池管理。
///
/// # 生命周期
/// 1. 通过 `Asr::connect()` 创建。创建时，它会自动连接并发送 `run-task` 消息。
/// 2. 使用 `send_audio()` 发送音频块。
/// 3. 使用 `finish()` 发送结束信号。
/// 4. 通过 `Stream` trait 异步接收 `AsrResponse` 消息。
/// 5. 当此对象被 `drop` 时，连接会自动关闭。
#[must_use = "streams do nothing unless polled"]
pub struct AsrConnection {
    task_id: String,
    ws_sink: WsSink,
    ws_stream: WsStream,
    is_finished: bool,
}

impl std::fmt::Debug for AsrConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsrConnection")
            .field("task_id", &self.task_id)
            .field("is_finished", &self.is_finished)
            .field("ws_sink", &"<WebSocket Sink>")
            .field("ws_stream", &"<WebSocket Stream>")
            .finish()
    }
}

impl AsrConnection {
    /// 内部构造函数，创建一个新的 ASR 连接。
    pub(crate) async fn new(
        ws_stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
        params: AsrParaformerParams,
    ) -> Result<Self> {
        let task_id = Uuid::new_v4().to_string();
        let (mut ws_sink, ws_stream) = ws_stream.split();

        // 1. 发送 'run-task' 消息
        let run_payload = RunTaskPayload::new(&params.model, &params.parameters);
        let header = MessageHeader::run_task(task_id.clone());
        let start_msg = WebSocketMessage {
            header,
            payload: run_payload,
        };
        let start_json = serde_json::to_string(&start_msg)
            .map_err(|e| DashScopeError::JSONSerialize(e.to_string()))?;

        ws_sink
            .send(WsMessage::Text(start_json.into()))
            .await
            .map_err(|e| DashScopeError::WebSocketError(e.to_string()))?;

        Ok(Self {
            task_id,
            ws_sink,
            ws_stream,
            is_finished: false,
        })
    }

    /// 异步发送一个音频数据块。
    ///
    /// # Arguments
    /// * `chunk` - 原始的音频二进制数据 (例如, 一个 Opus Ogg 页)。
    pub async fn send_audio(&mut self, chunk: Vec<u8>) -> Result<()> {
        if self.is_finished {
            return Err(DashScopeError::InvalidArgument(
                "Cannot send audio after finishing the stream.".to_string(),
            ));
        }
        self.ws_sink
            .send(WsMessage::Binary(chunk.into()))
            .await
            .map_err(|e| DashScopeError::WebSocketError(e.to_string()))
    }

    /// 异步发送结束信号，告知服务器音频流已结束。
    ///
    /// 这会发送 `finish-task` 消息。在此之后，不应再发送任何音频。
    /// 你仍然需要继续轮询 `Stream` 来接收最终的识别结果。
    pub async fn finish(&mut self) -> Result<()> {
        if self.is_finished {
            return Ok(()); // 幂等操作
        }
        self.is_finished = true;

        let finish_payload = FinishTaskPayload::default();
        let header = MessageHeader::finish_task(self.task_id.clone());
        let finish_msg = WebSocketMessage {
            header,
            payload: finish_payload,
        };
        let finish_json = serde_json::to_string(&finish_msg)
            .map_err(|e| DashScopeError::JSONSerialize(e.to_string()))?;

        self.ws_sink
            .send(WsMessage::Text(finish_json.into()))
            .await
            .map_err(|e| DashScopeError::WebSocketError(e.to_string()))
    }

    /// 获取当前识别任务的 ID。
    pub fn task_id(&self) -> &str {
        &self.task_id
    }
}

impl Stream for AsrConnection {
    type Item = Result<AsrResponse>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match self.ws_stream.poll_next_unpin(cx) {
                Poll::Ready(Some(Ok(ws_message))) => match ws_message {
                    WsMessage::Text(text) => {
                        // 先把文本消息解析成通用的 Value
                        let raw: serde_json::Value = match serde_json::from_str(&text) {
                            Ok(v) => v,
                            Err(e) => {
                                return Poll::Ready(Some(Err(DashScopeError::JSONDeserialize {
                                    source: e,
                                    raw_response: Bytes::from(text),
                                })));
                            }
                        };

                        // 只反序列化 header 部分
                        let header_val = &raw["header"];
                        let header: IncomingHeader =
                            match serde_json::from_value(header_val.clone()) {
                                Ok(h) => h,
                                Err(e) => {
                                    return Poll::Ready(Some(Err(
                                        DashScopeError::JSONDeserialize {
                                            source: e,
                                            raw_response: Bytes::from(text),
                                        },
                                    )));
                                }
                            };

                        // 检查任务 ID
                        if header.task_id != self.task_id {
                            // 收到不属于当前任务的消息，忽略并继续轮询
                            continue;
                        }

                        // 根据事件类型决定如何处理
                        let response = match header.event_type() {
                            AsrEventType::ResultGenerated => {
                                // 只在真正的结果生成时反序列化 payload.output
                                if let Some(payload_val) =
                                    raw.get("payload").and_then(|p| p.get("output"))
                                {
                                    match serde_json::from_value::<AsrOutput>(payload_val.clone()) {
                                        Ok(output) => AsrResponse::ResultGenerated(output),
                                        Err(e) => {
                                            return Poll::Ready(Some(Err(
                                                DashScopeError::JSONDeserialize {
                                                    source: e,
                                                    raw_response: Bytes::from(text),
                                                },
                                            )));
                                        }
                                    }
                                } else {
                                    // 没有 output 的 ResultGenerated，忽略
                                    continue;
                                }
                            }
                            AsrEventType::TaskStarted => AsrResponse::TaskStarted,
                            AsrEventType::TaskFinished => AsrResponse::TaskFinished,
                            AsrEventType::TaskFailed => AsrResponse::TaskFailed {
                                error_code: header.error_code,
                                error_message: header.error_message,
                            },
                            AsrEventType::Unknown(_) => {
                                // 未知事件，忽略
                                continue;
                            }
                        };
                        return Poll::Ready(Some(Ok(response)));
                    }
                    WsMessage::Ping(_) | WsMessage::Pong(_) => {
                        // 自动处理，继续轮询
                        continue;
                    }
                    WsMessage::Close(_) => return Poll::Ready(None),
                    _ => continue, // 忽略其他消息类型
                },
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(DashScopeError::WebSocketError(e.to_string()))));
                }
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
