//! ローカルの fake HTTP server（127.0.0.1 の TcpListener）に対する結合試験。実 API・外部ネットワークには出ない。
//! 遅延は server 側の注入フック（`Reply::Delay`）で作り、timeout の試験は tokio の時計を pause する。CPU 負荷は掛けない。

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jev_classifier::{
    ApiKey, ApiKeyProvider, ClassifyInput, ClassifyOutcome, ClientErrorDetail, Clock,
    FallbackReason, JevClassifier, JevConfig, JevError, Sleeper,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const SECRET: &str = "sk-or-v1-FAKE-SECRET-0123456789abcdef";
const OK: &str = r#"{"model":"typesafe/jev-1.14","answers":{"specificity":{"choice":"specified"},"work_nature":{"choice":"implementation"}}}"#;
const SCHEMA_BAD: &str = r#"{"answers":{"specificity":"specified"}}"#;
const UNKNOWN_LABEL: &str =
    r#"{"answers":{"specificity":{"choice":"weird"},"work_nature":{"choice":"implementation"}}}"#;

/// fake server の応答。`Delay` は server 側の注入フック（遅延してから応答する）。
#[derive(Clone)]
enum Reply {
    Json(u16, String),
    Delay(Duration, Box<Reply>),
}

fn json(status: u16, body: &str) -> Reply {
    Reply::Json(status, body.to_string())
}

struct Fake {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
    count: Arc<AtomicUsize>,
}

impl Fake {
    fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }
    fn requests(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

/// 応答を順に返し、尽きたら最後の応答を繰り返す。受信 request 全体（header + body）を記録する。
async fn serve(script: Vec<Reply>) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let count = Arc::new(AtomicUsize::new(0));
    let (r2, c2) = (requests.clone(), count.clone());
    let queue = Arc::new(Mutex::new(VecDeque::from(script)));
    tokio::spawn(async move {
        let mut last = json(500, "{}");
        while let Ok((mut sock, _)) = listener.accept().await {
            let reply = {
                let mut q = queue.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(r) = q.pop_front() {
                    last = r;
                }
                last.clone()
            };
            let (r3, c3) = (r2.clone(), c2.clone());
            tokio::spawn(async move {
                let mut buf = vec![0u8; 65536];
                let mut n = 0;
                while let Ok(k) = sock.read(&mut buf[n..]).await {
                    if k == 0 {
                        break;
                    }
                    n += k;
                    let text = String::from_utf8_lossy(&buf[..n]).to_string();
                    if let Some((head, body)) = text.split_once("\r\n\r\n") {
                        let len = head
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if body.len() >= len {
                            break;
                        }
                    }
                }
                r3.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(String::from_utf8_lossy(&buf[..n]).to_string());
                c3.fetch_add(1, Ordering::SeqCst);
                // 公式 schema の形でない要求は台本に関わらず 400 で拒否する（実 API と同じく）。
                let raw = String::from_utf8_lossy(&buf[..n]).to_string();
                let rejected = serde_json::from_str::<serde_json::Value>(body_of(&raw))
                    .ok()
                    .and_then(|v| schema_error(&v));
                let mut reply = match rejected {
                    Some(why) => json(
                        400,
                        &serde_json::json!({"error": {"code": "invalid_request", "message": why}})
                            .to_string(),
                    ),
                    None => reply,
                };
                while let Reply::Delay(d, next) = reply {
                    tokio::time::sleep(d).await;
                    reply = *next;
                }
                if let Reply::Json(status, body) = reply {
                    let resp = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                }
            });
        }
    });
    Fake {
        url: format!("http://{addr}/decisions"),
        requests,
        count,
    }
}

struct Key(Option<&'static str>);
impl ApiKeyProvider for Key {
    fn api_key(&self) -> Option<ApiKey> {
        self.0.map(ApiKey::new)
    }
}

#[derive(Default)]
struct RecSleeper(Mutex<Vec<Duration>>);
impl RecSleeper {
    fn calls(&self) -> usize {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}
impl Sleeper for RecSleeper {
    fn sleep(&self, d: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).push(d);
        Box::pin(async {})
    }
}

struct FakeClock(AtomicU64);
impl Clock for FakeClock {
    fn now(&self) -> Duration {
        Duration::from_secs(self.0.load(Ordering::SeqCst))
    }
}

struct Rig {
    c: JevClassifier,
    sleeper: Arc<RecSleeper>,
    clock: Arc<FakeClock>,
}

fn rig(url: &str, tweak: impl FnOnce(&mut JevConfig)) -> Rig {
    let mut cfg = JevConfig {
        endpoint: url.to_string(),
        max_retries: 2,
        breaker_failure_threshold: 2,
        breaker_cooldown_ms: 60_000,
        ..Default::default()
    };
    tweak(&mut cfg);
    let sleeper = Arc::new(RecSleeper::default());
    let clock = Arc::new(FakeClock(AtomicU64::new(0)));
    let c = JevClassifier::with_parts(cfg, sleeper.clone(), clock.clone()).expect("config");
    Rig { c, sleeper, clock }
}

fn input(text: &str) -> ClassifyInput {
    ClassifyInput {
        instruction: text.into(),
        ..Default::default()
    }
}

fn body_of(req: &str) -> &str {
    req.split_once("\r\n\r\n").map(|x| x.1).unwrap_or("")
}

fn fixed(reason: FallbackReason) -> Result<ClassifyOutcome, JevError> {
    Ok(ClassifyOutcome::FallbackToFixed { reason })
}

#[tokio::test]
async fn success_returns_typed_classification_with_one_request() {
    let f = serve(vec![json(200, OK)]).await;
    let r = rig(&f.url, |_| {});
    let out =
        r.c.classify(&input("add a cli flag"), &Key(Some(SECRET)))
            .await
            .expect("ok");
    let ClassifyOutcome::Classified {
        classification,
        cached,
    } = out
    else {
        panic!("expected Classified")
    };
    assert!(!cached);
    assert_eq!(classification.model_id, "typesafe/jev-1.14");
    assert_eq!(f.count(), 1);
    let req = &f.requests()[0];
    assert!(
        req.to_ascii_lowercase()
            .contains(&format!("authorization: bearer {SECRET}").to_ascii_lowercase())
    );
}

#[tokio::test]
async fn invalid_schema_retries_then_falls_back_to_fixed() {
    let f = serve(vec![
        json(200, "not json"),
        json(200, SCHEMA_BAD),
        json(200, SCHEMA_BAD),
    ])
    .await;
    let r = rig(&f.url, |_| {});
    let out = r.c.classify(&input("a"), &Key(Some(SECRET))).await;
    assert_eq!(out, fixed(FallbackReason::InvalidResponse));
    assert_eq!(f.count(), 3, "initial + max_retries");
    assert_eq!(r.sleeper.calls(), 2);
}

#[tokio::test]
async fn unknown_classification_retries_then_falls_back_to_fixed() {
    let f = serve(vec![json(200, UNKNOWN_LABEL)]).await;
    let r = rig(&f.url, |_| {});
    let out = r.c.classify(&input("a"), &Key(Some(SECRET))).await;
    assert_eq!(out, fixed(FallbackReason::UnknownClassification));
    assert_eq!(f.count(), 3);
}

#[tokio::test]
async fn invalid_then_valid_recovers_within_retry_bound() {
    let f = serve(vec![
        json(200, SCHEMA_BAD),
        json(200, UNKNOWN_LABEL),
        json(200, OK),
    ])
    .await;
    let r = rig(&f.url, |_| {});
    let out = r.c.classify(&input("a"), &Key(Some(SECRET))).await;
    assert!(matches!(
        out,
        Ok(ClassifyOutcome::Classified { cached: false, .. })
    ));
    assert_eq!(f.count(), 3);
}

/// server 側の遅延フックが client timeout を超える。時計は pause（待ちは tokio の仮想時間で進む）。
#[tokio::test(start_paused = true)]
async fn timeout_falls_back_to_fixed_with_paused_clock() {
    let slow = Reply::Delay(Duration::from_secs(3600), Box::new(json(200, OK)));
    let f = serve(vec![slow]).await;
    let r = rig(&f.url, |c| {
        c.timeout_ms = 50;
        c.max_retries = 1;
    });
    let out = r.c.classify(&input("a"), &Key(Some(SECRET))).await;
    assert_eq!(out, fixed(FallbackReason::Timeout));
    assert_eq!(r.sleeper.calls(), 1, "one backoff between the two attempts");
}

#[tokio::test]
async fn auth_errors_are_typed_without_retry_or_breaker_count() {
    for status in [401u16, 403] {
        let f = serve(vec![json(status, r#"{"error":"nope"}"#)]).await;
        let r = rig(&f.url, |_| {});
        // breaker 閾値 2 を超える回数を呼んでも開かない（計数されない）。
        for _ in 0..4 {
            let out = r.c.classify(&input("a"), &Key(Some(SECRET))).await;
            assert_eq!(
                out,
                Err(JevError::Auth {
                    status,
                    secret_id: "jev_api_key".into()
                })
            );
        }
        assert_eq!(
            f.count(),
            4,
            "every call reached the server: no retry, breaker never opened"
        );
        assert_eq!(r.sleeper.calls(), 0);
    }
}

#[tokio::test]
async fn rate_limit_and_5xx_retry_counted_by_server_then_fixed() {
    let cases: [(u16, FallbackReason); 3] = [
        (429, FallbackReason::RateLimited),
        (500, FallbackReason::ServerError(500)),
        (503, FallbackReason::ServerError(503)),
    ];
    for (status, reason) in cases {
        let f = serve(vec![json(status, "{}")]).await;
        let r = rig(&f.url, |c| c.max_retries = 3);
        let out = r.c.classify(&input("a"), &Key(Some(SECRET))).await;
        assert_eq!(out, fixed(reason));
        assert_eq!(f.count(), 4, "status {status}: 1 + max_retries requests");
        assert_eq!(r.sleeper.calls(), 3);
    }
}

#[tokio::test]
async fn breaker_opens_stops_requests_then_half_open_after_cooldown() {
    let f = serve(vec![json(503, "{}")]).await;
    let r = rig(&f.url, |_| {});
    // 閾値 2: classify 2 回の連続失敗で開く（1 回 = 3 要求）。
    assert_eq!(
        r.c.classify(&input("a"), &Key(Some(SECRET))).await,
        fixed(FallbackReason::ServerError(503))
    );
    assert_eq!(
        r.c.classify(&input("b"), &Key(Some(SECRET))).await,
        fixed(FallbackReason::ServerError(503))
    );
    assert_eq!(f.count(), 6);
    // 開いている間は server へ要求が飛ばない。
    for t in ["c", "d", "e"] {
        assert_eq!(
            r.c.classify(&input(t), &Key(Some(SECRET))).await,
            fixed(FallbackReason::BreakerOpen)
        );
    }
    assert_eq!(f.count(), 6, "no requests while open");
    r.clock.0.store(59, Ordering::SeqCst);
    assert_eq!(
        r.c.classify(&input("f"), &Key(Some(SECRET))).await,
        fixed(FallbackReason::BreakerOpen)
    );
    assert_eq!(f.count(), 6);
    // cooldown 後の半開: 試行が server へ届く。失敗すれば直ちに再び開く。
    r.clock.0.store(61, Ordering::SeqCst);
    assert_eq!(
        r.c.classify(&input("g"), &Key(Some(SECRET))).await,
        fixed(FallbackReason::ServerError(503))
    );
    assert_eq!(
        f.count(),
        9,
        "half-open probe (1 + retries) reached the server"
    );
    assert_eq!(
        r.c.classify(&input("h"), &Key(Some(SECRET))).await,
        fixed(FallbackReason::BreakerOpen)
    );
    assert_eq!(f.count(), 9);
}

#[tokio::test]
async fn half_open_success_closes_breaker() {
    let mut script = vec![json(500, "{}"); 6];
    script.push(json(200, OK));
    let f = serve(script).await;
    let r = rig(&f.url, |_| {});
    let _ = r.c.classify(&input("a"), &Key(Some(SECRET))).await;
    let _ = r.c.classify(&input("b"), &Key(Some(SECRET))).await;
    assert_eq!(
        r.c.classify(&input("c"), &Key(Some(SECRET))).await,
        fixed(FallbackReason::BreakerOpen)
    );
    r.clock.0.store(61, Ordering::SeqCst);
    let out = r.c.classify(&input("d"), &Key(Some(SECRET))).await;
    assert!(matches!(out, Ok(ClassifyOutcome::Classified { .. })));
    assert_eq!(f.count(), 7);
    // 閉じたので、次の別入力も server へ届く（最後の応答 OK が繰り返される）。
    let _ = r.c.classify(&input("e"), &Key(Some(SECRET))).await;
    assert_eq!(f.count(), 8);
}

#[tokio::test]
async fn api_key_never_appears_in_errors_logs_or_request_body() {
    // 入力に混入した秘密も body へは出ない。
    let leaky = format!("deploy with token={SECRET} and Authorization: Bearer {SECRET}");
    let f = serve(vec![json(200, OK)]).await;
    let r = rig(&f.url, |_| {});
    let _ = r.c.classify(&input(&leaky), &Key(Some(SECRET))).await;
    let body = body_of(&f.requests()[0]).to_string();
    assert!(!body.contains(SECRET), "secret in request body");
    // 構造化情報の欄は body に無い（入力の 5 欄だけが state に入る）。
    for forbidden in ["genre", "has_check", "budget", "dependency_done"] {
        assert!(!body.contains(forbidden), "{forbidden} leaked into body");
    }

    // 応答本文に鍵が反射されても、error・Debug・fallback 理由には出ない。
    let echo = format!(r#"{{"error":"bad key {SECRET}"}}"#);
    for status in [401u16, 403] {
        let f = serve(vec![json(status, &echo)]).await;
        let r = rig(&f.url, |_| {});
        let err =
            r.c.classify(&input("a"), &Key(Some(SECRET)))
                .await
                .expect_err("auth");
        let shown = format!("{err} {err:?} {:?}", r.c);
        assert!(!shown.contains(SECRET), "secret in error output: {shown}");
        assert!(shown.contains("jev_api_key"), "secret id is reported");
    }
    let f = serve(vec![json(500, &echo)]).await;
    let r = rig(&f.url, |_| {});
    let out = r.c.classify(&input("a"), &Key(Some(SECRET))).await;
    assert!(!format!("{out:?}").contains(SECRET));
    assert!(!format!("{:?}", ApiKey::new(SECRET)).contains(SECRET));
}

#[tokio::test]
async fn long_input_is_truncated_before_sending() {
    let f = serve(vec![json(200, OK)]).await;
    let r = rig(&f.url, |_| {});
    let huge = "x".repeat(100_000);
    let i = ClassifyInput {
        instruction: huge.clone(),
        acceptance: vec![huge.clone(); 50],
        recent_failure: Some(huge),
        ..Default::default()
    };
    let _ = r.c.classify(&i, &Key(Some(SECRET))).await;
    assert!(
        body_of(&f.requests()[0]).len() < 40_000,
        "payload is bounded"
    );
}

/// 公式 schema の形を検査する（choice の criteria は object、noul の criteria は true/false の object か省略）。
fn schema_error(body: &serde_json::Value) -> Option<&'static str> {
    let qs = body.get("questions")?.as_object()?;
    for q in qs.values() {
        match q.get("type").and_then(|t| t.as_str()) {
            Some("choice") => {
                let c = q.get("criteria").and_then(|c| c.as_object());
                if c.is_none_or(|c| c.is_empty() || c.values().any(|v| !v.is_string())) {
                    return Some("choice criteria must be an object of label to description");
                }
            }
            Some("noul") => {
                if let Some(c) = q.get("criteria") {
                    let ok = c.as_object().is_some_and(|o| {
                        o.len() == 2 && o.contains_key("true") && o.contains_key("false")
                    });
                    if !ok {
                        return Some("noul criteria must be {true, false}");
                    }
                }
            }
            _ => return Some("unknown question type"),
        }
        if q.get("instructions").and_then(|i| i.as_str()).is_none() {
            return Some("instructions is required");
        }
    }
    if body.get("model").and_then(|m| m.as_str()).is_none() || body.get("state").is_none() {
        return Some("model and state are required");
    }
    None
}

#[test]
fn classifier_questions_match_official_schema_and_array_criteria_is_rejected() {
    for with_failure in [false, true] {
        let body = serde_json::json!({
            "model": "typesafe/jev-1.14",
            "state": "x",
            "questions": jev_classifier::questions(with_failure),
        });
        assert_eq!(schema_error(&body), None);
    }
    let old = serde_json::json!({
        "model": "m", "state": "x",
        "questions": {"q": {"type": "choice", "instructions": "i", "criteria": ["a", "b"]}},
    });
    assert!(schema_error(&old).is_some());
    let old_noul = serde_json::json!({
        "model": "m", "state": "x",
        "questions": {"q": {"type": "noul", "instructions": "i", "criteria": "yes"}},
    });
    assert!(schema_error(&old_noul).is_some());
}

#[tokio::test]
async fn client_error_keeps_status_code_and_message_and_separates_400_from_402() {
    for (status, code, msg) in [
        (400u16, "invalid_request", "criteria must be an object"),
        (402u16, "insufficient_credits", "add credits"),
    ] {
        let err = format!(r#"{{"error":{{"code":"{code}","message":"{msg}"}}}}"#);
        let fake = serve(vec![json(status, &err)]).await;
        let rig = rig(&fake.url, |_| {});
        let out = rig
            .c
            .classify(&input("do the thing"), &Key(Some(SECRET)))
            .await;
        assert_eq!(
            out,
            fixed(FallbackReason::ClientError(ClientErrorDetail {
                status,
                code: Some(code.into()),
                message: Some(msg.into()),
            }))
        );
        assert_eq!(fake.count(), 1, "4xx is not retried");
    }
}

#[tokio::test]
async fn fake_server_rejects_array_criteria_with_400() {
    let f = serve(vec![json(200, OK)]).await;
    let old = serde_json::json!({
        "model": "typesafe/jev-1.14", "state": "x",
        "questions": {"q": {"type": "choice", "instructions": "i", "criteria": ["a", "b"]}},
    });
    let resp = reqwest::Client::new()
        .post(&f.url)
        .json(&old)
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status().as_u16(), 400);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["error"]["code"], "invalid_request");
}

const OK_WITH_SNAPSHOT: &str = r#"{"model":"resp-model","model_snapshot":"resp-model-20261002","answers":{"specificity":{"choice":"specified"},"work_nature":{"choice":"implementation"}}}"#;
const OK_NO_MODEL: &str = r#"{"answers":{"specificity":{"choice":"specified"},"work_nature":{"choice":"implementation"}}}"#;

async fn classified(f: &Fake, r: &Rig, text: &str) -> (Box<jev_classifier::Classification>, bool) {
    let _ = f;
    match r.c.classify(&input(text), &Key(Some(SECRET))).await {
        Ok(ClassifyOutcome::Classified {
            classification,
            cached,
        }) => (classification, cached),
        _ => panic!("expected Classified"),
    }
}

// fake HTTP による値の由来検査（実 API 成功ではない）。
#[tokio::test]
async fn response_model_and_snapshot_come_from_response_not_config() {
    let f = serve(vec![json(200, OK_WITH_SNAPSHOT)]).await;
    let r = rig(&f.url, |c| c.model_id = "requested/pinned-1".into());
    let (c, cached) = classified(&f, &r, "a").await;
    assert!(!cached);
    assert_eq!(c.model_id, "requested/pinned-1");
    assert_eq!(c.response_model.as_deref(), Some("resp-model"));
    assert_eq!(c.model_snapshot.as_deref(), Some("resp-model-20261002"));
}

#[tokio::test]
async fn model_only_response_has_no_independent_snapshot() {
    let f = serve(vec![json(200, OK)]).await;
    let r = rig(&f.url, |c| c.model_id = "requested/other".into());
    let (c, _) = classified(&f, &r, "a").await;
    assert_eq!(c.response_model.as_deref(), Some("typesafe/jev-1.14"));
    assert_eq!(c.model_snapshot, None);
    assert_eq!(c.model_id, "requested/other");
}

#[tokio::test]
async fn response_without_model_is_none_never_filled_from_config() {
    let f = serve(vec![json(200, OK_NO_MODEL)]).await;
    let r = rig(&f.url, |c| c.model_id = "requested/pinned-1".into());
    let (c, _) = classified(&f, &r, "a").await;
    assert_eq!((c.response_model, c.model_snapshot), (None, None));
    assert_eq!(c.model_id, "requested/pinned-1");
}

#[tokio::test]
async fn fallback_after_invalid_response_carries_no_model_provenance_and_cache_roundtrips() {
    let f = serve(vec![
        json(200, SCHEMA_BAD),
        json(200, SCHEMA_BAD),
        json(200, SCHEMA_BAD),
        json(200, OK_WITH_SNAPSHOT),
    ])
    .await;
    let r = rig(&f.url, |_| {});
    assert!(matches!(
        r.c.classify(&input("fb"), &Key(Some(SECRET))).await,
        Ok(ClassifyOutcome::FallbackToFixed { .. })
    ));
    let (first, cached1) = classified(&f, &r, "ok").await;
    let (second, cached2) = classified(&f, &r, "ok").await;
    assert!(!cached1 && cached2);
    assert_eq!(first, second);
    assert_eq!(second.response_model.as_deref(), Some("resp-model"));
    assert_eq!(
        second.model_snapshot.as_deref(),
        Some("resp-model-20261002")
    );
}
