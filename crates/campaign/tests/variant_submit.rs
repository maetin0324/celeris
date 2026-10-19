//! 専門分担 variant 参照を通常の POST・GET 照会・再開へ通す契約（fake Transport。network・LLM・org 変更なし）。
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use celeris_campaign::{
    celeris_exec::{CLUSTER_CANCEL_FILE, CelerisExecutor, HttpResponse, Transport, check_template},
    executor::{ErrKind, Executor, TrialRequest},
    manifest::{ArmRouting, Cap, Config, ConfigId},
    variant::{COMPARISON_ID, VariantId, VariantRef, selection_sha256},
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
                let mut campaign = json!({"campaign_id": "c1"});
                if let Some(v) = v {
                    campaign["variant"] = v;
                }
                resp(
                    200,
                    json!({"task": {"id": "01T0001", "status": "running", "campaign": campaign}}),
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
    CelerisExecutor::new(f.clone(), t, 1800, dir.join(CLUSTER_CANCEL_FILE)).with_variant(v)
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
