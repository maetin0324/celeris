//! SetFixed の各欄・role_models と、実際に動いた全 run（観測値）の照合（fail closed）。mock 実行器で作った out の
//! trials.jsonl の `observed` を差し替えて checker に通す。外部 network・LLM は使わない。
mod common;
use std::{collections::BTreeMap, fs};

use celeris_campaign::{
    checker::{check, reconcile_set_evidence},
    executor::MockExecutor,
    lock::{CampaignLock, GraderPin, LockedCase},
    manifest::{
        Budget, CampaignManifest, Cap, Config, ConfigId, ExperimentSet, InputRef, Secondary,
        SetFixed, SnapshotRef, Split, Termination, manifest_hash,
    },
    record::*,
    runner::{RunOptions, run},
};
use routing_eval::manifest::Manifest as Input;
use serde_json::{Value, json};

fn input(n: usize) -> Input {
    let problems: Vec<Value> = (0..n)
        .map(|i| json!({"id":format!("p{i}"),"task_id":"t","work_unit_id":"w","created_at":"","genre":"coding","kind":"leaf","command":"true","prompt":"fix","split":"dev","stratum":"s","subset_12":false,"head_commit":null,"base_commit":null,"features_sha256":"","features":{},"sha256":""}))
        .collect();
    let mut m: Input = serde_json::from_value(json!({"schema":routing_eval::MANIFEST_SCHEMA,"version":"v","cutoff":"c","seed":1,"problems":problems,"sha256":""})).expect("input");
    m.sha256 = manifest_hash(&m).expect("hash");
    m
}

fn set(arms: &[ConfigId], compare: (ConfigId, ConfigId)) -> ExperimentSet {
    ExperimentSet {
        arms: arms.to_vec(),
        compare: vec![compare],
        fixed: Default::default(),
    }
}

/// `which` = "ab"（A/B のみ）か "bc"（B/C のみ）。測定 ID は which。
fn campaign(i: &Input, which: &str, n: usize, max_tokens: u64) -> CampaignManifest {
    let cfg = |id, h: &str, model: &str| Config {
        id,
        harness: h.into(),
        harness_version: "1".into(),
        model: model.into(),
        model_version: "1".into(),
        policy_version: "p1".into(),
        routing: None,
    };
    let (arms, pair, problems): (Vec<ConfigId>, _, Vec<String>) = if which == "ab" {
        (
            vec![ConfigId::A, ConfigId::B],
            (ConfigId::A, ConfigId::B),
            (0..n).map(|k| format!("p{k}")).collect(),
        )
    } else {
        (
            vec![ConfigId::B, ConfigId::C],
            (ConfigId::B, ConfigId::C),
            vec!["p0".to_string(), "p1".to_string()],
        )
    };
    let configs = arms
        .iter()
        .map(|a| match a {
            ConfigId::A => cfg(ConfigId::A, "claude-code", "sonnet"),
            ConfigId::B => cfg(ConfigId::B, "celeris", "sonnet"),
            ConfigId::C => cfg(ConfigId::C, "celeris", "sonnet"),
        })
        .collect();
    CampaignManifest {
        schema: celeris_campaign::CAMPAIGN_SCHEMA.into(),
        campaign_id: which.into(),
        version: "v1".into(),
        split: Split::Dev,
        input: InputRef {
            path: "input.json".into(),
            sha256: i.sha256.clone(),
        },
        problems: problems.clone(),
        configs,
        experiment_sets: BTreeMap::from([(which.to_string(), set(&arms, pair))]),
        selected_sets: vec![which.to_string()],
        primary: (ConfigId::A, ConfigId::B),
        secondary: Secondary {
            pair: (ConfigId::B, ConfigId::C),
            subset: if which == "bc" { problems } else { Vec::new() },
        },
        budget: Budget {
            token_basis: celeris_campaign::manifest::TokenBasis::InputOutput,
            max_tokens,
            max_wall_secs: 1_000_000,
            max_disk_bytes: 1 << 40,
            max_parallel: 4,
            per_case: Cap {
                max_tokens: 100_000,
                max_wall_secs: 1_800,
            },
            per_arm: Cap {
                max_tokens: 10_000_000_000,
                max_wall_secs: 1_000_000,
            },
        },
        termination: Termination {
            trial_timeout_secs: 1800,
            max_attempts_per_trial: 1,
            stop_on_budget: true,
        },
        snapshot: SnapshotRef {
            repo_commit: "abc".into(),
            materials_sha256: "d".into(),
            kb_snapshot: "kb@1".into(),
        },
        order: Default::default(),
        seed: 0,
        repeats: 1,
        dependencies: BTreeMap::new(),
        sha256: String::new(),
    }
}

const SONNET: &str = "claude-sonnet-5-5";
const MATERIALS: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn full_fixed() -> SetFixed {
    SetFixed {
        policy: Some("tier-v1".into()),
        models: Some(vec![SONNET.into()]),
        harnesses: Some(vec!["claude-code".into()]),
        role_models: ["planner", "reviewer", "cos", "worker"]
            .iter()
            .map(|r| (r.to_string(), SONNET.to_string()))
            .collect(),
        ..SetFixed::default()
    }
}

/// B の全役割（worker・planner・reviewer）の正しい実測 3 件。run id は試行ごとに別。
fn good_runs(tag: &str) -> Vec<Value> {
    ["worker", "planner", "reviewer"]
        .iter()
        .map(|role| {
            json!({"run_id": format!("{tag}-{role}"), "role": role, "model": SONNET,
                   "harness": "claude-code", "lower_policy": "tier-v1"})
        })
        .collect()
}

/// 固定欄を宣言した A/B の測定を mock で走らせ、B の各試行の `observed` を `mutate` で作って差し替えた結果を check する。
fn checked(fixed: SetFixed, mutate: impl Fn(usize, &mut Vec<Value>)) -> Vec<String> {
    let d = tempfile::tempdir().expect("dir");
    let i = input(2);
    let mut m = campaign(&i, "ab", 2, 10_000_000);
    m.experiment_sets.get_mut("ab").expect("set").fixed = fixed;
    let out = d.path().join("out");
    run(
        &m,
        &i,
        &MockExecutor::scripted(BTreeMap::new()),
        &RunOptions {
            out: out.clone(),
            ..RunOptions::default()
        },
    )
    .expect("run");
    let trials = out.join(TRIALS_FILE);
    let mut n = 0;
    let lines: Vec<String> = fs::read_to_string(&trials)
        .expect("t")
        .lines()
        .map(|l| {
            let mut v: Value = serde_json::from_str(l).expect("j");
            if v["config"] == "B" {
                let mut obs = good_runs(&format!("t{n}"));
                mutate(n, &mut obs);
                v["observed"] = Value::Array(obs);
                n += 1;
            }
            v.to_string()
        })
        .collect();
    fs::write(&trials, lines.join("\n") + "\n").expect("w");
    check(&m, &out).violations
}

fn has(v: &[String], needle: &str) -> bool {
    v.iter().any(|x| x.contains(needle))
}

#[test]
fn positive_all_roles_observed_match_every_fixed_field() {
    let v = checked(full_fixed(), |_, _| {});
    assert!(v.is_empty(), "{v:?}");
}

#[test]
fn legacy_set_without_fixed_still_reads_with_no_observed_runs() {
    // 旧形式（固定欄の宣言なし・observed なし）は従来どおり通る。
    let v = checked(SetFixed::default(), |_, o| o.clear());
    assert!(v.is_empty(), "{v:?}");
}

#[test]
fn role_model_mismatch_is_a_violation() {
    let v = checked(full_fixed(), |n, o| {
        if n == 1 {
            o[1]["model"] = json!("claude-sonnet-4-1");
        }
    });
    assert!(has(&v, "role_models declares"), "{v:?}");
    assert!(has(&v, "outside the set's fixed models"), "{v:?}");
}

#[test]
fn non_sonnet_child_run_is_a_violation() {
    let v = checked(full_fixed(), |n, o| {
        if n == 0 {
            o.push(
                json!({"run_id":"t0-grandchild","role":"worker","model":"claude-opus-5-5",
                          "harness":"claude-code","lower_policy":"tier-v1"}),
            );
        }
    });
    assert!(has(&v, "t0-grandchild") && has(&v, "non-Sonnet"), "{v:?}");
}

#[test]
fn none_fields_are_violations_not_skipped() {
    for (field, needle) in [
        ("model", "model not observed"),
        ("harness", "harness not observed"),
        ("lower_policy", "lower_policy not observed"),
    ] {
        let v = checked(full_fixed(), |n, o| {
            if n == 0 {
                o[2][field] = Value::Null;
            }
        });
        assert!(has(&v, needle), "{field}: {v:?}");
    }
}

#[test]
fn harness_and_policy_mismatches_are_violations() {
    let v = checked(full_fixed(), |n, o| {
        if n == 0 {
            o[0]["harness"] = json!("codex");
            o[1]["lower_policy"] = json!("tier-v2");
        }
    });
    assert!(has(&v, "outside the set's fixed harnesses"), "{v:?}");
    assert!(has(&v, "differs from the set's fixed policy"), "{v:?}");
}

#[test]
fn each_single_fixed_field_is_checked_on_its_own() {
    let only = |f: SetFixed| f;
    let m = |x: &mut Vec<Value>| x[0]["model"] = json!("other");
    let v = checked(
        only(SetFixed {
            models: full_fixed().models,
            ..SetFixed::default()
        }),
        |_, o| m(o),
    );
    assert!(has(&v, "outside the set's fixed models"), "{v:?}");
    let v = checked(
        SetFixed {
            harnesses: full_fixed().harnesses,
            ..SetFixed::default()
        },
        |_, o| o[0]["harness"] = json!("codex"),
    );
    assert!(has(&v, "outside the set's fixed harnesses"), "{v:?}");
    let v = checked(
        SetFixed {
            policy: full_fixed().policy,
            ..SetFixed::default()
        },
        |_, o| o[0]["lower_policy"] = json!("x"),
    );
    assert!(has(&v, "differs from the set's fixed policy"), "{v:?}");
}

#[test]
fn missing_evidence_is_not_success() {
    // 観測 run が 1 件も無い。
    let v = checked(full_fixed(), |_, o| o.clear());
    assert!(has(&v, "no run was observed"), "{v:?}");
    // worker run が無い（planner/reviewer だけ）。
    let v = checked(full_fixed(), |_, o| {
        o.remove(0);
    });
    assert!(has(&v, "no worker run observed"), "{v:?}");
    // role 欠測は worker で補わない。
    let v = checked(full_fixed(), |n, o| {
        if n == 0 {
            o[1]["role"] = json!("");
        }
    });
    assert!(has(&v, "role not observed"), "{v:?}");
    // 宣言に無い role。
    let v = checked(full_fixed(), |n, o| {
        if n == 0 {
            o[1]["role"] = json!("jev");
        }
    });
    assert!(
        has(&v, "no declared model in the set's role_models"),
        "{v:?}"
    );
    // run id の欠測・別試行との重複。
    let v = checked(full_fixed(), |n, o| {
        if n == 0 {
            o[1]["run_id"] = json!("");
        }
    });
    assert!(has(&v, "run id not observed"), "{v:?}");
    let v = checked(full_fixed(), |_, o| o[0]["run_id"] = json!("same"));
    assert!(has(&v, "also claimed by trial"), "{v:?}");
}

fn lock(materials: &str, grader: (&str, &str)) -> CampaignLock {
    let mut l: CampaignLock = serde_json::from_value(json!({
        "schema": "s", "version": "v", "measurement_id": "ab", "campaign_sha256": "",
        "materials_sha256": materials, "kb_snapshot": "kb", "cases": [],
        "execution": {"configs": [], "budget": serde_json::to_value(campaign(&input(1), "ab", 1, 1).budget).expect("b"),
            "termination": {"trial_timeout_secs": 1, "max_attempts_per_trial": 1, "stop_on_budget": true},
            "order": "abc_rotation", "seed": 0, "repeats": 1}
    }))
    .expect("lock");
    l.cases.push(LockedCase {
        id: "k1".into(),
        split: Split::Dev,
        implementation: false,
        initial: None,
        inputs: Vec::new(),
        grader: GraderPin {
            private_sha256: grader.0.into(),
            grade_case_sha256: grader.1.into(),
        },
        tests: Vec::new(),
    });
    l
}

fn evidence(fixed: SetFixed, snap: Option<Value>, l: Option<&CampaignLock>) -> Vec<String> {
    let mut m = campaign(&input(1), "ab", 1, 1);
    m.experiment_sets.get_mut("ab").expect("set").fixed = fixed;
    let mut v = Vec::new();
    reconcile_set_evidence(&m, snap.as_ref(), l, &mut v);
    v
}

#[test]
fn initial_info_and_success_are_matched_to_real_hashes() {
    let l = lock(MATERIALS, ("p1", "g1"));
    let want = SetFixed {
        initial_info: Some(MATERIALS.into()),
        success: Some(l.grader_set_sha256()),
        ..SetFixed::default()
    };
    let snap = || Some(json!({"materials_sha256": MATERIALS}));
    // 正例。
    assert!(evidence(want.clone(), snap(), Some(&l)).is_empty());
    // 宣言した材料 hash の改変。
    let mut f = want.clone();
    f.initial_info = Some("b".repeat(64));
    assert!(has(&evidence(f, snap(), Some(&l)), "initial_info"));
    // 実際に hash した材料 tree が lock と違う。
    let other = Some(json!({"materials_sha256": "c".repeat(64)}));
    assert!(has(&evidence(want.clone(), other, Some(&l)), "differs"));
    // grader 宣言の改変・lock 側の grader 改変。
    let mut f = want.clone();
    f.success = Some("d".repeat(64));
    assert!(has(&evidence(f, snap(), Some(&l)), "set fixed success"));
    let l2 = lock(MATERIALS, ("p1", "g2"));
    assert!(has(
        &evidence(want.clone(), snap(), Some(&l2)),
        "set fixed success"
    ));
    // 証拠（snapshot・lock）が無い、材料 hash 欠測は宣言で補わず違反。
    assert!(has(&evidence(want.clone(), None, Some(&l)), "missing"));
    assert!(has(&evidence(want.clone(), snap(), None), "missing"));
    assert!(has(
        &evidence(want.clone(), Some(json!({})), Some(&l)),
        "missing"
    ));
    // 宣言が無い欄は見ない（旧形式）。
    assert!(evidence(SetFixed::default(), None, None).is_empty());
}

// ---- B1/C1（B/C のみ）: 共通候補集合・harness・tier 対応・根拠表不使用を観測 run と照合する ----

const OPUS: &str = "claude-opus-5-5";

fn arm_routing(models: &[&str], policy: &str, c: bool) -> celeris_campaign::manifest::ArmRouting {
    celeris_campaign::manifest::ArmRouting {
        lower_policy: Some(policy.into()),
        models: Some(models.iter().map(|s| s.to_string()).collect()),
        run_harnesses: Some(vec!["claude-code".into()]),
        no_evidence_table: c.then_some(true),
        ..Default::default()
    }
}

fn bc_runs(arm: &str, tag: &str) -> Vec<Value> {
    let mode = if arm == "B" {
        "fixed"
    } else {
        "decision_assisted"
    };
    [("worker", SONNET), ("reviewer", OPUS)]
        .iter()
        .map(|(role, model)| {
            json!({"run_id": format!("{tag}-{role}"), "role": role, "model": model,
                   "harness": "claude-code", "lower_policy": "tier-v1",
                   "routing_mode": mode, "routing_source": "task",
                   "evidence_mode": "none", "evidence_table_id": null})
        })
        .collect()
}

/// B/C のみの測定。`decl` で宣言（B, C の routing）を、`mutate(arm, n, runs)` で観測 run を差し替える。
fn bc_checked(
    decl: impl Fn(&mut CampaignManifest),
    mutate: impl Fn(&str, usize, &mut Vec<Value>),
) -> Vec<String> {
    let d = tempfile::tempdir().expect("dir");
    let i = input(2);
    let mut m = campaign(&i, "bc", 2, 10_000_000);
    for c in m.configs.iter_mut() {
        c.routing = Some(arm_routing(&[SONNET, OPUS], "tier-v1", c.id == ConfigId::C));
    }
    decl(&mut m);
    let out = d.path().join("out");
    run(
        &m,
        &i,
        &MockExecutor::scripted(BTreeMap::new()),
        &RunOptions {
            out: out.clone(),
            ..RunOptions::default()
        },
    )
    .expect("run");
    let trials = out.join(TRIALS_FILE);
    let (mut nb, mut nc) = (0, 0);
    let lines: Vec<String> = fs::read_to_string(&trials)
        .expect("t")
        .lines()
        .map(|l| {
            let mut v: Value = serde_json::from_str(l).expect("j");
            let arm = v["config"].as_str().unwrap_or("").to_string();
            if arm == "B" || arm == "C" {
                let n = if arm == "B" { &mut nb } else { &mut nc };
                let mut obs = bc_runs(&arm, &format!("{arm}{n}"));
                mutate(&arm, *n, &mut obs);
                *n += 1;
                v["observed"] = Value::Array(obs);
            }
            v.to_string()
        })
        .collect();
    fs::write(&trials, lines.join("\n") + "\n").expect("w");
    check(&m, &out).violations
}

#[test]
fn b1_c1_positive_same_conditions_only_routing_mode_differs() {
    let v = bc_checked(|_| {}, |_, _, _| {});
    assert!(v.is_empty(), "{v:?}");
}

#[test]
fn b1_c1_declaration_pin_and_evidence_table_are_violations() {
    // C を単一 model に pin。
    let v = bc_checked(
        |m| {
            for c in m.configs.iter_mut().filter(|c| c.id == ConfigId::C) {
                c.routing.as_mut().expect("r").models = Some(vec![SONNET.into()]);
            }
        },
        |_, _, _| {},
    );
    assert!(has(&v, "B1/C1"), "{v:?}");
    // 根拠表を使う。
    let v = bc_checked(
        |m| {
            for c in m.configs.iter_mut().filter(|c| c.id == ConfigId::C) {
                let r = c.routing.as_mut().expect("r");
                r.no_evidence_table = None;
                r.evidence_table_id = Some("t1".into());
            }
        },
        |_, _, _| {},
    );
    assert!(has(&v, "evidence table"), "{v:?}");
}

#[test]
fn b1_c1_observed_evidence_table_and_single_pin_runs_are_violations() {
    // 観測 run が根拠表を使った（名前は none と宣言していても実測で偽装を検出）。
    let v = bc_checked(
        |_| {},
        |a, n, o| {
            if a == "C" && n == 0 {
                o[0]["evidence_mode"] = json!("pinned");
                o[0]["evidence_table_id"] = json!("t1");
            }
        },
    );
    assert!(has(&v, "used an evidence table"), "{v:?}");
    // C の実行が候補集合外の単一 model に落ちた。
    let v = bc_checked(
        |_| {},
        |a, _, o| {
            if a == "C" {
                o[0]["model"] = json!("claude-haiku-5-5");
            }
        },
    );
    assert!(has(&v, "outside the set's fixed models"), "{v:?}");
}

#[test]
fn b1_c1_none_fields_are_violations() {
    for (field, needle) in [
        ("model", "model not observed"),
        ("harness", "harness not observed"),
        ("lower_policy", "lower_policy not observed"),
        ("routing_mode", "routing mode not observed"),
    ] {
        for arm in ["B", "C"] {
            let v = bc_checked(
                |_| {},
                |a, n, o| {
                    if a == arm && n == 0 {
                        o[1][field] = Value::Null;
                    }
                },
            );
            assert!(has(&v, needle), "{arm}/{field}: {v:?}");
        }
    }
}

#[test]
fn b1_c1_policy_harness_and_mode_mismatch_are_violations() {
    let v = bc_checked(
        |_| {},
        |a, n, o| {
            if a == "C" && n == 0 {
                o[0]["lower_policy"] = json!("tier-v2");
                o[1]["harness"] = json!("codex");
            }
            if a == "B" && n == 0 {
                o[0]["routing_mode"] = json!("decision_assisted");
            }
        },
    );
    assert!(has(&v, "differs from the set's fixed policy"), "{v:?}");
    assert!(has(&v, "outside the set's fixed harnesses"), "{v:?}");
    assert!(has(&v, "B ran routing mode"), "{v:?}");
}

#[test]
fn b1_c1_missing_and_extra_runs_are_violations() {
    // 観測 run が無い。
    let v = bc_checked(
        |_| {},
        |a, n, o| {
            if a == "C" && n == 1 {
                o.clear();
            }
        },
    );
    assert!(has(&v, "no run was observed"), "{v:?}");
    // worker run が無い。
    let v = bc_checked(
        |_| {},
        |a, n, o| {
            if a == "B" && n == 0 {
                o.remove(0);
            }
        },
    );
    assert!(has(&v, "no worker run observed"), "{v:?}");
    // 別試行の run id を借りた追加 run。
    let v = bc_checked(
        |_| {},
        |a, n, o| {
            if a == "C" && n == 1 {
                o[0]["run_id"] = json!("B0-worker");
            }
        },
    );
    assert!(has(&v, "also claimed by trial"), "{v:?}");
}

#[test]
fn b1_c1_name_only_match_is_not_enough() {
    // 宣言上は同じ候補集合だが、観測した C の run は設定名（routing_mode=decision_assisted）だけ合わせて
    // 全体 config（harness/model 不明）へ落ちている。
    let v = bc_checked(
        |_| {},
        |a, _, o| {
            if a == "C" {
                for r in o.iter_mut() {
                    r["model"] = Value::Null;
                    r["harness"] = Value::Null;
                }
            }
        },
    );
    assert!(
        has(&v, "model not observed") && has(&v, "harness not observed"),
        "{v:?}"
    );
}

// ---- 網羅: ObservedRun 全欄の改変・None、役割漏れ、旧形式の読み込み ----

#[test]
fn b1_c1_every_remaining_observed_field_mutation_is_a_violation() {
    type Mutator = fn(&mut Value);
    let cases: [(&str, Mutator, &str); 4] = [
        (
            "online_update",
            |r| r["online_update"] = json!(true),
            "online_update true differs",
        ),
        (
            "evidence_mode none",
            |r| r["evidence_mode"] = Value::Null,
            "evidence_mode None differs",
        ),
        (
            "evidence_mode",
            |r| r["evidence_mode"] = json!("latest"),
            "evidence_mode",
        ),
        (
            "evidence_table_id",
            |r| r["evidence_table_id"] = json!("t9"),
            "evidence_table_id",
        ),
    ];
    for (name, f, needle) in cases {
        for arm in ["B", "C"] {
            let v = bc_checked(
                |_| {},
                |a, n, o| {
                    if a == arm && n == 0 {
                        f(&mut o[0]);
                    }
                },
            );
            assert!(has(&v, needle), "{arm}/{name}: {v:?}");
        }
    }
}

#[test]
fn b1_c1_unchanged_extra_fields_stay_positive() {
    // source / online_update=false は宣言（既定 false）と一致すれば違反でない。
    let v = bc_checked(
        |_| {},
        |_, _, o| {
            for r in o.iter_mut() {
                r["source"] = json!("anthropic");
                r["online_update"] = json!(false);
            }
        },
    );
    assert!(v.is_empty(), "{v:?}");
}

#[test]
fn b0_planner_or_reviewer_with_none_or_wrong_values_is_a_violation() {
    // 全役割 × 全欄の None（役割ごとに別 run で検出される）。
    for role_idx in 0..3 {
        for (field, needle) in [
            ("model", "model not observed"),
            ("harness", "harness not observed"),
            ("lower_policy", "lower_policy not observed"),
        ] {
            let v = checked(full_fixed(), |n, o| {
                if n == 1 {
                    o[role_idx][field] = Value::Null;
                }
            });
            assert!(has(&v, needle), "role#{role_idx}/{field}: {v:?}");
        }
    }
    // cos run が宣言どおりでも他 model なら違反、宣言どおりなら正例。
    let ok = checked(full_fixed(), |n, o| {
        if n == 0 {
            o.push(json!({"run_id":"t0-cos","role":"cos","model":SONNET,
                          "harness":"claude-code","lower_policy":"tier-v1"}));
        }
    });
    assert!(ok.is_empty(), "{ok:?}");
    let v = checked(full_fixed(), |n, o| {
        if n == 0 {
            o.push(json!({"run_id":"t0-cos","role":"cos","model":OPUS,
                          "harness":"claude-code","lower_policy":"tier-v1"}));
        }
    });
    assert!(
        has(&v, "t0-cos") && has(&v, "role_models declares"),
        "{v:?}"
    );
}

#[test]
fn role_models_none_model_with_only_role_models_declared_is_a_violation() {
    let only_roles = || SetFixed {
        role_models: full_fixed().role_models,
        ..SetFixed::default()
    };
    let v = checked(only_roles(), |n, o| {
        if n == 0 {
            o[1]["model"] = Value::Null;
        }
    });
    assert!(has(&v, "model not observed"), "{v:?}");
    assert!(checked(only_roles(), |_, _| {}).is_empty());
}

#[test]
fn old_format_records_deserialize_with_missing_observed_fields_as_none() {
    // 旧形式: observed 欄が丸ごと無い。新形式でも run の任意欄が欠けた場合は None（宣言で補わない）。
    let o: ObservedRun = serde_json::from_value(json!({
        "run_id":"r","role":"worker","harness":null,"source":null,"model":null,
        "routing_mode":null,"routing_source":null,"lower_policy":null,"evidence_table_id":null,
        "online_update":null
    }))
    .expect("old observed");
    assert_eq!(o.evidence_mode, None);
    assert_eq!(o.model, None);
    let d = tempfile::tempdir().expect("dir");
    let i = input(2);
    let m = campaign(&i, "ab", 2, 10_000_000);
    let out = d.path().join("out");
    run(
        &m,
        &i,
        &MockExecutor::scripted(BTreeMap::new()),
        &RunOptions {
            out: out.clone(),
            ..RunOptions::default()
        },
    )
    .expect("run");
    let trials = out.join(TRIALS_FILE);
    let stripped: Vec<String> = fs::read_to_string(&trials)
        .expect("t")
        .lines()
        .map(|l| {
            let mut v: Value = serde_json::from_str(l).expect("j");
            v.as_object_mut().expect("obj").remove("observed");
            v.to_string()
        })
        .collect();
    fs::write(&trials, stripped.join("\n") + "\n").expect("w");
    let v = check(&m, &out).violations;
    assert!(v.is_empty(), "{v:?}");
}
