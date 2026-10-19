//! Jev 副比較（B1=固定方策 / C1=Jev）の variant 固定の試験。主比較（A0/B0）とは別の実験セット。
//! 現行コード上の B1/C1 は manifest の B/C（B=fixed、C=decision_assisted）。新しいルータは作らず、
//! 既存の manifest 検査（`check_resolved`）・`routing_override`・task 祖先継承（ADR
//! 2026-10-16-mlsys-routing-inheritance-reconciliation）を再利用して次を固定する。
//! (a) variant 設定の hash が固定され、方策以外を変えると hash と検査が落ちる
//! (b) 根拠表を使わない (c) C1 が単一モデルへ pin されていない (d) 子・孫・retry に継承される
use std::collections::BTreeMap;

use celeris_campaign::{
    CAMPAIGN_SCHEMA,
    celeris_exec::{check_template, routing_override},
    manifest::{
        ArmRouting, Budget, CampaignManifest, Cap, Config, ConfigId, InputRef, Secondary,
        SnapshotRef, Split, Termination, TokenBasis,
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use task_core::{
    SqliteStore, TaskId, TaskStore,
    route_select::{EvidenceMode, ModeSource, RoutingMode, RoutingOverride, resolve_routing},
};

const SONNET: &str = "claude-sonnet-5-5";
const OPUS: &str = "claude-opus-5-5";

/// B1/C1 の variant 設定の固定 hash（`variant_hash`）。意図して variant を変えるときだけ更新する。
const PINNED_VARIANT_HASH: &str =
    "6bccea892f27f5289bda04259ae2beb19d55bbe868a8f8fd6becbe00b25a9c1e";

fn cfg(id: ConfigId, policy: &str) -> Config {
    let routing = ArmRouting {
        lower_policy: Some("mlsys-min-1".into()),
        online_update: Some(false),
        models: Some(vec![SONNET.into(), OPUS.into()]),
        run_harnesses: Some(vec!["claude-code".into()]),
        // 根拠表は使わない（B は元から読まない）。
        no_evidence_table: (id == ConfigId::C).then_some(true),
        evidence_table_id: None,
    };
    Config {
        id,
        harness: if id == ConfigId::A {
            "claude-code".into()
        } else {
            "celeris".into()
        },
        harness_version: if id == ConfigId::A {
            "claude-code@2.0.0".into()
        } else {
            "0123456789ab".into()
        },
        model: SONNET.into(),
        model_version: SONNET.into(),
        policy_version: policy.into(),
        routing: (id != ConfigId::A).then_some(routing),
    }
}

fn manifest() -> CampaignManifest {
    let mut m = CampaignManifest {
        schema: CAMPAIGN_SCHEMA.into(),
        campaign_id: "jev-sidecomp-b1c1".into(),
        version: "v1".into(),
        split: Split::Final,
        input: InputRef {
            path: "input.json".into(),
            sha256: "1".repeat(64),
        },
        problems: vec!["p0".into(), "p1".into()],
        configs: vec![
            cfg(ConfigId::A, "ops-baseline"),
            cfg(ConfigId::B, "fixed"),
            cfg(ConfigId::C, "jev-1"),
        ],
        primary: (ConfigId::A, ConfigId::B),
        secondary: Secondary {
            pair: (ConfigId::B, ConfigId::C),
            subset: vec!["p0".into(), "p1".into()],
        },
        budget: Budget {
            token_basis: TokenBasis::InputOutput,
            max_tokens: 3_000_000,
            max_wall_secs: 86_400,
            max_disk_bytes: 1 << 40,
            max_parallel: 1,
            per_case: Cap {
                max_tokens: 300_000,
                max_wall_secs: 3_600,
            },
            per_arm: Cap {
                max_tokens: 1_300_000,
                max_wall_secs: 43_200,
            },
        },
        termination: Termination {
            trial_timeout_secs: 1800,
            max_attempts_per_trial: 1,
            stop_on_budget: true,
        },
        order: Default::default(),
        seed: 0,
        repeats: 1,
        snapshot: SnapshotRef {
            repo_commit: "abc".into(),
            materials_sha256: "def".into(),
            kb_snapshot: "kb@1".into(),
        },
        dependencies: BTreeMap::new(),
        sha256: String::new(),
    };
    m.sha256 = m.compute_sha256().expect("hash");
    m
}

fn arm(m: &CampaignManifest, id: ConfigId) -> &Config {
    m.configs.iter().find(|c| c.id == id).expect("arm")
}

fn arm_mut(m: &mut CampaignManifest, id: ConfigId) -> &mut Config {
    m.configs.iter_mut().find(|c| c.id == id).expect("arm")
}

/// 方策だけを表す欄（id・policy_version・根拠表の扱い）を除いた arm の設定。B1 と C1 で等しくなければならない。
fn common(c: &Config) -> Value {
    let mut v = serde_json::to_value(c).expect("json");
    let o = v.as_object_mut().expect("object");
    o.remove("id");
    o.remove("policy_version");
    if let Some(r) = o.get_mut("routing").and_then(Value::as_object_mut) {
        r.remove("no_evidence_table");
        r.remove("evidence_table_id");
    }
    v
}

/// 方策だけの欄。
fn policy(c: &Config) -> Value {
    json!({
        "id": c.id,
        "policy_version": c.policy_version,
        "no_evidence_table": c.routing.as_ref().and_then(|r| r.no_evidence_table),
        "evidence_table_id": c.routing.as_ref().and_then(|r| r.evidence_table_id.clone()),
    })
}

/// 副比較 variant の hash。共通部分（候補集合・階層対応・harness・予算・初期情報）と方策差を固定する。
/// 主比較の manifest hash とは別（A を含めない）。
fn variant_hash(m: &CampaignManifest) -> String {
    let (b, c) = (arm(m, ConfigId::B), arm(m, ConfigId::C));
    let v = json!({
        "common_b": common(b), "common_c": common(c),
        "policy_b": policy(b), "policy_c": policy(c),
        "budget": m.budget, "termination": m.termination,
        "input": m.input, "snapshot": m.snapshot, "problems": m.problems,
        "subset": m.secondary.subset,
    });
    let bytes = serde_json::to_vec(&v).expect("bytes"); // serde_json の object は key 順固定
    format!("{:x}", Sha256::digest(bytes))
}

/// B1/C1 が方策以外で同一かの検査（欠陥があれば Err）。
fn check_variant(m: &CampaignManifest) -> Result<(), String> {
    m.check_resolved()?;
    let (b, c) = (arm(m, ConfigId::B), arm(m, ConfigId::C));
    if common(b) != common(c) {
        return Err("B1 and C1 differ in more than the policy".into());
    }
    for (id, x) in [(ConfigId::B, b), (ConfigId::C, c)] {
        if x.routing
            .as_ref()
            .is_some_and(|r| r.evidence_table_id.is_some())
        {
            return Err(format!("{id:?} uses an evidence table"));
        }
        if routing_override(x)?["evidence_mode"] != "none" {
            return Err(format!("{id:?} evidence_mode is not none"));
        }
    }
    // C1 を単一モデルへ pin して Jev の効果を消さない: 候補集合は 2 以上の model（Sonnet と Opus）を宣言する。
    let models = c
        .routing
        .as_ref()
        .and_then(|r| r.models.clone())
        .unwrap_or_default();
    if models.len() < 2
        || !models.iter().any(|x| x.contains("sonnet"))
        || !models.iter().any(|x| x.contains("opus"))
    {
        return Err(format!("C1 is pinned to a single model/lane: {models:?}"));
    }
    // task へ明示 lane・model・担当を書けない（実行器が埋める欄の禁止）。
    for k in ["tier", "assignee", "routing_override"] {
        let t = json!({ k: "x" });
        if check_template(t.as_object().expect("object")).is_ok() {
            return Err(format!("task template may set {k:?}"));
        }
    }
    let o = routing_override(c)?;
    for k in ["lane", "tier", "model", "models"] {
        if o.get(k).is_some() {
            return Err(format!("C1 routing_override pins {k:?}"));
        }
    }
    Ok(())
}

#[test]
fn b1_and_c1_differ_only_in_the_policy() {
    let m = manifest();
    m.validate().expect("valid");
    check_variant(&m).expect("variant ok");
    // 実際に task へ載る固定も mode だけが違う。
    let (b, c) = (
        routing_override(arm(&m, ConfigId::B)).expect("b"),
        routing_override(arm(&m, ConfigId::C)).expect("c"),
    );
    assert_eq!(b["mode"], "fixed");
    assert_eq!(c["mode"], "decision_assisted");
    let strip = |mut v: Value| {
        v.as_object_mut().expect("object").remove("mode");
        v
    };
    assert_eq!(strip(b), strip(c));
}

#[test]
fn variant_hash_is_pinned() {
    assert_eq!(variant_hash(&manifest()), PINNED_VARIANT_HASH);
}

#[test]
fn changing_anything_but_the_policy_breaks_hash_and_check() {
    let base = manifest();
    let base_hash = variant_hash(&base);
    type Edit = (&'static str, fn(&mut CampaignManifest));
    let edits: Vec<Edit> = vec![
        ("C models", |m| {
            arm_mut(m, ConfigId::C).routing.as_mut().expect("r").models =
                Some(vec![SONNET.into(), "claude-haiku-5-5".into()])
        }),
        ("C lower_policy", |m| {
            arm_mut(m, ConfigId::C)
                .routing
                .as_mut()
                .expect("r")
                .lower_policy = Some("mlsys-min-2".into())
        }),
        ("C run_harnesses", |m| {
            arm_mut(m, ConfigId::C)
                .routing
                .as_mut()
                .expect("r")
                .run_harnesses = Some(vec!["codex".into()])
        }),
        ("C harness_version", |m| {
            arm_mut(m, ConfigId::C).harness_version = "ba9876543210".into()
        }),
        ("C model", |m| arm_mut(m, ConfigId::C).model = OPUS.into()),
        ("budget per_arm", |m| m.budget.per_arm.max_tokens += 1),
        ("input", |m| m.input.sha256 = "2".repeat(64)),
        ("snapshot", |m| m.snapshot.kb_snapshot = "kb@2".into()),
    ];
    for (name, edit) in edits {
        let mut m = base.clone();
        edit(&mut m);
        assert_ne!(variant_hash(&m), base_hash, "{name}: hash must change");
        // 予算・初期情報は B/C 共通の manifest 欄なので二つの arm の差ではなく hash で検出する。
        let arm_level = name.starts_with("C ");
        assert_eq!(
            check_variant(&m).is_err(),
            arm_level,
            "{name}: arm-level change must fail the variant check"
        );
    }
    // 方策の欄（policy_version）の変更は hash には出るが検査は通る（方策は違ってよい）。
    let mut m = base.clone();
    arm_mut(&mut m, ConfigId::C).policy_version = "jev-2".into();
    assert_ne!(variant_hash(&m), base_hash);
    check_variant(&m).expect("policy-only change keeps B1/C1 comparable");
}

#[test]
fn evidence_table_is_never_used() {
    let m = manifest();
    for id in [ConfigId::B, ConfigId::C] {
        let o = routing_override(arm(&m, id)).expect("override");
        assert_eq!(o["evidence_mode"], "none", "{id:?}");
        assert!(o.get("evidence_table_id").is_none(), "{id:?}");
    }
    // 根拠表を足すと検査が落ちる。
    let mut m = manifest();
    let r = arm_mut(&mut m, ConfigId::C).routing.as_mut().expect("r");
    r.no_evidence_table = None;
    r.evidence_table_id = Some("ev-1".into());
    assert!(check_variant(&m).is_err());
    // no_evidence_table と根拠表の併記は manifest が拒否する。
    let mut m = manifest();
    arm_mut(&mut m, ConfigId::C)
        .routing
        .as_mut()
        .expect("r")
        .evidence_table_id = Some("ev-1".into());
    assert!(m.check_resolved().is_err());
    // C が根拠表も no_evidence_table も無い（暗黙 latest）のは拒否。
    let mut m = manifest();
    arm_mut(&mut m, ConfigId::C)
        .routing
        .as_mut()
        .expect("r")
        .no_evidence_table = None;
    assert!(m.check_resolved().is_err());
}

#[test]
fn c1_pinned_to_a_single_model_or_lane_fails() {
    check_variant(&manifest()).expect("two-model candidate set passes");
    // B1/C1 ともに Sonnet 単独へ揃えると比較条件は整合するが、Jev の効果が消えるので失敗。
    let mut m = manifest();
    for id in [ConfigId::B, ConfigId::C] {
        arm_mut(&mut m, id).routing.as_mut().expect("r").models = Some(vec![SONNET.into()]);
    }
    m.check_resolved().expect("consistent but pinned");
    let e = check_variant(&m).expect_err("single model must fail");
    assert!(e.contains("single model"), "{e}");
    // 候補集合が未宣言（事後検査が効かない）でも pin の疑いとして失敗。
    let mut m = manifest();
    for id in [ConfigId::B, ConfigId::C] {
        arm_mut(&mut m, id).routing.as_mut().expect("r").models = None;
    }
    assert!(check_variant(&m).is_err());
    // 明示 lane / model / 担当を task へ書く経路は塞がれている。
    for k in ["tier", "assignee", "routing_override"] {
        assert!(check_template(json!({ k: "x" }).as_object().expect("o")).is_err());
    }
}

fn new_task(parent: Option<TaskId>) -> task_core::Task {
    let mut t = task_worker::protocol::sample_task_for_tests();
    t.id = TaskId::new();
    t.parent_id = parent;
    t
}

#[test]
fn variant_is_inherited_by_child_grandchild_and_retry() {
    let m = manifest();
    for (id, mode) in [
        (ConfigId::B, RoutingMode::Fixed),
        (ConfigId::C, RoutingMode::DecisionAssisted),
    ] {
        let store = SqliteStore::open_in_memory().expect("store");
        let root = new_task(None);
        let child = new_task(Some(root.id));
        let grand = new_task(Some(child.id));
        let retry = new_task(Some(root.id));
        let outsider = new_task(None);
        for t in [&root, &child, &grand, &retry, &outsider] {
            store.insert(t).expect("insert");
        }
        let o: RoutingOverride =
            serde_json::from_value(routing_override(arm(&m, id)).expect("override")).expect("de");
        store
            .set_task_routing_override(root.id, &o, "2026-10-19")
            .expect("set");
        // 全体 config の mode が逆でも、root の固定が子孫と retry に及ぶ。
        let config_mode = if mode == RoutingMode::Fixed {
            RoutingMode::DecisionAssisted
        } else {
            RoutingMode::Fixed
        };
        for t in [&root, &child, &grand, &retry] {
            let got = store.task_routing_override(t.id).expect("read");
            let r = resolve_routing(config_mode, got.as_ref());
            assert_eq!((r.mode, r.source), (mode, ModeSource::Task), "{id:?}");
            assert_eq!(r.evidence_mode, EvidenceMode::None);
            assert_eq!(r.evidence_table_id, None);
            assert!(!r.online_update);
            assert_eq!(r.lower_policy, "mlsys-min-1");
        }
        // 負例: pin の無い task は config に落ちる（継承が切れた状態を検出できる）。
        let r = resolve_routing(
            config_mode,
            store
                .task_routing_override(outsider.id)
                .expect("read")
                .as_ref(),
        );
        assert_eq!((r.mode, r.source), (config_mode, ModeSource::Config));
        assert!(
            r.online_update,
            "unpinned task would update the evidence online"
        );
    }
}

#[test]
fn pin_on_a_sibling_branch_does_not_cover_a_retry_elsewhere() {
    // 子にだけ pin した場合、root の別の子（retry）は覆われない。継承の欠落を負例として固定する。
    let m = manifest();
    let store = SqliteStore::open_in_memory().expect("store");
    let root = new_task(None);
    let pinned_child = new_task(Some(root.id));
    let sibling_retry = new_task(Some(root.id));
    for t in [&root, &pinned_child, &sibling_retry] {
        store.insert(t).expect("insert");
    }
    let o: RoutingOverride =
        serde_json::from_value(routing_override(arm(&m, ConfigId::C)).expect("o")).expect("de");
    store
        .set_task_routing_override(pinned_child.id, &o, "2026-10-19")
        .expect("set");
    assert!(
        store
            .task_routing_override(pinned_child.id)
            .expect("r")
            .is_some()
    );
    assert!(
        store
            .task_routing_override(sibling_retry.id)
            .expect("r")
            .is_none(),
        "pin must be set at the campaign root to cover retries"
    );
}
