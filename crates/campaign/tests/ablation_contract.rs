//! 専門分担 variant の契約（型・hash・B0 との同値）。外部 LLM・ネットワークなし。
//! 固定 selection は変更しない。投入・再開・冪等照会は fake Transport で POST 回数と保存状態まで確かめる。org 実適用・子孫 Assigned event の証明は次段で、この試験はその証拠ではない。
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
};

use celeris_campaign::{
    celeris_exec::{CLUSTER_CANCEL_FILE, CelerisExecutor, HttpResponse, Transport},
    executor::{ErrKind, Executor, TrialRequest},
    lock::GraderPin,
    manifest::{ArmRouting, Budget, Cap, Config, ConfigId, Termination, TokenBasis},
    variant::*,
};
use serde_json::{Value, json};
use task_core::{model::Tier, profile::EffectiveProfile};

fn profile(node: &str) -> EffectiveProfile {
    EffectiveProfile {
        node_id: node.into(),
        chain: vec!["cos".into(), node.into()],
        harnesses_allowed: vec!["claude_code".into()],
        review_harness: Some("claude_code".into()),
        review_tier: Some(Tier::Standard),
        max_lane: Some(Tier::Standard),
        max_attempts: Some(3),
        deny_tools: vec!["rm".into()],
        approvals: vec!["merge".into()],
        ..Default::default()
    }
}

fn config(id: ConfigId) -> Config {
    Config {
        id,
        harness: "claude_code".into(),
        harness_version: "1".into(),
        model: "sonnet".into(),
        model_version: "5.5".into(),
        policy_version: "p1".into(),
        routing: None,
    }
}

fn non_target() -> NonTargetPin {
    let (review, safety) = NonTargetPin::profile_pins(&profile("coding")).unwrap();
    NonTargetPin {
        configs: vec![
            config(ConfigId::A),
            config(ConfigId::B),
            config(ConfigId::C),
        ],
        role_models: REQUIRED_ROLES
            .iter()
            .map(|r| (r.to_string(), "sonnet".to_string()))
            .collect(),
        review,
        safety,
        materials_sha256: "m".repeat(8),
        kb_snapshot: "kb1".into(),
        budget: Budget {
            token_basis: TokenBasis::InputOutput,
            max_tokens: 1000,
            max_wall_secs: 100,
            max_disk_bytes: 1 << 20,
            max_parallel: 1,
            per_case: Cap {
                max_tokens: 100,
                max_wall_secs: 10,
            },
            per_arm: Cap {
                max_tokens: 500,
                max_wall_secs: 50,
            },
        },
        termination: Termination {
            trial_timeout_secs: 60,
            max_attempts_per_trial: 2,
            stop_on_budget: true,
        },
        graders: BTreeMap::from([(
            "c1".to_string(),
            GraderPin {
                private_sha256: "p".into(),
                grade_case_sha256: "g".into(),
            },
        )]),
    }
}

fn baseline() -> BaselinePin {
    BaselinePin {
        org: OrgPin {
            nodes: vec!["cos".into(), "coding".into(), "research".into()],
            coding_node: "coding".into(),
            effective_profile_sha256: effective_profile_sha256(&profile("coding")).unwrap(),
        },
        non_target: non_target(),
    }
}

fn spec_on() -> VariantSpec {
    let b = baseline();
    VariantSpec::new(VariantId::SpecOn, b.org, b.non_target)
        .seal()
        .unwrap()
}

fn spec_off() -> VariantSpec {
    let b = baseline();
    let org = OrgPin {
        nodes: vec!["cos".into(), "worker".into()],
        coding_node: "worker".into(),
        effective_profile_sha256: effective_profile_sha256(&profile("worker")).unwrap(),
    };
    VariantSpec::new(VariantId::SpecOff, org, b.non_target)
        .seal()
        .unwrap()
}

#[test]
fn selection_is_fixed_and_matches_its_hash() {
    assert_eq!(selection_json_sha256(), selection_sha256());
    assert_eq!(
        selection_sha256(),
        "b6132a4456d00be83fa03bf3a54deaaec5b9874fa7bc0397c0d8bdd8937964bc"
    );
    let v: serde_json::Value = serde_json::from_str(SELECTION_JSON).unwrap();
    assert_eq!(v["comparison_set"]["id"], COMPARISON_ID);
    assert_eq!(v["mechanism"], "specialization");
}

#[test]
fn both_variants_verify_against_b0_and_profile_equivalence_ignores_node_and_chain() {
    let b = baseline();
    spec_on().verify(&b).unwrap();
    spec_off().verify(&b).unwrap();
    assert_eq!(
        effective_profile_sha256(&profile("coding")).unwrap(),
        effective_profile_sha256(&profile("worker")).unwrap()
    );
}

#[test]
fn content_hash_is_stable_and_ignores_node_order() {
    let a = spec_off();
    assert_eq!(
        a.content_sha256().unwrap(),
        spec_off().content_sha256().unwrap()
    );
    let mut r = a.clone();
    r.org.nodes.reverse();
    assert_eq!(r.content_sha256().unwrap(), a.sha256);
    assert_ne!(spec_on().sha256, spec_off().sha256);
    let r = a.to_ref().unwrap();
    assert_eq!(
        (r.comparison_id.as_str(), r.id),
        (COMPARISON_ID, VariantId::SpecOff)
    );
    assert_eq!(serde_json::to_value(r.id).unwrap(), "M1-spec-off");
}

#[test]
fn content_tamper_is_detected_without_resealing() {
    let mut s = spec_on();
    s.non_target.kb_snapshot = "kb2".into();
    assert!(matches!(
        s.verify(&baseline()),
        Err(VariantError::ContentHash(_))
    ));
    let mut u = spec_on();
    u.sha256.clear();
    assert!(matches!(
        u.verify(&baseline()),
        Err(VariantError::ContentHash(_))
    ));
    assert!(u.to_ref().is_err());
}

#[test]
fn selection_hash_tamper_is_detected_even_when_resealed() {
    let mut s = spec_on();
    s.selection_sha256 = "0".repeat(64);
    assert!(matches!(
        s.clone().verify(&baseline()),
        Err(VariantError::ContentHash(_)) | Err(VariantError::SelectionMismatch(_))
    ));
    let s = s.seal().unwrap();
    assert!(matches!(
        s.verify(&baseline()),
        Err(VariantError::SelectionMismatch(_))
    ));
}

#[test]
fn resealed_non_target_differences_are_rejected_against_the_fixed_baseline() {
    type Edit = fn(&mut NonTargetPin);
    let edits: [(&str, Edit); 9] = [
        ("configs", |n| n.configs[1].model = "opus".into()),
        ("role_models", |n| {
            n.role_models.insert("reviewer".into(), "haiku".into());
        }),
        ("review", |n| n.review.tier = "Cheap".into()),
        ("safety", |n| n.safety.deny_tools.clear()),
        ("materials_sha256", |n| n.materials_sha256 = "other".into()),
        ("kb_snapshot", |n| n.kb_snapshot = "kb2".into()),
        ("budget", |n| n.budget.per_case.max_tokens += 1),
        ("termination", |n| n.termination.max_attempts_per_trial += 1),
        ("graders", |n| {
            n.graders.get_mut("c1").unwrap().private_sha256 = "x".into();
        }),
    ];
    for (name, edit) in edits {
        for mut s in [spec_on(), spec_off()] {
            edit(&mut s.non_target);
            let s = s.seal().unwrap(); // 自己 hash を作り直しても
            match s.verify(&baseline()) {
                Err(VariantError::NonTargetDiff(m)) => assert!(m.contains(name), "{name}: {m}"),
                o => panic!("{name}: {o:?}"),
            }
        }
    }
}

#[test]
fn missing_or_unknown_inputs_are_rejected_not_defaulted() {
    let mut s = spec_on();
    s.non_target.role_models.remove("planner");
    let s = s.seal().unwrap();
    assert!(
        matches!(s.verify(&baseline()), Err(VariantError::Missing(m)) if m.contains("planner"))
    );

    let mut s = spec_on();
    s.non_target.kb_snapshot = "unknown".into();
    assert!(matches!(
        s.seal().unwrap().verify(&baseline()),
        Err(VariantError::Missing(_))
    ));

    let mut s = spec_on();
    s.non_target.graders.clear();
    assert!(matches!(
        s.seal().unwrap().verify(&baseline()),
        Err(VariantError::Missing(_))
    ));

    // 基準側の欠落も成功にしない。
    let mut b = baseline();
    b.non_target.configs.clear();
    assert!(matches!(
        spec_on().verify(&b),
        Err(VariantError::Missing(_))
    ));

    // profile に review / lane 上限が無ければ pin を作れない。
    let mut p = profile("coding");
    p.max_lane = None;
    assert!(matches!(
        NonTargetPin::profile_pins(&p),
        Err(VariantError::Missing(_))
    ));
}

#[test]
fn org_shape_and_profile_equivalence_are_enforced() {
    let b = baseline();
    let mut off = spec_off();
    off.org.nodes.push("extra".into());
    assert!(matches!(
        off.seal().unwrap().verify(&b),
        Err(VariantError::Org(_))
    ));

    let mut off = spec_off();
    off.org.coding_node = "cos".into();
    off.org.nodes = vec!["cos".into()];
    assert!(matches!(
        off.seal().unwrap().verify(&b),
        Err(VariantError::Org(_))
    ));

    let mut off = spec_off();
    off.org.effective_profile_sha256 = "f".repeat(64); // 再 seal しても B0 の基準と合わない
    assert!(matches!(
        off.seal().unwrap().verify(&b),
        Err(VariantError::Org(_))
    ));

    let mut on = spec_on();
    on.org.nodes.pop();
    assert!(matches!(
        on.seal().unwrap().verify(&b),
        Err(VariantError::Org(_))
    ));

    let mut s = spec_on();
    s.comparison_id = "other".into();
    assert!(matches!(
        s.seal().unwrap().verify(&b),
        Err(VariantError::Schema(_))
    ));
}

fn reverse_keys(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .rev()
                .map(|(k, v)| (k.clone(), reverse_keys(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(reverse_keys).collect()),
        o => o.clone(),
    }
}

#[test]
fn hash_survives_key_order_and_reload_from_disk() {
    let dir = tempfile::tempdir().unwrap();
    for s in [spec_on(), spec_off()] {
        let v = serde_json::to_value(&s).unwrap();
        for (i, text) in [
            serde_json::to_string(&v).unwrap(),
            serde_json::to_string_pretty(&reverse_keys(&v)).unwrap(),
        ]
        .iter()
        .enumerate()
        {
            let p = dir.path().join(format!("{}-{i}.json", s.variant.as_str()));
            std::fs::write(&p, text).unwrap();
            let back: VariantSpec =
                serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
            assert_eq!(back, s);
            assert_eq!(back.content_sha256().unwrap(), s.sha256);
            back.verify(&baseline()).unwrap();
            assert_eq!(back.to_ref().unwrap(), s.to_ref().unwrap());
        }
    }
    // 未知の欄は読み込みで拒否する。
    let mut v = serde_json::to_value(spec_on()).unwrap();
    v["extra"] = json!(1);
    assert!(serde_json::from_value::<VariantSpec>(v).is_err());
}

#[test]
fn every_single_field_tamper_changes_the_hash_and_is_refused_unsealed() {
    let base = spec_on();
    type Edit = fn(&mut VariantSpec);
    let edits: [(&str, Edit); 7] = [
        ("variant", |s| s.variant = VariantId::SpecOff),
        ("org.coding_node", |s| s.org.coding_node = "x".into()),
        ("org.nodes", |s| s.org.nodes.push("x".into())),
        ("org.profile", |s| {
            s.org.effective_profile_sha256 = "0".repeat(64)
        }),
        ("kb", |s| s.non_target.kb_snapshot = "kb9".into()),
        ("budget", |s| s.non_target.budget.max_parallel += 1),
        ("grader", |s| {
            s.non_target
                .graders
                .get_mut("c1")
                .unwrap()
                .grade_case_sha256 = "z".into()
        }),
    ];
    for (n, e) in edits {
        let mut s = base.clone();
        e(&mut s);
        assert_ne!(s.content_sha256().unwrap(), base.sha256, "{n}");
        assert!(
            matches!(s.verify(&baseline()), Err(VariantError::ContentHash(_))),
            "{n}"
        );
        assert!(s.to_ref().is_err(), "{n}");
    }
}

#[test]
fn b0_org_single_node_and_effective_profile_gaps_are_rejected() {
    type Edit = fn(&mut BaselinePin);
    // B0 側（基準）の欠落は、どちらの variant も成功にしない。
    let baseline_gaps: [(&str, Edit); 5] = [
        ("coding node not in org", |b| {
            b.org.nodes.retain(|n| n != "coding")
        }),
        ("root as coding node", |b| b.org.coding_node = "cos".into()),
        ("empty coding node", |b| b.org.coding_node.clear()),
        ("missing profile hash", |b| {
            b.org.effective_profile_sha256.clear()
        }),
        ("unknown profile hash", |b| {
            b.org.effective_profile_sha256 = "unknown".into()
        }),
    ];
    for (n, e) in baseline_gaps {
        let mut b = baseline();
        e(&mut b);
        for s in [spec_on(), spec_off()] {
            assert!(s.verify(&b).is_err(), "{n} {:?}", s.variant);
        }
    }
    // spec 側: 単一 node の形・EffectiveProfile 欠落。再 seal しても拒否。
    type SEdit = fn(&mut VariantSpec);
    let spec_gaps: [(&str, SEdit); 6] = [
        ("off: root only", |s| {
            s.org.nodes = vec!["cos".into()];
            s.org.coding_node = "cos".into();
        }),
        ("off: two work nodes", |s| s.org.nodes.push("w2".into())),
        ("off: no root", |s| s.org.nodes = vec!["worker".into()]),
        ("off: empty node", |s| s.org.coding_node.clear()),
        ("off: empty profile hash", |s| {
            s.org.effective_profile_sha256.clear()
        }),
        ("off: other profile", |s| {
            s.org.effective_profile_sha256 = effective_profile_sha256(&EffectiveProfile {
                max_attempts: Some(9),
                ..profile("worker")
            })
            .unwrap()
        }),
    ];
    for (n, e) in spec_gaps {
        let mut s = spec_off();
        e(&mut s);
        assert!(s.seal().unwrap().verify(&baseline()).is_err(), "{n}");
    }
    // spec-on は B0 org と完全一致のみ。
    let mut on = spec_on();
    on.org.nodes = vec!["cos".into(), "worker".into()];
    on.org.coding_node = "worker".into();
    assert!(matches!(
        on.seal().unwrap().verify(&baseline()),
        Err(VariantError::Org(_))
    ));
}

#[test]
fn profile_equivalence_covers_every_effective_profile_field_but_node_and_chain() {
    let base = effective_profile_sha256(&profile("coding")).unwrap();
    type Edit = fn(&mut EffectiveProfile);
    let edits: [(&str, Edit); 8] = [
        ("harnesses", |p| p.harnesses_allowed.push("codex".into())),
        ("review_harness", |p| {
            p.review_harness = Some("codex".into())
        }),
        ("review_tier", |p| p.review_tier = Some(Tier::Cheap)),
        ("max_lane", |p| p.max_lane = Some(Tier::Cheap)),
        ("max_attempts", |p| p.max_attempts = Some(4)),
        ("deny_tools", |p| p.deny_tools.clear()),
        ("approvals", |p| p.approvals.clear()),
        ("tools", |p| p.tools.push("x".into())),
    ];
    for (n, e) in edits {
        let mut p = profile("worker");
        e(&mut p);
        assert_ne!(effective_profile_sha256(&p).unwrap(), base, "{n}");
    }
}

// ---- 投入・再開・冪等照会（fake Transport。実 LLM・本番 API なし） ----

const KEY: &str = "campaign-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[derive(Default)]
struct State {
    posts: Vec<Value>,
    gets: u32,
    cancels: u32,
    existing: Option<Option<Value>>,
    stored_variant: Option<Option<Value>>,
}

#[derive(Clone, Default)]
struct Fake(Arc<Mutex<State>>);

impl Fake {
    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// (POST 回数, GET 回数, cancel 回数)。1 式で `st()` を 2 回呼ぶとデッドロックするので 1 回で読む。
    fn counts(&self) -> (usize, u32, u32) {
        let s = self.st();
        (s.posts.len(), s.gets, s.cancels)
    }
}

impl Transport for Fake {
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<HttpResponse, String> {
        let mut s = self.st();
        let p = path.split('?').next().unwrap_or(path);
        let ok = |status, body| Ok(HttpResponse { status, body });
        match method {
            "GET" if p == "/api/v1/tasks" => {
                s.gets += 1;
                let any = !s.posts.is_empty() || s.existing.is_some();
                let items = if any {
                    vec![json!({"id": "01T1"})]
                } else {
                    vec![]
                };
                ok(200, json!({"items": items}))
            }
            "GET" => {
                s.gets += 1;
                let v = if s.posts.is_empty() {
                    s.existing.clone().flatten()
                } else {
                    match &s.stored_variant {
                        Some(o) => o.clone(),
                        None => s.posts[0]["campaign"].get("variant").cloned(),
                    }
                };
                let mut campaign = json!({"campaign_id": "c1"});
                if let Some(v) = v {
                    campaign["variant"] = v;
                }
                ok(
                    200,
                    json!({"task": {"id": "01T1", "status": "running", "campaign": campaign}}),
                )
            }
            "POST" if p == "/api/v1/tasks" => {
                s.posts.push(body.cloned().unwrap_or(Value::Null));
                ok(201, json!({"id": "01T1"}))
            }
            "POST" => {
                s.cancels += 1;
                ok(200, json!({}))
            }
            _ => ok(404, json!({})),
        }
    }
}

fn vref(id: VariantId, content: char) -> VariantRef {
    VariantRef {
        comparison_id: COMPARISON_ID.into(),
        id,
        content_sha256: content.to_string().repeat(64),
        selection_sha256: selection_sha256(),
    }
}

fn exec(f: &Fake, dir: &Path, v: Option<VariantRef>) -> CelerisExecutor<Fake> {
    let t = json!({"acceptance": [{"type": "reviewer"}]})
        .as_object()
        .cloned()
        .unwrap_or_default();
    CelerisExecutor::new(f.clone(), t, 1800, dir.join(CLUSTER_CANCEL_FILE)).with_variant(v)
}

fn trial_cfg() -> Config {
    Config {
        id: ConfigId::B,
        harness: "celeris".into(),
        harness_version: "1".into(),
        model: "m".into(),
        model_version: "1".into(),
        policy_version: "p1".into(),
        routing: Some(ArmRouting::default()),
    }
}

fn req<'a>(c: &'a Config, dir: &'a Path, v: Option<&'a VariantRef>) -> TrialRequest<'a> {
    TrialRequest {
        campaign_id: "c1",
        manifest_sha256: "m1",
        config: c,
        problem_id: "p0",
        prompt: "fix",
        trial_no: 1,
        rep: 1,
        workspace: dir,
        kb: dir,
        timeout_secs: 1800,
        budget: Cap {
            max_tokens: u64::MAX,
            max_wall_secs: u64::MAX,
        },
        variant: v,
    }
}

#[test]
fn first_resume_and_same_label_lookup_agree_on_the_same_variant() {
    let (d, c) = (tempfile::tempdir().unwrap(), trial_cfg());
    for id in [VariantId::SpecOn, VariantId::SpecOff] {
        let v = vref(id, 'a');
        let want = serde_json::to_value(&v).unwrap();
        // 初回: POST 1 回、campaign.variant は参照と一致。
        let f = Fake::default();
        let x = exec(&f, d.path(), Some(v.clone()));
        x.submit(&req(&c, d.path(), Some(&v)), KEY).unwrap();
        assert_eq!(f.st().posts.len(), 1);
        assert_eq!(f.st().posts[0]["campaign"]["variant"], want);
        assert_eq!(f.st().cancels, 0);
        // 同一 label の再照会・再開: 新規 POST なしで採用。
        assert!(x.lookup(KEY).unwrap().is_some());
        x.resolve(&req(&c, d.path(), Some(&v)), KEY).unwrap();
        assert_eq!(f.st().posts.len(), 1);
        // 事前に存在する一致 task からの再開も POST しない。
        let f2 = Fake::default();
        f2.st().existing = Some(Some(want));
        let x2 = exec(&f2, d.path(), Some(v.clone()));
        x2.resolve(&req(&c, d.path(), Some(&v)), KEY).unwrap();
        assert!(f2.st().posts.is_empty());
        assert_eq!(f2.st().cancels, 0);
    }
}

#[test]
fn bad_variant_or_selection_hash_in_the_request_is_refused_before_any_side_effect() {
    let (d, c) = (tempfile::tempdir().unwrap(), trial_cfg());
    let good = vref(VariantId::SpecOn, 'a');
    let mut bad_sel = good.clone();
    bad_sel.selection_sha256 = "0".repeat(64);
    let mut short = good.clone();
    short.content_sha256 = "abc".into();
    let mut empty = good.clone();
    empty.content_sha256.clear();
    let mut cmp = good.clone();
    cmp.comparison_id = "other".into();
    let mut nonhex = good.clone();
    nonhex.content_sha256 = "g".repeat(64);
    for v in [bad_sel, short, empty, cmp, nonhex] {
        let f = Fake::default();
        let e = exec(&f, d.path(), Some(v.clone()))
            .submit(&req(&c, d.path(), Some(&v)), KEY)
            .unwrap_err();
        assert_eq!(e.kind, ErrKind::Refused, "{v:?}");
        assert!(f.st().posts.is_empty());
        assert_eq!(f.counts(), (0, 0, 0));
    }
    // 請求と executor の参照が食い違う／片方欠落。
    let other = vref(VariantId::SpecOff, 'b');
    for (ex, rq) in [
        (Some(good.clone()), Some(other.clone())),
        (Some(good.clone()), None),
        (None, Some(good.clone())),
        (Some(good.clone()), Some(vref(VariantId::SpecOn, 'c'))),
    ] {
        let f = Fake::default();
        let e = exec(&f, d.path(), ex)
            .submit(&req(&c, d.path(), rq.as_ref()), KEY)
            .unwrap_err();
        assert_eq!(e.kind, ErrKind::Refused);
        assert!(f.st().posts.is_empty());
        assert_eq!(f.st().gets, 0);
    }
}

#[test]
fn stored_variant_missing_or_different_is_never_adopted_on_resume_or_lookup() {
    let (d, c) = (tempfile::tempdir().unwrap(), trial_cfg());
    let v = vref(VariantId::SpecOn, 'a');
    let mut sel = serde_json::to_value(&v).unwrap();
    sel["selection_sha256"] = json!("0".repeat(64));
    let mut cmp = serde_json::to_value(&v).unwrap();
    cmp["comparison_id"] = json!("other");
    for stored in [
        None,
        Some(serde_json::to_value(vref(VariantId::SpecOn, 'c')).unwrap()),
        Some(serde_json::to_value(vref(VariantId::SpecOff, 'a')).unwrap()),
        Some(sel),
        Some(cmp),
        Some(json!("M1-spec-on")),
    ] {
        let f = Fake::default();
        f.st().existing = Some(stored);
        let x = exec(&f, d.path(), Some(v.clone()));
        let r = req(&c, d.path(), Some(&v));
        assert_eq!(x.lookup(KEY).unwrap_err().kind, ErrKind::Refused);
        assert_eq!(x.resolve(&r, KEY).unwrap_err().kind, ErrKind::Refused);
        assert_eq!(x.submit(&r, KEY).unwrap_err().kind, ErrKind::Refused);
        assert!(f.st().posts.is_empty());
        assert_eq!(f.st().cancels, 0);
    }
}

#[test]
fn a_response_that_drops_or_rewrites_the_variant_is_not_applied_and_the_new_task_is_closed() {
    let (d, c) = (tempfile::tempdir().unwrap(), trial_cfg());
    let v = vref(VariantId::SpecOff, 'a');
    let mut sel = serde_json::to_value(&v).unwrap();
    sel["selection_sha256"] = json!("1".repeat(64));
    for stored in [
        None,
        Some(serde_json::to_value(vref(VariantId::SpecOff, 'c')).unwrap()),
        Some(serde_json::to_value(vref(VariantId::SpecOn, 'a')).unwrap()),
        Some(sel),
    ] {
        let f = Fake::default();
        f.st().stored_variant = Some(stored);
        let e = exec(&f, d.path(), Some(v.clone()))
            .submit(&req(&c, d.path(), Some(&v)), KEY)
            .unwrap_err();
        assert_eq!(e.kind, ErrKind::Refused);
        assert_eq!(f.st().posts.len(), 1);
        assert_eq!(f.st().cancels, 1);
    }
}

#[test]
fn the_legacy_non_variant_path_is_unchanged() {
    let (d, c) = (tempfile::tempdir().unwrap(), trial_cfg());
    let f = Fake::default();
    let x = exec(&f, d.path(), None);
    x.submit(&req(&c, d.path(), None), KEY).unwrap();
    assert_eq!(f.st().posts.len(), 1);
    assert!(f.st().posts[0]["campaign"].get("variant").is_none());
    assert!(x.lookup(KEY).unwrap().is_some());
    x.resolve(&req(&c, d.path(), None), KEY).unwrap();
    let (posts, _, cancels) = f.counts();
    assert_eq!((posts, cancels), (1, 0));
    // 既存 task（variant なし）も従来どおり採用する。
    let f2 = Fake::default();
    f2.st().existing = Some(None);
    let x2 = exec(&f2, d.path(), None);
    assert!(x2.lookup(KEY).unwrap().is_some());
    x2.resolve(&req(&c, d.path(), None), KEY).unwrap();
    assert!(f2.st().posts.is_empty());
}
