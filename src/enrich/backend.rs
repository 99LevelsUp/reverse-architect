//! The single Tier 1 contract toward an LLM (T1-KTD7): text-in → text-out.
//! Kept minimal so further backends are purely additive.

use std::cell::Cell;

/// A backend failure — always readable, never a panic (Tier 0 E4 discipline).
#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("LLM backend unreachable: {0}")]
    Unreachable(String),
    #[error("LLM backend returned an error: {0}")]
    Response(String),
}

/// One synchronous narration call. Implementors are single-shot and stateless
/// with respect to prior calls (temperature 0 where the backend supports it).
pub trait LlmBackend {
    fn narrate(&self, prompt: &str) -> Result<String, BackendError>;
    /// Stable id recorded in the narrative's provenance, e.g. "ollama:llama3.1".
    fn id(&self) -> &'static str;
}

/// Test double: returns scripted responses in order, or a scripted failure.
/// The deterministic spine of every Tier 1 test (T1-KTD6).
pub struct FakeBackend {
    responses: Vec<String>,
    cursor: Cell<usize>,
    fail_unreachable: Option<String>,
    fail_response: Option<String>,
}

impl FakeBackend {
    pub fn new(responses: Vec<String>) -> Self {
        FakeBackend {
            responses,
            cursor: Cell::new(0),
            fail_unreachable: None,
            fail_response: None,
        }
    }

    /// A run-fatal (unreachable) failure — the whole enrich run should abort.
    pub fn failing(msg: impl Into<String>) -> Self {
        FakeBackend {
            responses: Vec::new(),
            cursor: Cell::new(0),
            fail_unreachable: Some(msg.into()),
            fail_response: None,
        }
    }

    /// A transient per-call (response) failure — the current leaf should degrade
    /// while the run continues.
    pub fn failing_response(msg: impl Into<String>) -> Self {
        FakeBackend {
            responses: Vec::new(),
            cursor: Cell::new(0),
            fail_unreachable: None,
            fail_response: Some(msg.into()),
        }
    }
}

use crate::enrich::claude_cli::ClaudeCliBackend;
use crate::enrich::openai::OpenAiCompatBackend;

/// Which backend the user selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// OpenAI-compatible client pointed at a local/networked Ollama.
    Ollama,
    /// OpenAI-compatible client pointed at an arbitrary gateway (needs auth).
    OpenAiCompat,
    /// Shell-out to the `claude` CLI.
    ClaudeCli,
}

/// The resolved backend selection (from CLI/config).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendChoice {
    pub kind: BackendKind,
    pub base_url: Option<String>,
    pub model: Option<String>,
    /// Name of the env var holding the auth header value (never the value itself).
    pub auth_env: Option<String>,
}

/// Build a boxed backend from a choice. The auth token is read from the named
/// environment variable only for the OpenAI-compatible backends that send an
/// Authorization header; it is meaningless for the claude-cli backend (which
/// authenticates via the `claude` binary's own session), so pairing `--auth-env`
/// with claude-cli is a configuration error rather than a silent no-op. Missing
/// required fields produce a readable error.
pub fn build_backend(choice: &BackendChoice) -> Result<Box<dyn LlmBackend>, BackendError> {
    match choice.kind {
        BackendKind::Ollama | BackendKind::OpenAiCompat => {
            let auth = match &choice.auth_env {
                Some(var) => Some(format!(
                    "Bearer {}",
                    std::env::var(var).map_err(|_| BackendError::Unreachable(format!(
                        "auth env var `{var}` is not set"
                    )))?
                )),
                None => None,
            };
            let base_url = choice.base_url.clone().ok_or_else(|| {
                BackendError::Unreachable("--base-url is required for this backend".into())
            })?;
            let model = choice.model.clone().ok_or_else(|| {
                BackendError::Unreachable("--model is required for this backend".into())
            })?;
            // Never send a bearer token in cleartext (security). When auth is
            // present, require https:// unless the host is loopback — otherwise an
            // http:// gateway would leak the Authorization header to any on-path
            // observer. Localhost over http is allowed (loopback never hits the wire).
            if auth.is_some() && !transport_is_safe(&base_url) {
                return Err(BackendError::Unreachable(format!(
                    "refusing to send an auth token over an unencrypted connection: \
                     `{base_url}` is neither https nor a loopback host"
                )));
            }
            Ok(Box::new(OpenAiCompatBackend::new(base_url, model, auth)))
        }
        BackendKind::ClaudeCli => {
            // `--auth-env` has no meaning here (the `claude` subprocess uses its
            // own auth), and the parent env would leak into the child regardless,
            // so reject the combination rather than pretend it did something.
            if choice.auth_env.is_some() {
                return Err(BackendError::Unreachable(
                    "--auth-env is not supported with the claude-cli backend".into(),
                ));
            }
            let binary = choice
                .base_url
                .clone()
                .unwrap_or_else(|| "claude".to_string());
            Ok(Box::new(ClaudeCliBackend::new(
                binary,
                choice.model.clone(),
            )))
        }
    }
}

/// True when a base URL is safe to send a bearer token over: an `https` scheme,
/// or `http` to a loopback host (localhost / 127.0.0.1 / ::1) where the bytes
/// never hit the wire. Uses a real URL parser (`url`, already in the tree via
/// reqwest) rather than hand-rolled string splitting — the naive `split(['/',':'])`
/// approach mis-parses userinfo (`http://localhost:x@evil.com` looks like host
/// `localhost` but the real host is `evil.com`) and bracketed IPv6. Conservative:
/// an unparseable or non-https/non-loopback URL is unsafe.
fn transport_is_safe(base_url: &str) -> bool {
    let Ok(u) = url::Url::parse(base_url) else {
        return false;
    };
    match u.scheme() {
        "https" => true,
        "http" => match u.host() {
            Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        },
        _ => false,
    }
}

impl LlmBackend for FakeBackend {
    fn narrate(&self, _prompt: &str) -> Result<String, BackendError> {
        if let Some(msg) = &self.fail_unreachable {
            return Err(BackendError::Unreachable(msg.clone()));
        }
        if let Some(msg) = &self.fail_response {
            return Err(BackendError::Response(msg.clone()));
        }
        let i = self.cursor.get();
        let out = self.responses.get(i).cloned().unwrap_or_default();
        self.cursor.set(i + 1);
        Ok(out)
    }

    fn id(&self) -> &'static str {
        "fake"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_backend_returns_scripted_response_in_order() {
        let backend = FakeBackend::new(vec!["first".into(), "second".into()]);
        assert_eq!(backend.narrate("p1").unwrap(), "first");
        assert_eq!(backend.narrate("p2").unwrap(), "second");
    }

    #[test]
    fn fake_backend_reports_id() {
        let backend = FakeBackend::new(vec![]);
        assert_eq!(backend.id(), "fake");
    }

    #[test]
    fn fake_backend_can_be_scripted_to_fail() {
        let backend = FakeBackend::failing("boom");
        let err = backend.narrate("p").unwrap_err();
        assert!(err.to_string().contains("boom"));
    }

    #[test]
    fn factory_builds_ollama_from_choice() {
        let choice = BackendChoice {
            kind: BackendKind::Ollama,
            base_url: Some("http://localhost:11434".into()),
            model: Some("llama3.1".into()),
            auth_env: None,
        };
        let backend = build_backend(&choice).unwrap();
        assert_eq!(backend.id(), "openai-compat");
    }

    #[test]
    fn factory_errors_when_ollama_has_no_base_url() {
        let choice = BackendChoice {
            kind: BackendKind::Ollama,
            base_url: None,
            model: Some("m".into()),
            auth_env: None,
        };
        assert!(build_backend(&choice).is_err());
    }

    #[test]
    fn factory_reads_auth_from_named_env_var_not_the_value() {
        // The token is read from the env var named by auth_env — never passed inline.
        std::env::set_var("RA_TEST_TOKEN", "secret-value");
        let choice = BackendChoice {
            kind: BackendKind::OpenAiCompat,
            base_url: Some("https://gateway.example".into()),
            model: Some("m".into()),
            auth_env: Some("RA_TEST_TOKEN".into()),
        };
        let backend = build_backend(&choice).unwrap();
        assert_eq!(backend.id(), "openai-compat");
        std::env::remove_var("RA_TEST_TOKEN");
    }

    #[test]
    fn factory_rejects_auth_over_plaintext_http() {
        std::env::set_var("RA_TEST_TOKEN2", "secret");
        let choice = BackendChoice {
            kind: BackendKind::OpenAiCompat,
            base_url: Some("http://gateway.example".into()), // non-loopback + http + auth
            model: Some("m".into()),
            auth_env: Some("RA_TEST_TOKEN2".into()),
        };
        let err = build_backend(&choice).err().unwrap();
        assert!(err.to_string().contains("unencrypted"));
        std::env::remove_var("RA_TEST_TOKEN2");
    }

    #[test]
    fn factory_allows_auth_over_localhost_http() {
        std::env::set_var("RA_TEST_TOKEN3", "secret");
        let choice = BackendChoice {
            kind: BackendKind::OpenAiCompat,
            base_url: Some("http://localhost:8080".into()), // loopback http + auth is fine
            model: Some("m".into()),
            auth_env: Some("RA_TEST_TOKEN3".into()),
        };
        assert!(build_backend(&choice).is_ok());
        std::env::remove_var("RA_TEST_TOKEN3");
    }

    #[test]
    fn factory_rejects_userinfo_host_spoof() {
        // http://localhost:x@evil.com — the real host is evil.com; a naive parser sees
        // "localhost". url::Url resolves the true host, so auth is refused.
        std::env::set_var("RA_TEST_TOKEN4", "secret");
        let choice = BackendChoice {
            kind: BackendKind::OpenAiCompat,
            base_url: Some("http://localhost:x@evil.com".into()),
            model: Some("m".into()),
            auth_env: Some("RA_TEST_TOKEN4".into()),
        };
        assert!(build_backend(&choice).is_err());
        std::env::remove_var("RA_TEST_TOKEN4");
    }

    #[test]
    fn factory_rejects_auth_env_with_claude_cli() {
        // `--auth-env` is meaningless for claude-cli; the combination is a
        // configuration error, not a silent no-op that leaks the parent env.
        let choice = BackendChoice {
            kind: BackendKind::ClaudeCli,
            base_url: None,
            model: Some("claude-x".into()),
            auth_env: Some("RA_TEST_TOKEN_CLI".into()),
        };
        let err = build_backend(&choice).err().unwrap();
        assert!(err.to_string().contains("claude-cli"));
    }

    #[test]
    fn factory_builds_claude_cli_without_auth() {
        let choice = BackendChoice {
            kind: BackendKind::ClaudeCli,
            base_url: None,
            model: None,
            auth_env: None,
        };
        assert!(build_backend(&choice).is_ok());
    }

    #[test]
    fn factory_allows_auth_over_ipv6_loopback_http() {
        std::env::set_var("RA_TEST_TOKEN5", "secret");
        let choice = BackendChoice {
            kind: BackendKind::OpenAiCompat,
            base_url: Some("http://[::1]:8080".into()),
            model: Some("m".into()),
            auth_env: Some("RA_TEST_TOKEN5".into()),
        };
        assert!(build_backend(&choice).is_ok());
        std::env::remove_var("RA_TEST_TOKEN5");
    }
}
