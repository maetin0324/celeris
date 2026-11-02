//! S2: `route_priority` の比較 fixture（control-ts/fixtures/route_priority/cases.jsonl）。
//! 入力は合成の `Registry`（複数 harness。候補 key の列だけを埋める）を `RoutePriorityInput::redacted`
//! に通したものだけ。期待値は `llm_route::route_priority_decide` の出力。
//! 手書き例（`hand/…`）の後に、固定 seed の SplitMix64 で作った例（`gen/…`）が続く。
//! `examples/dump-route-priority-fixtures.rs` と `tests/script_fixtures.rs` が同じ関数を使う。

use std::collections::BTreeSet;

use serde::Serialize;

use super::{RoutePriorityInput, RoutePriorityOutput};
use crate::llm_route::{
    CandidateKey, ExecutionClass, HarnessId, ModelId, Registry, SourceId, route_priority_decide,
};

/// 生成する例の件数（入力が重ならない件数）。
const GENERATED_CASES: usize = 2000;
/// SplitMix64 の固定 seed。変えると fixture 全体が変わる。
const SEED: u64 = 0x5EED_2026_1102_0002;

#[derive(Serialize)]
struct FixtureLine<'a> {
    case: &'a str,
    input: &'a RoutePriorityInput,
    #[serde(flatten)]
    output: RoutePriorityOutput,
}

/// 1 件の問い合わせ（`route_priority` の引数のうち registry 以外）。
struct Query {
    harness: HarnessId,
    lane: Option<String>,
    cos: bool,
    execution_class: Option<ExecutionClass>,
}

impl Query {
    fn new(harness: &str, lane: Option<&str>, cos: bool, class: Option<&str>) -> Self {
        Self {
            harness: harness.into(),
            lane: lane.map(str::to_owned),
            cos,
            execution_class: class.map(ExecutionClass::from),
        }
    }

    fn input(&self, registry: &Registry) -> RoutePriorityInput {
        RoutePriorityInput::redacted(
            registry,
            &self.harness,
            self.lane.as_deref(),
            self.cos,
            self.execution_class.as_ref(),
        )
    }

    fn output(&self, registry: &Registry) -> RoutePriorityOutput {
        RoutePriorityOutput::from_decision(route_priority_decide(
            registry,
            &self.harness,
            self.lane.as_deref(),
            self.cos,
            self.execution_class.as_ref(),
        ))
    }
}

fn key(source: &str, model: &str) -> CandidateKey {
    CandidateKey {
        source: SourceId::from(source),
        model: ModelId::from(model),
    }
}

fn keys(pairs: &[(&str, &str)]) -> Vec<CandidateKey> {
    pairs.iter().map(|(s, m)| key(s, m)).collect()
}

fn set_defaults(r: &mut Registry, harness: &str, pairs: &[(&str, &str)]) {
    r.route_defaults.insert(harness.into(), keys(pairs));
}

fn set_lane(r: &mut Registry, harness: &str, lane: &str, pairs: &[(&str, &str)]) {
    r.lane_route_defaults
        .insert((harness.into(), lane.to_owned()), keys(pairs));
}

fn set_class(r: &mut Registry, harness: &str, class: &str, pairs: &[(&str, &str)]) {
    r.route_class_defaults
        .entry(harness.into())
        .or_default()
        .insert(class.into(), keys(pairs));
}

/// 手書き例の土台: 対象の `claude_code` と、別の harness `codex` の行を持つ registry。
/// `codex` の行は `redacted` で落ちることを fixture で示すためにある。
fn base() -> Registry {
    let mut r = Registry::default();
    set_defaults(
        &mut r,
        "claude_code",
        &[("anthropic", "sonnet"), ("pool", "opus")],
    );
    set_lane(&mut r, "claude_code", "frontier", &[("pool", "opus")]);
    set_lane(
        &mut r,
        "claude_code",
        "standard",
        &[("anthropic", "sonnet"), ("pool", "opus")],
    );
    set_defaults(&mut r, "codex", &[("openai", "gpt-codex")]);
    set_lane(&mut r, "codex", "cheap", &[("openai", "gpt-mini")]);
    set_class(&mut r, "codex", "coding", &[("openai", "gpt-codex")]);
    r.cos_route_candidates = keys(&[("anthropic", "opus-cos")]);
    r
}

fn hand_cases() -> Vec<(&'static str, Registry, Query)> {
    let mut out = Vec::new();
    let cc = "claude_code";

    let mut r = base();
    set_class(
        &mut r,
        cc,
        "coding",
        &[("pool", "opus"), ("anthropic", "sonnet")],
    );
    out.push((
        "hand/class-declared",
        r.clone(),
        Query::new(cc, Some("standard"), false, Some("coding")),
    ));
    out.push((
        "hand/class-unknown-falls-to-lane",
        r.clone(),
        Query::new(cc, Some("standard"), false, Some("review")),
    ));
    out.push((
        "hand/class-unknown-falls-to-harness",
        r,
        Query::new(cc, None, false, Some("review")),
    ));

    let mut r = base();
    set_class(&mut r, cc, "coding", &[]);
    out.push((
        "hand/class-empty-row-is-none",
        r,
        Query::new(cc, Some("standard"), false, Some("coding")),
    ));

    out.push((
        "hand/class-declared-only-for-other-harness",
        base(),
        Query::new(cc, Some("frontier"), false, Some("coding")),
    ));

    out.push((
        "hand/lane-declared",
        base(),
        Query::new(cc, Some("frontier"), false, None),
    ));
    out.push((
        "hand/lane-unknown-falls-to-harness",
        base(),
        Query::new(cc, Some("turbo"), false, None),
    ));
    out.push((
        "hand/lane-case-differs-falls-to-harness",
        base(),
        Query::new(cc, Some("Frontier"), false, None),
    ));

    let mut r = base();
    set_lane(&mut r, cc, "cheap", &[]);
    out.push((
        "hand/lane-empty-row-is-none",
        r,
        Query::new(cc, Some("cheap"), false, None),
    ));

    let mut r = base();
    set_lane(&mut r, cc, "", &[("local", "qwen")]);
    out.push((
        "hand/lane-empty-string-row",
        r.clone(),
        Query::new(cc, Some(""), false, None),
    ));
    out.push((
        "hand/lane-null-takes-empty-string-row",
        r,
        Query::new(cc, None, false, None),
    ));
    out.push((
        "hand/lane-empty-string-without-row",
        base(),
        Query::new(cc, Some(""), false, None),
    ));
    out.push((
        "hand/lane-null-without-empty-string-row",
        base(),
        Query::new(cc, None, false, None),
    ));

    out.push((
        "hand/harness-without-rows",
        base(),
        Query::new("pi", Some("standard"), false, Some("coding")),
    ));
    let mut r = base();
    set_defaults(&mut r, cc, &[]);
    out.push((
        "hand/harness-defaults-empty-is-none",
        r,
        Query::new(cc, Some("turbo"), false, None),
    ));
    let mut r = Registry::default();
    set_lane(&mut r, cc, "frontier", &[("pool", "opus")]);
    out.push((
        "hand/lane-row-without-harness-defaults",
        r.clone(),
        Query::new(cc, Some("standard"), false, None),
    ));

    out.push((
        "hand/cos-candidates",
        base(),
        Query::new(cc, Some("frontier"), true, None),
    ));
    let mut r = base();
    r.cos_route_candidates.clear();
    out.push((
        "hand/cos-empty-is-none",
        r,
        Query::new(cc, Some("frontier"), true, None),
    ));
    let mut r = base();
    set_class(&mut r, cc, "coding", &[("pool", "opus")]);
    out.push((
        "hand/cos-with-class",
        r,
        Query::new(cc, Some("standard"), true, Some("coding")),
    ));

    let mut r = base();
    set_defaults(
        &mut r,
        cc,
        &[
            ("pool", "opus"),
            ("pool", "opus"),
            ("anthropic", "sonnet"),
            ("pool", "opus"),
        ],
    );
    out.push((
        "hand/duplicate-candidates",
        r,
        Query::new(cc, Some("turbo"), false, None),
    ));

    let mut r = base();
    set_lane(
        &mut r,
        "ハーネス",
        "速い",
        &[("ローカル", "qwen‐3.8 🚀"), ("Ünïcode", "模型-β")],
    );
    set_defaults(&mut r, "ハーネス", &[("ローカル", "既定")]);
    out.push((
        "hand/non-ascii-ids",
        r,
        Query::new("ハーネス", Some("速い"), false, Some("符号化")),
    ));

    let mut r = base();
    set_defaults(
        &mut r,
        cc,
        &[("src\"quote", "model\\back"), ("tab\there", "line\nbreak")],
    );
    out.push((
        "hand/json-escapes",
        r,
        Query::new(cc, Some("turbo"), false, None),
    ));

    let mut r = base();
    set_lane(&mut r, cc, "__proto__", &[("proto", "lane")]);
    set_class(&mut r, cc, "toString", &[("proto", "class")]);
    out.push((
        "hand/object-prototype-names-lane",
        r.clone(),
        Query::new(cc, Some("__proto__"), false, Some("constructor")),
    ));
    out.push((
        "hand/object-prototype-names-class",
        r,
        Query::new(cc, Some("constructor"), false, Some("toString")),
    ));

    out.push((
        "hand/all-empty",
        Registry::default(),
        Query::new(cc, None, false, None),
    ));
    out.push((
        "hand/all-empty-cos",
        Registry::default(),
        Query::new(cc, Some("standard"), true, Some("coding")),
    ));
    out
}

/// 固定 seed の SplitMix64（依存を足さない手書き）。
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// `0..n`（n > 0）。
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    /// 確率 num/den で真。
    fn chance(&mut self, num: usize, den: usize) -> bool {
        self.below(den) < num
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

const HARNESSES: [&str; 4] = ["claude_code", "codex", "pi", "ハーネス"];
/// 問い合わせだけに使う harness（registry に行が無い）。
const QUERY_ONLY_HARNESSES: [&str; 1] = ["ghost"];
const LANES: [&str; 5] = ["frontier", "standard", "cheap", "", "__proto__"];
const QUERY_ONLY_LANES: [&str; 2] = ["turbo", "Standard"];
const CLASSES: [&str; 4] = ["coding", "conversation", "review", "constructor"];
const QUERY_ONLY_CLASSES: [&str; 1] = ["plan"];
const SOURCES: [&str; 4] = ["anthropic", "pool", "local", "ローカル"];
const MODELS: [&str; 5] = ["opus", "sonnet", "haiku", "qwen", "模型"];

/// 長さ `1..=max_len` の列。確率 1/6 で空の列（宣言された空の行）。
fn random_list(rng: &mut SplitMix64, max_len: usize) -> Vec<CandidateKey> {
    let len = if rng.chance(1, 6) {
        0
    } else {
        1 + rng.below(max_len)
    };
    (0..len)
        .map(|_| {
            let s = rng.pick(&SOURCES);
            let m = rng.pick(&MODELS);
            key(s, m)
        })
        .collect()
}

fn random_registry(rng: &mut SplitMix64) -> Registry {
    let mut r = Registry::default();
    for harness in HARNESSES {
        if rng.chance(1, 8) {
            continue;
        }
        if rng.chance(3, 4) {
            r.route_defaults.insert(harness.into(), random_list(rng, 3));
        }
        for lane in LANES {
            if rng.chance(1, 3) {
                r.lane_route_defaults
                    .insert((harness.into(), lane.to_owned()), random_list(rng, 3));
            }
        }
        for class in CLASSES {
            if rng.chance(1, 4) {
                r.route_class_defaults
                    .entry(harness.into())
                    .or_default()
                    .insert(class.into(), random_list(rng, 3));
            }
        }
    }
    r.cos_route_candidates = random_list(rng, 2);
    r
}

fn random_query(rng: &mut SplitMix64) -> Query {
    let harness = if rng.chance(1, 10) {
        rng.pick(&QUERY_ONLY_HARNESSES)
    } else {
        rng.pick(&HARNESSES)
    };
    let lane = if rng.chance(1, 4) {
        None
    } else if rng.chance(1, 8) {
        Some(rng.pick(&QUERY_ONLY_LANES))
    } else {
        Some(rng.pick(&LANES))
    };
    let cos = rng.chance(1, 8);
    let class = if rng.chance(1, 2) {
        None
    } else if rng.chance(1, 6) {
        Some(rng.pick(&QUERY_ONLY_CLASSES))
    } else {
        Some(rng.pick(&CLASSES))
    };
    Query::new(harness, lane, cos, class)
}

fn push_line(
    out: &mut String,
    case: &str,
    input: &RoutePriorityInput,
    output: RoutePriorityOutput,
) -> Result<(), serde_json::Error> {
    out.push_str(&serde_json::to_string(&FixtureLine {
        case,
        input,
        output,
    })?);
    out.push('\n');
    Ok(())
}

/// S2: 手書き例と生成例を 1 行 1 件にした JSONL（末尾改行つき）。
/// 1 行は `{"case":…,"input":<RoutePriorityInput>,"basis":…,"candidates":[…]}`。
/// 入力（`input` の直列化）が同じ行は作らない。
pub fn route_priority_fixture_jsonl() -> Result<String, serde_json::Error> {
    let mut out = String::new();
    let mut seen = BTreeSet::new();
    for (case, registry, query) in hand_cases() {
        let input = query.input(&registry);
        seen.insert(serde_json::to_string(&input)?);
        push_line(&mut out, case, &input, query.output(&registry))?;
    }
    let mut rng = SplitMix64(SEED);
    let mut emitted = 0usize;
    while emitted < GENERATED_CASES {
        let registry = random_registry(&mut rng);
        let query = random_query(&mut rng);
        let input = query.input(&registry);
        if !seen.insert(serde_json::to_string(&input)?) {
            continue;
        }
        let case = format!("gen/{emitted:04}");
        push_line(&mut out, &case, &input, query.output(&registry))?;
        emitted += 1;
    }
    Ok(out)
}
