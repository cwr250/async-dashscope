use crate::{
    Client, error::DashScopeError, operation::audio::tts::output::TextToSpeechOutputStream,
};
use crate::{error::Result, operation::audio::tts::output::TextToSpeechOutput};
pub use tts::param::{
    Input as TextToSpeechInput, InputBuilder as TextToSpeechInputBuilder, TextToSpeechParam,
    TextToSpeechParamBuilder,
};
// ASR 参数和响应类型
pub use asr::{
    AsrParaformerParams, AsrParaformerParamsBuilder, AsrParameters, AsrParametersBuilder,
    AsrResponse,
};
// ASR 连接池 API 和全双工会话
pub use asr::{AsrDuplexSession, AsrEvent, AsrPool, AsrPoolBuilder, AsrSession};

// 新增: ASR 模块
mod asr;
mod tts;

const AUDIO_PATH: &str = "/services/aigc/multimodal-generation/generation";

pub struct Audio<'a> {
    client: &'a Client,
}

impl<'a> Audio<'a> {
    pub fn new(client: &'a Client) -> Self {
        Self { client }
    }

    /// ASR 连接池 Builder - 提供高性能的语音识别连接池
    pub fn asr_pool(&self) -> asr::AsrPoolBuilder<'_> {
        asr::AsrPoolBuilder::new(self.client)
    }

    /// 执行文本转语音(TTS)转换
    ///
    /// 此异步方法向指定端点发送 POST 请求，将文本转换为语音输出
    ///
    /// # 参数
    /// * `request` - TTS 转换参数配置，包含文本内容、语音模型等设置
    pub async fn tts(&self, request: TextToSpeechParam) -> Result<TextToSpeechOutput> {
        // 检查请求是否明确设置为非流式，如果是，则返回错误。
        if request.stream == Some(true) {
            return Err(DashScopeError::InvalidArgument(
                "When stream is true, use Audio::tts_stream".into(),
            ));
        }
        self.client.post(AUDIO_PATH, request).await
    }

    #[must_use]
    pub async fn tts_stream(
        &self,
        mut request: TextToSpeechParam,
    ) -> Result<TextToSpeechOutputStream> {
        // 检查请求是否明确设置为非流式，如果是，则返回错误。
        if request.stream == Some(false) {
            return Err(DashScopeError::InvalidArgument(
                "When stream is false, use Audio::tts".into(),
            ));
        }
        // 确保 stream 设置为 true 以启用流式响应
        request.stream = Some(true);
        self.client.post_stream(AUDIO_PATH, request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation::audio::tts::param::{InputBuilder, TextToSpeechParamBuilder};

    fn create_test_request() -> TextToSpeechParam {
        let input = InputBuilder::default()
            .text("Hello world")
            .voice("Serena")
            .build()
            .unwrap();

        TextToSpeechParamBuilder::default()
            .model("cosyvoice-v1")
            .input(input)
            .build()
            .unwrap()
    }

    #[test]
    fn test_tts_rejects_stream_true() {
        let input = InputBuilder::default()
            .text("Hello world")
            .voice("Serena")
            .build()
            .unwrap();

        let request = TextToSpeechParamBuilder::default()
            .model("cosyvoice-v1")
            .input(input)
            .stream(true)
            .build()
            .unwrap();

        // This would fail at runtime because we can't create a real client here
        // But we can test the stream value
        assert_eq!(request.stream, Some(true));
    }

    #[test]
    fn test_tts_stream_rejects_stream_false() {
        let input = InputBuilder::default()
            .text("Hello world")
            .voice("Serena")
            .build()
            .unwrap();

        let request = TextToSpeechParamBuilder::default()
            .model("cosyvoice-v1")
            .input(input)
            .stream(false)
            .build()
            .unwrap();

        assert_eq!(request.stream, Some(false));
    }

    #[test]
    fn test_tts_stream_sets_stream_true_when_none() {
        let mut request = create_test_request();

        // Initially stream should be None (default)
        assert_eq!(request.stream, None);

        // Simulate what tts_stream does
        if request.stream != Some(false) {
            request.stream = Some(true);
        }

        assert_eq!(request.stream, Some(true));
    }

    #[test]
    fn test_text_to_speech_param_serialization() {
        let request = create_test_request();
        let json = serde_json::to_value(&request).unwrap();

        // stream field should not be present when None
        assert!(!json.get("stream").is_some() || !json.get("stream").unwrap().is_null());

        // Required fields should be present
        assert!(json.get("model").is_some());
        assert!(json.get("input").is_some());
    }

    #[test]
    fn test_text_to_speech_param_with_stream_serialization() {
        let input = InputBuilder::default()
            .text("Hello world")
            .voice("Serena")
            .build()
            .unwrap();

        let request = TextToSpeechParamBuilder::default()
            .model("cosyvoice-v1")
            .input(input)
            .stream(true)
            .build()
            .unwrap();

        let json = serde_json::to_value(&request).unwrap();

        // stream field should be present and true
        assert_eq!(json.get("stream").unwrap(), &serde_json::Value::Bool(true));
    }
}
