//! `gr8r external` — helpers for externally-detected agent sessions.
//!
//! `gr8r external view <transcript.jsonl>` renders a live, human-readable
//! tail of a Claude Code or Codex transcript and follows it as the session
//! progresses. This is what a sidebar click on an external agent opens: the
//! session's PTY belongs to another terminal, but its transcript is a
//! faithful live feed of the conversation.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Duration;

const POLL_INTERVAL: Duration = Duration::from_millis(300);
/// How much history to render when the viewer starts.
const INITIAL_TAIL_BYTES: u64 = 512 * 1024;
/// Cap for a single rendered block of text.
const BLOCK_CHAR_LIMIT: usize = 4000;
/// Cap for one-line summaries (tool calls, results).
const SUMMARY_CHAR_LIMIT: usize = 200;

const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RESET: &str = "\x1b[0m";

pub fn run_external_command(args: &[String]) -> std::io::Result<i32> {
    match args.first().map(|arg| arg.as_str()) {
        Some("view") => run_view(&args[1..]),
        Some("help" | "--help" | "-h") | None => {
            print_help();
            Ok(0)
        }
        Some(other) => {
            eprintln!("unknown external subcommand: {other}");
            print_help();
            Ok(2)
        }
    }
}

fn print_help() {
    println!("gr8r external commands:");
    println!("  gr8r external view <transcript.jsonl> [--label <label>]");
    println!();
    println!("Renders a live view of an externally-running agent session by");
    println!("following its transcript file (Claude Code or Codex).");
}

fn run_view(args: &[String]) -> std::io::Result<i32> {
    let mut path: Option<String> = None;
    let mut label: Option<String> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--label" => label = iter.next().cloned(),
            other if path.is_none() => path = Some(other.to_string()),
            other => {
                eprintln!("unexpected argument: {other}");
                return Ok(2);
            }
        }
    }
    let Some(path) = path else {
        print_help();
        return Ok(2);
    };
    let path = Path::new(&path);
    if !path.is_file() {
        eprintln!("transcript not found: {}", path.display());
        return Ok(1);
    }

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(
        out,
        "{BOLD}{}{RESET} {DIM}· live external session view · following {}{RESET}",
        label.as_deref().unwrap_or("external session"),
        path.display()
    )?;
    writeln!(
        out,
        "{DIM}read-only: this session runs in another terminal. ctrl+c or close the pane to stop.{RESET}"
    )?;
    writeln!(out)?;

    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let mut offset = len.saturating_sub(INITIAL_TAIL_BYTES);
    if offset > 0 {
        // Skip the (likely partial) first line of the tail window.
        file.seek(SeekFrom::Start(offset))?;
        let mut probe = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            match file.read(&mut byte) {
                Ok(0) => break,
                Ok(_) => {
                    offset += 1;
                    if byte[0] == b'\n' {
                        break;
                    }
                    probe.push(byte[0]);
                }
                Err(err) => return Err(err),
            }
        }
    }

    let mut carry = String::new();
    loop {
        let len = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
        if len < offset {
            // Truncated/rotated; start over from the beginning.
            offset = 0;
            carry.clear();
        }
        if len > offset {
            file.seek(SeekFrom::Start(offset))?;
            let mut buf = Vec::with_capacity((len - offset) as usize);
            let read = (&mut file).take(len - offset).read_to_end(&mut buf)?;
            offset += read as u64;
            carry.push_str(&String::from_utf8_lossy(&buf));
            while let Some(newline) = carry.find('\n') {
                let line: String = carry.drain(..=newline).collect();
                let line = line.trim_end();
                if !line.is_empty() {
                    render_line(&mut out, line)?;
                }
            }
            out.flush()?;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn render_line(out: &mut impl Write, line: &str) -> std::io::Result<()> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return Ok(());
    };
    match value.get("type").and_then(|ty| ty.as_str()) {
        // Claude Code transcript entries
        Some("user") => render_claude_user(out, &value),
        Some("assistant") => render_claude_assistant(out, &value),
        // Codex rollout entries
        Some("event_msg") | Some("response_item") => render_codex(out, &value),
        _ => Ok(()),
    }
}

fn render_claude_user(out: &mut impl Write, value: &serde_json::Value) -> std::io::Result<()> {
    let Some(content) = value
        .get("message")
        .and_then(|message| message.get("content"))
    else {
        return Ok(());
    };
    if let Some(text) = content.as_str() {
        return write_block(out, &format!("{BOLD}{CYAN}you{RESET}"), text);
    }
    let Some(blocks) = content.as_array() else {
        return Ok(());
    };
    for block in blocks {
        match block.get("type").and_then(|ty| ty.as_str()) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(|text| text.as_str()) {
                    write_block(out, &format!("{BOLD}{CYAN}you{RESET}"), text)?;
                }
            }
            Some("tool_result") => {
                let summary = tool_result_summary(block);
                writeln!(out, "  {GREEN}✓{RESET} {DIM}{summary}{RESET}")?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn render_claude_assistant(out: &mut impl Write, value: &serde_json::Value) -> std::io::Result<()> {
    let Some(blocks) = value
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(|content| content.as_array())
    else {
        return Ok(());
    };
    for block in blocks {
        match block.get("type").and_then(|ty| ty.as_str()) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(|text| text.as_str()) {
                    write_block(out, &format!("{BOLD}claude{RESET}"), text)?;
                }
            }
            Some("tool_use") => {
                let name = block
                    .get("name")
                    .and_then(|name| name.as_str())
                    .unwrap_or("tool");
                let input = block
                    .get("input")
                    .map(|input| truncate(&compact_json(input), SUMMARY_CHAR_LIMIT))
                    .unwrap_or_default();
                writeln!(out, "  {YELLOW}→ {name}{RESET} {DIM}{input}{RESET}")?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn render_codex(out: &mut impl Write, value: &serde_json::Value) -> std::io::Result<()> {
    let Some(payload) = value.get("payload") else {
        return Ok(());
    };
    match payload.get("type").and_then(|ty| ty.as_str()) {
        Some("user_message") => {
            if let Some(message) = payload.get("message").and_then(|msg| msg.as_str()) {
                write_block(out, &format!("{BOLD}{CYAN}you{RESET}"), message)?;
            }
        }
        Some("agent_message") => {
            if let Some(message) = payload.get("message").and_then(|msg| msg.as_str()) {
                write_block(out, &format!("{BOLD}codex{RESET}"), message)?;
            }
        }
        Some("function_call") => {
            let name = payload
                .get("name")
                .and_then(|name| name.as_str())
                .unwrap_or("tool");
            let args = payload
                .get("arguments")
                .and_then(|args| args.as_str())
                .map(|args| truncate(args, SUMMARY_CHAR_LIMIT))
                .unwrap_or_default();
            writeln!(out, "  {YELLOW}→ {name}{RESET} {DIM}{args}{RESET}")?;
        }
        Some("function_call_output") => {
            let output = payload
                .get("output")
                .map(|output| truncate(&compact_json(output), SUMMARY_CHAR_LIMIT))
                .unwrap_or_default();
            writeln!(out, "  {GREEN}✓{RESET} {DIM}{output}{RESET}")?;
        }
        Some("task_started") => {
            writeln!(out, "{DIM}── turn started ──{RESET}")?;
        }
        Some("task_complete") => {
            writeln!(out, "{DIM}── turn complete ──{RESET}")?;
        }
        Some("turn_aborted") => {
            writeln!(out, "{DIM}── turn aborted ──{RESET}")?;
        }
        _ => {}
    }
    Ok(())
}

fn tool_result_summary(block: &serde_json::Value) -> String {
    let text = match block.get("content") {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.get("text").and_then(|text| text.as_str()))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    };
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        "tool result".to_string()
    } else {
        truncate(&format!("tool result: {text}"), SUMMARY_CHAR_LIMIT)
    }
}

fn compact_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.split_whitespace().collect::<Vec<_>>().join(" "),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn write_block(out: &mut impl Write, speaker: &str, text: &str) -> std::io::Result<()> {
    let text = truncate(text.trim(), BLOCK_CHAR_LIMIT);
    if text.is_empty() {
        return Ok(());
    }
    writeln!(out)?;
    writeln!(out, "{speaker}")?;
    for line in text.lines() {
        writeln!(out, "  {line}")?;
    }
    Ok(())
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let kept: String = text.chars().take(limit).collect();
    let dropped = text.chars().count() - limit;
    format!("{kept}{DIM}… (+{dropped} chars){RESET}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_to_string(line: &str) -> String {
        let mut buf = Vec::new();
        render_line(&mut buf, line).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn renders_claude_text_and_tool_use() {
        let user = r#"{"type":"user","message":{"role":"user","content":"fix the bug"}}"#;
        assert!(render_to_string(user).contains("fix the bug"));

        let assistant = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"on it"},{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}"#;
        let rendered = render_to_string(assistant);
        assert!(rendered.contains("on it"));
        assert!(rendered.contains("Bash"));
        assert!(rendered.contains("ls"));
    }

    #[test]
    fn renders_codex_messages_and_lifecycle() {
        let user = r#"{"type":"event_msg","payload":{"type":"user_message","message":"hello"}}"#;
        assert!(render_to_string(user).contains("hello"));

        let done = r#"{"type":"event_msg","payload":{"type":"task_complete"}}"#;
        assert!(render_to_string(done).contains("turn complete"));

        let call = r#"{"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"cmd\":\"ls\"}"}}"#;
        assert!(render_to_string(call).contains("shell"));
    }

    #[test]
    fn ignores_noise_lines() {
        assert!(render_to_string(r#"{"type":"file-history-snapshot"}"#).is_empty());
        assert!(render_to_string("not json").is_empty());
    }
}
