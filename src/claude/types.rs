//! Serde models for the Messages API. Only the subset notebender uses.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

/// A content block inside a message (request or response).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    Image {
        source: Source,
    },
    Document {
        source: Source,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    /// Thinking blocks come back on responses; we keep them opaque.
    Thinking {
        #[serde(default)]
        thinking: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    RedactedThinking {
        #[serde(default)]
        data: String,
    },
    /// Emitted when a server-side fallback switched models.
    Fallback {
        #[serde(default)]
        from: Value,
        #[serde(default)]
        to: Value,
    },
    #[serde(other)]
    Other,
}

impl ContentBlock {
    pub fn text(s: impl Into<String>) -> Self {
        ContentBlock::Text {
            text: s.into(),
            cache_control: None,
        }
    }

    pub fn image_png(base64_data: impl Into<String>) -> Self {
        ContentBlock::Image {
            source: Source::Base64 {
                media_type: "image/png".into(),
                data: base64_data.into(),
            },
        }
    }

    pub fn pdf(base64_data: impl Into<String>, title: Option<String>) -> Self {
        ContentBlock::Document {
            source: Source::Base64 {
                media_type: "application/pdf".into(),
                data: base64_data.into(),
            },
            title,
            cache_control: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Source {
    Base64 { media_type: String, data: String },
    Text { media_type: String, data: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub kind: String,
}

impl CacheControl {
    pub fn ephemeral() -> Self {
        CacheControl {
            kind: "ephemeral".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn user(content: Vec<ContentBlock>) -> Self {
        Message {
            role: Role::User,
            content,
        }
    }
    pub fn user_text(text: impl Into<String>) -> Self {
        Message::user(vec![ContentBlock::text(text)])
    }
}

/// System prompt block (supports cache_control on the last block).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SystemBlock {
    #[serde(rename = "type")]
    pub kind: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

impl SystemBlock {
    pub fn text(text: impl Into<String>) -> Self {
        SystemBlock {
            kind: "text".into(),
            text: text.into(),
            cache_control: None,
        }
    }
    pub fn cached(text: impl Into<String>) -> Self {
        SystemBlock {
            kind: "text".into(),
            text: text.into(),
            cache_control: Some(CacheControl::ephemeral()),
        }
    }
}

/// `thinking` request parameter. Which variant is valid depends on the model (see
/// [`crate::models::Tier::uses_thinking_budget`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Thinking {
    Adaptive {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<String>,
    },
    Enabled {
        budget_tokens: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct OutputConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<crate::models::Effort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<OutputFormat>,
}

/// Structured outputs: `{"type":"json_schema","schema":{...}}`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputFormat {
    JsonSchema { schema: Value },
}

/// Request body for `POST /v1/messages`. `stream` is always set by the client.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessagesRequest {
    pub model: String,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub system: Vec<SystemBlock>,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Thinking>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<OutputConfig>,
    /// `"default"` for server-side refusal fallbacks (needs beta header `server-side-fallback-2026-07-01`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallbacks: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
}

impl Usage {
    pub fn add(&mut self, other: &Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_creation_input_tokens += other.cache_creation_input_tokens;
        self.cache_read_input_tokens += other.cache_read_input_tokens;
    }

    /// Total billable-ish input tokens (cache reads are cheaper, but this is an estimate).
    pub fn total_input(&self) -> u64 {
        self.input_tokens + self.cache_creation_input_tokens + self.cache_read_input_tokens
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct StopDetails {
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub explanation: Option<String>,
}

/// A fully assembled response (the client reassembles it from the SSE stream).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct MessagesResponse {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub stop_details: Option<StopDetails>,
    #[serde(default)]
    pub usage: Usage,
}

impl MessagesResponse {
    /// Concatenated text of all text blocks.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for b in &self.content {
            if let ContentBlock::Text { text, .. } = b {
                out.push_str(text);
            }
        }
        out
    }

    pub fn is_refusal(&self) -> bool {
        self.stop_reason.as_deref() == Some("refusal")
    }

    pub fn hit_max_tokens(&self) -> bool {
        self.stop_reason.as_deref() == Some("max_tokens")
    }
}

/// Error payload returned by the API on non-2xx responses.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiErrorBody {
    #[serde(default)]
    pub error: ApiErrorDetail,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ApiErrorDetail {
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub message: String,
}
