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

impl RequestTrait for MultiModalConversationParam {
    fn model(&self) -> &str {
        &self.model
    }

    fn parameters(&self) -> Option<&Parameters> {
        self.parameters.as_ref()
    }
}
