//! ADR 2026-11-05-a9-llm-pilot-findings D2: llm_pilot の課題（C06・C08）は、公開の指示（`public/<case>/case.json` の
//! acceptance と `repo-seed/`）だけに従えば hidden の期待（`oracle/<case>/hidden/expected.json`）に届く。
//!
//! 公開の repo-seed だけを一時 dir に写し、acceptance が書くコマンドを実行して、採点と同じ `outcome_reached` で到達を
//! 確かめる。oracle の file は読むだけで、作業 dir には写さない。LLM を呼ばない（子 process は python3 だけ）。

use celeris_campaign::long_horizon::outcome_reached;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

const CASES: [&str; 2] = ["C06", "C08"];

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/long-horizon")
}

fn read(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn hidden(case: &str) -> BTreeMap<String, String> {
    serde_json::from_value(read(
        &fixtures()
            .join("oracle")
            .join(case)
            .join("hidden/expected.json"),
    ))
    .unwrap()
}

/// 公開の repo-seed だけを写した作業 dir。
fn seed_dir(case: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let seed = fixtures().join("public").join(case).join("repo-seed");
    for e in std::fs::read_dir(&seed).unwrap() {
        let e = e.unwrap();
        std::fs::copy(e.path(), dir.path().join(e.file_name())).unwrap();
    }
    dir
}

/// acceptance の文から、backtick で囲んだ公開の手順を取り出す。
fn public_step(acceptance: &[Value]) -> Vec<String> {
    let text = acceptance
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    let start = text
        .find('`')
        .expect("acceptance names a command in backticks")
        + 1;
    let len = text[start..].find('`').unwrap();
    text[start..start + len]
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

fn run(dir: &Path, argv: &[String]) {
    assert_eq!(argv[0], "python3", "{argv:?}");
    let st = Command::new("python3")
        .args(&argv[1..])
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(st.success(), "{argv:?}: {st}");
}

#[test]
fn long_horizon_public_contract_reaches_hidden_from_public_steps() {
    for case in CASES {
        let public = read(&fixtures().join("public").join(case).join("case.json"));
        let hidden = hidden(case);
        let dir = seed_dir(case);
        let outcomes = public["request_outcomes"].as_array().unwrap();
        assert_eq!(outcomes.len(), hidden.len(), "{case}");
        for o in outcomes {
            let id = o["id"].as_str().unwrap();
            assert!(
                !outcome_reached(dir.path(), &hidden, id),
                "{case} {id} before"
            );
            let argv = public_step(o["acceptance"].as_array().unwrap());
            assert_eq!(
                argv[..3],
                ["python3", "minichunk.py", "verify"].map(String::from),
                "{case} {id}"
            );
            run(dir.path(), &argv);
            assert!(
                outcome_reached(dir.path(), &hidden, id),
                "{case} {id}: the public step does not reach the hidden acceptance"
            );
        }
    }
}

/// 公開の手順と違う成果（手で書いた報告・別の case 名での実行）は到達にならない。
#[test]
fn long_horizon_public_contract_rejects_other_outputs() {
    for case in CASES {
        let hidden = hidden(case);
        let dir = seed_dir(case);
        std::fs::create_dir_all(dir.path().join("out")).unwrap();
        std::fs::write(dir.path().join("out/o1.txt"), "o1: 調査と実装の報告\n").unwrap();
        assert!(!outcome_reached(dir.path(), &hidden, "o1"), "{case}");
        let other = if case == "C06" { "C08" } else { "C06" };
        run(
            dir.path(),
            &["python3", "minichunk.py", "verify", other, "o1"].map(String::from),
        );
        assert!(!outcome_reached(dir.path(), &hidden, "o1"), "{case}");
    }
}

/// 公開側と台本の整合: acceptance の文は public と scenario で同じ（scripted 試験と pilot が同じ root を作る）、
/// 台本の content は公開の手順が書く内容と同じ。公開側は台本の content も hidden の hash も文字どおりには持たない。
#[test]
fn long_horizon_public_contract_matches_scenario_without_copying_it() {
    for case in CASES {
        let public = read(&fixtures().join("public").join(case).join("case.json"));
        let scenario = read(&fixtures().join("oracle").join(case).join("scenario.json"));
        let strip = |v: &Value| -> Vec<Value> {
            v.as_array()
                .unwrap()
                .iter()
                .map(|o| serde_json::json!([o["id"], o["title"], o["acceptance"], o["depends_on"]]))
                .collect()
        };
        assert_eq!(
            strip(&public["request_outcomes"]),
            strip(&scenario["outcomes"]),
            "{case}"
        );
        let dir = seed_dir(case);
        let mut public_text = String::new();
        for sub in [
            "case.json",
            "kb/README.md",
            "repo-seed/README.md",
            "repo-seed/minichunk.py",
        ] {
            public_text.push_str(
                &std::fs::read_to_string(fixtures().join("public").join(case).join(sub)).unwrap(),
            );
        }
        for o in public["request_outcomes"].as_array().unwrap() {
            let id = o["id"].as_str().unwrap();
            run(
                dir.path(),
                &public_step(o["acceptance"].as_array().unwrap()),
            );
            let wrote =
                std::fs::read_to_string(dir.path().join("out").join(format!("{id}.txt"))).unwrap();
            assert_eq!(
                scenario["content"][id].as_str().unwrap(),
                wrote,
                "{case} {id}"
            );
            assert!(
                !public_text.contains(wrote.trim_end()),
                "{case} {id}: public copies the content"
            );
            assert!(
                !public_text.contains(&hidden(case)[id]),
                "{case} {id}: public copies the hash"
            );
        }
    }
}
