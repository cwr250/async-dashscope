use derive_builder::Builder;
use serde::Serialize;
use std::collections::HashMap;

/// ASR 任务的核心参数
/// 控制 ASR 引擎如何处理音频输入，定义了音频格式、语言和各种处理选项。
#[derive(Debug, Clone, Default, Builder, Serialize, PartialEq)]
#[builder(setter(into, strip_option), build_fn(validate = "Self::validate"))]
pub struct AsrParameters {
    /// 音频格式 (例如, "opus", "wav", "pcm")。对于实时识别，推荐 "opus"。
    #[builder(default = r#""opus".to_string()"#)]
    pub format: String,

    /// 采样率 (例如, 16000, 8000)。
    #[builder(default = "16000")]
    pub sample_rate: u32,

    /// (可选) 热词词表ID，用于提升特定词汇的识别准确率。
    #[builder(default)]
    pub vocabulary_id: Option<String>,

    /// (可选) 是否开启去除口语“嗯”、“啊”等冗余词的功能。
    #[builder(default)]
    pub disfluency_removal_enabled: Option<bool>,

    /// (可选) 语言提示，用于辅助模型在多语言场景下进行识别。
    #[builder(default, setter(each(name = "language_hint", into)))]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub language_hints: Vec<String>,

    /// (可选) 是否在识别结果中增加智能标点。
    #[builder(default)]
    pub semantic_punctuation_enabled: Option<bool>,
}

impl AsrParametersBuilder {
    fn validate(&self) -> Result<(), String> {
        if let Some(format) = &self.format {
            if format.is_empty() {
                return Err("format 不能为空".to_string());
            }
        }
        if let Some(rate) = &self.sample_rate {
            if *rate == 0 {
                return Err("sample_rate 必须大于 0".to_string());
            }
        }
        Ok(())
    }
}

/// 启动 ASR 任务的完整请求参数
/// 这是启动一个新的 ASR 识别会话时发送的顶层结构。
#[derive(Debug, Clone, Builder, Serialize, PartialEq)]
#[builder(setter(into, strip_option))]
pub struct AsrParaformerParams {
    /// 要使用的 ASR 模型 (例如, "paraformer-realtime-v2")。
    pub model: String,
    /// ASR 核心处理参数。
    #[builder(default)]
    pub parameters: AsrParameters,
}

// 内部使用的序列化结构

#[derive(Debug, Serialize)]
pub(crate) struct RunTaskPayload<'a> {
    pub task_group: &'static str,
    pub task: &'static str,
    pub function: &'static str,
    pub model: &'a str,
    pub parameters: &'a AsrParameters,
    pub input: HashMap<String, serde_json::Value>,
}

impl<'a> RunTaskPayload<'a> {
    pub(crate) fn new(model: &'a str, parameters: &'a AsrParameters) -> Self {
        Self {
            task_group: "audio",
            task: "asr",
            function: "recognition",
            model,
            parameters,
            input: HashMap::new(),
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct FinishTaskPayload {
    pub input: HashMap<String, serde_json::Value>,
}

impl Default for FinishTaskPayload {
    fn default() -> Self {
        Self {
            input: HashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_task_payload_serialization_includes_empty_input() {
        let parameters = AsrParametersBuilder::default().build().unwrap();
        let payload = RunTaskPayload::new("paraformer-realtime-v2", &parameters);

        let json = serde_json::to_string(&payload).unwrap();

        // 验证即使 input 是空的，也会被序列化为 "input": {}
        assert!(json.contains("\"input\":{}"));

        // 验证完整的 JSON 结构
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.get("input").is_some());
        assert!(parsed["input"].is_object());
        assert!(parsed["input"].as_object().unwrap().is_empty());
    }

    #[test]
    fn test_finish_task_payload_serialization_includes_empty_input() {
        let payload = FinishTaskPayload::default();

        let json = serde_json::to_string(&payload).unwrap();

        // 验证即使 input 是空的，也会被序列化为 "input": {}
        assert!(json.contains("\"input\":{}"));

        // 验证完整的 JSON 结构
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.get("input").is_some());
        assert!(parsed["input"].is_object());
        assert!(parsed["input"].as_object().unwrap().is_empty());
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct MessageHeader {
    pub action: String,
    pub task_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub streaming: Option<String>,
}

impl MessageHeader {
    pub(crate) fn run_task(task_id: String) -> Self {
        Self {
            action: "run-task".to_string(),
            task_id,
            streaming: Some("duplex".to_string()),
        }
    }

    pub(crate) fn finish_task(task_id: String) -> Self {
        Self {
            action: "finish-task".to_string(),
            task_id,
            streaming: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct WebSocketMessage<T> {
    pub header: MessageHeader,
    pub payload: T,
}
