//! S2: commit 済みの lane 規則 fixture（control-ts/fixtures/lane_rules/all-features.jsonl）が
//! `match_rule` の現在の出力と一致することを確かめる。食い違ったら
//! `cargo run -q -p task-core --example dump-lane-fixtures > control-ts/fixtures/lane_rules/all-features.jsonl`
//! で作り直し、規則表の変更として扱う。

use std::path::PathBuf;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../control-ts/fixtures/lane_rules/all-features.jsonl")
}

#[test]
fn script_fixtures_lane_rules_match_committed_fixture() {
    let generated = task_core::model_policy::lane_fixture_jsonl().expect("serialize fixture");
    let committed = std::fs::read_to_string(fixture_path()).expect("read committed fixture");
    assert_eq!(generated.lines().count(), 19_683);
    assert_eq!(committed.lines().count(), 19_683);
    assert!(
        generated == committed,
        "lane_rules fixture differs from match_rule output; regenerate with the dump-lane-fixtures example"
    );
}

#[test]
fn script_fixtures_lane_rules_cover_all_combinations_in_order() {
    let all = task_core::model_policy::all_features();
    assert_eq!(all.len(), 19_683);
    let keys: Vec<String> = all
        .iter()
        .map(|f| serde_json::to_string(f).expect("serialize features"))
        .collect();
    let unique: std::collections::BTreeSet<&String> = keys.iter().collect();
    assert_eq!(unique.len(), 19_683);
    let first = &all[0];
    let last = &all[all.len() - 1];
    assert!(
        serde_json::to_string(first)
            .expect("ser")
            .matches("\"low\"")
            .count()
            == 9
    );
    assert!(
        serde_json::to_string(last)
            .expect("ser")
            .matches("\"high\"")
            .count()
            == 9
    );
    // 最下位の軸（cross_cutting）が最も速く回る。
    assert_eq!(all[1].cross_cutting, task_core::Level::Medium);
    assert_eq!(all[1].judgment, task_core::Level::Low);
    assert_eq!(all[3usize.pow(8)].judgment, task_core::Level::Medium);
}
