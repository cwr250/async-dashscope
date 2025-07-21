use serde::Deserialize;

/// ASR 服务返回的事件类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsrEventType {
    /// 已生成中间或最终识别结果
    ResultGenerated,
    /// 任务已成功开始
    TaskStarted,
    /// 任务已成功结束
    TaskFinished,
    /// 任务失败
    TaskFailed,
    /// 未知事件
    Unknown(String),
}

/// 包含单个词信息的识别结果
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Word {
    /// 词的起始时间（毫秒）
    pub begin_time: Option<i64>,
    /// 词的结束时间（毫秒）
    pub end_time: Option<i64>,
    /// 识别出的词文本
    pub text: String,
}

/// 包含单个句子信息的识别结果
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Sentence {
    /// 句子的起始时间（毫秒）
    pub begin_time: Option<i64>,
    /// 句子的结束时间（毫秒）
    pub end_time: Option<i64>,
    /// 完整的句子文本
    pub text: String,
    /// 句子包含的词列表
    #[serde(default)]
    pub words: Vec<Word>,
}

/// ASR 识别的核心输出内容
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct AsrOutput {
    /// 识别出的句子
    pub sentence: Sentence,
}

/// 从 ASR 服务接收到的完整响应消息
#[derive(Debug, Clone, PartialEq)]
pub enum AsrResponse {
    /// 任务已成功启动
    TaskStarted,
    /// 识别结果已生成
    ResultGenerated(AsrOutput),
    /// 任务已成功完成
    TaskFinished,
    /// 任务失败
    TaskFailed {
        error_code: Option<String>,
        error_message: Option<String>,
    },
}

// 内部使用的反序列化结构

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct IncomingHeader {
    pub event: String,
    pub task_id: String,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

impl IncomingHeader {
    pub(crate) fn event_type(&self) -> AsrEventType {
        match self.event.as_str() {
            "result-generated" => AsrEventType::ResultGenerated,
            "task-started" => AsrEventType::TaskStarted,
            "task-finished" => AsrEventType::TaskFinished,
            "task-failed" => AsrEventType::TaskFailed,
            other => AsrEventType::Unknown(other.to_string()),
        }
    }
}
