use super::*;
use crate::execution_plan::{PhaseSpec, WorkUnitKind};

fn phases() -> Vec<PhaseSpec> {
    vec![
        PhaseSpec {
            key: "design".into(),
            kind: WorkUnitKind::Design,
            title: "design".into(),
        },
        PhaseSpec {
            key: "build".into(),
            kind: WorkUnitKind::Implement,
            title: "build".into(),
        },
        PhaseSpec {
            key: "verify".into(),
            kind: WorkUnitKind::Test,
            title: "verify".into(),
        },
    ]
}

/// ADR-0079 D2 / D5（Phase R1b）: /3 の `review: human` の段階は停止点になり、`pause_after` の解決と
/// 和をとる（段階の順・重複なし）。/2 は従来の解決と同じ（`review` を持たない）。
#[test]
fn plan_pause_points_include_review_human_stages() {
    let v3: crate::execution_plan::ExecutionPlanSpec = serde_json::from_value(serde_json::json!({
        "schema": crate::execution_plan::EXECUTION_PLAN_SCHEMA_V3,
        "rationale": "r",
        "stages": [
            {"key": "p1", "kind": "implement", "title": "Phase 1"},
            {"key": "p2", "kind": "design", "title": "Phase 2", "review": "human"},
            {"key": "p3", "kind": "implement", "title": "Phase 3"}
        ],
        "units": []
    }))
    .unwrap();
    assert_eq!(
        resolve_plan_pause_points(&PausePolicy::None, &v3),
        vec!["p2".to_string()]
    );
    assert_eq!(
        resolve_plan_pause_points(
            &PausePolicy::After {
                phases: vec!["p1".into(), "p2".into()]
            },
            &v3
        ),
        vec!["p1".to_string(), "p2".to_string()]
    );
    assert_eq!(
        resolve_plan_pause_points(&PausePolicy::EachPhase, &v3),
        vec!["p1".to_string(), "p2".to_string()]
    );
    let v2 = crate::execution_plan::ExecutionPlanSpec {
        schema: crate::execution_plan::EXECUTION_PLAN_SCHEMA_V2.into(),
        rationale: "r".into(),
        phases: phases(),
        work_units: Vec::new(),
        children: Vec::new(),
        stages: Vec::new(),
        units: Vec::new(),
        decisions: Vec::new(),
    };
    for policy in [
        PausePolicy::None,
        PausePolicy::EachPhase,
        PausePolicy::After {
            phases: vec!["build".into()],
        },
    ] {
        assert_eq!(
            resolve_plan_pause_points(&policy, &v2),
            resolve_pause_points(&policy, &v2.phases)
        );
    }
}

#[test]
fn none_never_pauses() {
    assert_eq!(
        resolve_pause_points(&PausePolicy::None, &phases()),
        Vec::<String>::new()
    );
    assert_eq!(
        resolve_pause_points(&PausePolicy::None, &[]),
        Vec::<String>::new()
    );
}

#[test]
fn each_phase_excludes_the_last_phase() {
    assert_eq!(
        resolve_pause_points(&PausePolicy::EachPhase, &phases()),
        vec!["design".to_string(), "build".to_string()]
    );
    // 1 工程しかない計画では、除いた残りが空。
    let one = vec![PhaseSpec {
        key: "only".into(),
        kind: WorkUnitKind::Other,
        title: "only".into(),
    }];
    assert_eq!(
        resolve_pause_points(&PausePolicy::EachPhase, &one),
        Vec::<String>::new()
    );
}

#[test]
fn after_matches_by_key_or_kind() {
    let by_key = PausePolicy::After {
        phases: vec!["build".to_string()],
    };
    assert_eq!(
        resolve_pause_points(&by_key, &phases()),
        vec!["build".to_string()]
    );
    // `kind` でも当たる（計画の前に書ける。D2.1）。
    let by_kind = PausePolicy::After {
        phases: vec!["design".to_string()],
    };
    assert_eq!(
        resolve_pause_points(&by_kind, &phases()),
        vec!["design".to_string()]
    );
    let unknown = PausePolicy::After {
        phases: vec!["release".to_string()],
    };
    assert_eq!(
        resolve_pause_points(&unknown, &phases()),
        Vec::<String>::new()
    );
}

#[test]
fn v1_and_atomic_plans_have_no_phases_so_nothing_resolves() {
    for policy in [
        PausePolicy::None,
        PausePolicy::EachPhase,
        PausePolicy::After {
            phases: vec!["design".to_string()],
        },
    ] {
        assert_eq!(resolve_pause_points(&policy, &[]), Vec::<String>::new());
    }
}

fn sample_report() -> PhaseReport {
    PhaseReport {
        phase: "build".into(),
        phase_title: "build".into(),
        phases_done: vec!["design: 2 WU / 3 run / 12m".to_string()],
        work_units: vec!["wu-a: did the thing".to_string()],
        child_units: vec!["p1 Phase 1: done / 3 run / $0.40 — 完了しました".to_string()],
        integration: vec!["merged wu-a @ abc123".to_string()],
        diff_stat: vec!["src/main.rs | 3 +++".to_string()],
        next_phase: Some("verify".to_string()),
        next_phase_work_units: vec!["wu-verify".to_string()],
        quota_summary: "claude-oauth: 12% (measured)".to_string(),
        artifact_paths: vec!["artifacts/report.md".to_string()],
    }
}

fn report_byte_len(r: &PhaseReport) -> usize {
    serde_json::to_vec(r).map(|v| v.len()).unwrap_or(0)
}

fn pop_one(v: &mut Vec<String>) -> bool {
    if v.is_empty() {
        false
    } else {
        v.pop().is_some()
    }
}

/// Phase SD-3 以前の `truncate_phase_report` の実装そのまま（上限を引数にしただけ）。
/// 速い実装がこれとバイト単位で同じ出力を返すことを等価性テストで確かめる基準。
fn truncate_phase_report_reference(report: &mut PhaseReport, cap: usize) {
    while report_byte_len(report) > cap {
        let shrank = pop_one(&mut report.diff_stat)
            || pop_one(&mut report.integration)
            || pop_one(&mut report.phases_done)
            || pop_one(&mut report.work_units)
            || pop_one(&mut report.child_units)
            || pop_one(&mut report.next_phase_work_units)
            || pop_one(&mut report.artifact_paths);
        if !shrank {
            // 見出しだけでも 16 KiB を超えることは実運用上ない（`quota_summary` は 1 行）が、
            // 無限ループにはしない。
            break;
        }
    }
}

/// 旧実装の 1 回分の pop（見出しだけになったら false）。
fn reference_pop(r: &mut PhaseReport) -> bool {
    pop_one(&mut r.diff_stat)
        || pop_one(&mut r.integration)
        || pop_one(&mut r.phases_done)
        || pop_one(&mut r.work_units)
        || pop_one(&mut r.child_units)
        || pop_one(&mut r.next_phase_work_units)
        || pop_one(&mut r.artifact_paths)
}

/// 決定的な擬似乱数（splitmix64、固定 seed）。
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
    fn string(&mut self, max_len: usize) -> String {
        // JSON でエスケープされる文字・多バイト文字・サロゲートペアになる文字を混ぜる。
        const ALPHABET: &[char] = &[
            'a', 'b', 'z', 'A', '0', '9', ' ', '|', '+', '-', '/', '.', '"', '\\', '\n', '\t',
            '\r', '\u{1}', '\u{1f}', '\u{7f}', 'é', 'ß', '日', '本', '語', '🦀', '😀', '\u{2028}',
            '\u{feff}',
        ];
        let len = self.below(max_len + 1);
        (0..len)
            .map(|_| ALPHABET[self.below(ALPHABET.len())])
            .collect()
    }
    fn strings(&mut self, max_count: usize, max_len: usize) -> Vec<String> {
        let n = self.below(max_count + 1);
        (0..n).map(|_| self.string(max_len)).collect()
    }
}

fn generated_report(rng: &mut Rng, max_count: usize, max_len: usize) -> PhaseReport {
    // 一部の Vec だけを大きくする・空にする形も混ぜる。
    let count = |rng: &mut Rng| match rng.below(4) {
        0 => 0,
        1 => rng.below(3),
        _ => max_count,
    };
    let c = [
        count(rng),
        count(rng),
        count(rng),
        count(rng),
        count(rng),
        count(rng),
        count(rng),
    ];
    PhaseReport {
        phase: rng.string(12),
        phase_title: rng.string(40),
        phases_done: rng.strings(c[0], max_len),
        work_units: rng.strings(c[1], max_len),
        integration: rng.strings(c[2], max_len),
        diff_stat: rng.strings(c[3], max_len),
        next_phase: if rng.below(2) == 0 {
            None
        } else {
            Some(rng.string(20))
        },
        next_phase_work_units: rng.strings(c[4], max_len),
        quota_summary: rng.string(60),
        artifact_paths: rng.strings(c[5], max_len),
        child_units: rng.strings(c[6], max_len),
    }
}

/// 旧実装の pop 列に沿った直列化の長さ（0 回, 1 回, …, 見出しだけ）。
fn reference_len_sequence(r: &PhaseReport) -> Vec<usize> {
    let mut r = r.clone();
    let mut out = vec![report_byte_len(&r)];
    while reference_pop(&mut r) {
        out.push(report_byte_len(&r));
    }
    out
}

/// 速い実装と旧実装を同じ入力・同じ上限で回し、直列化のバイト列が一致することを確かめる。
fn assert_equivalent(input: &PhaseReport, cap: usize, case: &str) {
    let mut fast = input.clone();
    let mut reference = input.clone();
    truncate_phase_report_to(&mut fast, cap);
    truncate_phase_report_reference(&mut reference, cap);
    let fast_bytes = serde_json::to_vec(&fast).expect("serialize fast");
    let ref_bytes = serde_json::to_vec(&reference).expect("serialize reference");
    assert!(
        fast_bytes == ref_bytes && fast == reference,
        "mismatch ({case}, cap {cap}): fast {} bytes, reference {} bytes",
        fast_bytes.len(),
        ref_bytes.len()
    );
}

#[test]
fn truncate_matches_the_reference_implementation_on_generated_inputs() {
    let mut rng = Rng(0x5D3_7A11_C0FF_EE01);
    let mut cases = 0usize;
    // (レポート数, Vec あたりの最大要素数, 文字列の最大長): 任意の上限で境界を網羅する群
    // （旧実装は O(n²) なので入力は小さめ）。実際の 16 KiB 上限をまたぐ群は下で別に回す。
    for &(reports, max_count, max_len) in &[(600usize, 6usize, 8usize), (100, 12, 16)] {
        for i in 0..reports {
            let r = generated_report(&mut rng, max_count, max_len);
            let lens = reference_len_sequence(&r);
            // 不変条件（二分探索の前提）: pop するたびに直列化は狭義に短くなる。
            assert!(
                lens.windows(2).all(|w| w[1] < w[0]),
                "lengths not strictly decreasing: {lens:?}"
            );
            let full = lens[0];
            let headline = lens[lens.len() - 1];
            let mut caps = vec![
                PHASE_REPORT_MAX_BYTES,
                PHASE_REPORT_MAX_BYTES - 1,
                PHASE_REPORT_MAX_BYTES + 1,
                0,
                1,
                headline.saturating_sub(1),
                headline,
                headline + 1,
                full.saturating_sub(1),
                full,
                full + 1,
                usize::MAX,
            ];
            // pop 列の途中の長さちょうど・その前後（境界の上・下）。
            for _ in 0..6 {
                let l = lens[rng.below(lens.len())];
                caps.extend([l.saturating_sub(1), l, l + 1]);
            }
            // 範囲内の任意の上限。
            caps.push(headline + rng.below(full - headline + 1));
            for cap in caps {
                assert_equivalent(&r, cap, &format!("report {i} max_count {max_count}"));
                cases += 1;
            }
        }
    }
    // 実際の上限（公開関数）で、上限をまたぐ大きさの入力。
    let mut over_cap = 0usize;
    for i in 0..40 {
        let r = generated_report(&mut rng, 200, 60);
        if report_byte_len(&r) > PHASE_REPORT_MAX_BYTES {
            over_cap += 1;
        }
        let mut fast = r.clone();
        let mut reference = r.clone();
        truncate_phase_report(&mut fast);
        truncate_phase_report_reference(&mut reference, PHASE_REPORT_MAX_BYTES);
        assert_eq!(
            serde_json::to_vec(&fast).expect("serialize fast"),
            serde_json::to_vec(&reference).expect("serialize reference"),
            "public fn mismatch on large report {i}"
        );
        cases += 1;
    }
    // 見出しだけで上限を超える退化した入力（全部落として止まる）。
    let mut huge = sample_report();
    huge.quota_summary = "x".repeat(PHASE_REPORT_MAX_BYTES + 10);
    for cap in [0, 1, PHASE_REPORT_MAX_BYTES, PHASE_REPORT_MAX_BYTES + 200] {
        assert_equivalent(&huge, cap, "huge headline");
        cases += 1;
    }
    assert_equivalent(&PhaseReport::default(), 0, "empty report");
    cases += 1;
    eprintln!(
        "truncate equivalence: {cases} generated cases, all byte-identical \
             ({over_cap}/40 large reports were over the 16 KiB cap)"
    );
    assert!(cases > 10_000, "too few cases: {cases}");
    assert!(
        over_cap >= 10,
        "too few large reports over the cap: {over_cap}"
    );
}

/// ADR-0079 §7 R4a (c): 子の要約の行（`child_units`）も 16 KiB の切り詰めを通る。落とす順は `work_units` の後
/// （子の要約は WU の段落より後まで残る）で、見出しと `quota_summary` は残る。
#[test]
fn child_unit_summaries_are_truncated_after_work_units() {
    let mut r = sample_report();
    for i in 0..400 {
        r.work_units.push(format!("wu-{i}: {}", "x".repeat(40)));
        r.child_units.push(format!(
            "c{i} child {i}: done / 2 run / $0.10 — {}",
            "y".repeat(40)
        ));
    }
    let child_before = r.child_units.clone();
    let mut expected = r.clone();
    truncate_phase_report(&mut r);
    assert!(report_byte_len(&r) <= PHASE_REPORT_MAX_BYTES);
    assert!(r.work_units.is_empty(), "work units go first");
    assert!(!r.child_units.is_empty(), "child summaries survive longer");
    assert_eq!(r.child_units[..], child_before[..r.child_units.len()]);
    assert_eq!(r.quota_summary, sample_report().quota_summary);
    truncate_phase_report_reference(&mut expected, PHASE_REPORT_MAX_BYTES);
    assert_eq!(r, expected, "same cut as the reference implementation");
}

/// 遅かったテストと同じ 1 万要素の入力で、旧実装と同じ点（上限以下になる最小の pop 回数）で止まることを
/// 確かめる（旧実装そのものを回すと約 46 s かかるので、最小性で代える: 1 つ戻すと上限を超える）。
#[test]
fn truncate_stops_at_the_first_pop_that_fits_on_the_large_input() {
    let mut original = sample_report();
    for i in 0..5000 {
        original.diff_stat.push(format!("file-{i}.rs | 1 +"));
        original
            .integration
            .push(format!("merged wu-{i} @ deadbeef"));
    }
    let mut r = original.clone();
    truncate_phase_report(&mut r);
    assert!(report_byte_len(&r) <= PHASE_REPORT_MAX_BYTES);
    // diff_stat が全部落ち、integration の途中で止まるはず。
    assert!(r.diff_stat.is_empty());
    assert!(!r.integration.is_empty());
    assert_eq!(
        r.integration[..],
        original.integration[..r.integration.len()]
    );
    let mut one_back = r.clone();
    one_back
        .integration
        .push(original.integration[r.integration.len()].clone());
    assert!(report_byte_len(&one_back) > PHASE_REPORT_MAX_BYTES);
    assert_eq!(r.phases_done, original.phases_done);
    assert_eq!(r.work_units, original.work_units);
    assert_eq!(r.next_phase_work_units, original.next_phase_work_units);
    assert_eq!(r.artifact_paths, original.artifact_paths);
}

#[test]
fn truncate_is_a_no_op_under_the_cap() {
    let mut r = sample_report();
    let before = r.clone();
    truncate_phase_report(&mut r);
    assert_eq!(r, before);
}

#[test]
fn truncate_shrinks_to_the_overall_byte_cap_and_keeps_the_headline() {
    let mut r = sample_report();
    for i in 0..5000 {
        r.diff_stat.push(format!("file-{i}.rs | 1 +"));
        r.integration.push(format!("merged wu-{i} @ deadbeef"));
    }
    truncate_phase_report(&mut r);
    assert!(
        report_byte_len(&r) <= PHASE_REPORT_MAX_BYTES,
        "got {}",
        report_byte_len(&r)
    );
    assert_eq!(r.phase, "build");
    assert_eq!(r.quota_summary, "claude-oauth: 12% (measured)");
}

#[test]
fn format_wall_ms_examples() {
    assert_eq!(format_wall_ms(0), "0m");
    assert_eq!(format_wall_ms(59_000), "0m");
    assert_eq!(format_wall_ms(60_000), "1m");
    assert_eq!(format_wall_ms(12 * 60_000), "12m");
    assert_eq!(format_wall_ms(3_600_000 + 5 * 60_000), "1h5m");
    assert_eq!(format_wall_ms(-1), "0m", "負値は 0 として扱う");
}

#[test]
fn quota_summary_line_reports_no_consumption_when_metrics_has_no_quota() {
    let task = crate::model_policy::tests::task("o", vec![]);
    let metrics = crate::execution_metrics::summarize(&task, &[]);
    let line = quota_summary_line(&metrics);
    assert!(line.starts_with("quota: 消費なし"), "{line}");
    assert!(line.contains("参考"), "{line}");
}

#[test]
fn quota_summary_line_formats_measured_quota_and_cost() {
    use crate::model::Event;
    use crate::quota::{QuotaMethod, QuotaWindow, QuotaWindowUse};
    let task = crate::model_policy::tests::task("o", vec![]);
    let events = vec![Event::QuotaEstimated {
        run_id: "r1".to_string(),
        work_unit_id: None,
        source: "claude-oauth".to_string(),
        account: Some("acct-a".to_string()),
        windows: vec![QuotaWindowUse {
            window: QuotaWindow::FiveHour,
            before: None,
            after: None,
            resets_at: None,
            used_pct: Some(3.25),
            method: QuotaMethod::Measured,
        }],
        weighted_tokens: 100.0,
        method: QuotaMethod::Measured,
        calibration: None,
        weights_version: "quota-weights/1".to_string(),
        list_price_usd: Some(1.5),
    }];
    let metrics = crate::execution_metrics::summarize(&task, &events);
    let line = quota_summary_line(&metrics);
    assert!(line.contains("acct-a"), "{line}");
    assert!(line.contains("five_hour"), "{line}");
    assert!(line.contains("3.3pt") || line.contains("3.2pt"), "{line}");
    assert!(
        line.contains("参考 $1.50") || line.contains("参考 不明"),
        "{line}"
    );
}
