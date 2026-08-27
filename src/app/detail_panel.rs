use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::agent_resume::AgentSessionRefKind;
use crate::app::state::AppState;

const TAIL_BYTES: u64 = 512 * 1024;
const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

/// Focused tab's transcript view for the right-side detail panel.
pub struct DetailPanelCache {
    pub session_key: String,
    pub agent: String,
    pub transcript: Option<PathBuf>,
    pub prompt: String,
    pub reply: String,
    checked_at: Instant,
    file_sig: (u64, u64),
}

impl AppState {
    pub(crate) fn refresh_detail_panel(&mut self) {
        let session = self
            .active
            .and_then(|i| self.workspaces.get(i))
            .and_then(|ws| {
                let pane_id = ws.focused_pane_id()?;
                let terminal_id = ws.terminal_id(pane_id)?;
                self.terminals.get(terminal_id)
            })
            .and_then(|terminal| {
                let session = terminal.persisted_agent_session.as_ref()?;
                Some((
                    session.agent.clone(),
                    session.session_ref.kind,
                    session.session_ref.value.clone(),
                    terminal.cwd.clone(),
                ))
            });
        let Some((agent, kind, value, cwd)) = session else {
            self.detail_panel = None;
            return;
        };

        if let Some(cache) = &self.detail_panel {
            if cache.session_key == value && cache.checked_at.elapsed() < REFRESH_INTERVAL {
                return;
            }
        }

        let transcript = match kind {
            AgentSessionRefKind::Path => Some(PathBuf::from(&value)),
            AgentSessionRefKind::Id if agent == "claude" => claude_transcript_path(&cwd, &value),
            AgentSessionRefKind::Id => None,
        };

        let file_sig = transcript.as_deref().map(file_signature).unwrap_or((0, 0));
        if let Some(cache) = &mut self.detail_panel {
            if cache.session_key == value && cache.file_sig == file_sig {
                cache.checked_at = Instant::now();
                return;
            }
        }

        let (prompt, reply) = transcript
            .as_deref()
            .map(read_prompt_and_reply)
            .unwrap_or_default();
        self.detail_panel = Some(DetailPanelCache {
            session_key: value,
            agent,
            transcript,
            prompt,
            reply,
            checked_at: Instant::now(),
            file_sig,
        });
    }
}

fn file_signature(path: &Path) -> (u64, u64) {
    std::fs::metadata(path)
        .map(|meta| {
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            (meta.len(), mtime)
        })
        .unwrap_or((0, 0))
}

/// Claude Code stores transcripts under ~/.claude/projects/<munged-cwd>/<id>.jsonl.
fn claude_transcript_path(cwd: &Path, session_id: &str) -> Option<PathBuf> {
    if session_id.contains('/') || session_id.contains("..") {
        return None;
    }
    let home = std::env::var_os("HOME")?;
    let projects = Path::new(&home).join(".claude").join("projects");
    let munged: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let file = format!("{session_id}.jsonl");
    let direct = projects.join(&munged).join(&file);
    if direct.is_file() {
        return Some(direct);
    }
    for entry in std::fs::read_dir(&projects).ok()?.flatten() {
        let candidate = entry.path().join(&file);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn read_prompt_and_reply(path: &Path) -> (String, String) {
    use std::io::{Read, Seek, SeekFrom};

    let Ok(mut file) = std::fs::File::open(path) else {
        return Default::default();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(TAIL_BYTES);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Default::default();
    }
    let mut buf = Vec::with_capacity((len - start) as usize);
    if file.read_to_end(&mut buf).is_err() {
        return Default::default();
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines = text.lines();
    if start > 0 {
        lines.next();
    }

    let mut prompt = String::new();
    let mut reply = String::new();
    for line in lines {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match value.get("type").and_then(|t| t.as_str()) {
            Some("user") => {
                if let Some(text) = user_prompt_text(&value) {
                    prompt = text;
                    reply.clear();
                }
            }
            Some("assistant") => {
                if let Some(text) = message_text(&value) {
                    reply = text;
                }
            }
            _ => {}
        }
    }
    (prompt, reply)
}

fn user_prompt_text(value: &serde_json::Value) -> Option<String> {
    if value
        .get("isMeta")
        .and_then(|m| m.as_bool())
        .unwrap_or(false)
    {
        return None;
    }
    let text = message_text(value)?;
    // skip system reminders, hook output and local-command echo wrappers
    if text.starts_with('<') || text.starts_with("Caveat:") {
        return None;
    }
    Some(text)
}

fn message_text(value: &serde_json::Value) -> Option<String> {
    let content = value.get("message")?.get("content")?;
    let text = match content {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(items) => items
            .iter()
            .filter(|item| item.get("type").and_then(|t| t.as_str()) == Some("text"))
            .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_parse_keeps_last_prompt_and_following_reply() {
        let dir = std::env::temp_dir().join(format!("herdr-detail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"user","message":{"role":"user","content":"first ask"}}"#,
                "\n",
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"first answer"}]}}"#,
                "\n",
                r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"ignored"}]}}"#,
                "\n",
                r#"{"type":"user","message":{"role":"user","content":"second ask"}}"#,
                "\n",
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#,
                "\n",
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"second answer"}]}}"#,
                "\n",
            ),
        )
        .unwrap();

        let (prompt, reply) = read_prompt_and_reply(&path);
        assert_eq!(prompt, "second ask");
        assert_eq!(reply, "second answer");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn meta_and_reminder_user_lines_are_not_prompts() {
        let meta: serde_json::Value = serde_json::from_str(
            r#"{"type":"user","isMeta":true,"message":{"content":"housekeeping"}}"#,
        )
        .unwrap();
        assert_eq!(user_prompt_text(&meta), None);

        let reminder: serde_json::Value = serde_json::from_str(
            r#"{"type":"user","message":{"content":"<system-reminder>x</system-reminder>"}}"#,
        )
        .unwrap();
        assert_eq!(user_prompt_text(&reminder), None);
    }
}
