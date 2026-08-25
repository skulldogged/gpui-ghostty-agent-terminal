//! Optional recognition of agent programs and their visible terminal state.
//!
//! This follows HerdR's detection boundary: process inspection identifies the
//! agent, while rendered terminal content and OSC title signals describe its
//! state. Failure to recognize either leaves the terminal fully interactive.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentProgram {
    Codex,
    Claude,
    Gemini,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentState {
    Idle,
    Working,
    Blocked,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentSnapshot {
    pub program: AgentProgram,
    pub state: AgentState,
}

impl AgentProgram {
    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
            Self::Gemini => "Gemini CLI",
        }
    }

    pub(crate) fn from_process(process_name: &str, command: &[String]) -> Option<Self> {
        let executable = normalized_executable(process_name);
        direct_agent_name(&executable).or_else(|| {
            let command = command
                .iter()
                .map(|argument| argument.to_lowercase().replace('\\', "/"))
                .collect::<Vec<_>>()
                .join(" ");
            if command.contains("@openai/codex")
                || command.contains("/codex/bin/")
                || command.contains("/codex.js")
            {
                Some(Self::Codex)
            } else if command.contains("@anthropic-ai/claude-code")
                || command.contains("/claude-code/")
                || command.contains("/claude/cli.js")
            {
                Some(Self::Claude)
            } else if command.contains("@google/gemini-cli")
                || command.contains("/gemini-cli/")
                || command.contains("/gemini.js")
            {
                Some(Self::Gemini)
            } else {
                None
            }
        })
    }
}

impl AgentSnapshot {
    pub(crate) fn from_screen(program: AgentProgram, title: Option<&str>, screen: &str) -> Self {
        Self {
            program,
            state: detect_state(program, title.unwrap_or_default(), screen),
        }
    }

    pub(crate) fn priority(self) -> u8 {
        match self.state {
            AgentState::Blocked => 4,
            AgentState::Working => 3,
            AgentState::Idle => 2,
            AgentState::Unknown => 1,
        }
    }
}

fn normalized_executable(process_name: &str) -> String {
    let basename = process_name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(process_name)
        .to_lowercase();
    [".exe", ".cmd", ".bat", ".ps1", ".js"]
        .iter()
        .find_map(|extension| basename.strip_suffix(extension))
        .unwrap_or(&basename)
        .to_string()
}

fn direct_agent_name(name: &str) -> Option<AgentProgram> {
    match name {
        "codex" | "codex-cli" => Some(AgentProgram::Codex),
        "claude" | "claude-code" => Some(AgentProgram::Claude),
        "gemini" | "gemini-cli" => Some(AgentProgram::Gemini),
        _ if name.starts_with("codex-") => Some(AgentProgram::Codex),
        _ => None,
    }
}

fn detect_state(program: AgentProgram, title: &str, screen: &str) -> AgentState {
    let title_lower = title.to_lowercase();

    if title_lower.contains("action required") {
        return AgentState::Blocked;
    }

    if program == AgentProgram::Codex && title.chars().any(is_braille_spinner) {
        return AgentState::Working;
    }

    for line in screen.lines().rev() {
        let line = line.trim_start();
        let line_lower = line.to_lowercase();
        if line_lower.contains("waiting for user confirmation")
            || line_lower.contains("do you want to proceed?")
            || line_lower.contains("allow execution")
            || line_lower.contains("would you like to run")
            || line_lower.contains("press enter to confirm")
        {
            return AgentState::Blocked;
        }
        if line_lower.contains("esc to interrupt") || line_lower.contains("esc to cancel") {
            return AgentState::Working;
        }
        if matches!(program, AgentProgram::Claude | AgentProgram::Gemini)
            && (line.starts_with('❯') || (program == AgentProgram::Gemini && line.starts_with('>')))
        {
            return AgentState::Idle;
        }
    }

    match program {
        AgentProgram::Codex if !title.trim().is_empty() => AgentState::Idle,
        AgentProgram::Claude if title.starts_with("✳ ") => AgentState::Idle,
        _ => AgentState::Unknown,
    }
}

fn is_braille_spinner(character: char) -> bool {
    ('\u{2800}'..='\u{28ff}').contains(&character)
}

#[cfg(test)]
mod tests {
    use super::{AgentProgram, AgentSnapshot, AgentState};

    #[test]
    fn process_detection_unwraps_node_agent_launchers() {
        assert_eq!(
            AgentProgram::from_process(
                "node.exe",
                &[
                    "node.exe".into(),
                    "C:/npm/node_modules/@openai/codex/bin/codex.js".into()
                ]
            ),
            Some(AgentProgram::Codex)
        );
        assert_eq!(
            AgentProgram::from_process(
                "node.exe",
                &[
                    "node.exe".into(),
                    "C:/npm/node_modules/@anthropic-ai/claude-code/cli.js".into()
                ]
            ),
            Some(AgentProgram::Claude)
        );
        assert_eq!(
            AgentProgram::from_process(
                "node.exe",
                &[
                    "node.exe".into(),
                    "C:/npm/node_modules/@google/gemini-cli/dist/index.js".into()
                ]
            ),
            Some(AgentProgram::Gemini)
        );
    }

    #[test]
    fn unrelated_runtime_is_not_an_agent() {
        assert_eq!(
            AgentProgram::from_process("node.exe", &["node.exe".into(), "server.js".into()]),
            None
        );
    }

    #[test]
    fn visible_state_uses_herdr_style_screen_and_title_signals() {
        assert_eq!(
            AgentSnapshot::from_screen(
                AgentProgram::Codex,
                Some("Action Required"),
                "Approve this command"
            )
            .state,
            AgentState::Blocked
        );
        assert_eq!(
            AgentSnapshot::from_screen(AgentProgram::Claude, None, "Thinking…  esc to interrupt")
                .state,
            AgentState::Working
        );
        assert_eq!(
            AgentSnapshot::from_screen(AgentProgram::Claude, None, "❯ ").state,
            AgentState::Idle
        );
        assert_eq!(
            AgentSnapshot::from_screen(
                AgentProgram::Claude,
                None,
                "Do you want to proceed?\nApproved\nThinking…  esc to interrupt"
            )
            .state,
            AgentState::Working
        );
    }

    #[test]
    fn state_priority_puts_attention_before_activity() {
        let priority = |state| {
            AgentSnapshot {
                program: AgentProgram::Codex,
                state,
            }
            .priority()
        };
        assert!(priority(AgentState::Blocked) > priority(AgentState::Working));
        assert!(priority(AgentState::Working) > priority(AgentState::Idle));
        assert!(priority(AgentState::Idle) > priority(AgentState::Unknown));
    }
}
