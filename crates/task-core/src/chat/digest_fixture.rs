//! S4: the `cos_digest` comparison fixture (`control-ts/fixtures/cos_digest/cases.jsonl`).
//!
//! Hand-written boundary cases (`hand/…`) come first, then cases generated with a fixed-seed
//! SplitMix64 (`gen/…`). One line is `{"name":…,"input":<CosDigestInput>,"commands":[…]}`;
//! `commands` is what [`cos_digest_decide`] returns, never hand-written.

use std::collections::BTreeSet;

use serde::Serialize;

use super::digest::{CosDigestCommand, CosDigestInput, cos_digest_decide};
use super::triage::{CosTriageDecision, CosTriageItemReport, CosTriageOption};

/// SplitMix64 fixed seed. Changing it changes the whole generated part.
const SEED: u64 = 0x5EED_C05D_1635_0004;
/// Generated cases (distinct inputs).
pub const COS_DIGEST_GENERATED_CASES: usize = 2000;

struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.below(xs.len())]
    }
    fn chance(&mut self, num: u64, den: u64) -> bool {
        self.next() % den < num
    }
}

#[derive(Serialize)]
struct Line<'a> {
    name: &'a str,
    input: &'a CosDigestInput,
    commands: &'a [CosDigestCommand],
}

const RUN_STATES: &[&str] = &["completed", "failed", "stopped", "interrupted", "cancelled"];
const ITEM_STATES: &[&str] = &[
    "answered",
    "observed",
    "escalated",
    "fallback",
    "resolved",
    "pending",
    "running",
    "handled",
];
const SOURCE_KINDS: &[&str] = &[
    "decision",
    "question",
    "authorization",
    "acceptance_check",
    "plan_gate",
    "phase_gate",
    "notice",
    "task",
];
const TEXTS: &[&str] = &["", " ", "方針を決める", "Pick A or B", "  前後に空白  "];

fn item(n: usize, source_kind: &str, state: &str) -> CosTriageItemReport {
    CosTriageItemReport {
        item_id: format!("item-{n:02}"),
        thread_id: "thread-1".into(),
        source_kind: source_kind.into(),
        source_key: format!("{source_kind}:01TASK{n:02}"),
        source_revision: "1".into(),
        state: state.into(),
        summary: format!("件 {n}"),
        detail: String::new(),
        reason: Some(format!("理由 {n}")),
        operation_id: None,
        run_id: Some("run-1".into()),
        created_at: "2026-11-02T00:00:00Z".into(),
        route: None,
        decision: None,
    }
}

fn input(run_state: &str, reason: Option<&str>, items: Vec<CosTriageItemReport>) -> CosDigestInput {
    CosDigestInput {
        thread: "thread-1".into(),
        run_id: "run-1".into(),
        run_state: run_state.into(),
        reason: reason.map(str::to_owned),
        items,
    }
}

fn option(key: &str, label: &str, detail: Option<&str>) -> CosTriageOption {
    CosTriageOption {
        key: key.into(),
        label: label.into(),
        detail: detail.map(str::to_owned),
    }
}

fn full_decision() -> CosTriageDecision {
    CosTriageDecision {
        summary: "どちらで進めるか".into(),
        background: Some("背景の説明".into()),
        on_answer: Some("答えると再開する".into()),
        options: vec![option("a", "A 案", None), option("b", "B 案", None)],
        recommended: Some("a".into()),
        recommendation_reason: Some("A は戻しやすい".into()),
        web_path: "/tasks/01TASK/decisions".into(),
    }
}

fn hand_cases() -> Vec<(String, CosDigestInput)> {
    let mut cases = Vec::new();
    // Run states (4 known and an unknown value), with one item of every state.
    for run_state in RUN_STATES {
        let items = ITEM_STATES
            .iter()
            .enumerate()
            .map(|(n, s)| item(n, SOURCE_KINDS[n % SOURCE_KINDS.len()], s))
            .collect();
        cases.push((
            format!("hand/run-state/{run_state}"),
            input(run_state, None, items),
        ));
        cases.push((
            format!("hand/run-state/{run_state}/empty"),
            input(run_state, None, Vec::new()),
        ));
    }
    // The failed reason: present, blank, absent.
    for (name, reason) in [
        ("present", Some("timeout")),
        ("blank", Some("  ")),
        ("absent", None),
    ] {
        cases.push((
            format!("hand/failed-reason/{name}"),
            input("failed", reason, vec![item(0, "decision", "answered")]),
        ));
    }
    // Item reason: absent, empty, blank, padded.
    for (name, reason) in [
        ("absent", None),
        ("empty", Some("")),
        ("blank", Some("   ")),
        ("padded", Some("  前後に空白  ")),
    ] {
        let mut it = item(0, "question", "observed");
        it.reason = reason.map(str::to_owned);
        cases.push((
            format!("hand/item-reason/{name}"),
            input("completed", None, vec![it]),
        ));
    }
    // Blank summary: the title falls back to the source triple.
    let mut it = item(0, "notice", "resolved");
    it.summary = "  ".into();
    cases.push((
        "hand/title-fallback".into(),
        input("completed", None, vec![it]),
    ));
    // Decision packets on escalated / fallback / running / handled items, routed to the person
    // (`escalation`) or to the external operator (`operator`; ADR 2026-11-12-cos-triage-scope D2:
    // the judgment line and the card state of an escalated item differ, other states do not).
    for route in ["escalation", "operator"] {
        for state in ["escalated", "fallback", "running", "handled"] {
            for (name, decision) in [
                ("none", None),
                ("empty", Some(CosTriageDecision::default())),
                ("full", Some(full_decision())),
                (
                    "empty-strings",
                    Some(CosTriageDecision {
                        background: Some(String::new()),
                        on_answer: Some(String::new()),
                        recommendation_reason: Some(String::new()),
                        recommended: Some(String::new()),
                        ..CosTriageDecision::default()
                    }),
                ),
                (
                    "option-details",
                    Some(CosTriageDecision {
                        options: vec![
                            option("a", "A 案", Some("A を選ぶと…")),
                            option("b", "B 案", None),
                        ],
                        recommended: Some("b".into()),
                        ..full_decision()
                    }),
                ),
                (
                    "recommended-unknown",
                    Some(CosTriageDecision {
                        recommended: Some("z".into()),
                        ..full_decision()
                    }),
                ),
            ] {
                for run_state in ["completed", "stopped"] {
                    let mut it = item(0, "decision", state);
                    it.route =
                        (route == "operator" || decision.is_some()).then(|| route.to_owned());
                    it.decision = decision.clone();
                    it.operation_id = Some("op-1".into());
                    cases.push((
                        format!("hand/decision/{route}/{state}/{name}/{run_state}"),
                        input(run_state, None, vec![it]),
                    ));
                }
            }
        }
    }
    cases
}

fn random_text(rng: &mut SplitMix64) -> String {
    rng.pick(TEXTS).to_owned()
}

fn random_opt_text(rng: &mut SplitMix64) -> Option<String> {
    if rng.chance(1, 3) {
        None
    } else {
        Some(random_text(rng))
    }
}

fn random_decision(rng: &mut SplitMix64) -> Option<CosTriageDecision> {
    if rng.chance(1, 3) {
        return None;
    }
    let keys = ["a", "b", "c"];
    let options = (0..rng.below(4))
        .map(|n| {
            let detail = if rng.chance(1, 3) {
                Some(random_text(rng))
            } else {
                None
            };
            option(
                keys[n],
                &format!("{} 案", keys[n].to_uppercase()),
                detail.as_deref(),
            )
        })
        .collect();
    let recommended = if rng.chance(1, 2) {
        Some(rng.pick(&["a", "b", "c", "z", ""]).to_owned())
    } else {
        None
    };
    Some(CosTriageDecision {
        summary: random_text(rng),
        background: random_opt_text(rng),
        on_answer: random_opt_text(rng),
        options,
        recommended,
        recommendation_reason: random_opt_text(rng),
        web_path: rng.pick(&["", "/inbox/x", "/tasks/01TASK"]).to_owned(),
    })
}

fn random_input(rng: &mut SplitMix64) -> CosDigestInput {
    let run_state = rng.pick(RUN_STATES);
    let reason = random_opt_text(rng);
    let items = (0..rng.below(5))
        .map(|n| {
            let mut it = item(n, rng.pick(SOURCE_KINDS), rng.pick(ITEM_STATES));
            it.summary = random_text(rng);
            it.reason = random_opt_text(rng);
            it.operation_id = if rng.chance(1, 4) {
                Some(format!("op-{n}"))
            } else {
                None
            };
            it.decision = random_decision(rng);
            it.route = it.decision.as_ref().map(|_| "escalation".to_owned());
            it
        })
        .collect();
    input(run_state, reason.as_deref(), items)
}

fn push_line(
    out: &mut String,
    name: &str,
    input: &CosDigestInput,
) -> Result<(), serde_json::Error> {
    let commands = cos_digest_decide(input);
    out.push_str(&serde_json::to_string(&Line {
        name,
        input,
        commands: &commands,
    })?);
    out.push('\n');
    Ok(())
}

/// S4: hand-written cases then generated cases, one JSON object per line (trailing newline).
/// No two lines have the same input.
pub fn cos_digest_fixture_jsonl() -> Result<String, serde_json::Error> {
    let mut out = String::new();
    let mut seen = BTreeSet::new();
    for (name, input) in hand_cases() {
        if seen.insert(serde_json::to_string(&input)?) {
            push_line(&mut out, &name, &input)?;
        }
    }
    let mut rng = SplitMix64(SEED);
    let mut emitted = 0usize;
    while emitted < COS_DIGEST_GENERATED_CASES {
        let input = random_input(&mut rng);
        if !seen.insert(serde_json::to_string(&input)?) {
            continue;
        }
        push_line(&mut out, &format!("gen/{emitted:04}"), &input)?;
        emitted += 1;
    }
    Ok(out)
}
