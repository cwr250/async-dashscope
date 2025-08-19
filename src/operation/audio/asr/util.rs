use bytes::Bytes;
use serde_json::Value;
use futures_util::Stream;
use tokio_stream::{StreamExt, wrappers::BroadcastStream};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::error::DashScopeError;
use crate::ws::pool::WsResult;
use super::output;

/// 集中封装 ASR WebSocket 文本解析逻辑，减少重复代码和维护成本
///
/// # Arguments
/// * `task_id` - 期望的任务ID
/// * `txt` - WebSocket 接收到的文本消息
///
/// # Returns
/// * `None` - 消息不属于指定的 task_id 或者是无关消息
/// * `Some(Ok(AsrResponse))` - 成功解析的 ASR 响应
/// * `Some(Err(DashScopeError))` - 解析过程中发生的错误
pub(crate) fn parse_asr_ws_text_for_task(
    task_id: &str,
    txt: &str,
) -> Option<Result<output::AsrResponse, DashScopeError>> {
    // 快速路径：先做字符串检查避免不必要的JSON解析
    if !txt.contains(task_id) {
        return None;
    }

    // 解析 JSON
    let v: Value = match serde_json::from_str(txt) {
        Ok(v) => v,
        Err(e) => {
            return Some(Err(DashScopeError::JSONDeserialize {
                source: e,
                raw_response: Bytes::from(txt.to_owned()),
            }));
        }
    };

    // 验证 task_id 是否匹配
    let hid = v
        .get("header")
        .and_then(|h| h.get("task_id"))
        .and_then(|s| s.as_str());

    if hid != Some(task_id) {
        return None;
    }

    // 解析 header
    let header: output::IncomingHeader = match serde_json::from_value(v["header"].clone()) {
        Ok(h) => h,
        Err(e) => {
            return Some(Err(DashScopeError::JSONDeserialize {
                source: e,
                raw_response: Bytes::from(txt.to_owned()),
            }));
        }
    };

    // 根据事件类型构造响应
    let resp = match header.event_type() {
        output::AsrEventType::ResultGenerated => {
            if let Some(out_val) = v.get("payload").and_then(|p| p.get("output")) {
                match serde_json::from_value::<output::AsrOutput>(out_val.clone()) {
                    Ok(o) => output::AsrResponse::ResultGenerated(o),
                    Err(e) => {
                        return Some(Err(DashScopeError::JSONDeserialize {
                            source: e,
                            raw_response: Bytes::from(txt.to_owned()),
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

/// 统一的 ASR 广播流映射函数，减少重复逻辑
///
/// # Arguments
/// * `rx` - WebSocket 广播接收器
/// * `task_id` - 任务ID
///
/// # Returns
/// 过滤并解析后的 ASR 响应流
pub(crate) fn subscribe_asr_stream_for_task(
    rx: tokio::sync::broadcast::Receiver<WsResult>,
    task_id: String,
) -> impl Stream<Item = Result<output::AsrResponse, DashScopeError>> {
    StreamExt::filter_map(BroadcastStream::new(rx), move |ev| {
        let task_id = task_id.clone();
        match ev {
            Ok(Ok(WsMessage::Text(txt))) => {
                parse_asr_ws_text_for_task(&task_id, &txt)
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
