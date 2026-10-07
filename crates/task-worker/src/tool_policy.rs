//! ADR 2026-10-07-worker-no-subagents-no-llm-cli: worker は内部で subagent や別の LLM CLI を起動しない。
//!
//! ここにあるのは 3 つだけで、どれも純粋関数（LLM も I/O も無い）:
//!
//! 1. **既定禁止の材料**（D1–D3）: claude-code の `--disallowedTools` の値、codex の `-c features.*=false`、
//!    opencode の `OPENCODE_CONFIG_CONTENT` への `tools.task=false` の重ね方。adapter はこれをそのまま argv / env に置く。
//! 2. **設定の 3 択**（D7）: [`SubagentPolicy`]（`deny` 既定 / `allow_cos` / `allow`）と、run ごとの判定 [`SubagentPolicy::allows`]。
//! 3. **検出**（D5）: [`inspect_tool_use`]。adapter の進行の写像（ADR-0048 D2）と同じ場所で `tool_use` を見て、
//!    subagent 道具・LLM CLI の起動・LLM API の直接呼び出しを決定的に見つける。検出は**警告**で、run は止めない。

use task_core::ToolPolicyKind;

use crate::protocol::{ConversationAddressee, RunRequest};

/// subagent を起こす道具の名前（D1 / D5-1）。claude-code の `Agent`（旧名 `Task`）と `Workflow`、opencode の `task`、
/// codex の `spawn_agent` 系。将来の同種の道具はここに足す。
pub const SUBAGENT_TOOLS: &[&str] = &[
    "Agent",
    "Task",
    "Workflow",
    "task",
    "spawn_agent",
    "spawn_agents_on_csv",
];

/// claude-code の `--disallowedTools` に渡す道具（D1）。`SUBAGENT_TOOLS` のうち Claude Code の道具名だけ。
pub const CLAUDE_CODE_DISALLOWED_TOOLS: &[&str] = &["Agent", "Task", "Workflow"];

/// codex の `-c key=value`（D2）。`codex features list` の `multi_agent`（stable、既定 true）と `multi_agent_v2`。
pub const CODEX_FEATURE_OVERRIDES: &[&str] = &[
    "features.multi_agent=false",
    "features.multi_agent_v2=false",
];

/// opencode の subagent 起動の道具 id（D3。`Tool.define("task")`、入力 `subagent_type`）。
pub const OPENCODE_SUBAGENT_TOOL: &str = "task";

/// shell から起動されると同じ汚染が起きる LLM CLI の basename（D5-2）。
pub const LLM_CLIS: &[&str] = &[
    "claude",
    "codex",
    "opencode",
    "gemini",
    "aider",
    "cursor-agent",
    "copilot",
    "goose",
    "amp",
    "qwen",
    "kimi",
    "llm",
    "ollama",
];

/// shell から直接呼ばれると同じ汚染が起きる LLM API の host（D5-3）。
pub const LLM_API_HOSTS: &[&str] = &[
    "api.anthropic.com",
    "api.openai.com",
    "generativelanguage.googleapis.com",
    "openrouter.ai",
    "opencode.ai/zen",
    "api.x.ai",
    "api.mistral.ai",
    "api.deepseek.com",
    "api.groq.com",
    "api.together.xyz",
];

/// `command` に残す長さ（文字）。
pub const COMMAND_MAX_CHARS: usize = 500;

/// 先頭語として現れても「何かを起動する」のではなく読むだけの道具。この区間に API host が含まれても検出しない。
const READ_ONLY_HEADS: &[&str] = &[
    "grep", "rg", "git", "cat", "sed", "awk", "head", "tail", "less", "find", "echo", "printf",
    "diff", "wc", "sort",
];

/// 包む語の option のうち引数を 1 つ取るもの（`sudo -u me` / `nice -n 10` / `xargs -I {}` / `env -C dir`）。
const ARG_OPTIONS: &[&str] = &["-u", "-g", "-n", "-C", "-I", "-L", "-P"];

/// 先頭語の前に付く「包む」語。これらを剥がした次の語を先頭語として見る。
const WRAPPERS: &[&str] = &[
    "sudo", "env", "nohup", "exec", "command", "nice", "xargs", "time", "setsid", "doas", "builtin",
];

/// `[adapters.<id>] subagents`（D7）。既定 `Deny`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SubagentPolicy {
    /// 全 run で禁止（既定・推奨）。
    #[default]
    Deny,
    /// CoS の対話 run（`conversation_addressee == Secretary`）にだけ許す。
    AllowCos,
    /// 全 run で許す（応急処置前の挙動）。
    Allow,
}

impl SubagentPolicy {
    /// 設定の文字列から決定的に解決する。未知の値・空は `Deny`（安全側）。
    pub fn parse(s: &str) -> Self {
        match s.trim() {
            "allow" => SubagentPolicy::Allow,
            "allow_cos" | "allow-cos" => SubagentPolicy::AllowCos,
            _ => SubagentPolicy::Deny,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            SubagentPolicy::Deny => "deny",
            SubagentPolicy::AllowCos => "allow_cos",
            SubagentPolicy::Allow => "allow",
        }
    }

    /// この run で subagent を許すか。
    pub fn allows(self, req: &RunRequest) -> bool {
        match self {
            SubagentPolicy::Allow => true,
            SubagentPolicy::Deny => false,
            SubagentPolicy::AllowCos => {
                req.context.conversation_addressee == Some(ConversationAddressee::Secretary)
            }
        }
    }
}

/// claude-code の `--disallowedTools` の値（D1）。
pub fn claude_code_disallowed_tools_value() -> String {
    CLAUDE_CODE_DISALLOWED_TOOLS.join(",")
}

/// opencode の `OPENCODE_CONFIG_CONTENT` に `tools.task=false` を重ねる（D3）。
///
/// `existing` が JSON object ならその object に merge する（他の鍵・`tools` の他の道具は保つ）。無ければ
/// `{"tools":{"task":false}}`。JSON object でない値（壊れた設定）はそのまま返す（上書きしない。運用側の誤りを
/// 黙って隠さない）。
pub fn opencode_overlay(existing: Option<&str>) -> String {
    let mut root = match existing {
        None => serde_json::json!({}),
        Some(body) => match serde_json::from_str::<serde_json::Value>(body) {
            Ok(value) if value.is_object() => value,
            _ => return body.to_string(),
        },
    };
    let Some(object) = root.as_object_mut() else {
        return root.to_string();
    };
    let tools = object
        .entry("tools")
        .or_insert_with(|| serde_json::json!({}));
    if !tools.is_object() {
        *tools = serde_json::json!({});
    }
    if let Some(tools) = tools.as_object_mut() {
        tools.insert(OPENCODE_SUBAGENT_TOOL.to_string(), serde_json::json!(false));
    }
    root.to_string()
}

/// 検出 1 件（D5）。`Event::WorkerPolicyViolation` の材料。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPolicyViolation {
    pub kind: ToolPolicyKind,
    /// 道具名（`Bash` / `command_execution` / `Agent` …）。
    pub tool: String,
    /// 一致した語（道具名・CLI の basename・API host）。
    pub matched: String,
    /// 起動しようとした command（`COMMAND_MAX_CHARS` で切る）。subagent 道具では入力の要約。
    pub command: String,
}

impl ToolPolicyViolation {
    /// 人が Console で読む 1 行。
    pub fn message(&self) -> String {
        match self.kind {
            ToolPolicyKind::SubagentTool => format!(
                "policy: subagent tool `{}` used (forbidden; ADR 2026-10-07): {}",
                self.matched, self.command
            ),
            ToolPolicyKind::LlmCli => format!(
                "policy: LLM CLI `{}` launched from {} (forbidden; ADR 2026-10-07): {}",
                self.matched, self.tool, self.command
            ),
            ToolPolicyKind::LlmApi => format!(
                "policy: LLM API `{}` called from {} (forbidden; ADR 2026-10-07): {}",
                self.matched, self.tool, self.command
            ),
        }
    }
}

/// adapter が `tool_use` を観測するたびに呼ぶ（D5）。`input` は道具の入力（claude-code の `input`、
/// codex の `item`、acp の `rawInput`）。command は `command` / `cmd` / `commands`（配列）の鍵から取る。
pub fn inspect_tool_use(
    tool: &str,
    input: Option<&serde_json::Value>,
) -> Option<ToolPolicyViolation> {
    if SUBAGENT_TOOLS.contains(&tool) {
        let summary = input
            .map(|v| crate::progress::one_line(&v.to_string()))
            .unwrap_or_default();
        return Some(ToolPolicyViolation {
            kind: ToolPolicyKind::SubagentTool,
            tool: tool.to_string(),
            matched: tool.to_string(),
            command: truncate_chars(&summary, COMMAND_MAX_CHARS),
        });
    }
    let command = input.and_then(command_text)?;
    inspect_command(tool, &command)
}

/// shell の command 文字列だけを見る版（codex の `command_execution.command`、acp の `title` など）。
pub fn inspect_command(tool: &str, command: &str) -> Option<ToolPolicyViolation> {
    let shown = truncate_chars(&crate::progress::one_line(command), COMMAND_MAX_CHARS);
    for segment in split_segments(command) {
        let words = shell_words(segment);
        let Some(head) = head_word(&words) else {
            continue;
        };
        let base = head.rsplit('/').next().unwrap_or(head);
        if LLM_CLIS.contains(&base) {
            return Some(ToolPolicyViolation {
                kind: ToolPolicyKind::LlmCli,
                tool: tool.to_string(),
                matched: base.to_string(),
                command: shown,
            });
        }
        if READ_ONLY_HEADS.contains(&base) {
            continue;
        }
        if let Some(host) = LLM_API_HOSTS.iter().find(|h| segment.contains(**h)) {
            return Some(ToolPolicyViolation {
                kind: ToolPolicyKind::LlmApi,
                tool: tool.to_string(),
                matched: (*host).to_string(),
                command: shown,
            });
        }
    }
    None
}

/// 道具の入力から shell command を取り出す。
fn command_text(input: &serde_json::Value) -> Option<String> {
    for key in ["command", "cmd", "script"] {
        match input.get(key) {
            Some(serde_json::Value::String(s)) => return Some(s.clone()),
            Some(serde_json::Value::Array(items)) => {
                let joined: Vec<&str> = items.iter().filter_map(|v| v.as_str()).collect();
                if !joined.is_empty() {
                    return Some(joined.join(" "));
                }
            }
            _ => {}
        }
    }
    if let Some(serde_json::Value::Array(items)) = input.get("commands") {
        let joined: Vec<&str> = items.iter().filter_map(|v| v.as_str()).collect();
        if !joined.is_empty() {
            return Some(joined.join("\n"));
        }
    }
    None
}

/// `;` `&&` `||` `|` 改行 `$(` `` ` `` で区切る（引用の中身は追わない。決定的で安い近似）。
fn split_segments(command: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = command.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let cut = match bytes[i] {
            b';' | b'\n' | b'|' | b'&' | b'`' | b'(' | b')' | b'{' | b'}' => 1,
            b'$' if i + 1 < bytes.len() && bytes[i + 1] == b'(' => 2,
            _ => 0,
        };
        if cut > 0 {
            out.push(&command[start..i]);
            i += cut;
            start = i;
        } else {
            i += 1;
        }
    }
    out.push(&command[start..]);
    out
}

/// 空白で語に分ける（引用符は剥がす。中の空白は追わない）。
fn shell_words(segment: &str) -> Vec<&str> {
    segment
        .split_whitespace()
        .map(|w| w.trim_matches(|c| c == '"' || c == '\'' || c == '\\'))
        .filter(|w| !w.is_empty())
        .collect()
}

/// 環境変数代入（`FOO=bar`）と包む語（`sudo` / `env` / `timeout 60` …）を剥がした先頭語。
fn head_word<'a>(words: &[&'a str]) -> Option<&'a str> {
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        if is_env_assignment(w) {
            i += 1;
            continue;
        }
        if WRAPPERS.contains(&w) {
            i += 1;
            // `env -i` / `nice -n 10` / `sudo -u me` / `xargs -I {}` のような option（引数を取るものは次の語も）は飛ばす。
            while i < words.len() && words[i].starts_with('-') {
                let takes_arg = ARG_OPTIONS.contains(&words[i]);
                i += 1;
                if takes_arg {
                    i += 1;
                }
            }
            continue;
        }
        if w == "timeout" || w == "stdbuf" {
            i += 1;
            while i < words.len()
                && (words[i].starts_with('-')
                    || words[i]
                        .chars()
                        .all(|c| c.is_ascii_digit() || "smhd.".contains(c)))
            {
                i += 1;
            }
            continue;
        }
        return Some(w);
    }
    None
}

fn is_env_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !name.starts_with(|c: char| c.is_ascii_digit())
}

fn truncate_chars(s: &str, max: usize) -> String {
    crate::progress::truncate_chars(s, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bash(cmd: &str) -> Option<ToolPolicyViolation> {
        inspect_tool_use("Bash", Some(&serde_json::json!({ "command": cmd })))
    }

    #[test]
    fn subagent_tools_are_detected_by_name() {
        let v = inspect_tool_use(
            "Agent",
            Some(&serde_json::json!({"prompt": "review the plan"})),
        )
        .expect("Agent detected");
        assert_eq!(v.kind, ToolPolicyKind::SubagentTool);
        assert_eq!(v.matched, "Agent");
        assert!(v.command.contains("review the plan"), "{v:?}");
        assert!(inspect_tool_use("Task", None).is_some());
        assert!(inspect_tool_use("Workflow", None).is_some());
        assert!(inspect_tool_use("task", None).is_some());
        assert!(inspect_tool_use("spawn_agent", None).is_some());
        assert!(inspect_tool_use("Read", Some(&serde_json::json!({"file_path": "x"}))).is_none());
    }

    #[test]
    fn llm_cli_launches_are_detected_as_the_head_word_of_a_segment() {
        for cmd in [
            "claude -p 'review this'",
            "FOO=1 timeout 60 codex exec --json -",
            "cd repo && opencode run 'fix'",
            "sudo -u me env -i gemini -p hi",
            "nice -n 10 xargs -I {} claude -p {}",
            "echo hi | /home/me/.local/bin/claude -p",
            "x=$(claude -p 'summarize')",
            "nohup aider --message x &",
        ] {
            let v = bash(cmd).unwrap_or_else(|| panic!("not detected: {cmd}"));
            assert_eq!(v.kind, ToolPolicyKind::LlmCli, "{cmd}");
            assert_eq!(v.tool, "Bash");
        }
        assert_eq!(bash("claude -p x").unwrap().matched, "claude");
        assert_eq!(
            bash("echo hi | /home/me/.local/bin/claude -p")
                .unwrap()
                .matched,
            "claude"
        );
    }

    #[test]
    fn mentions_of_cli_names_that_are_not_launches_are_not_detected() {
        for cmd in [
            "which claude",
            "cargo test -p task-worker claude_code",
            "git commit -m 'claude adapter: deny subagents'",
            "grep -rn codex crates/",
            "ls ~/.local/bin/claude",
            "cat /tmp/opencode.log",
            "CLAUDE_CONFIG_DIR=/x cargo run",
            "echo claude",
        ] {
            assert!(bash(cmd).is_none(), "false positive: {cmd}");
        }
    }

    #[test]
    fn direct_llm_api_calls_are_detected_unless_only_read_tools_mention_the_host() {
        let v = bash("curl -sS https://api.anthropic.com/v1/messages -d @req.json").unwrap();
        assert_eq!(v.kind, ToolPolicyKind::LlmApi);
        assert_eq!(v.matched, "api.anthropic.com");
        assert!(bash("python3 -c \"import requests; requests.post('https://api.openai.com/v1/chat/completions')\"").is_some());
        assert!(bash("grep -rn api.anthropic.com crates/").is_none());
        assert!(bash("git log --grep api.openai.com").is_none());
        assert!(bash("rg openrouter.ai docs").is_none());
    }

    #[test]
    fn command_is_truncated_and_single_line() {
        let long = format!("claude -p '{}'", "x".repeat(2000));
        let v = bash(&long).unwrap();
        assert!(
            v.command.chars().count() <= COMMAND_MAX_CHARS + 1,
            "{}",
            v.command.len()
        );
        assert!(!v.command.contains('\n'));
        let multi = bash("echo a\nclaude -p b").unwrap();
        assert!(!multi.command.contains('\n'));
    }

    #[test]
    fn command_text_accepts_arrays_and_alternate_keys() {
        assert!(
            inspect_tool_use(
                "command_execution",
                Some(&serde_json::json!({"command": ["codex", "exec", "hi"]}))
            )
            .is_some()
        );
        assert!(
            inspect_tool_use("shell", Some(&serde_json::json!({"cmd": "opencode run x"})))
                .is_some()
        );
        assert!(
            inspect_tool_use("Bash", Some(&serde_json::json!({"description": "claude"}))).is_none()
        );
    }

    #[test]
    fn subagent_policy_parses_and_applies_per_run() {
        assert_eq!(SubagentPolicy::parse("allow"), SubagentPolicy::Allow);
        assert_eq!(SubagentPolicy::parse("allow_cos"), SubagentPolicy::AllowCos);
        assert_eq!(SubagentPolicy::parse("allow-cos"), SubagentPolicy::AllowCos);
        assert_eq!(SubagentPolicy::parse("deny"), SubagentPolicy::Deny);
        assert_eq!(SubagentPolicy::parse(""), SubagentPolicy::Deny);
        assert_eq!(SubagentPolicy::parse("yes please"), SubagentPolicy::Deny);
        assert_eq!(SubagentPolicy::default(), SubagentPolicy::Deny);

        let dir = std::path::PathBuf::from("/tmp/ws");
        let mut req = RunRequest {
            cargo_target_dir: None,
            protocol: crate::protocol::PROTOCOL_VERSION,
            task: crate::protocol::tests::sample_task(),
            artifacts_dir: dir.join("artifacts"),
            workspace: dir,
            work_dir: None,
            context: crate::protocol::RunContext::default(),
        };
        assert!(!SubagentPolicy::Deny.allows(&req));
        assert!(SubagentPolicy::Allow.allows(&req));
        assert!(!SubagentPolicy::AllowCos.allows(&req));
        req.context.conversation_addressee = Some(ConversationAddressee::Other);
        assert!(!SubagentPolicy::AllowCos.allows(&req));
        req.context.conversation_addressee = Some(ConversationAddressee::Secretary);
        assert!(SubagentPolicy::AllowCos.allows(&req));
        assert!(!SubagentPolicy::Deny.allows(&req));
    }

    #[test]
    fn opencode_overlay_merges_tools_task_false_into_existing_json() {
        let fresh: serde_json::Value = serde_json::from_str(&opencode_overlay(None)).unwrap();
        assert_eq!(fresh, serde_json::json!({"tools": {"task": false}}));

        let merged: serde_json::Value = serde_json::from_str(&opencode_overlay(Some(
            r#"{"model":"opencode/celeris/frontier","tools":{"webfetch":true},"provider":{"x":{"options":{"baseURL":"http://127.0.0.1:1/v1"}}}}"#,
        )))
        .unwrap();
        assert_eq!(merged["model"], "opencode/celeris/frontier");
        assert_eq!(merged["tools"]["webfetch"], true);
        assert_eq!(merged["tools"]["task"], false);
        assert_eq!(
            merged["provider"]["x"]["options"]["baseURL"],
            "http://127.0.0.1:1/v1"
        );

        // 運用側が `task: true` を書いても false に倒す（許すのは `subagents = "allow"` だけ）。
        let forced: serde_json::Value =
            serde_json::from_str(&opencode_overlay(Some(r#"{"tools":{"task":true}}"#))).unwrap();
        assert_eq!(forced["tools"]["task"], false);

        // JSON でない値は触らない。
        assert_eq!(opencode_overlay(Some("not json")), "not json");
        assert_eq!(opencode_overlay(Some("[1,2]")), "[1,2]");
    }

    #[test]
    fn disallowed_tools_value_lists_the_claude_code_subagent_tools() {
        assert_eq!(claude_code_disallowed_tools_value(), "Agent,Task,Workflow");
        for t in CLAUDE_CODE_DISALLOWED_TOOLS {
            assert!(SUBAGENT_TOOLS.contains(t));
        }
        assert!(CODEX_FEATURE_OVERRIDES.contains(&"features.multi_agent=false"));
    }
}
