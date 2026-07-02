//! Universal external agent session detection.
//!
//! Discovers Claude Code and Codex sessions that were NOT started inside a
//! Gr8R pane by tailing the transcript files both CLIs write on every turn:
//!
//! - Claude Code: `~/.claude/projects/<encoded-cwd>/<session-uuid>.jsonl`
//! - Codex:      `~/.codex/sessions/<yyyy>/<mm>/<dd>/rollout-*.jsonl`
//!
//! Because the CLIs write these regardless of where they run (plain terminal,
//! tmux, VS Code, SSH), scanning them detects every session on the machine.
//! State is derived from transcript contents plus write-recency, modeled on
//! agentflow-live's watcher heuristics.
//!
//! A background thread scans on an interval and pushes a snapshot into the
//! main loop through `AppEvent::ExternalAgentsUpdated` whenever it changes.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::detect::{Agent, AgentState};
use crate::events::AppEvent;

/// Sessions with no transcript writes for longer than this are dropped.
const ACTIVE_WINDOW: Duration = Duration::from_secs(10 * 60);
/// An in-turn session with no writes for longer than this is shown idle
/// (abandoned mid-turn, crashed, or the process was killed).
const STALE_TURN_WINDOW: Duration = Duration::from_secs(5 * 60);
/// A pending tool call quiet for at least this long is shown blocked
/// (permission prompt or long-running tool needing attention).
const PENDING_TOOL_BLOCKED_AFTER: Duration = Duration::from_secs(10);
/// Scan cadence for the background thread.
const SCAN_INTERVAL: Duration = Duration::from_secs(2);
/// How much of the head of a transcript to inspect for metadata.
const HEAD_READ_BYTES: usize = 64 * 1024;
/// How much of the tail of a transcript to inspect for state.
const TAIL_READ_BYTES: u64 = 256 * 1024;

/// Environment variable that disables the external scanner when set to `0`,
/// `false`, or `off`.
pub const EXTERNAL_AGENTS_ENV_VAR: &str = "GR8R_EXTERNAL_AGENTS";

/// One externally-running agent session discovered from its transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalAgentSnapshot {
    /// Claude session uuid or Codex thread id.
    pub session_id: String,
    pub agent: Agent,
    pub state: AgentState,
    pub cwd: Option<PathBuf>,
    pub transcript_path: PathBuf,
}

impl ExternalAgentSnapshot {
    /// Short human label for the session's working directory.
    pub fn cwd_label(&self) -> String {
        self.cwd
            .as_deref()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "external".to_string())
    }
}

/// Whether the external scanner should run. Defaults to on in release
/// builds and off in debug builds — like the other background threads —
/// so unit and integration tests never race against snapshot events from
/// the developer's real `~/.claude` / `~/.codex` trees. The env var
/// overrides the default in both directions.
pub fn external_detection_enabled() -> bool {
    match std::env::var(EXTERNAL_AGENTS_ENV_VAR) {
        Ok(value) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        ),
        Err(_) => !cfg!(debug_assertions),
    }
}

/// Spawn the background scanner thread. Sends a snapshot only when it
/// differs from the previous one, so an unchanged system stays quiet.
pub fn spawn_scanner(event_tx: tokio::sync::mpsc::Sender<AppEvent>) {
    std::thread::spawn(move || {
        let Some(home) = home_dir() else {
            return;
        };
        let mut previous: Option<Vec<ExternalAgentSnapshot>> = None;
        loop {
            let agents = scan_external_sessions(&home, SystemTime::now());
            if previous.as_ref() != Some(&agents) {
                previous = Some(agents.clone());
                if event_tx
                    .blocking_send(AppEvent::ExternalAgentsUpdated { agents })
                    .is_err()
                {
                    return;
                }
            }
            std::thread::sleep(SCAN_INTERVAL);
        }
    });
}

fn home_dir() -> Option<PathBuf> {
    #[allow(deprecated)]
    std::env::home_dir()
}

/// Scan both CLI transcript trees rooted under `home`.
pub fn scan_external_sessions(home: &Path, now: SystemTime) -> Vec<ExternalAgentSnapshot> {
    let mut agents = Vec::new();
    agents.extend(scan_claude_sessions(
        &home.join(".claude").join("projects"),
        now,
    ));
    agents.extend(scan_codex_sessions(
        &home.join(".codex").join("sessions"),
        now,
    ));
    agents.sort_by(|a, b| {
        crate::detect::agent_label(a.agent)
            .cmp(crate::detect::agent_label(b.agent))
            .then_with(|| a.cwd.cmp(&b.cwd))
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    agents
}

// ---------------------------------------------------------------------------
// Claude Code
// ---------------------------------------------------------------------------

fn scan_claude_sessions(projects_root: &Path, now: SystemTime) -> Vec<ExternalAgentSnapshot> {
    let mut out = Vec::new();
    let Ok(project_dirs) = std::fs::read_dir(projects_root) else {
        return out;
    };
    for project_dir in project_dirs.flatten() {
        if !project_dir.file_type().is_ok_and(|ty| ty.is_dir()) {
            continue;
        }
        let Ok(files) = std::fs::read_dir(project_dir.path()) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            // Subagent/sidechain transcripts are not top-level sessions.
            if stem.starts_with("agent-") {
                continue;
            }
            let Some(age) = transcript_age(&file, now) else {
                continue;
            };
            if age > ACTIVE_WINDOW {
                continue;
            }
            let head = read_head(&path, HEAD_READ_BYTES);
            if head_marks_sidechain(&head) {
                continue;
            }
            let state = claude_state_from_tail(&read_tail(&path, TAIL_READ_BYTES), age);
            out.push(ExternalAgentSnapshot {
                session_id: stem.to_string(),
                agent: Agent::Claude,
                state,
                cwd: extract_json_string(&head, "cwd").map(PathBuf::from),
                transcript_path: path,
            });
        }
    }
    out
}

fn head_marks_sidechain(head: &str) -> bool {
    for line in head.lines().take(20) {
        if line.contains("\"isSidechain\":true") {
            return true;
        }
        if line.contains("\"isSidechain\":false") {
            return false;
        }
    }
    false
}

/// Derive Claude session state from the transcript tail.
///
/// The turn structure is: user prompt -> assistant (tool_use) -> user
/// (tool_result) -> ... -> assistant (text only, turn ends). So:
/// - last conversational entry is a text-only assistant message: turn is
///   over, the session sits at the prompt -> Idle.
/// - last entry is an assistant message with a `tool_use` block: a tool is
///   pending. Quiet beyond a short threshold means a permission prompt or a
///   long tool run -> Blocked; otherwise Working.
/// - last entry is a user message (prompt or tool_result): Claude is
///   thinking/streaming -> Working.
/// - anything mid-turn but quiet past the stale window -> Idle.
fn claude_state_from_tail(tail: &str, age: Duration) -> AgentState {
    #[derive(Clone, Copy, PartialEq)]
    enum LastEntry {
        AssistantToolUse,
        AssistantText,
        User,
    }

    let mut last = None;
    for line in tail.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match value.get("type").and_then(|ty| ty.as_str()) {
            Some("assistant") => {
                let has_tool_use = value
                    .get("message")
                    .and_then(|message| message.get("content"))
                    .and_then(|content| content.as_array())
                    .is_some_and(|blocks| {
                        blocks.iter().any(|block| {
                            block.get("type").and_then(|ty| ty.as_str()) == Some("tool_use")
                        })
                    });
                last = Some(if has_tool_use {
                    LastEntry::AssistantToolUse
                } else {
                    LastEntry::AssistantText
                });
            }
            Some("user") => last = Some(LastEntry::User),
            _ => {}
        }
    }

    match last {
        Some(LastEntry::AssistantText) => AgentState::Idle,
        Some(LastEntry::AssistantToolUse) => {
            if age >= STALE_TURN_WINDOW {
                AgentState::Idle
            } else if age >= PENDING_TOOL_BLOCKED_AFTER {
                AgentState::Blocked
            } else {
                AgentState::Working
            }
        }
        Some(LastEntry::User) => {
            if age >= STALE_TURN_WINDOW {
                AgentState::Idle
            } else {
                AgentState::Working
            }
        }
        None => AgentState::Unknown,
    }
}

// ---------------------------------------------------------------------------
// Codex
// ---------------------------------------------------------------------------

fn scan_codex_sessions(sessions_root: &Path, now: SystemTime) -> Vec<ExternalAgentSnapshot> {
    let mut out = Vec::new();
    walk_codex_dir(sessions_root, now, 0, &mut out);
    out
}

fn walk_codex_dir(dir: &Path, now: SystemTime, depth: usize, out: &mut Vec<ExternalAgentSnapshot>) {
    // Layout is sessions/<yyyy>/<mm>/<dd>/rollout-*.jsonl; keep a hard depth
    // cap so a corrupt tree cannot recurse unbounded.
    if depth > 4 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            walk_codex_dir(&path, now, depth + 1, out);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(age) = transcript_age(&entry, now) else {
            continue;
        };
        if age > ACTIVE_WINDOW {
            continue;
        }
        let head = read_head(&path, HEAD_READ_BYTES);
        // Sub-agent threads are children of a root session; skip them.
        if head.contains("\"parent_thread_id\":\"") || head.contains("\"subagent\"") {
            continue;
        }
        let Some(session_id) =
            extract_json_string(&head, "id").or_else(|| extract_json_string(&head, "session_id"))
        else {
            continue;
        };
        let state = codex_state_from_tail(&read_tail(&path, TAIL_READ_BYTES), age);
        out.push(ExternalAgentSnapshot {
            session_id,
            agent: Agent::Codex,
            state,
            cwd: extract_json_string(&head, "cwd").map(PathBuf::from),
            transcript_path: path,
        });
    }
}

/// Derive Codex session state from the transcript tail.
///
/// Codex writes explicit lifecycle events: `task_started` opens a turn and
/// `task_complete` / `turn_aborted` close it. Approval requests surface as
/// `*approval_request` events.
fn codex_state_from_tail(tail: &str, age: Duration) -> AgentState {
    let mut state = AgentState::Unknown;
    for line in tail.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let payload_type = value
            .get("payload")
            .and_then(|payload| payload.get("type"))
            .and_then(|ty| ty.as_str());
        match payload_type {
            Some("task_complete") | Some("turn_aborted") => state = AgentState::Idle,
            Some("task_started") => state = AgentState::Working,
            Some(ty) if ty.ends_with("approval_request") => state = AgentState::Blocked,
            Some(_) => {
                if matches!(state, AgentState::Unknown) {
                    state = AgentState::Working;
                }
            }
            None => {}
        }
    }
    if matches!(state, AgentState::Working) && age >= STALE_TURN_WINDOW {
        return AgentState::Idle;
    }
    state
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn transcript_age(entry: &std::fs::DirEntry, now: SystemTime) -> Option<Duration> {
    let modified = entry.metadata().ok()?.modified().ok()?;
    Some(now.duration_since(modified).unwrap_or(Duration::ZERO))
}

fn read_head(path: &Path, limit: usize) -> String {
    use std::io::Read;
    let Ok(file) = std::fs::File::open(path) else {
        return String::new();
    };
    let mut buf = Vec::with_capacity(limit.min(8192));
    let _ = file.take(limit as u64).read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

fn read_tail(path: &Path, limit: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let start = len.saturating_sub(limit);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut buf = Vec::new();
    let _ = file.read_to_end(&mut buf);
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if start > 0 {
        // Drop the first (likely partial) line.
        if let Some(newline) = text.find('\n') {
            text.drain(..=newline);
        }
    }
    text
}

/// Extract the first `"key":"value"` string for `key` from raw JSONL text.
/// Fast-path metadata sniffing that avoids parsing megabyte-scale lines.
fn extract_json_string(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":\"");
    let start = text.find(&needle)? + needle.len();
    let rest = &text[start..];
    let mut value = String::new();
    let mut chars = rest.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => return Some(value),
            '\\' => {
                let escaped = chars.next()?;
                match escaped {
                    'n' => value.push('\n'),
                    't' => value.push('\t'),
                    'u' => {
                        // Keep it simple: skip the 4 hex digits.
                        for _ in 0..4 {
                            chars.next()?;
                        }
                        value.push('\u{FFFD}');
                    }
                    other => value.push(other),
                }
            }
            other => value.push(other),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRESH: Duration = Duration::from_secs(2);
    const QUIET: Duration = Duration::from_secs(30);
    const STALE: Duration = Duration::from_secs(6 * 60);

    fn claude_line(entry_type: &str, content_blocks: &str) -> String {
        format!(
            r#"{{"type":"{entry_type}","cwd":"/tmp/project","sessionId":"s","message":{{"role":"{entry_type}","content":[{content_blocks}]}}}}"#
        )
    }

    #[test]
    fn claude_text_only_assistant_is_idle() {
        let tail = [
            claude_line("user", r#"{"type":"text","text":"do it"}"#),
            claude_line("assistant", r#"{"type":"text","text":"done"}"#),
        ]
        .join("\n");
        assert_eq!(claude_state_from_tail(&tail, FRESH), AgentState::Idle);
        assert_eq!(claude_state_from_tail(&tail, STALE), AgentState::Idle);
    }

    #[test]
    fn claude_pending_tool_use_is_working_then_blocked() {
        let tail = [
            claude_line("user", r#"{"type":"text","text":"do it"}"#),
            claude_line(
                "assistant",
                r#"{"type":"tool_use","id":"t1","name":"Bash"}"#,
            ),
        ]
        .join("\n");
        assert_eq!(claude_state_from_tail(&tail, FRESH), AgentState::Working);
        assert_eq!(claude_state_from_tail(&tail, QUIET), AgentState::Blocked);
        assert_eq!(claude_state_from_tail(&tail, STALE), AgentState::Idle);
    }

    #[test]
    fn claude_tool_result_means_working() {
        let tail = [
            claude_line(
                "assistant",
                r#"{"type":"tool_use","id":"t1","name":"Bash"}"#,
            ),
            claude_line("user", r#"{"type":"tool_result","tool_use_id":"t1"}"#),
        ]
        .join("\n");
        assert_eq!(claude_state_from_tail(&tail, FRESH), AgentState::Working);
        assert_eq!(claude_state_from_tail(&tail, STALE), AgentState::Idle);
    }

    #[test]
    fn claude_non_conversation_trailers_are_ignored() {
        let tail = [
            claude_line("assistant", r#"{"type":"text","text":"done"}"#),
            r#"{"type":"file-history-snapshot","messageId":"m"}"#.to_string(),
            r#"{"type":"last-prompt","sessionId":"s"}"#.to_string(),
        ]
        .join("\n");
        assert_eq!(claude_state_from_tail(&tail, FRESH), AgentState::Idle);
    }

    #[test]
    fn codex_lifecycle_events_map_to_states() {
        let started = r#"{"type":"event_msg","payload":{"type":"task_started"}}"#;
        let message = r#"{"type":"event_msg","payload":{"type":"agent_message"}}"#;
        let complete = r#"{"type":"event_msg","payload":{"type":"task_complete"}}"#;
        let approval = r#"{"type":"event_msg","payload":{"type":"exec_approval_request"}}"#;

        let working = format!("{started}\n{message}");
        assert_eq!(codex_state_from_tail(&working, FRESH), AgentState::Working);
        assert_eq!(codex_state_from_tail(&working, STALE), AgentState::Idle);

        let done = format!("{started}\n{message}\n{complete}");
        assert_eq!(codex_state_from_tail(&done, FRESH), AgentState::Idle);

        let blocked = format!("{started}\n{approval}");
        assert_eq!(codex_state_from_tail(&blocked, FRESH), AgentState::Blocked);
    }

    #[test]
    fn scans_claude_and_codex_trees_end_to_end() {
        let root = tempfile::tempdir().expect("tempdir");
        let home = root.path();

        let claude_dir = home.join(".claude/projects/-tmp-project");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(
            claude_dir.join("11111111-2222-3333-4444-555555555555.jsonl"),
            [
                claude_line("user", r#"{"type":"text","text":"hello"}"#),
                claude_line("assistant", r#"{"type":"text","text":"hi"}"#),
            ]
            .join("\n"),
        )
        .unwrap();
        // A sidechain transcript must be skipped.
        std::fs::write(
            claude_dir.join("99999999-2222-3333-4444-555555555555.jsonl"),
            r#"{"type":"user","isSidechain":true,"cwd":"/tmp/project","message":{"role":"user","content":[]}}"#,
        )
        .unwrap();

        let codex_dir = home.join(".codex/sessions/2026/07/02");
        std::fs::create_dir_all(&codex_dir).unwrap();
        std::fs::write(
            codex_dir.join("rollout-2026-07-02T10-00-00-abc.jsonl"),
            [
                r#"{"type":"session_meta","payload":{"id":"thread-1","cwd":"/tmp/other"}}"#,
                r#"{"type":"event_msg","payload":{"type":"task_started"}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let agents = scan_external_sessions(home, SystemTime::now());
        assert_eq!(agents.len(), 2);

        let claude = agents
            .iter()
            .find(|agent| agent.agent == Agent::Claude)
            .expect("claude entry");
        assert_eq!(claude.session_id, "11111111-2222-3333-4444-555555555555");
        assert_eq!(claude.state, AgentState::Idle);
        assert_eq!(claude.cwd.as_deref(), Some(Path::new("/tmp/project")));

        let codex = agents
            .iter()
            .find(|agent| agent.agent == Agent::Codex)
            .expect("codex entry");
        assert_eq!(codex.session_id, "thread-1");
        assert_eq!(codex.state, AgentState::Working);
        assert_eq!(codex.cwd.as_deref(), Some(Path::new("/tmp/other")));
    }

    #[test]
    fn stale_transcripts_are_dropped() {
        let root = tempfile::tempdir().expect("tempdir");
        let home = root.path();
        let claude_dir = home.join(".claude/projects/-tmp-project");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(
            claude_dir.join("11111111-2222-3333-4444-555555555555.jsonl"),
            claude_line("assistant", r#"{"type":"text","text":"hi"}"#),
        )
        .unwrap();

        let future = SystemTime::now() + ACTIVE_WINDOW + Duration::from_secs(60);
        assert!(scan_external_sessions(home, future).is_empty());
    }

    #[test]
    fn extract_json_string_handles_escapes() {
        let text = r#"{"cwd":"/tmp/with \"quotes\" dir","id":"abc"}"#;
        assert_eq!(
            extract_json_string(text, "cwd").as_deref(),
            Some(r#"/tmp/with "quotes" dir"#)
        );
        assert_eq!(extract_json_string(text, "id").as_deref(), Some("abc"));
        assert_eq!(extract_json_string(text, "missing"), None);
    }
}
