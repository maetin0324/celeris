//! ADR 2026-11-22-strategy-review-critic D2・D3.7・D4・D5・D7: `POST /cos/operations` の `task.create` の実行の入口。
//! 守る不変条件: 実験段階の版（`selected` の無い承認済みの版）は open の候補ごとに `execution: atomic` の識別実験を
//! 1 件ずつだけ通す。実験の結果を見て `selected` を書いた版（前の版の実験が全部終端で、終端の実験を全部証拠で参照し、
//! 文書の task が done で Critic の対象）だけが本実装（compound）を通す。却下した候補の起票は拒否する。拒否は 422 と
//! その規則の code で、task を作らず、監査（`rejected` の行）を残し、`instructed_by` でも外れない。
//! task-core の `strategy_review_contract_` の試験は `check_execution` に渡した事実だけを見るので、gate が store から
//! X(v)・task の status・Critic の対象を引く経路と、task が作られないことは検出できない（この試験が見る）。

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::tests::{World, approved, baseline, created_tasks, op, post, publish, task_body, world};

const CUT: &str = "cut-min-edges";
const PILOTS: &str = "continue-pilots";
const PHASE: &str = "strategy_experiment_phase";
const UNCRITIQUED: &str = "strategy_uncritiqued";
const REVISIT: &str = "strategy_revisit_rejected";
const UNREAD: &str = "strategy_failure_unread";

/// 基準の文書を別の系列 `id` にする。
fn named(id: &str, mut doc: Value) -> Value {
    doc["strategy_id"] = json!(id);
    doc["idempotency_key"] = json!(format!("strategy:{id}:v1"));
    doc
}

/// 参照に候補を書く（`None` は省略）。
fn pick(s: &Value, candidate: Option<&str>) -> Value {
    let mut s = s.clone();
    if let Some(c) = candidate {
        s["candidate"] = json!(c);
    }
    s
}

/// 起票の本文（`execution` は `None` で省略）。
fn body(w: &World, execution: Option<&str>) -> Value {
    let mut b = task_body(w, &w.project);
    match execution {
        Some(e) => b["execution"] = json!(e),
        None => {
            b.as_object_mut().map(|o| o.remove("execution"));
        }
    }
    b
}

async fn send(w: &World, key: &str, s: &Value, body: Value) -> (StatusCode, Value) {
    post(w, &op(key, "/api/v1/tasks", body, Some(s))).await
}

/// 422 で、code の集合がちょうど `[want]`、task は作られない。
async fn only(w: &World, key: &str, s: &Value, body: Value, want: &str) -> Value {
    let before = created_tasks(w);
    let (status, resp) = send(w, key, s, body).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{key}: {resp}");
    assert_eq!(resp["code"], want, "{key}: {resp}");
    assert_eq!(
        resp["codes"],
        json!([want]),
        "{key}: only its rule refuses: {resp}"
    );
    assert_eq!(created_tasks(w), before, "{key}: a refusal created a task");
    resp
}

/// 通り、作った task の id（`cos_operations.target_id`）を返す。
async fn passes(w: &World, key: &str, s: &Value, body: Value) -> String {
    let (status, resp) = send(w, key, s, body).await;
    assert_eq!(status, StatusCode::OK, "{key}: {resp}");
    let id = resp["operation"]["target_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let task = id.parse().ok().and_then(|t| w.store.get(t).ok().flatten());
    assert!(
        task.is_some(),
        "{key}: target_id is the created task: {resp}"
    );
    id
}

fn set_status(w: &World, task_id: &str, status: &str) {
    rusqlite::Connection::open(w.root.join("celeris.db"))
        .expect("db")
        .execute(
            "UPDATE tasks SET status = ?1, json = json_set(json, '$.status', ?1) WHERE id = ?2",
            rusqlite::params![status, task_id],
        )
        .expect("status");
}

/// 実験の後の版: `selected` を `CUT` にし、`PILOTS` を反証の証拠で却下し、`cite` の task を証拠で参照する。
fn selected_next(v1: &Value, s1: &Value, cite: &[&str]) -> Value {
    let id = v1["strategy_id"].as_str().expect("strategy_id");
    let mut v2 = v1.clone();
    v2["version"] = json!(2);
    v2["idempotency_key"] = json!(format!("strategy:{id}:v2"));
    v2["supersedes"] = s1.clone();
    v2["change_reason"] = json!("識別実験の結果を見て候補を選んだ");
    v2["candidates"][0]["selected"] = json!(true);
    v2["candidates"][1]["state"] = json!("rejected");
    v2["candidates"][1]["rejected_by"] = json!(["e-pilot-refute"]);
    // 反証の証拠（evidence[1]）は次の版で変えない（子 2 D2.3）ので、実験の task は支持の証拠に引く。
    let refs = v2["evidence"][0]["references"]
        .as_array_mut()
        .expect("references");
    for t in cite {
        refs.push(json!({"kind": "task", "task_id": t}));
    }
    v2
}

/// v1 を承認し、open の候補ごとに実験を 1 件ずつ起票する。`(v1, 参照, [CUT の実験, PILOTS の実験])`。
async fn experimented(w: &World, id: &str) -> (Value, Value, [String; 2]) {
    let v1 = named(id, baseline(w));
    let s1 = approved(w, &v1);
    let cut = passes(
        w,
        &format!("{id}-x-cut"),
        &pick(&s1, Some(CUT)),
        body(w, Some("atomic")),
    )
    .await;
    let pilots = passes(
        w,
        &format!("{id}-x-pilots"),
        &pick(&s1, Some(PILOTS)),
        body(w, Some("atomic")),
    )
    .await;
    (v1, s1, [cut, pilots])
}

/// D2.4 の 1・2・4: 実験段階の版は compound・execution の省略・候補の省略・予算を超える max_wall_secs を
/// それぞれ `strategy_experiment_phase` だけで拒否し、task を作らない。予算ちょうどの atomic の実験は通る。
#[tokio::test]
async fn strategy_gate_review_compound_in_experiment_phase_is_422_and_creates_nothing() {
    let w = world();
    let s = approved(&w, &named("phase", baseline(&w)));
    only(
        &w,
        "p-compound",
        &pick(&s, Some(CUT)),
        body(&w, Some("compound")),
        PHASE,
    )
    .await;
    only(&w, "p-omitted", &pick(&s, Some(CUT)), body(&w, None), PHASE).await;
    only(
        &w,
        "p-no-candidate",
        &pick(&s, None),
        body(&w, Some("atomic")),
        PHASE,
    )
    .await;
    let mut over = body(&w, Some("atomic"));
    over["max_wall_secs"] = json!(6 * 3600 + 1);
    only(&w, "p-over-budget", &pick(&s, Some(CUT)), over, PHASE).await;
    assert_eq!(created_tasks(&w), 0);
    let mut within = body(&w, Some("atomic"));
    within["max_wall_secs"] = json!(6 * 3600);
    passes(&w, "p-within", &pick(&s, Some(CUT)), within).await;
    assert_eq!(created_tasks(&w), 1);
}

/// D2.4 の 3: 1 つの版の実験は open の候補ごとに 1 件、合計で open の候補の数（2）まで。同じ候補の 2 件目と
/// 文書に無い候補は `strategy_experiment_phase`。
#[tokio::test]
async fn strategy_gate_review_atomic_experiments_up_to_open_candidates() {
    let w = world();
    let (_, s1, _) = experimented(&w, "count").await;
    assert_eq!(created_tasks(&w), 2);
    only(
        &w,
        "c-again",
        &pick(&s1, Some(CUT)),
        body(&w, Some("atomic")),
        PHASE,
    )
    .await;
    only(
        &w,
        "c-pilots-again",
        &pick(&s1, Some(PILOTS)),
        body(&w, Some("atomic")),
        PHASE,
    )
    .await;
    only(
        &w,
        "c-unknown",
        &pick(&s1, Some("no-such")),
        body(&w, Some("atomic")),
        PHASE,
    )
    .await;
    assert_eq!(created_tasks(&w), 2);
}

/// D4.1: 却下した候補の task は `strategy_revisit_rejected` だけで拒否する（実験段階の次の版でも）。同じ版の open の
/// 候補の実験は通る。
#[tokio::test]
async fn strategy_gate_review_rejected_candidate_task_is_revisit() {
    let w = world();
    let v1 = named("revisit", baseline(&w));
    let s1 = approved(&w, &v1);
    let mut v2 = v1.clone();
    v2["version"] = json!(2);
    v2["idempotency_key"] = json!("strategy:revisit:v2");
    v2["supersedes"] = s1.clone();
    v2["change_reason"] = json!("pilot の反証で continue-pilots を却下し、代わりの案を足した");
    v2["candidates"][1]["state"] = json!("rejected");
    v2["candidates"][1]["rejected_by"] = json!(["e-pilot-refute"]);
    let mut third = v2["candidates"][0].clone();
    third["id"] = json!("measure-scc");
    third["title"] = json!("SCC を先に測る");
    v2["candidates"]
        .as_array_mut()
        .expect("candidates")
        .push(third);
    let s2 = approved(&w, &v2);
    only(
        &w,
        "r-rejected",
        &pick(&s2, Some(PILOTS)),
        body(&w, Some("atomic")),
        REVISIT,
    )
    .await;
    passes(
        &w,
        "r-open",
        &pick(&s2, Some("measure-scc")),
        body(&w, Some("atomic")),
    )
    .await;
    assert_eq!(created_tasks(&w), 1);
}

/// D3.7: 文書を作った task が done でない（review 中・reopen）と `strategy_uncritiqued`。同じ task の新しい版（未承認）が
/// あると、承認済みの古い版は Critic の対象でなく `strategy_uncritiqued`（stale ではない）。
#[tokio::test]
async fn strategy_gate_review_source_task_not_done_is_uncritiqued() {
    let w = world();
    let v1 = named("crit", baseline(&w));
    let s1 = approved(&w, &v1);
    let source = w.source.id.to_string();
    set_status(&w, &source, "reviewing");
    only(
        &w,
        "u-reviewing",
        &pick(&s1, Some(CUT)),
        body(&w, Some("atomic")),
        UNCRITIQUED,
    )
    .await;
    set_status(&w, &source, "ready");
    only(
        &w,
        "u-reopened",
        &pick(&s1, Some(CUT)),
        body(&w, Some("atomic")),
        UNCRITIQUED,
    )
    .await;
    set_status(&w, &source, "done");

    let mut v2 = v1.clone();
    v2["version"] = json!(2);
    v2["idempotency_key"] = json!("strategy:crit:v2");
    v2["supersedes"] = s1.clone();
    v2["change_reason"] = json!("候補の説明を直した");
    v2["candidates"][0]["title"] = json!("最小の辺の集合を切る（直した版）");
    publish(&w, &v2);
    only(
        &w,
        "u-older",
        &pick(&s1, Some(CUT)),
        body(&w, Some("atomic")),
        UNCRITIQUED,
    )
    .await;
    assert_eq!(created_tasks(&w), 0);
}

/// D4.2: `selected` を書いた次の版が、前の版の終端の実験 task（done と failed）の 1 件を証拠で参照しないと
/// `strategy_failure_unread` だけで拒否する。両方を参照した版は通る。
#[tokio::test]
async fn strategy_gate_review_failure_unread_is_422() {
    let w = world();
    let (v1, s1, [cut, pilots]) = experimented(&w, "unread").await;
    set_status(&w, &cut, "done");
    set_status(&w, &pilots, "failed");
    let s2 = approved(&w, &selected_next(&v1, &s1, &[&cut]));
    only(
        &w,
        "f-unread",
        &pick(&s2, None),
        body(&w, Some("compound")),
        UNREAD,
    )
    .await;
    assert_eq!(created_tasks(&w), 2);
}

/// D2.4 の 6: 実験をせずに v1 で `selected` を書いた版と、前の版の実験が終わる前に選んだ版は
/// `strategy_experiment_phase` だけで拒否する（結果を見てから選ぶ）。
#[tokio::test]
async fn strategy_gate_review_selected_without_experiments_is_422() {
    let w = world();
    let mut eager = named("eager", baseline(&w));
    eager["candidates"][0]["selected"] = json!(true);
    let s = approved(&w, &eager);
    only(
        &w,
        "s-eager",
        &pick(&s, None),
        body(&w, Some("compound")),
        PHASE,
    )
    .await;

    let (v1, s1, [cut, pilots]) = experimented(&w, "early").await;
    set_status(&w, &cut, "done");
    let s2 = approved(&w, &selected_next(&v1, &s1, &[&cut, &pilots]));
    only(
        &w,
        "s-running",
        &pick(&s2, None),
        body(&w, Some("compound")),
        PHASE,
    )
    .await;
    assert_eq!(created_tasks(&w), 2);
}

/// D2.4 の 5・6 と D4.1: 前の版の実験が全部終端で、全部を証拠で参照し、`selected` を書いた承認済みの版は、
/// compound の本実装を候補の省略でも `selected` の候補でも通す。却下した候補は `strategy_revisit_rejected`。
#[tokio::test]
async fn strategy_gate_review_selected_version_allows_compound() {
    let w = world();
    let (v1, s1, [cut, pilots]) = experimented(&w, "chosen").await;
    set_status(&w, &cut, "done");
    set_status(&w, &pilots, "cancelled");
    let s2 = approved(&w, &selected_next(&v1, &s1, &[&cut, &pilots]));
    passes(
        &w,
        "e-compound",
        &pick(&s2, None),
        body(&w, Some("compound")),
    )
    .await;
    passes(
        &w,
        "e-selected",
        &pick(&s2, Some(CUT)),
        body(&w, Some("compound")),
    )
    .await;
    only(
        &w,
        "e-rejected",
        &pick(&s2, Some(PILOTS)),
        body(&w, Some("compound")),
        REVISIT,
    )
    .await;
    assert_eq!(created_tasks(&w), 4);
}

/// D5: 4 つの code の拒否はどれも `rejected` の操作の行（理由と要求の strategy）を残し、task を作らない。
#[tokio::test]
async fn strategy_gate_review_refusals_are_audited() {
    let w = world();
    let s = approved(&w, &named("audit", baseline(&w)));
    only(
        &w,
        "a-phase",
        &pick(&s, Some(CUT)),
        body(&w, Some("compound")),
        PHASE,
    )
    .await;
    let r = approved(&w, &{
        let mut v = named("audit-rejected", baseline(&w));
        v["candidates"][0]["selected"] = json!(true);
        v["candidates"][1]["state"] = json!("rejected");
        v["candidates"][1]["rejected_by"] = json!(["e-pilot-refute"]);
        v
    });
    // v1 で選んだ版の却下した候補: 却下（revisit）と実験の未了（phase）の 2 つ。監査の行は 1 件。
    let (status, resp) = send(
        &w,
        "a-revisit",
        &pick(&r, Some(PILOTS)),
        body(&w, Some("compound")),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{resp}");
    assert!(
        resp["codes"]
            .as_array()
            .is_some_and(|c| c.contains(&json!(REVISIT))),
        "{resp}"
    );
    let (v1, s1, [cut, pilots]) = experimented(&w, "audit-unread").await;
    set_status(&w, &cut, "done");
    set_status(&w, &pilots, "done");
    let s2 = approved(&w, &selected_next(&v1, &s1, &[&cut]));
    only(
        &w,
        "a-unread",
        &pick(&s2, None),
        body(&w, Some("compound")),
        UNREAD,
    )
    .await;
    set_status(&w, &w.source.id.to_string(), "running");
    only(
        &w,
        "a-uncritiqued",
        &pick(&s, Some(PILOTS)),
        body(&w, Some("atomic")),
        UNCRITIQUED,
    )
    .await;

    let conn = rusqlite::Connection::open(w.root.join("celeris.db")).expect("db");
    for (key, candidate) in [
        ("a-phase", Some(CUT)),
        ("a-revisit", Some(PILOTS)),
        ("a-unread", None),
        ("a-uncritiqued", Some(PILOTS)),
    ] {
        let (action, reason, payload): (String, String, String) = conn
            .query_row(
                "SELECT action, reason, payload_json FROM cos_operations WHERE idempotency_key = ?1",
                [key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap_or_else(|e| panic!("{key}: no audit row: {e}"));
        assert_eq!(action, "rejected", "{key}");
        assert!(
            reason.starts_with("rejected: the strategy cannot be executed"),
            "{key}: {reason}"
        );
        let payload: Value = serde_json::from_str(&payload).expect("payload");
        assert_eq!(
            payload["strategy"]["candidate"].as_str(),
            candidate,
            "{key}: {payload}"
        );
    }
    assert_eq!(
        created_tasks(&w),
        2,
        "only the two experiments were created"
    );
}

/// D5: 人の発言の中継（`instructed_by`）でも実験段階・却下・失敗の未読・Critic 未了の拒否は外れない。
#[tokio::test]
async fn strategy_gate_review_instructed_by_cannot_waive() {
    let w = world();
    let relayed = |key: &str, s: &Value, b: Value| {
        let mut o = op(key, "/api/v1/tasks", b, Some(s));
        o["instructed_by"] = json!(w.message);
        o
    };
    let s = approved(&w, &named("relay", baseline(&w)));
    let (status, resp) = post(
        &w,
        &relayed("i-phase", &pick(&s, Some(CUT)), body(&w, Some("compound"))),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{resp}");
    assert_eq!(resp["codes"], json!([PHASE]), "{resp}");

    let (v1, s1, [cut, pilots]) = experimented(&w, "relay-unread").await;
    set_status(&w, &cut, "done");
    set_status(&w, &pilots, "failed");
    let s2 = approved(&w, &selected_next(&v1, &s1, &[&cut]));
    let (status, resp) = post(
        &w,
        &relayed(
            "i-unread",
            &pick(&s2, Some(PILOTS)),
            body(&w, Some("compound")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{resp}");
    let mut codes: Vec<String> = resp["codes"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| c.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    codes.sort();
    assert_eq!(
        codes,
        vec![UNREAD.to_string(), REVISIT.to_string()],
        "{resp}"
    );

    set_status(&w, &w.source.id.to_string(), "reviewing");
    let (status, resp) = post(
        &w,
        &relayed(
            "i-uncritiqued",
            &pick(&s, Some(PILOTS)),
            body(&w, Some("atomic")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{resp}");
    assert_eq!(resp["codes"], json!([UNCRITIQUED]), "{resp}");
    assert_eq!(
        created_tasks(&w),
        2,
        "only the two experiments were created"
    );
}
