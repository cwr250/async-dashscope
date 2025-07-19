use derive_builder::Builder;
use serde::{Deserialize, Serialize};

use crate::operation::common::{Parameters, StreamOptions};
use crate::operation::request::RequestTrait;

#[derive(Debug, Clone, Builder, Serialize, Deserialize, PartialEq)]
pub struct GenerationParam {
    #[builder(setter(into, strip_option))]
    pub model: String,

    pub input: Input,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[builder(setter(into, strip_option))]
    #[builder(default=None)]
    pub parameters: Option<Parameters>,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[builder(setter(into, strip_option))]
    #[builder(default=Some(false))]
    pub stream: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[builder(setter(into, strip_option))]
    #[builder(default=None)]
    pub stream_options: Option<StreamOptions>,
}

#[derive(Debug, Clone, Builder, Serialize, Deserialize, PartialEq)]
pub struct Input {
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, Builder, Serialize, Deserialize, PartialEq)]
pub struct Message {
    #[builder(setter(into))]
    pub role: String,
    #[builder(setter(into))]
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[builder(setter(into, strip_option))]
    #[builder(default=Some(false))]
    pub partial: Option<bool>,
}

impl Message {
    /// Creates a new `Message`.
    ///
    /// A convenience method for creating a message without the builder pattern.
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
            partial: Some(false),
        }
    }
}

impl RequestTrait for GenerationParam {
    fn model(&self) -> &str {
        &self.model
    }

    fn parameters(&self) -> Option<&Parameters> {
        self.parameters.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn test_generation_param_serialization_skips_none_fields() {
        // Test that None optional fields are not serialized
        let messages = vec![Message::new("user", "test content")];
        let input = InputBuilder::default().messages(messages).build().unwrap();

        let param = GenerationParamBuilder::default()
            .model("qwen-turbo")
            .input(input)
            .build()
            .unwrap();

        let json = serde_json::to_value(&param).unwrap();

        // These fields should not be present when None
        assert!(
            !json.get("parameters").is_some() || json.get("parameters").unwrap().is_null() == false
        );
        assert!(
            !json.get("stream_options").is_some()
                || json.get("stream_options").unwrap().is_null() == false
        );

        // stream field should be present but not null since it has a default value
        if let Some(stream_value) = json.get("stream") {
            assert!(!stream_value.is_null());
        }
    }

    #[test]
    fn test_generation_param_serialization_includes_set_fields() {
        // Test that explicitly set optional fields are serialized
        let messages = vec![Message::new("user", "test content")];
        let input = InputBuilder::default().messages(messages).build().unwrap();

        let param = GenerationParamBuilder::default()
            .model("qwen-turbo")
            .input(input)
            .stream(true)
            .build()
            .unwrap();

        let json = serde_json::to_value(&param).unwrap();

        // stream field should be present and have the correct value
        assert_eq!(json.get("stream").unwrap(), &serde_json::Value::Bool(true));
    }

    #[test]
    fn test_message_partial_field_serialization() {
        // Test that partial field is properly handled
        let message = MessageBuilder::default()
            .role("user")
            .content("test")
            .build()
            .unwrap();

        let json = serde_json::to_value(&message).unwrap();

        // partial field should be present with default value or skipped if None
        if let Some(partial_value) = json.get("partial") {
            assert!(!partial_value.is_null());
        }
    }

    #[test]
    fn test_message_convenience_constructor() {
        let message = Message::new("user", "Hello world");

        assert_eq!(message.role, "user");
        assert_eq!(message.content, "Hello world");
        assert_eq!(message.partial, Some(false));
    }
}
