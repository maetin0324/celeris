use super::{fixtures::*, *};

#[test]
fn hash_is_deterministic_and_order_insensitive() {
    let i = input(4, "dev");
    let a = campaign(&i, Split::Dev, 4);
    let mut b = a.clone();
    b.problems.reverse();
    b.configs.reverse();
    assert_eq!(a.compute_sha256().unwrap(), b.compute_sha256().unwrap());
    assert_eq!(a.compute_sha256().unwrap(), a.compute_sha256().unwrap());
    let mut c = a.clone();
    c.version = "v2".into();
    assert_ne!(a.compute_sha256().unwrap(), c.compute_sha256().unwrap());
}

#[test]
fn sealed_hash_must_match_content() {
    let i = input(4, "dev");
    let mut m = campaign(&i, Split::Dev, 4);
    m.sha256 = m.compute_sha256().unwrap();
    assert!(m.validate().is_ok());
    m.budget.max_tokens += 1;
    assert!(m.validate().unwrap_err().contains("sha256"));
}

#[test]
fn missing_budget_field_is_rejected_at_parse_and_zero_at_validate() {
    let i = input(4, "dev");
    let m = campaign(&i, Split::Dev, 4);
    let mut v = serde_json::to_value(&m).unwrap();
    v["budget"].as_object_mut().unwrap().remove("max_tokens");
    assert!(serde_json::from_value::<CampaignManifest>(v).is_err());
    // token_basis も必須（暗黙の既定なし）。
    let mut v = serde_json::to_value(&m).unwrap();
    v["budget"].as_object_mut().unwrap().remove("token_basis");
    let e = serde_json::from_value::<CampaignManifest>(v).unwrap_err();
    assert!(e.to_string().contains("token_basis"), "{e}");
    let mut z = m.clone();
    z.budget.max_wall_secs = 0;
    assert!(z.validate().unwrap_err().contains("budget"));
}

#[test]
fn split_mixing_is_rejected() {
    let mut i = input(4, "dev");
    i.problems[2].split = "held-out".into();
    i.sha256 = manifest_hash(&i).unwrap();
    let m = campaign(&i, Split::Dev, 4);
    assert!(
        m.check_against_input(&i)
            .unwrap_err()
            .contains("must not be mixed")
    );
    let ok = input(4, "dev");
    assert!(
        campaign(&ok, Split::Dev, 4)
            .check_against_input(&ok)
            .is_ok()
    );
    // final campaign over dev problems is also a mix.
    assert!(
        campaign(&ok, Split::Final, 4)
            .check_against_input(&ok)
            .is_err()
    );
    let held = input(4, "held-out");
    let mut fin = campaign(&held, Split::Final, 4);
    for c in &mut fin.configs {
        if c.id != ConfigId::A {
            c.harness = "celeris".into();
            c.harness_version = "0123456789ab".into();
            c.routing = Some(ArmRouting {
                lower_policy: Some("p".into()),
                evidence_table_id: Some("e".into()),
                online_update: Some(false),
                ..Default::default()
            });
        }
    }
    assert!(fin.check_against_input(&held).is_ok());
}

#[test]
fn input_hash_mismatch_is_rejected() {
    let i = input(4, "dev");
    let mut m = campaign(&i, Split::Dev, 4);
    m.input.sha256 = "0".repeat(64);
    assert!(
        m.check_against_input(&i)
            .unwrap_err()
            .contains("hash mismatch")
    );
    let mut tampered = i.clone();
    tampered.problems[0].prompt = "changed".into();
    assert!(
        campaign(&i, Split::Dev, 4)
            .check_against_input(&tampered)
            .is_err()
    );
}

#[test]
fn case_reading_another_cases_output_is_rejected() {
    // pilot 2026-10-16: review case が含まれない case の出力 out/report.md を読んでいた。
    let mut i = input(4, "dev");
    i.problems[0].command = "test -s out/report.md".into();
    i.problems[1].prompt =
        "out/report.md と materials/raw.json を突き合わせ out/review.md に書け".into();
    i.problems[1].command = "test -s out/review.md".into();
    i.sha256 = manifest_hash(&i).unwrap();
    let m = campaign(&i, Split::Dev, 4);
    let e = m.check_against_input(&i).unwrap_err();
    assert!(
        e.contains("p1 reads out/report.md") && e.contains("p0"),
        "{e}"
    );
    // 自分の出力だけを読む／書く case は通る。
    i.problems[1].prompt = "materials/report-draft.md を読み out/review.md に書け".into();
    i.sha256 = manifest_hash(&i).unwrap();
    assert!(campaign(&i, Split::Dev, 4).check_against_input(&i).is_ok());
    assert_eq!(
        out_paths("see out/a.md, and out/b/c.csv。 not_out/x"),
        ["out/a.md", "out/b/c.csv"].map(String::from).into()
    );
}

#[test]
fn subset_must_be_declared_and_contained() {
    let i = input(4, "dev");
    let mut m = campaign(&i, Split::Dev, 4);
    m.secondary.subset.clear();
    assert!(m.validate().is_err());
    m.secondary.subset = vec!["nope".into()];
    assert!(m.validate().is_err());
}

fn set(arms: &[ConfigId], compare: &[(ConfigId, ConfigId)]) -> ExperimentSet {
    ExperimentSet {
        arms: arms.to_vec(),
        compare: compare.to_vec(),
        fixed: SetFixed::default(),
    }
}

/// 選択セットだけの manifest（未使用 arm の config は持たない）。
fn with_sets(sets: &[(&str, ExperimentSet)], sel: &[&str]) -> CampaignManifest {
    let i = input(4, "dev");
    let mut m = campaign(&i, Split::Dev, 4);
    m.experiment_sets = sets
        .iter()
        .map(|(n, s)| (n.to_string(), s.clone()))
        .collect();
    m.selected_sets = sel.iter().map(|s| s.to_string()).collect();
    let active = m.active_arms();
    m.configs.retain(|c| active.contains(&c.id));
    m
}

#[test]
fn legacy_manifest_is_all_abc() {
    let i = input(4, "dev");
    let m = campaign(&i, Split::Dev, 4);
    assert_eq!(m.active_arms(), [ConfigId::A, ConfigId::B, ConfigId::C]);
    assert_eq!(
        m.planned_configs("p0"),
        [ConfigId::A, ConfigId::B, ConfigId::C]
    );
    assert_eq!(m.planned_configs("p3"), [ConfigId::A, ConfigId::B]);
    let v = serde_json::to_value(&m).unwrap();
    assert!(v.get("experiment_sets").is_none() && v.get("selected_sets").is_none());
}

#[test]
fn ab_only_set_validates_without_c() {
    let mut m = with_sets(
        &[(
            "main",
            set(&[ConfigId::A, ConfigId::B], &[(ConfigId::A, ConfigId::B)]),
        )],
        &["main"],
    );
    m.secondary.subset.clear();
    assert!(m.validate().is_ok(), "{:?}", m.validate());
    assert_eq!(m.compare_pairs(), [(ConfigId::A, ConfigId::B)]);
    for p in &m.problems {
        assert_eq!(m.planned_configs(p), [ConfigId::A, ConfigId::B]);
    }
    assert_eq!(m.expected_trials(), 8);
    // C の config を残すと拒否（未使用 arm を黙って持たない）。
    let i = input(4, "dev");
    m.configs = campaign(&i, Split::Dev, 4).configs;
    assert!(m.validate().unwrap_err().contains("active arms"));
}

#[test]
fn bc_only_set_validates_without_a() {
    let mut m = with_sets(
        &[(
            "jev",
            set(&[ConfigId::B, ConfigId::C], &[(ConfigId::B, ConfigId::C)]),
        )],
        &["jev"],
    );
    m.problems = m.secondary.subset.clone();
    assert!(m.validate().is_ok(), "{:?}", m.validate());
    assert_eq!(m.compare_pairs(), [(ConfigId::B, ConfigId::C)]);
    for p in &m.problems {
        assert_eq!(m.planned_configs(p), [ConfigId::B, ConfigId::C]);
    }
    // 比較しない問題が分母に残るなら拒否。
    m.problems.push("p3".into());
    assert!(m.validate().unwrap_err().contains("denominator"));
}

#[test]
fn both_sets_selected_is_union_and_unselected_set_is_ignored() {
    let main = (
        "main",
        set(&[ConfigId::A, ConfigId::B], &[(ConfigId::A, ConfigId::B)]),
    );
    let jev = (
        "jev",
        set(&[ConfigId::B, ConfigId::C], &[(ConfigId::B, ConfigId::C)]),
    );
    let all = with_sets(&[main.clone(), jev.clone()], &[]);
    assert_eq!(all.active_arms(), [ConfigId::A, ConfigId::B, ConfigId::C]);
    assert!(all.validate().is_ok());
    let only = with_sets(&[main, jev], &["main"]);
    assert!(!only.is_active(ConfigId::C));
    assert_eq!(only.compare_pairs(), [(ConfigId::A, ConfigId::B)]);
    let mut bad = only.clone();
    bad.selected_sets = vec!["nope".into()];
    assert!(bad.validate().unwrap_err().contains("unknown set"));
}

#[test]
fn resume_with_changed_active_arms_or_declaration_is_rejected() {
    let ab = (
        "main",
        set(&[ConfigId::A, ConfigId::B], &[(ConfigId::A, ConfigId::B)]),
    );
    let bc = (
        "jev",
        set(&[ConfigId::B, ConfigId::C], &[(ConfigId::B, ConfigId::C)]),
    );
    let prior = with_sets(&[ab.clone(), bc.clone()], &["main"])
        .plan_identity()
        .unwrap();
    // 同じ宣言・同じ選択は再開できる。
    assert!(
        with_sets(&[ab.clone(), bc.clone()], &["main"])
            .plan_identity()
            .unwrap()
            .check_resume(&prior)
            .is_ok()
    );
    // 同じ測定 ID で有効 arm を変える。
    let swapped = with_sets(&[ab.clone(), bc.clone()], &["jev"])
        .plan_identity()
        .unwrap();
    assert!(
        swapped
            .check_resume(&prior)
            .unwrap_err()
            .contains("active arms changed")
    );
    // arm は同じでも set 宣言（固定欄）を変える。
    let mut ab2 = ab.clone();
    ab2.1.fixed.policy = Some("other".into());
    let changed = with_sets(&[ab2, bc.clone()], &["main"])
        .plan_identity()
        .unwrap();
    assert!(
        changed
            .check_resume(&prior)
            .unwrap_err()
            .contains("declaration changed")
    );
    // 有効 arm の設定を変える。
    let mut m = with_sets(&[ab, bc], &["main"]);
    m.configs[0].model_version = "2".into();
    assert!(
        m.plan_identity()
            .unwrap()
            .check_resume(&prior)
            .unwrap_err()
            .contains("configs changed")
    );
    // 別の測定 ID は別計画。
    let mut other = prior.clone();
    other.measurement_id = "other".into();
    assert!(other.active_arms == prior.active_arms && prior.check_resume(&other).is_ok());
}

#[test]
fn set_declaration_changes_manifest_sha() {
    let ab = (
        "main",
        set(&[ConfigId::A, ConfigId::B], &[(ConfigId::A, ConfigId::B)]),
    );
    let m1 = with_sets(std::slice::from_ref(&ab), &["main"]);
    let mut m2 = m1.clone();
    m2.experiment_sets.get_mut("main").unwrap().fixed.success = Some("g2".into());
    assert_ne!(m1.compute_sha256().unwrap(), m2.compute_sha256().unwrap());
    assert_ne!(m1.sets_sha256().unwrap(), m2.sets_sha256().unwrap());
}

#[test]
fn allocation_shares_the_global_ledger_without_growth_or_double_count() {
    let sh = |tokens, trials| Share {
        tokens,
        trials,
        provider_slots: 2,
    };
    let g = GlobalCap {
        total: Share {
            tokens: 100,
            trials: 10,
            provider_slots: 4,
        },
        consumed: sh(30, 3),
    };
    let mut shares = BTreeMap::new();
    shares.insert("main".to_string(), sh(40, 4));
    shares.insert("jev".to_string(), sh(30, 3));
    // 既消費 30 + 70 = 100: ちょうど全体。
    assert!(check_shares(&g, &shares).is_ok());
    // 分割で全体を超える（既消費を無かったことにしない）。
    shares.insert("jev".to_string(), sh(31, 3));
    assert!(check_shares(&g, &shares).unwrap_err().contains("tokens"));
    shares.insert("jev".to_string(), sh(30, 4));
    assert!(check_shares(&g, &shares).unwrap_err().contains("trials"));
    shares.insert(
        "jev".to_string(),
        Share {
            provider_slots: 5,
            ..sh(30, 3)
        },
    );
    assert!(
        check_shares(&g, &shares)
            .unwrap_err()
            .contains("provider_slots")
    );

    // manifest 内: セット配分の合計が全体予算・期待試行数を超えたら拒否。共有 arm B は 1 回だけ数える。
    let ab = (
        "main",
        set(&[ConfigId::A, ConfigId::B], &[(ConfigId::A, ConfigId::B)]),
    );
    let bc = (
        "jev",
        set(&[ConfigId::B, ConfigId::C], &[(ConfigId::B, ConfigId::C)]),
    );
    let mut m = with_sets(&[ab, bc], &[]);
    let trials = m.expected_trials();
    assert_eq!(trials, 4 * 2 + 2); // A,B 全 4 問 + C は subset 2 問
    let half = Share {
        tokens: m.budget.max_tokens / 2,
        trials: trials / 2,
        provider_slots: 2,
    };
    for n in ["main", "jev"] {
        m.experiment_sets.get_mut(n).unwrap().fixed.allocation = Some(half);
    }
    // 共有承認は全セットで同じ絶対 path の台帳宣言を要る（無い・片方だけ・食い違い・相対 path は拒否）。
    let decl = |m: &mut CampaignManifest, n: &str, p: Option<&str>| {
        m.experiment_sets.get_mut(n).unwrap().fixed.shared_ledger = p.map(str::to_string);
    };
    assert!(m.validate().unwrap_err().contains("shared_ledger"));
    decl(&mut m, "main", Some("/approved/ledger"));
    assert!(m.validate().unwrap_err().contains("shared_ledger"));
    decl(&mut m, "jev", Some("/other/ledger"));
    assert!(m.validate().unwrap_err().contains("shared_ledger"));
    decl(&mut m, "jev", Some("approved/ledger"));
    decl(&mut m, "main", Some("approved/ledger"));
    assert!(m.validate().unwrap_err().contains("absolute"));
    decl(&mut m, "jev", Some("/approved/ledger"));
    decl(&mut m, "main", Some("/approved/ledger"));
    assert!(m.validate().is_ok(), "{:?}", m.validate());
    // 宣言した台帳でだけ走る: 省略・別 path は拒否、同じ dir（末尾 / の違いは同じ）は通る。
    assert!(m.check_shared_ledger(None).is_err());
    assert!(
        m.check_shared_ledger(Some(Path::new("/tmp/fresh")))
            .unwrap_err()
            .contains("not the declared shared ledger")
    );
    assert!(
        m.check_shared_ledger(Some(Path::new("/approved/ledger/")))
            .is_ok()
    );
    // 旧形式（共有承認の宣言なし）は台帳を省略しても、任意の台帳を渡しても従来どおり。
    let legacy = with_sets(&[], &[]);
    assert_eq!(legacy.shared_ledger_decl(), Ok(None));
    assert!(legacy.check_shared_ledger(None).is_ok());
    assert!(legacy.check_shared_ledger(Some(Path::new("/any"))).is_ok());
    m.experiment_sets.get_mut("jev").unwrap().fixed.allocation = Some(Share { trials, ..half });
    assert!(m.validate().unwrap_err().contains("trials"));
}

fn routed(id: ConfigId, models: &[&str], ev: bool) -> Config {
    Config {
        id,
        harness: "celeris".into(),
        harness_version: "1".into(),
        model: "m".into(),
        model_version: "1".into(),
        policy_version: "p".into(),
        routing: Some(ArmRouting {
            lower_policy: Some("tiers-v1".into()),
            models: Some(models.iter().map(|s| s.to_string()).collect()),
            no_evidence_table: (id == ConfigId::C && !ev).then_some(true),
            evidence_table_id: ev.then(|| "t1".to_string()),
            ..Default::default()
        }),
    }
}

#[test]
fn b0_and_b1_c1_checks_are_separate() {
    let ok: BTreeMap<String, String> = B0_ROLES
        .iter()
        .map(|r| (r.to_string(), "claude-sonnet-4-5".to_string()))
        .collect();
    assert!(check_b0_all_sonnet(&ok).is_ok());
    let mut opus = ok.clone();
    opus.insert("reviewer".into(), "claude-opus-4".into());
    assert!(check_b0_all_sonnet(&opus).unwrap_err().contains("reviewer"));
    let mut missing = ok.clone();
    missing.remove("cos");
    assert!(check_b0_all_sonnet(&missing).unwrap_err().contains("cos"));
    assert!(check_b0_observed(&[("worker".into(), "claude-sonnet-4-5".into())]).is_ok());
    assert!(check_b0_observed(&[("planner".into(), "claude-opus-4".into())]).is_err());

    let m = ["claude-sonnet-4-5", "claude-opus-4"];
    assert!(
        check_b1_c1_common(
            &routed(ConfigId::B, &m, false),
            &routed(ConfigId::C, &m, false)
        )
        .is_ok()
    );
    // C1 を単一モデルに pin すると拒否。
    let one = ["claude-sonnet-4-5"];
    assert!(
        check_b1_c1_common(
            &routed(ConfigId::B, &one, false),
            &routed(ConfigId::C, &one, false)
        )
        .unwrap_err()
        .contains("2+")
    );
    // 候補集合が違う。
    assert!(
        check_b1_c1_common(
            &routed(ConfigId::B, &m, false),
            &routed(ConfigId::C, &one, false)
        )
        .unwrap_err()
        .contains("same")
    );
    // 根拠表を使う。
    assert!(
        check_b1_c1_common(
            &routed(ConfigId::B, &m, false),
            &routed(ConfigId::C, &m, true)
        )
        .unwrap_err()
        .contains("evidence")
    );
}
