#[cfg(test)]
#[allow(clippy::duplicate_mod)] // 同じ支援 file を試験 crate ごとに読む
#[path = "../../../tests/support/http_async.rs"]
mod fake_http;

use std::fmt;
use std::sync::Arc;

use serde_json::json;
use sha2::{Digest, Sha256};

use crate::breaker::CircuitBreaker;
use crate::cache::BoundedCache;
use crate::config::JevConfig;
use crate::error::JevError;
use crate::input::ClassifyInput;
use crate::question::{Classification, Meta, ParseError, parse_response, questions};
use crate::time::{Clock, Sleeper, SystemClock, TokioSleeper};

/// API key。Debug は伏せ、Display は実装しない。値は送信時だけ `expose` で取り出す。
#[derive(Clone)]
pub struct ApiKey(String);

impl ApiKey {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

/// 鍵の取得元（secrets_dir の読み出しなど。呼び出し側が実装する）。未登録は None。
pub trait ApiKeyProvider: Send + Sync {
    fn api_key(&self) -> Option<ApiKey>;
}

/// 4xx 応答の記録。鍵・入力本文は含めない（応答の `error.code`・`error.message` のみ。
/// message は 200 文字で切る）。400（schema 不備）と 402（残高不足）を区別するために status を残す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientErrorDetail {
    pub status: u16,
    pub code: Option<String>,
    pub message: Option<String>,
}

impl ClientErrorDetail {
    /// OpenRouter 形式 `{"error":{"code":…,"message":…}}` を読む。読めなければ status のみ。
    pub fn from_body(status: u16, body: &str) -> Self {
        let v: Option<serde_json::Value> = serde_json::from_str(body).ok();
        let err = v.as_ref().and_then(|v| v.get("error"));
        let code = err.and_then(|e| e.get("code")).and_then(|c| match c {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            _ => None,
        });
        let message = err
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .map(|m| m.chars().take(200).collect());
        Self {
            status,
            code,
            message,
        }
    }
}

/// fixed 方策へ戻した理由。値・応答本文は含めない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FallbackReason {
    BreakerOpen,
    Timeout,
    Transport,
    RateLimited,
    ServerError(u16),
    /// 4xx（401/403/429 以外）。status と応答の `error.code`・`error.message` だけを持つ。
    ClientError(ClientErrorDetail),
    InvalidResponse,
    UnknownClassification,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClassifyOutcome {
    Classified {
        classification: Box<Classification>,
        cached: bool,
    },
    FallbackToFixed {
        reason: FallbackReason,
    },
}

enum Attempt {
    Done(Box<Classification>),
    Retry(FallbackReason),
    Fail(FallbackReason),
    Auth(u16),
}

pub struct JevClassifier {
    config: JevConfig,
    http: reqwest::Client,
    sleeper: Arc<dyn Sleeper>,
    clock: Arc<dyn Clock>,
    breaker: CircuitBreaker,
    cache: BoundedCache<Classification>,
}

impl fmt::Debug for JevClassifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JevClassifier")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl JevClassifier {
    pub fn new(config: JevConfig) -> Result<Self, JevError> {
        Self::with_parts(config, Arc::new(TokioSleeper), Arc::new(SystemClock::new()))
    }

    /// sleep と時計を注入する（試験用に実時間を使わない）。
    pub fn with_parts(
        config: JevConfig,
        sleeper: Arc<dyn Sleeper>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, JevError> {
        config.validate()?;
        let http = reqwest::Client::builder()
            .timeout(config.timeout())
            .build()
            .map_err(|_| JevError::Config("http client build failed".into()))?;
        let breaker =
            CircuitBreaker::new(config.breaker_failure_threshold, config.breaker_cooldown());
        let cache = BoundedCache::new(config.cache_max_entries);
        Ok(Self {
            config,
            http,
            sleeper,
            clock,
            breaker,
            cache,
        })
    }

    pub fn config(&self) -> &JevConfig {
        &self.config
    }

    fn payload(&self, input: &ClassifyInput) -> serde_json::Value {
        json!({
            "model": self.config.model_id,
            "state": input.to_state(),
            "questions": questions(input.recent_failure.is_some()),
        })
    }

    /// 送信 payload と版から決まる cache key。
    fn cache_key(&self, payload: &serde_json::Value) -> String {
        let mut h = Sha256::new();
        h.update(self.config.schema_version.as_bytes());
        h.update([0]);
        h.update(self.config.prompt_version.as_bytes());
        h.update([0]);
        h.update(payload.to_string().as_bytes());
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    /// 要求を分類する。401/403 は `Err(Auth)`（retry・breaker 計数なし）。
    /// それ以外の失敗は bounded retry 後に `FallbackToFixed`。
    pub async fn classify(
        &self,
        input: &ClassifyInput,
        keys: &dyn ApiKeyProvider,
    ) -> Result<ClassifyOutcome, JevError> {
        let input = input.sanitized();
        let payload = self.payload(&input);
        let key_hash = self.cache_key(&payload);
        if let Some(c) = self.cache.get(&key_hash) {
            return Ok(ClassifyOutcome::Classified {
                classification: Box::new(c),
                cached: true,
            });
        }
        let Some(key) = keys.api_key() else {
            return Err(JevError::KeyMissing {
                secret_id: self.config.secret_id.clone(),
            });
        };
        if !self.breaker.allow(self.clock.now()) {
            return Ok(ClassifyOutcome::FallbackToFixed {
                reason: FallbackReason::BreakerOpen,
            });
        }
        let with_failure = input.recent_failure.is_some();
        let mut last = FallbackReason::Transport;
        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                self.sleeper.sleep(self.config.backoff(attempt - 1)).await;
            }
            match self.attempt(&payload, &key, with_failure).await {
                Attempt::Done(c) => {
                    self.breaker.record_success();
                    self.cache.put(key_hash, (*c).clone());
                    return Ok(ClassifyOutcome::Classified {
                        classification: c,
                        cached: false,
                    });
                }
                Attempt::Auth(status) => {
                    return Err(JevError::Auth {
                        status,
                        secret_id: self.config.secret_id.clone(),
                    });
                }
                Attempt::Fail(r) => {
                    last = r;
                    break;
                }
                Attempt::Retry(r) => last = r,
            }
        }
        self.breaker.record_failure(self.clock.now());
        Ok(ClassifyOutcome::FallbackToFixed { reason: last })
    }

    async fn attempt(
        &self,
        payload: &serde_json::Value,
        key: &ApiKey,
        with_failure: bool,
    ) -> Attempt {
        let resp = self
            .http
            .post(&self.config.endpoint)
            .bearer_auth(key.expose())
            .json(payload)
            .send()
            .await;
        let resp = match resp {
            Ok(r) => r,
            Err(e) if e.is_timeout() => return Attempt::Retry(FallbackReason::Timeout),
            Err(_) => return Attempt::Retry(FallbackReason::Transport),
        };
        let status = resp.status().as_u16();
        match status {
            401 | 403 => return Attempt::Auth(status),
            429 => return Attempt::Retry(FallbackReason::RateLimited),
            500..=599 => return Attempt::Retry(FallbackReason::ServerError(status)),
            200..=299 => {}
            _ => {
                let body = resp.text().await.unwrap_or_default();
                return Attempt::Fail(FallbackReason::ClientError(ClientErrorDetail::from_body(
                    status, &body,
                )));
            }
        }
        let body = match resp.text().await {
            Ok(b) => b,
            Err(e) if e.is_timeout() => return Attempt::Retry(FallbackReason::Timeout),
            Err(_) => return Attempt::Retry(FallbackReason::Transport),
        };
        let meta = Meta {
            model_id: &self.config.model_id,
            schema_version: &self.config.schema_version,
            prompt_version: &self.config.prompt_version,
        };
        match parse_response(&body, with_failure, meta) {
            Ok(c) => Attempt::Done(Box::new(c)),
            Err(ParseError::Invalid) => Attempt::Retry(FallbackReason::InvalidResponse),
            Err(ParseError::UnknownLabel) => Attempt::Retry(FallbackReason::UnknownClassification),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::time::Duration;

    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    use super::*;

    const SECRET: &str = "sk-test-SECRET-value-123456";

    struct Key(Option<&'static str>);
    impl ApiKeyProvider for Key {
        fn api_key(&self) -> Option<ApiKey> {
            self.0.map(ApiKey::new)
        }
    }

    #[derive(Default)]
    struct RecSleeper(Mutex<Vec<Duration>>);
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

    /// 応答を順に返す最小の HTTP server。受信 request を記録する。
    async fn serve(
        responses: Vec<(u16, String)>,
    ) -> (String, Arc<Mutex<Vec<String>>>, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|_| unreachable!());
        let addr = listener.local_addr().unwrap_or_else(|_| unreachable!());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let count = Arc::new(AtomicUsize::new(0));
        let (s2, c2) = (seen.clone(), count.clone());
        let mut q: VecDeque<_> = responses.into();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let Some(raw) =
                    fake_http::read(&mut sock, |r| fake_http::matches(r, "POST", "/decisions"))
                        .await
                else {
                    continue;
                };
                s2.lock().unwrap_or_else(|e| e.into_inner()).push(raw);
                c2.fetch_add(1, Ordering::SeqCst);
                let (status, body) = q.pop_front().unwrap_or((500, String::new()));
                let resp = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        (format!("http://{addr}/decisions"), seen, count)
    }

    const OK: &str = r#"{"answers":{"specificity":{"choice":"specified"},"work_nature":{"choice":"implementation"}}}"#;

    fn classifier(
        endpoint: String,
        sleeper: Arc<RecSleeper>,
        clock: Arc<FakeClock>,
    ) -> JevClassifier {
        let cfg = JevConfig {
            endpoint,
            max_retries: 2,
            breaker_failure_threshold: 2,
            ..Default::default()
        };
        JevClassifier::with_parts(cfg, sleeper, clock).unwrap_or_else(|_| unreachable!())
    }

    fn input(text: &str) -> ClassifyInput {
        ClassifyInput {
            instruction: text.into(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn success_sends_bearer_and_redacted_payload_then_caches() {
        let (url, seen, count) = serve(vec![(200, OK.into())]).await;
        fake_http::inject_noise(&url).await;
        let c = classifier(url, Arc::default(), Arc::new(FakeClock(AtomicU64::new(0))));
        let i = ClassifyInput {
            instruction: format!("deploy with token={SECRET} now"),
            ..Default::default()
        };
        let out = c.classify(&i, &Key(Some(SECRET))).await;
        assert!(matches!(
            out,
            Ok(ClassifyOutcome::Classified { cached: false, .. })
        ));
        let req = seen.lock().unwrap_or_else(|e| e.into_inner())[0].clone();
        assert!(
            req.to_ascii_lowercase()
                .contains(&format!("authorization: bearer {SECRET}").to_ascii_lowercase())
        );
        let body = req.split_once("\r\n\r\n").map(|x| x.1).unwrap_or("");
        assert!(!body.contains(SECRET), "secret leaked into payload");
        assert!(body.contains("typesafe/jev-1.14"));
        let again = c.classify(&i, &Key(None)).await;
        assert!(matches!(
            again,
            Ok(ClassifyOutcome::Classified { cached: true, .. })
        ));
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn auth_errors_are_typed_without_retry_or_breaker() {
        for status in [401u16, 403] {
            let (url, _, count) = serve(vec![(status, "{}".into()); 5]).await;
            let sleeper = Arc::new(RecSleeper::default());
            let c = classifier(url, sleeper.clone(), Arc::new(FakeClock(AtomicU64::new(0))));
            for _ in 0..3 {
                let r = c.classify(&input("x"), &Key(Some(SECRET))).await;
                assert_eq!(
                    r,
                    Err(JevError::Auth {
                        status,
                        secret_id: "jev_api_key".into()
                    })
                );
            }
            assert_eq!(count.load(Ordering::SeqCst), 3);
            assert!(
                sleeper
                    .0
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_empty()
            );
        }
    }

    #[tokio::test]
    async fn missing_key_is_typed() {
        let c = classifier(
            "http://127.0.0.1:1/x".into(),
            Arc::default(),
            Arc::new(FakeClock(AtomicU64::new(0))),
        );
        let r = c.classify(&input("x"), &Key(None)).await;
        assert_eq!(
            r,
            Err(JevError::KeyMissing {
                secret_id: "jev_api_key".into()
            })
        );
    }

    #[tokio::test]
    async fn retries_with_backoff_then_falls_back_and_breaker_opens() {
        let (url, _, count) = serve(vec![
            (429, "{}".into()),
            (503, "{}".into()),
            (500, "{}".into()),
        ])
        .await;
        let sleeper = Arc::new(RecSleeper::default());
        let clock = Arc::new(FakeClock(AtomicU64::new(0)));
        let c = classifier(url, sleeper.clone(), clock.clone());
        let out = c.classify(&input("a"), &Key(Some(SECRET))).await;
        assert_eq!(
            out,
            Ok(ClassifyOutcome::FallbackToFixed {
                reason: FallbackReason::ServerError(500)
            })
        );
        assert_eq!(count.load(Ordering::SeqCst), 3);
        assert_eq!(
            *sleeper.0.lock().unwrap_or_else(|e| e.into_inner()),
            vec![Duration::from_millis(200), Duration::from_millis(400)]
        );
        // 2 回目の失敗で breaker が開く（threshold 2）。以後は呼ばずに fixed。
        let _ = c.classify(&input("b"), &Key(Some(SECRET))).await;
        let before = count.load(Ordering::SeqCst);
        let out = c.classify(&input("c"), &Key(Some(SECRET))).await;
        assert_eq!(
            out,
            Ok(ClassifyOutcome::FallbackToFixed {
                reason: FallbackReason::BreakerOpen
            })
        );
        assert_eq!(count.load(Ordering::SeqCst), before);
        clock.0.store(3600, Ordering::SeqCst);
        assert!(c.breaker.allow(clock.now()));
    }

    #[tokio::test]
    async fn invalid_schema_and_unknown_label_fall_back() {
        let bad =
            r#"{"answers":{"specificity":{"choice":"weird"},"work_nature":{"choice":"design"}}}"#;
        let (url, _, count) = serve(vec![
            (200, "garbage".into()),
            (200, bad.into()),
            (200, OK.into()),
        ])
        .await;
        let c = classifier(url, Arc::default(), Arc::new(FakeClock(AtomicU64::new(0))));
        // 2 回の無効応答のあと 3 回目で成功（bounded retry 内）
        let out = c.classify(&input("a"), &Key(Some(SECRET))).await;
        assert!(matches!(out, Ok(ClassifyOutcome::Classified { .. })));
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn timeout_falls_back() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|_| unreachable!());
        let addr = listener.local_addr().unwrap_or_else(|_| unreachable!());
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((s, _)) = listener.accept().await {
                held.push(s); // 応答しない
            }
        });
        let cfg = JevConfig {
            endpoint: format!("http://{addr}/d"),
            timeout_ms: 50,
            max_retries: 1,
            ..Default::default()
        };
        let c = JevClassifier::with_parts(
            cfg,
            Arc::new(RecSleeper::default()),
            Arc::new(FakeClock(AtomicU64::new(0))),
        )
        .unwrap_or_else(|_| unreachable!());
        let out = c.classify(&input("a"), &Key(Some(SECRET))).await;
        assert_eq!(
            out,
            Ok(ClassifyOutcome::FallbackToFixed {
                reason: FallbackReason::Timeout
            })
        );
    }

    #[test]
    fn key_never_appears_in_debug_or_errors() {
        let k = ApiKey::new(SECRET);
        assert!(!format!("{k:?}").contains(SECRET));
        let e = JevError::Auth {
            status: 401,
            secret_id: "jev_api_key".into(),
        };
        assert!(!format!("{e} {e:?}").contains(SECRET));
        let c = classifier(
            "http://127.0.0.1:1/x".into(),
            Arc::default(),
            Arc::new(FakeClock(AtomicU64::new(0))),
        );
        assert!(!format!("{c:?}").contains(SECRET));
    }
}
