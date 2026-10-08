//! ADR 2026-10-08-cos-chat-prompt-cache D3.4-1: the deterministic prompt-bytes bench (no LLM, no
//! network). `cargo test -p task-worker cos_chat_bench_ -- --nocapture` prints one JSON object;
//! with `COS_CHAT_BENCH_OUT=<file>` it is also written there. The inputs are fixed in this file so
//! the after measurement (T8) reproduces them.

use serde_json::{Value, json};

use crate::protocol::tests::sample_task;
use crate::protocol::{
    CosChatContext, CosChatHistory, CosChatHistoryMessage, CosChatInput, RunContext,
};

const SKILLS: [&str; 2] = ["cos-operator", "cos-inbox-triage"];
/// A fixed single-turn consultation (script S1/S3).
const CONSULT: &str = "今の受信箱と走っている task の状況を 3 行で教えてください。";

fn id(prefix: &str, n: usize) -> String {
    // 26 chars like a ULID, differing in the tail only
    format!("01M4{prefix}{:0>width$}", n, width = 22 - prefix.len())
}

fn chat(
    thread: usize,
    run: usize,
    inputs: Vec<CosChatInput>,
    history: Vec<CosChatHistoryMessage>,
) -> CosChatContext {
    let through = inputs.iter().map(|i| i.seq).max().unwrap_or(0);
    CosChatContext {
        thread_id: id("THR", thread),
        run_id: id("CRUN", run),
        ..CosChatContext::default()
    }
    .with(inputs, history, through)
}

trait With {
    fn with(
        self,
        inputs: Vec<CosChatInput>,
        history: Vec<CosChatHistoryMessage>,
        through: i64,
    ) -> Self;
}
impl With for CosChatContext {
    fn with(
        mut self,
        inputs: Vec<CosChatInput>,
        history: Vec<CosChatHistoryMessage>,
        _through: i64,
    ) -> Self {
        let first = inputs.first().map(|i| i.seq).unwrap_or(1);
        self.unsummarized = CosChatHistory {
            from_seq: 1,
            through_seq: first - 1,
            messages: history,
        };
        self.inputs = inputs;
        self.skills = SKILLS.iter().map(|s| s.to_string()).collect();
        self.credential_env = "CELERIS_COS_RUN_TOKEN".into();
        self.api_base_url = "http://127.0.0.1:17932/api/v1".into();
        self
    }
}

fn input(seq: i64, text: &str) -> CosChatInput {
    CosChatInput {
        id: id("MSG", seq as usize),
        seq,
        text: text.into(),
        ..CosChatInput::default()
    }
}

fn parts(chat: &CosChatContext, n: usize) -> super::CosChatPrompt {
    let mut task = sample_task();
    task.id = serde_json::from_value(json!(id("TASK", n))).unwrap();
    let ctx = RunContext {
        cos_chat: Some(chat.clone()),
        ..RunContext::default()
    };
    crate::claude_code::build_cos_chat_parts(&task, &ctx, &id("WRUN", n), "artifacts")
        .expect("CoS chat parts")
}

/// The single input of acp / pi (and the layout before D6): Core first, then the rest.
fn prompt(chat: &CosChatContext, n: usize) -> String {
    let mut task = sample_task();
    task.id = serde_json::from_value(json!(id("TASK", n))).unwrap();
    let ctx = RunContext {
        cos_chat: Some(chat.clone()),
        ..RunContext::default()
    };
    crate::claude_code::build_prompt(&task, &ctx, &id("WRUN", n), "artifacts")
}

/// claude-code: the `--append-system-prompt` argument and stdin.
fn claude_channels(chat: &CosChatContext, n: usize) -> (String, String) {
    let p = parts(chat, n);
    (
        crate::claude_code::cos_chat_system_prompt(&p.core),
        p.variable,
    )
}

/// The bytes two claude-code runs share from the start of what celeris sends: the whole
/// system argument when it is identical, plus the common prefix of stdin.
fn claude_shared_prefix(a: &(String, String), b: &(String, String)) -> usize {
    if a.0 == b.0 {
        a.0.len() + lcp(&a.1, &b.1)
    } else {
        lcp(&a.0, &b.0)
    }
}

fn lcp(a: &str, b: &str) -> usize {
    a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count()
}

fn lcp_all(ps: &[String]) -> usize {
    ps.iter().skip(1).map(|p| lcp(&ps[0], p)).min().unwrap_or(0)
}

fn skill_bytes(name: &str) -> Value {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/skills")
        .join(name);
    let skill = std::fs::metadata(dir.join("SKILL.md"))
        .map(|m| m.len())
        .ok();
    json!({ "skill_md_bytes": skill })
}

#[test]
fn cos_chat_bench_prompt_bytes() {
    // S1: ten independent new threads, the same consultation, turn 1.
    let s1: Vec<String> = (1..=10)
        .map(|t| prompt(&chat(t, t, vec![input(1, CONSULT)], vec![]), t))
        .collect();
    let s1_total = s1[0].len();
    let s1_lcp = lcp_all(&s1);
    let s1_claude: Vec<(String, String)> = (1..=10)
        .map(|t| claude_channels(&chat(t, t, vec![input(1, CONSULT)], vec![]), t))
        .collect();
    let s1_core_identical = s1_claude.iter().all(|c| c.0 == s1_claude[0].0);
    let s1_claude_shared = s1_claude
        .iter()
        .skip(1)
        .map(|c| claude_shared_prefix(&s1_claude[0], c))
        .min()
        .unwrap_or(0);

    // S2: one thread, ten turns, every turn resumed. The history holds all earlier turns
    // (a 120 B user text and a 480 B assistant reply each), because the dispatcher resends it.
    let user_text = "あ".repeat(40); // 120 B
    let reply_text = "い".repeat(160); // 480 B
    let mut s2 = Vec::new();
    let mut s2_claude = Vec::new();
    let mut hist: Vec<CosChatHistoryMessage> = Vec::new();
    for turn in 1..=10usize {
        let seq = (2 * turn - 1) as i64;
        let c = chat(100, 100 + turn, vec![input(seq, &user_text)], hist.clone());
        s2.push(prompt(&c, 100 + turn));
        s2_claude.push(claude_channels(&c, 100 + turn));
        hist.push(CosChatHistoryMessage {
            id: id("MSG", seq as usize),
            seq,
            role: "user".into(),
            text: user_text.clone(),
        });
        hist.push(CosChatHistoryMessage {
            id: id("MSG", seq as usize + 1),
            seq: seq + 1,
            role: "assistant".into(),
            text: reply_text.clone(),
        });
    }
    let s2_consecutive: Vec<usize> = s2.windows(2).map(|w| lcp(&w[0], &w[1])).collect();
    let s2_claude_consecutive: Vec<usize> = s2_claude
        .windows(2)
        .map(|w| claude_shared_prefix(&w[0], &w[1]))
        .collect();
    let s2_core_identical = s2_claude.iter().all(|c| c.0 == s2_claude[0].0);

    // Section sizes of the turn-1 prompt (ADR 2026-10-08 D6 layout: Core, then the run specific part).
    let c1 = chat(1, 1, vec![input(1, CONSULT)], vec![]);
    let core = super::core(&c1).len();
    let header = format!("# CoS chat: thread {}\n\n", c1.thread_id).len();
    let skills_sec = super::skills_section(&c1).len();
    let params_sec = super::run_parameters_section(&c1, &id("WRUN", 1), &{
        let mut t = sample_task();
        t.id = serde_json::from_value(json!(id("TASK", 1))).unwrap();
        t
    })
    .len();
    let inputs_sec = super::inputs_section(&c1).len();
    let history_sec = super::history_section(&c1).len();
    let variable = s1_claude[0].1.len();
    let preamble_variable =
        variable - header - skills_sec - params_sec - super::cos_chat_section(&c1).len();
    let fixed_rules = core + skills_sec; // the same bytes for every run of the same config
    let resent: Vec<Value> = s2
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let turn = i + 1;
            let history_bytes = super::history_section(&chat(
                100,
                100 + turn,
                vec![input((2 * turn - 1) as i64, &user_text)],
                s2_history(turn, &user_text, &reply_text),
            ))
            .len();
            json!({
                "turn": turn,
                "stdin_bytes": p.len(),
                "history_section_bytes": history_bytes,
                "resent_fixed_bytes": fixed_rules,
                "claude_stdin_bytes": s2_claude[i].1.len(),
                "duplicated_share": (history_bytes + fixed_rules) as f64 / p.len() as f64,
            })
        })
        .collect();

    let out = json!({
        "schema": "celeris.cos-chat-bench-prompt/2",
        "script": "cargo test -p task-worker cos_chat_bench_ -- --nocapture",
        "builder": "task_worker::claude_code::{build_prompt, build_cos_chat_parts} (context.cos_chat)",
        "headless_run_note_bytes": crate::preamble::HEADLESS_RUN_NOTE.len(),
        "skills": { "cos-operator": skill_bytes("cos-operator"), "cos-inbox-triage": skill_bytes("cos-inbox-triage") },
        "sections_turn1_bytes": {
            "core": core, "header": header, "skills_list": skills_sec, "run_parameters": params_sec,
            "preamble_variable": preamble_variable, "inputs": inputs_sec, "history": history_sec,
            "fixed_rules_total": fixed_rules, "variable_total": variable, "total": s1_total,
        },
        "claude_code_channels": {
            "append_system_prompt_bytes": s1_claude[0].0.len(),
            "core_bytes": core,
            "stdin_bytes_turn1": variable,
            "core_identical_s1_10_threads": s1_core_identical,
            "core_identical_s2_10_turns": s2_core_identical,
            "s1_shared_prefix_bytes_all_threads": s1_claude_shared,
            "s2_shared_prefix_with_previous_turn_bytes": s2_claude_consecutive,
        },
        "s1_independent_new_threads": {
            "threads": 10,
            "prompt_bytes": s1_total,
            "common_prefix_bytes_all_threads": s1_lcp,
            "variable_bytes_after_prefix": s1_total - s1_lcp,
            "fixed_share_of_prompt": fixed_rules as f64 / s1_total as f64,
        },
        "s2_same_thread_10_turns_resumed": {
            "prompt_bytes_by_turn": s2.iter().map(|p| p.len()).collect::<Vec<_>>(),
            "common_prefix_with_previous_turn_bytes": s2_consecutive,
            "per_turn": resent,
        },
    });
    let text = serde_json::to_string_pretty(&out).unwrap();
    println!("COS_CHAT_BENCH_JSON {text}");
    if let Ok(path) = std::env::var("COS_CHAT_BENCH_OUT") {
        std::fs::write(path, &text).unwrap();
    }
    // The bench is a measurement, not a gate: only check it measured something sane.
    assert!(s1_total > 3000 && s1_lcp > 0 && s1_core_identical);
}

fn s2_history(turn: usize, user: &str, reply: &str) -> Vec<CosChatHistoryMessage> {
    let mut h = Vec::new();
    for t in 1..turn {
        let seq = (2 * t - 1) as i64;
        h.push(CosChatHistoryMessage {
            id: id("MSG", seq as usize),
            seq,
            role: "user".into(),
            text: user.into(),
        });
        h.push(CosChatHistoryMessage {
            id: id("MSG", seq as usize + 1),
            seq: seq + 1,
            role: "assistant".into(),
            text: reply.into(),
        });
    }
    h
}
