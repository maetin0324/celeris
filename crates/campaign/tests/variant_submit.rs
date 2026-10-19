//! 専門分担 variant 参照を通常の POST・GET 照会・再開へ通す契約（fake Transport。network・LLM・org 変更なし）。
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use celeris_campaign::{
    celeris_exec::{CLUSTER_CANCEL_FILE, CelerisExecutor, HttpResponse, Transport, check_template},
    executor::{ErrKind, Executor, TrialRequest},
    manifest::{ArmRouting, Cap, Config, ConfigId},
    variant::{COMPARISON_ID, OrgPin, VariantId, VariantRef, selection_sha256},
};
use serde_json::{Map, Value, json};

const KEY: &str = "campaign-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn vref(id: VariantId, content: char) -> VariantRef {
    VariantRef {
        comparison_id: COMPARISON_ID.into(),
        id,
        content_sha256: content.to_string().repeat(64),
        selection_sha256: selection_sha256(),
    }
}

#[derive(Default)]
struct State {
    posts: Vec<Value>,
    gets: u32,
    /// 識別欄の基準（`exec` が executor の body から作る）。
    base: Option<Value>,
    /// GET 応答の campaign を試験が書き換える（欠落・変更の模擬）。
    mutate_campaign: Option<fn(&mut Value)>,
    /// 作った task を返すときの campaign.variant（None = 欠落、Some = 差し替え）。
    stored_variant: Option<Option<Value>>,
    /// 事前に存在する task（label 一致）の campaign.variant。
    existing: Option<Option<Value>>,
    cancels: u32,
    fail_post: Option<u16>,
}

#[derive(Clone, Default)]
struct Fake(Arc<Mutex<State>>);

impl Fake {
    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn resp(status: u16, body: Value) -> Result<HttpResponse, String> {
    Ok(HttpResponse { status, body })
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
        match method {
            "GET" if p == "/api/v1/tasks" => {
                s.gets += 1;
                let created = !s.posts.is_empty();
                let items = if created || s.existing.is_some() {
                    vec![json!({"id": "01T0001"})]
                } else {
                    vec![]
                };
                resp(200, json!({"items": items}))
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
                let mut campaign = if s.posts.is_empty() {
                    s.base.clone().unwrap_or(json!({}))
                } else {
                    s.posts[0]["campaign"].clone()
                };
                if let Some(m) = campaign.as_object_mut() {
                    m.remove("variant");
                }
                if let Some(v) = v {
                    campaign["variant"] = v;
                }
                if let Some(f) = s.mutate_campaign {
                    f(&mut campaign);
                }
                resp(
                    200,
                    json!({"task": {"id": "01T0001", "status": "running"}, "campaign": campaign}),
                )
            }
            "POST" if p == "/api/v1/tasks" => {
                if let Some(c) = s.fail_post {
                    return resp(c, json!({"title": "unknown field `variant`"}));
                }
                s.posts.push(body.cloned().unwrap_or(Value::Null));
                resp(201, json!({"id": "01T0001"}))
            }
            "POST" => {
                s.cancels += 1;
                resp(200, json!({}))
            }
            _ => resp(404, json!({})),
        }
    }
}

fn cfg() -> Config {
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

fn exec(f: &Fake, dir: &Path, v: Option<VariantRef>) -> CelerisExecutor<Fake> {
    let t = json!({"acceptance": [{"type": "reviewer"}]})
        .as_object()
        .cloned()
        .unwrap_or_default();
    let org = v.as_ref().map(|_| pin_org());
    let x = CelerisExecutor::new(f.clone(), t, 1800, dir.join(CLUSTER_CANCEL_FILE))
        .with_variant(v.clone())
        .with_variant_org(org);
    let c = cfg();
    if let Ok(b) = x.body(&req(&c, dir, v.as_ref()), KEY) {
        f.st().base = b.get("campaign").cloned();
    }
    x
}

fn pin_org() -> OrgPin {
    OrgPin {
        nodes: vec!["cos".into(), "coding".into()],
        coding_node: "coding".into(),
        effective_profile_sha256: "e".repeat(64),
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

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

#[test]
fn post_carries_the_variant_ref_and_omitting_it_keeps_the_old_body() {
    let (d, c) = (tmp(), cfg());
    let v = vref(VariantId::SpecOn, 'a');
    let f = Fake::default();
    exec(&f, d.path(), Some(v.clone()))
        .submit(&req(&c, d.path(), Some(&v)), KEY)
        .expect("submit");
    assert_eq!(
        f.st().posts[0]["campaign"]["variant"],
        serde_json::to_value(&v).expect("json")
    );

    let f2 = Fake::default();
    exec(&f2, d.path(), None)
        .submit(&req(&c, d.path(), None), KEY)
        .expect("submit");
    assert!(f2.st().posts[0]["campaign"].get("variant").is_none());
}

#[test]
fn request_and_executor_variant_disagreement_is_refused_before_any_http() {
    let (d, c) = (tmp(), cfg());
    let (a, b) = (vref(VariantId::SpecOn, 'a'), vref(VariantId::SpecOff, 'b'));
    for (ex, rq) in [
        (Some(a.clone()), Some(b.clone())),
        (Some(a.clone()), None),
        (None, Some(a.clone())),
    ] {
        let f = Fake::default();
        let e = exec(&f, d.path(), ex)
            .submit(&req(&c, d.path(), rq.as_ref()), KEY)
            .expect_err("refused");
        assert_eq!(e.kind, ErrKind::Refused);
        assert!(f.st().posts.is_empty());
        assert_eq!(f.st().gets, 0);
    }
}

#[test]
fn a_ref_not_matching_the_fixed_selection_is_refused() {
    let (d, c) = (tmp(), cfg());
    let mut v = vref(VariantId::SpecOn, 'a');
    v.selection_sha256 = "0".repeat(64);
    let f = Fake::default();
    let e = exec(&f, d.path(), Some(v.clone()))
        .submit(&req(&c, d.path(), Some(&v)), KEY)
        .expect_err("refused");
    assert_eq!(e.kind, ErrKind::Refused);
    assert!(f.st().posts.is_empty());
}

#[test]
fn lookup_and_resume_reuse_only_a_task_recording_the_same_variant() {
    let (d, c) = (tmp(), cfg());
    let v = vref(VariantId::SpecOn, 'a');
    let want = serde_json::to_value(&v).expect("json");
    let other = serde_json::to_value(vref(VariantId::SpecOn, 'c')).expect("json");
    let off = serde_json::to_value(vref(VariantId::SpecOff, 'a')).expect("json");

    // 一致: 再利用でき、新規 POST は無い。
    let f = Fake::default();
    f.st().existing = Some(Some(want.clone()));
    let x = exec(&f, d.path(), Some(v.clone()));
    assert!(x.lookup(KEY).expect("lookup").is_some());
    x.resolve(&req(&c, d.path(), Some(&v)), KEY)
        .expect("resolve");
    assert!(f.st().posts.is_empty());

    // 欠落・内容違い・別 variant: lookup / resolve / submit のどれも refused で、POST も cancel も無い。
    for stored in [None, Some(other), Some(off)] {
        let f = Fake::default();
        f.st().existing = Some(stored);
        let x = exec(&f, d.path(), Some(v.clone()));
        assert_eq!(x.lookup(KEY).expect_err("lookup").kind, ErrKind::Refused);
        let r = req(&c, d.path(), Some(&v));
        assert_eq!(
            x.resolve(&r, KEY).expect_err("resolve").kind,
            ErrKind::Refused
        );
        assert_eq!(
            x.submit(&r, KEY).expect_err("submit").kind,
            ErrKind::Refused
        );
        assert!(f.st().posts.is_empty());
        assert_eq!(f.st().cancels, 0);
    }
}

#[test]
fn a_server_that_drops_or_rewrites_the_variant_is_not_treated_as_applied() {
    let (d, c) = (tmp(), cfg());
    let v = vref(VariantId::SpecOn, 'a');
    for stored in [
        None,
        Some(serde_json::to_value(vref(VariantId::SpecOn, 'c')).expect("json")),
    ] {
        let f = Fake::default();
        f.st().stored_variant = Some(stored);
        let e = exec(&f, d.path(), Some(v.clone()))
            .submit(&req(&c, d.path(), Some(&v)), KEY)
            .expect_err("refused");
        assert_eq!(e.kind, ErrKind::Refused);
        // 自分が作った task は閉じる。
        assert_eq!(f.st().cancels, 1);
    }
}

#[test]
fn an_api_without_variant_support_fails_closed() {
    let (d, c) = (tmp(), cfg());
    let v = vref(VariantId::SpecOn, 'a');
    let f = Fake::default();
    f.st().fail_post = Some(400);
    let e = exec(&f, d.path(), Some(v.clone()))
        .submit(&req(&c, d.path(), Some(&v)), KEY)
        .expect_err("refused");
    assert_eq!(e.kind, ErrKind::Refused);
}

#[test]
fn task_template_cannot_override_campaign_identity() {
    for k in ["campaign", "campaign_idempotency_key"] {
        let mut t = Map::new();
        t.insert(k.into(), json!({"variant": {}}));
        assert!(check_template(&t).is_err(), "{k}");
    }
}

// ---- campaign 公開型 ↔ task-core 下位 wire 型の明示変換 ----

use task_core::variant_wire::{CampaignOrgPin, CampaignVariantRef};

#[test]
fn conversions_keep_the_wire_shape_roundtrip_and_hashes() {
    for id in [VariantId::SpecOn, VariantId::SpecOff] {
        let v = vref(id, 'a');
        let w = CampaignVariantRef::from(&v);
        assert_eq!(
            serde_json::to_value(&w).expect("json"),
            serde_json::to_value(&v).expect("json")
        );
        assert!(w.validate().is_ok());
        assert_eq!(VariantRef::try_from(&w).expect("back"), v);
        // JSON 経由の往復も同一。
        let j = serde_json::to_string(&w).expect("json");
        let w2: CampaignVariantRef = serde_json::from_str(&j).expect("parse");
        assert_eq!(VariantRef::try_from(&w2).expect("back"), v);
    }
    let org = pin_org();
    let o = CampaignOrgPin::from(&org);
    assert_eq!(
        serde_json::to_value(&o).expect("json"),
        serde_json::to_value(&org).expect("json")
    );
    assert!(o.validate().is_ok());
    assert_eq!(OrgPin::from(&o), org);
    // 固定 selection の hash は変換で動かない。
    assert_eq!(
        selection_sha256(),
        "b6132a4456d00be83fa03bf3a54deaaec5b9874fa7bc0397c0d8bdd8937964bc"
    );
    assert_eq!(
        CampaignVariantRef::from(&vref(VariantId::SpecOn, 'a')).selection_sha256,
        selection_sha256()
    );
}

#[test]
fn unknown_variant_ids_are_not_converted_back() {
    let mut w = CampaignVariantRef::from(&vref(VariantId::SpecOn, 'a'));
    for bad in ["M1-spec-maybe", "", "m1-spec-on"] {
        w.id = bad.into();
        assert!(VariantRef::try_from(&w).is_err(), "{bad:?}");
    }
}

#[test]
fn profile_hash_is_delegated_to_task_core_with_a_pinned_fixture() {
    use task_core::profile::EffectiveProfile;
    let p = EffectiveProfile {
        node_id: "coding".into(),
        chain: vec!["cos".into(), "coding".into()],
        harnesses_allowed: vec!["claude_code".into()],
        max_attempts: Some(3),
        deny_tools: vec!["rm".into()],
        ..Default::default()
    };
    let a = celeris_campaign::variant::effective_profile_sha256(&p).expect("campaign");
    let b = task_core::variant_wire::effective_profile_sha256(&p).expect("core");
    assert_eq!(a, b);
    assert_eq!(
        a,
        "066bd493b6de85f87ff940783814184507e8939bf8104f1cbe401035267ef071"
    );
    // node_id・chain は結果に影響しない。
    let mut q = p.clone();
    q.node_id = "x".into();
    q.chain.clear();
    assert_eq!(
        celeris_campaign::variant::effective_profile_sha256(&q).expect("campaign"),
        a
    );
}

// ---- POST に variant_org、GET は detail.campaign を全欄照合 ----

#[test]
fn post_carries_variant_org_and_the_executor_requires_it() {
    let (d, c) = (tmp(), cfg());
    let v = vref(VariantId::SpecOff, 'a');
    let f = Fake::default();
    exec(&f, d.path(), Some(v.clone()))
        .submit(&req(&c, d.path(), Some(&v)), KEY)
        .expect("submit");
    assert_eq!(
        f.st().posts[0]["campaign"]["variant_org"],
        serde_json::to_value(CampaignOrgPin::from(&pin_org())).expect("json")
    );
    // variant_org の無い executor は投入前に拒否（HTTP なし）。
    let f2 = Fake::default();
    let t = json!({"acceptance": [{"type": "reviewer"}]})
        .as_object()
        .cloned()
        .unwrap_or_default();
    let x = CelerisExecutor::new(f2.clone(), t, 1800, d.path().join(CLUSTER_CANCEL_FILE))
        .with_variant(Some(v.clone()));
    let e = x
        .submit(&req(&c, d.path(), Some(&v)), KEY)
        .expect_err("refused");
    assert_eq!(e.kind, ErrKind::Refused);
    assert!(f2.st().posts.is_empty());
    assert_eq!(f2.st().gets, 0);
}

fn drop_top(k: &'static str) -> fn(&mut Value) {
    match k {
        "variant_org" => |c| {
            c.as_object_mut().map(|m| m.remove("variant_org"));
        },
        "campaign_id" => |c| {
            c.as_object_mut().map(|m| m.remove("campaign_id"));
        },
        "arm" => |c| c["arm"] = json!("A"),
        "repetition" => |c| c["repetition"] = json!(9),
        "manifest_hash" => |c| c["manifest_hash"] = json!("other"),
        "org_nodes" => |c| c["variant_org"]["nodes"] = json!(["cos", "coding", "extra"]),
        "org_profile" => |c| c["variant_org"]["effective_profile_sha256"] = json!("0".repeat(64)),
        "org_coding" => |c| c["variant_org"]["coding_node"] = json!("cos"),
        "whole" => |c| *c = Value::Null,
        _ => |_| {},
    }
}

#[test]
fn a_dropped_or_changed_field_or_org_pin_in_detail_campaign_is_refused() {
    let (d, c) = (tmp(), cfg());
    let v = vref(VariantId::SpecOn, 'a');
    for k in [
        "variant_org",
        "campaign_id",
        "arm",
        "repetition",
        "manifest_hash",
        "org_nodes",
        "org_profile",
        "org_coding",
        "whole",
    ] {
        // 既存 task の照会・再開
        let f = Fake::default();
        f.st().existing = Some(serde_json::to_value(&v).ok());
        let x = exec(&f, d.path(), Some(v.clone()));
        f.st().mutate_campaign = Some(drop_top(k));
        let r = req(&c, d.path(), Some(&v));
        let identity_only = ["arm", "repetition", "manifest_hash", "campaign_id"].contains(&k);
        if !identity_only {
            assert_eq!(x.lookup(KEY).expect_err(k).kind, ErrKind::Refused, "{k}");
            assert_eq!(
                x.resolve(&r, KEY).expect_err(k).kind,
                ErrKind::Refused,
                "{k}"
            );
        }
        // 投入前の再照会は識別の全欄を見る。
        assert_eq!(
            x.submit(&r, KEY).expect_err(k).kind,
            ErrKind::Refused,
            "{k}"
        );
        assert!(f.st().posts.is_empty(), "{k}");
        // 自分が作った task の応答検証: 閉じて拒否。
        let f = Fake::default();
        let x = exec(&f, d.path(), Some(v.clone()));
        f.st().mutate_campaign = Some(drop_top(k));
        let e = x.submit(&r, KEY).expect_err(k);
        assert_eq!(e.kind, ErrKind::Refused, "{k}");
        assert_eq!(f.st().cancels, 1, "{k}");
    }
    // 変更が無ければ採用される（試験の対照）。
    let f = Fake::default();
    exec(&f, d.path(), Some(v.clone()))
        .submit(&req(&c, d.path(), Some(&v)), KEY)
        .expect("ok");
}
