//! OpenAI-compatible chat backend (T1-KTD8). One client covers local Ollama,
//! a networked company Ollama, and any OpenAI-compatible gateway — they differ
//! only by `base_url` and an optional auth header. Temperature 0.

use crate::enrich::backend::{BackendError, LlmBackend};
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub struct OpenAiCompatBackend {
    base_url: String,
    pub model: String,
    /// Full header value, e.g. "Bearer <token>". Read from an env var by the
    /// factory, never from the command line (spec Security).
    auth: Option<String>,
    client: reqwest::blocking::Client,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    temperature: f32,
    messages: Vec<Message<'a>>,
}

#[derive(Serialize)]
struct Message<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Deserialize)]
struct ResponseMessage {
    content: String,
}

impl OpenAiCompatBackend {
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        auth: Option<String>,
    ) -> Self {
        OpenAiCompatBackend {
            base_url: base_url.into(),
            model: model.into(),
            auth,
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .expect("reqwest client builds with default config"),
        }
    }
}

impl LlmBackend for OpenAiCompatBackend {
    fn narrate(&self, prompt: &str) -> Result<String, BackendError> {
        let url = format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        );
        let body = ChatRequest {
            model: &self.model,
            temperature: 0.0,
            messages: vec![Message {
                role: "user",
                content: prompt,
            }],
        };

        let mut req = self.client.post(&url).json(&body);
        if let Some(auth) = &self.auth {
            req = req.header("Authorization", auth);
        }

        let resp = req
            .send()
            .map_err(|e| BackendError::Unreachable(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            // A persistent client/auth/config error (bad token, wrong model,
            // unknown endpoint) returns the same status on EVERY leaf — so it is
            // run-fatal, not a per-leaf degrade. Classify these as Unreachable so
            // narrate_one aborts the run instead of silently degrading the whole
            // corpus. Transient/server errors (5xx, 429 rate limit) stay Response
            // and degrade just this leaf.
            if matches!(status.as_u16(), 401 | 403 | 404) {
                return Err(BackendError::Unreachable(format!(
                    "HTTP {status} — client/auth/config error (token, model, or endpoint)"
                )));
            }
            return Err(BackendError::Response(format!("HTTP {status}")));
        }
        let parsed: ChatResponse = resp
            .json()
            .map_err(|e| BackendError::Response(format!("malformed response: {e}")))?;
        parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .ok_or_else(|| BackendError::Response("no choices in response".into()))
    }

    fn id(&self) -> &'static str {
        "openai-compat"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enrich::backend::LlmBackend;

    #[test]
    fn id_is_the_backend_family_without_model() {
        let b = OpenAiCompatBackend::new("http://localhost:11434", "llama3.1", None);
        assert_eq!(b.id(), "openai-compat");
    }

    #[test]
    fn model_is_stored_verbatim() {
        let b = OpenAiCompatBackend::new("http://localhost:11434", "llama3.1", None);
        assert_eq!(b.model, "llama3.1");
    }
}
