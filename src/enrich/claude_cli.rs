//! Claude CLI backend (T1-KTD8): shells out to an installed `claude` in headless
//! print mode, reusing its stored subscription OAuth — the corporate no-API-key
//! path. Guards against running inside a Claude Code session (CLAUDECODE=1).
//!
//! Every call has a time limit (plan A, U12): `verify` makes one call per
//! usage site, and a `claude` that never answers would otherwise hold the run
//! forever with nothing to say why.
//!
//! ## A completion, not an agent
//!
//! `claude -p` is a full Claude Code agent: by default it has its tools, the
//! operator's MCP servers, hooks and settings, and the directory it starts in
//! as its workspace — print mode even skips the workspace trust dialog. The
//! prompt it gets carries text from the repository being read (doc comments
//! for `enrich`, raw source around a usage site for `verify`), and that
//! repository may not be the operator's own: a comment planted beside a call
//! would be an instruction to an agent that can act. Neither task needs
//! anything but an answer, so each call is a text completion and nothing
//! more (see [`ClaudeCliBackend::command`]):
//!
//! - `--tools ""` — no built-in tool at all (`claude --help`: `""` disables
//!   all tools);
//! - `--strict-mcp-config` with no `--mcp-config` — no MCP server;
//! - `--setting-sources ""` — no user, project or local settings, so none of
//!   their hooks, permissions or plugins;
//! - `--no-session-persistence` — nothing of the call saved to resume;
//! - `--permission-prompts none` — anything that would still ask is denied;
//! - started in a fresh, empty temporary directory, never in the repository.
//!
//! Checked against Claude Code 2.1.288 on 03.10.2026: the session's `init`
//! event reports no tools and no MCP servers, and the stored subscription
//! login still answers. Not `--bare`: it limits the login to
//! `ANTHROPIC_API_KEY`, which is the very path this backend exists to avoid.

use crate::enrich::backend::{BackendError, LlmBackend};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long one `claude -p` call may take before it is killed. Generous: an
/// enrich narration from a large model can take minutes, and a call killed
/// halfway is paid for all the same.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// How often a running call is checked.
const POLL: Duration = Duration::from_millis(50);

pub struct ClaudeCliBackend {
    binary: String,
    model: Option<String>,
    timeout: Duration,
}

impl ClaudeCliBackend {
    pub fn new(binary: impl Into<String>, model: Option<String>) -> Self {
        ClaudeCliBackend {
            binary: binary.into(),
            model,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Testable core: `in_session` is injected so the guard can be tested without
    /// mutating the process environment, and `run` is injected for the same reason.
    fn narrate_guarded(
        &self,
        prompt: &str,
        run: impl FnOnce(&str) -> Result<String, BackendError>,
        in_session: bool,
    ) -> Result<String, BackendError> {
        if in_session {
            return Err(BackendError::Unreachable(
                "cannot run `claude` from inside a Claude Code session (CLAUDECODE=1); \
                 run reverse-architect from a normal terminal"
                    .into(),
            ));
        }
        run(prompt)
    }

    /// The call for one prompt, started in `cwd`: a text completion with no
    /// tools, no MCP servers and no settings (module docs). The prompt comes
    /// last, after `-p`: `--tools` takes a list, and the prompt must not be
    /// read as one of its names.
    fn command(&self, prompt: &str, cwd: &Path) -> Command {
        let mut cmd = Command::new(&self.binary);
        cmd.args([
            "--tools",
            "",
            "--strict-mcp-config",
            "--setting-sources",
            "",
            "--no-session-persistence",
            "--permission-prompts",
            "none",
        ]);
        if let Some(model) = &self.model {
            cmd.arg("--model").arg(model);
        }
        cmd.arg("-p").arg(prompt).current_dir(cwd);
        cmd
    }
}

impl LlmBackend for ClaudeCliBackend {
    fn narrate(&self, prompt: &str) -> Result<String, BackendError> {
        let in_session = std::env::var("CLAUDECODE").as_deref() == Ok("1");
        self.narrate_guarded(
            prompt,
            |p| {
                // Removed when it drops, after the call has ended.
                let workspace = tempfile::tempdir().map_err(|e| {
                    BackendError::Unreachable(format!(
                        "could not create an empty working directory for `{}`: {e}",
                        self.binary
                    ))
                })?;
                let cmd = self.command(p, workspace.path());
                let output = run_with_timeout(cmd, self.timeout).map_err(|e| match e {
                    Timed::Spawn(e) => BackendError::Unreachable(format!(
                        "could not launch `{}`: {e}",
                        self.binary
                    )),
                    Timed::TimedOut => BackendError::Unreachable(format!(
                        "`{}` did not answer within the {} s time limit",
                        self.binary,
                        self.timeout.as_secs()
                    )),
                })?;
                if !output.status.success() {
                    return Err(BackendError::Response(
                        String::from_utf8_lossy(&output.stderr).trim().to_string(),
                    ));
                }
                Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
            },
            in_session,
        )
    }

    fn id(&self) -> &'static str {
        "claude-cli"
    }
}

/// Why a timed run produced no output.
#[derive(Debug)]
enum Timed {
    Spawn(std::io::Error),
    TimedOut,
}

/// Run `cmd` to completion, or kill it once `timeout` has passed.
///
/// Spawn + poll, because std has no wait-with-timeout, as `index::run_timed`
/// does. Both pipes are drained on threads while it runs: a child that writes
/// more than a pipe buffer holds — a long narration — would otherwise block on
/// its own output and look like a hang.
fn run_with_timeout(mut cmd: Command, timeout: Duration) -> Result<std::process::Output, Timed> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(Timed::Spawn)?;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            bytes
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let started = Instant::now();
    let status = loop {
        match child.try_wait().map_err(Timed::Spawn)? {
            Some(status) => break status,
            None if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Timed::TimedOut);
            }
            None => std::thread::sleep(POLL.min(timeout.saturating_sub(started.elapsed()))),
        }
    };
    Ok(std::process::Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enrich::backend::LlmBackend;

    #[test]
    fn id_is_claude_cli() {
        let b = ClaudeCliBackend::new("claude", Some("opus".into()));
        assert_eq!(b.id(), "claude-cli");
    }

    /// The call is a text completion: no tools, no MCP servers, no settings,
    /// nothing persisted, no prompt answered, the model kept, the prompt
    /// last — and it starts in the directory it is given, never the
    /// caller's.
    #[test]
    fn a_call_has_no_tools_no_servers_no_settings_and_starts_in_the_given_directory() {
        let b = ClaudeCliBackend::new("claude", Some("opus".into()));
        let cwd = std::env::temp_dir().join("ra-empty");
        let cmd = b.command("Which candidate? c1 or c2", &cwd);
        let args: Vec<&str> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(
            args,
            [
                "--tools",
                "",
                "--strict-mcp-config",
                "--setting-sources",
                "",
                "--no-session-persistence",
                "--permission-prompts",
                "none",
                "--model",
                "opus",
                "-p",
                "Which candidate? c1 or c2",
            ]
        );
        assert_eq!(cmd.get_current_dir(), Some(cwd.as_path()));

        let without_model = ClaudeCliBackend::new("claude", None).command("hi", &cwd);
        assert!(!without_model.get_args().any(|a| a == "--model"));
    }

    #[test]
    fn refuses_to_run_inside_a_claude_code_session() {
        // T1-KTD8 caveat: CLAUDECODE=1 guards subprocess `claude`. The backend
        // detects it and returns a readable error rather than hanging/looping.
        let b = ClaudeCliBackend::new("claude", None);
        let err = b
            .narrate_guarded("hi", |_| unreachable!(), true)
            .unwrap_err();
        assert!(err
            .to_string()
            .to_lowercase()
            .contains("claude code session"));
    }

    /// A command that outlives its limit is killed, and the call returns soon
    /// after the limit rather than when the command would have finished.
    #[test]
    fn a_call_past_its_time_limit_is_killed() {
        let mut cmd = if cfg!(windows) {
            let mut cmd = Command::new("powershell");
            cmd.args(["-NoProfile", "-Command", "Start-Sleep -Seconds 20"]);
            cmd
        } else {
            let mut cmd = Command::new("sleep");
            cmd.arg("20");
            cmd
        };
        cmd.current_dir(std::env::temp_dir());
        let started = Instant::now();
        let result = run_with_timeout(cmd, Duration::from_millis(1500));
        assert!(matches!(result, Err(Timed::TimedOut)), "{result:?}");
        assert!(started.elapsed() < Duration::from_secs(15));
    }

    /// Output larger than a pipe buffer is read while the command runs, so it
    /// finishes instead of blocking on a full pipe until the limit.
    #[test]
    fn a_call_that_writes_a_lot_finishes_within_its_limit() {
        let cmd = if cfg!(windows) {
            let mut cmd = Command::new("powershell");
            cmd.args(["-NoProfile", "-Command", "'x' * 200000"]);
            cmd
        } else {
            let mut cmd = Command::new("sh");
            cmd.args(["-c", "head -c 200000 /dev/zero | tr '\\0' x"]);
            cmd
        };
        let output = run_with_timeout(cmd, Duration::from_secs(60)).expect("it finishes");
        assert!(output.status.success());
        assert!(output.stdout.len() >= 200_000, "{}", output.stdout.len());
    }
}
