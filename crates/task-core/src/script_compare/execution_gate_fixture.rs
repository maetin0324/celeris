//! ADR 2026-11-04-ts-execution-gate-domain: `execution_gate` の比較 fixture（control-ts/fixtures/execution_gate/cases.jsonl）。
//! 期待値は `execution_gate::score` の出力。境界の手書き例（`hand/…`）、規則が読む 5 軸の全組み合わせ（`axes/…`）、
//! 固定 seed の SplitMix64 で作った例（`gen/…`）の順に並ぶ。本文は合成の語だけで、実 task の本文を使わない。

use serde::Serialize;

use crate::execution_gate::{ExecutionGateInput, ExecutionGateOutput, score};
use crate::model_policy::{Level, TaskFeatures};

/// 生成する例の件数。
const GENERATED_CASES: usize = 2000;
/// SplitMix64 の固定 seed。変えると fixture 全体が変わる。
const SEED: u64 = 0x5EED_2026_1104_0002;

const LEVELS: [Level; 3] = [Level::Low, Level::Medium, Level::High];

/// 工程語の群を 0〜6 種含む本文。大小文字・全角を混ぜる。
const TEXTS: [&str; 7] = [
    "report the numbers",
    "調査する",
    "DESIGN と実装",
    "Investigate, 設計, Implement",
    "survey; design; テスト; REVIEW",
    "調査 design 実装 test レビュー",
    "調査 設計 実装 テスト review 配送",
];

/// 小文字化の差が出やすい本文（Rust の `to_lowercase` と JS の `toLowerCase` が同じであることを固定する）。
const UNICODE_TEXTS: [&str; 10] = [
    "REVİEW",
    "TEST",
    "ＴＥＳＴ",
    "ΣΑΣ design",
    "DEPLOY ß ẞ",
    "ſurvey",
    "\u{212A}elvin RELEASE",
    "テスト\u{0301}",
    "te\u{00AD}st",
    "🚀 IMPLEMENT 🚀 DESIGN",
];

/// `{:.2}` の丸めの境界（`m/8` の奇数の `m` は 2 桁の正確な中点で、Rust は偶数へ丸める）。
const RATIOS: [f64; 13] = [
    0.0,
    0.29,
    0.299_999_9,
    0.3,
    0.305,
    0.333_333_333_333_333_3,
    0.375,
    0.5,
    0.625,
    0.666_666_666_666_666_6,
    0.875,
    0.995,
    1.0,
];

#[derive(Serialize)]
struct FixtureLine<'a> {
    case: &'a str,
    input: &'a ExecutionGateInput,
    #[serde(flatten)]
    output: ExecutionGateOutput,
}

fn features(level: Level) -> TaskFeatures {
    TaskFeatures {
        judgment: level,
        ambiguity: level,
        verifiability: level,
        reversibility: level,
        consequence: level,
        context_size: level,
        tool_intensity: level,
        expected_length: level,
        cross_cutting: level,
    }
}

fn base() -> ExecutionGateInput {
    ExecutionGateInput {
        features: features(Level::Low),
        title: "title".to_owned(),
        objective: "objective".to_owned(),
        acceptance_total: 1,
        artifact_exists: 0,
        has_human: false,
        has_command: true,
        multi_environment: false,
        recent_budget_exhausted_ratio: None,
        cos_hint_compound: false,
        max_turns: 60,
        tree_child: false,
        threshold: 5,
    }
}

/// 境界の手書き例。
fn hand_cases() -> Vec<(String, ExecutionGateInput)> {
    let mut out: Vec<(String, ExecutionGateInput)> = Vec::new();
    let mut push = |name: String, edit: &dyn Fn(&mut ExecutionGateInput)| {
        let mut input = base();
        edit(&mut input);
        out.push((format!("hand/{name}"), input));
    };
    push("base".into(), &|_| {});
    for (i, text) in TEXTS.iter().enumerate() {
        push(format!("stages/{i}"), &|v| v.objective = (*text).to_owned());
        push(format!("stages-in-title/{i}"), &|v| {
            v.title = (*text).to_owned();
        });
    }
    // 題名と目的をまたぐ語は改行で切れる。
    push("word-split-by-newline".into(), &|v| {
        v.title = "te".into();
        v.objective = "st".into();
    });
    for (i, text) in UNICODE_TEXTS.iter().enumerate() {
        push(format!("unicode/{i}"), &|v| {
            v.objective = (*text).to_owned();
        });
    }
    for (i, ratio) in RATIOS.iter().enumerate() {
        push(format!("ratio/{i}"), &|v| {
            v.recent_budget_exhausted_ratio = Some(*ratio);
        });
    }
    for n in [3usize, 399, 400, 401, 2000, 2001] {
        for (word, unit) in [("ascii", "a"), ("kana", "あ"), ("emoji", "🚀")] {
            for max_turns in [10u32, 11] {
                push(format!("objective-{word}-{n}/turns-{max_turns}"), &|v| {
                    v.objective = unit.repeat(n);
                    v.max_turns = max_turns;
                });
            }
        }
    }
    for tree_child in [false, true] {
        push(format!("small/tree-child-{tree_child}"), &|v| {
            v.max_turns = 5;
            v.tree_child = tree_child;
            v.cos_hint_compound = true;
            v.features.context_size = Level::High;
            v.features.cross_cutting = Level::High;
            v.multi_environment = true;
        });
    }
    for (total, artifacts) in [(5, 2), (5, 3), (6, 0), (6, 3), (0, 0)] {
        push(format!("acceptance/{total}-{artifacts}"), &|v| {
            v.acceptance_total = total;
            v.artifact_exists = artifacts;
        });
    }
    for (human, command) in [(false, false), (true, false), (false, true), (true, true)] {
        push(format!("checks/{human}-{command}"), &|v| {
            v.has_human = human;
            v.has_command = command;
        });
    }
    // 閾値の両側。score 5（F1 2・F4 2・F2 の代わりに S4 1）。
    for threshold in [4, 5, 6, 7, 9, 11] {
        push(format!("threshold/{threshold}"), &|v| {
            v.threshold = threshold;
            v.features.context_size = Level::High;
            v.features.cross_cutting = Level::High;
            v.multi_environment = true;
        });
    }
    push("forced/long-and-broad-beats-small".into(), &|v| {
        v.max_turns = 1;
        v.features.expected_length = Level::High;
        v.features.cross_cutting = Level::High;
        v.threshold = 100;
    });
    push("features/low-tool-judgment-high".into(), &|v| {
        v.features.judgment = Level::High;
        v.features.tool_intensity = Level::Low;
    });
    out
}

/// 規則が読む 5 軸（context_size・expected_length・tool_intensity・cross_cutting・judgment）の全組み合わせ。
fn axes_cases() -> Vec<(String, ExecutionGateInput)> {
    let mut out = Vec::new();
    for context_size in LEVELS {
        for expected_length in LEVELS {
            for tool_intensity in LEVELS {
                for cross_cutting in LEVELS {
                    for judgment in LEVELS {
                        let mut input = base();
                        input.features.context_size = context_size;
                        input.features.expected_length = expected_length;
                        input.features.tool_intensity = tool_intensity;
                        input.features.cross_cutting = cross_cutting;
                        input.features.judgment = judgment;
                        let name = format!(
                            "axes/{}-{}-{}-{}-{}",
                            context_size.as_str(),
                            expected_length.as_str(),
                            tool_intensity.as_str(),
                            cross_cutting.as_str(),
                            judgment.as_str(),
                        );
                        out.push((name, input));
                    }
                }
            }
        }
    }
    out
}

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
        usize::try_from(self.next() % n as u64).unwrap_or(0)
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len())]
    }

    fn flag(&mut self) -> bool {
        self.next() & 1 == 1
    }
}

fn generated_cases() -> Vec<(String, ExecutionGateInput)> {
    let mut rng = SplitMix64(SEED);
    (0..GENERATED_CASES)
        .map(|i| {
            let mut level = || rng.pick(&LEVELS);
            let features = TaskFeatures {
                judgment: level(),
                ambiguity: level(),
                verifiability: level(),
                reversibility: level(),
                consequence: level(),
                context_size: level(),
                tool_intensity: level(),
                expected_length: level(),
                cross_cutting: level(),
            };
            let padding = rng.pick(&[0usize, 10, 398, 399, 400, 1999, 2000, 2001]);
            let objective = format!(
                "{}{}",
                rng.pick(&TEXTS),
                "x".repeat(if padding > 0 && rng.below(8) == 0 {
                    padding
                } else {
                    padding.min(20)
                })
            );
            let input = ExecutionGateInput {
                features,
                title: rng.pick(&TEXTS).to_owned(),
                objective,
                acceptance_total: rng.pick(&[0, 1, 3, 5, 6, 7, 20]),
                artifact_exists: rng.pick(&[0, 1, 2, 3, 4]),
                has_human: rng.flag(),
                has_command: rng.flag(),
                multi_environment: rng.flag(),
                recent_budget_exhausted_ratio: if rng.flag() {
                    None
                } else {
                    Some(rng.pick(&RATIOS))
                },
                cos_hint_compound: rng.flag(),
                max_turns: rng.pick(&[1, 5, 10, 11, 24, 60, 120]),
                tree_child: rng.flag(),
                threshold: rng.pick(&[5, 7, 9, 11]),
            };
            (format!("gen/{i}"), input)
        })
        .collect()
}

/// fixture 全体（1 行 1 例の JSONL）。`score` の変更はこの file の差分として現れる。
pub fn execution_gate_fixture_jsonl() -> Result<String, serde_json::Error> {
    let mut body = String::new();
    for (case, input) in hand_cases()
        .into_iter()
        .chain(axes_cases())
        .chain(generated_cases())
    {
        let line = FixtureLine {
            case: &case,
            input: &input,
            output: score(&input),
        };
        body.push_str(&serde_json::to_string(&line)?);
        body.push('\n');
    }
    Ok(body)
}
