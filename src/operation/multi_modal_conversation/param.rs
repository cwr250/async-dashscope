use derive_builder::Builder;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{operation::common::Parameters, oss_util};
use crate::{operation::request::RequestTrait, oss_util::is_valid_url};

#[derive(Debug, Clone, Builder, Serialize, Deserialize, PartialEq)]
pub struct MultiModalConversationParam {
    #[builder(setter(into))]
    pub model: String,
    pub input: Input,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[builder(setter(into, strip_option))]
    #[builder(default=None)]
    pub stream: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[builder(setter(into, strip_option))]
    #[builder(default=None)]
    pub parameters: Option<Parameters>,
}

impl MultiModalConversationParam {
    pub(crate) async fn upload_file_to_oss(
        mut self,
        api_key: &str,
    ) -> Result<Self, crate::error::DashScopeError> {
        for message in self.input.messages.iter_mut() {
            for content in message.contents.iter_mut() {
                match content {
                    Element::Image(url) => {
                        if !is_valid_url(url) {
                            let oss_url =
                                oss_util::upload_file_and_get_url(api_key, &self.model, url)
                                    .await?;
                            *content = Element::Image(oss_url);
                        }
                    }
                    Element::Audio(url) => {
                        if !is_valid_url(url) {
                            let oss_url =
                                oss_util::upload_file_and_get_url(api_key, &self.model, url)
                                    .await?;
                            *content = Element::Audio(oss_url);
                        }
                    }
                    Element::Video(url) => {
                        if !is_valid_url(url) {
                            let oss_url =
                                oss_util::upload_file_and_get_url(api_key, &self.model, url)
                                    .await?;
                            *content = Element::Video(oss_url);
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok(self)
    }
}

#[derive(Debug, Clone, Builder, Serialize, Deserialize, PartialEq)]
pub struct Input {
    messages: Vec<Message>,
}

#[derive(Debug, Clone, Builder, Serialize, Deserialize, PartialEq)]
pub struct Message {
    #[builder(setter(into))]
    role: String,
    #[serde(rename = "content")]
    contents: Vec<Element>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Element {
    Image(String),
    Video(String),
    Audio(String),
    Text(String),
}

impl TryFrom<Value> for Element {
    type Error = crate::error::DashScopeError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        // Ensure the value is an object with exactly one key
        let obj = value.as_object().ok_or_else(|| {
            crate::error::DashScopeError::ElementError("Element must be a JSON object".into())
        })?;

        if obj.len() != 1 {
            return Err(crate::error::DashScopeError::ElementError(
                "Element must be a single-key object".into(),
            ));
        }

        // Get the single key-value pair
        let (key, val) = obj.iter().next().unwrap();
        let s = val.as_str().ok_or_else(|| {
            crate::error::DashScopeError::ElementError("Element value must be a string".into())
        })?;

        match key.as_str() {
            "image" => Ok(Element::Image(s.to_string())),
            "video" => Ok(Element::Video(s.to_string())),
            "audio" => Ok(Element::Audio(s.to_string())),
            "text" => Ok(Element::Text(s.to_string())),
            _ => Err(crate::error::DashScopeError::ElementError(format!(
                "Unknown element type: {}",
                key
            ))),
        }
    }
}

impl Message {
    /// Creates a new `Message` with a single content item.
    ///
    /// A convenience method for creating a message without the builder pattern.
    pub fn new(role: impl Into<String>, contents: Vec<Element>) -> Self {
        Self {
            role: role.into(),
            contents,
        }
    }

    pub fn push_content(&mut self, content: Element) {
        self.contents.push(content);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_element_try_from_valid_single_key() {
        // Test valid image element
        let image_json = json!({"image": "https://example.com/image.jpg"});
        let element = Element::try_from(image_json).unwrap();
        match element {
            Element::Image(url) => assert_eq!(url, "https://example.com/image.jpg"),
            _ => panic!("Expected Image element"),
        }

        // Test valid text element
        let text_json = json!({"text": "Hello world"});
        let element = Element::try_from(text_json).unwrap();
        match element {
            Element::Text(text) => assert_eq!(text, "Hello world"),
            _ => panic!("Expected Text element"),
        }

        // Test valid video element
        let video_json = json!({"video": "https://example.com/video.mp4"});
        let element = Element::try_from(video_json).unwrap();
        match element {
            Element::Video(url) => assert_eq!(url, "https://example.com/video.mp4"),
            _ => panic!("Expected Video element"),
        }

        // Test valid audio element
        let audio_json = json!({"audio": "https://example.com/audio.mp3"});
        let element = Element::try_from(audio_json).unwrap();
        match element {
            Element::Audio(url) => assert_eq!(url, "https://example.com/audio.mp3"),
            _ => panic!("Expected Audio element"),
        }
    }

    #[test]
    fn test_element_try_from_rejects_multiple_keys() {
        let multi_key_json = json!({
            "image": "https://example.com/image.jpg",
            "text": "Some text"
        });

        let result = Element::try_from(multi_key_json);
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("single-key object"));
    }

    #[test]
    fn test_element_try_from_rejects_empty_object() {
        let empty_json = json!({});

        let result = Element::try_from(empty_json);
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("single-key object"));
    }

    #[test]
    fn test_element_try_from_rejects_non_object() {
        let array_json = json!(["image", "https://example.com/image.jpg"]);

        let result = Element::try_from(array_json);
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("must be a JSON object"));

        let string_json = json!("just a string");
        let result = Element::try_from(string_json);
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("must be a JSON object"));
    }

    #[test]
    fn test_element_try_from_rejects_non_string_values() {
        let number_value_json = json!({"image": 123});

        let result = Element::try_from(number_value_json);
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("must be a string"));

        let object_value_json = json!({"text": {"content": "hello"}});
        let result = Element::try_from(object_value_json);
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("must be a string"));
    }

    #[test]
    fn test_element_try_from_rejects_unknown_type() {
        let unknown_type_json = json!({"document": "https://example.com/doc.pdf"});

        let result = Element::try_from(unknown_type_json);
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("Unknown element type: document"));
    }
}

impl RequestTrait for MultiModalConversationParam {
    fn model(&self) -> &str {
        &self.model
    }

    fn parameters(&self) -> Option<&Parameters> {
        self.parameters.as_ref()
    }
}
