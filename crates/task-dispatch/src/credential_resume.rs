//! credential 待ちの検証と一度だけの再開（ADR 2026-10-15-mlsys-min-decision-assisted D4）。
//!
//! 秘密の保存完了（`secret_stored`）を契機に、provider adapter 境界の [`CredentialVerifier`] で最小費用の
//! 接続/権限検証を 1 回だけ行い、結果で待ち（`credential_waits`）を動かす:
//! - 有効 → `verified` → `claim_resume`（一度だけ）→ 同じ work unit を `ready` / `needs_continuation` へ。
//! - 誤キー・権限不足 → `rejected`（再登録待ち。完了扱いにせず、同じ fingerprint では再検証しない）。
//!   受信箱 item は `credential_waits` から導出されるので、同じ item の文面が更新される（新しい通知は重ねない）。
//! - 429・予算不足・一時障害 → 待ちを保つ。再登録要求にせず、上限付き backoff（注入した時計）で後から再検証する。
//!
//! 取り逃し対策（reconcile）: 開いている待ちの秘密ファイルの fingerprint を `secret_stored` の記録と突き合わせ、
//! 食い違えば記録を足す。LLM は呼ばない。秘密の値は検証関数へ渡すだけで、log・event・エラー本文に出さない。
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::{Digest, Sha256};
use task_core::{
    CredentialVerifyKind, CredentialWait, CredentialWaitState, Event, SqliteStore, StoreError,
    TaskStore, WorkUnitStatus,
};
use time::OffsetDateTime;

/// 検証の結果。429・予算不足・5xx・通信失敗は [`VerifyOutcome::Transient`]（再登録要求にしない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyOutcome {
    Valid,
    InvalidKey,
    InsufficientPermission,
    Transient,
}

/// provider adapter 境界。`value` は秘密の値（保持・出力しない）。実装は最小費用の 1 呼び出しだけを行う。
pub trait CredentialVerifier: Send + Sync {
    fn verify(&self, secret_id: &str, value: &str) -> VerifyOutcome;
}

/// 秘密の読み出し（ADR-0030 の `secrets_dir`。1 秘密 = 1 ファイル、末尾の改行は落とす）。
pub trait SecretSource: Send + Sync {
    fn read(&self, secret_id: &str) -> Option<String>;
}

pub struct FileSecretSource {
    pub dir: PathBuf,
}

impl SecretSource for FileSecretSource {
    fn read(&self, secret_id: &str) -> Option<String> {
        if secret_id.is_empty()
            || !secret_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return None;
        }
        let text = std::fs::read_to_string(self.dir.join(secret_id)).ok()?;
        let value = text.trim_end_matches(['\n', '\r']).to_string();
        (!value.trim().is_empty()).then_some(value)
    }
}

/// task-api の `secrets::fingerprint`（値の sha256 の先頭 8 桁）と同じ規則。
pub fn secret_fingerprint(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .take(4)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 一時障害の backoff の初期値と上限。
pub const BACKOFF_BASE: Duration = Duration::from_secs(30);
pub const BACKOFF_CAP: Duration = Duration::from_secs(15 * 60);

/// `attempt` 回目（1 始まり）の失敗の後に待つ時間（倍々、上限あり）。
pub fn backoff_delay(attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1).min(16);
    BACKOFF_BASE.saturating_mul(1u32 << shift).min(BACKOFF_CAP)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ResumeReport {
    pub verified_calls: usize,
    pub resumed: usize,
    pub rejected: usize,
    pub transient: usize,
    pub cancelled: usize,
}

#[derive(Default)]
struct Inner {
    /// 秘密ごとに最後に判定した (fingerprint, 結果)。同じ fingerprint を再検証しない。
    judged: HashMap<String, (String, VerifyOutcome)>,
    /// 秘密ごとの一時障害の連続回数と次に試す時刻。
    retry: HashMap<String, (u32, OffsetDateTime)>,
}

pub struct CredentialResumer {
    store: Arc<SqliteStore>,
    secrets: Arc<dyn SecretSource>,
    verifiers: BTreeMap<String, Arc<dyn CredentialVerifier>>,
    clock: Arc<dyn Fn() -> OffsetDateTime + Send + Sync>,
    inner: Mutex<Inner>,
}

impl CredentialResumer {
    pub fn new(
        store: Arc<SqliteStore>,
        secrets: Arc<dyn SecretSource>,
        clock: Arc<dyn Fn() -> OffsetDateTime + Send + Sync>,
    ) -> Self {
        Self {
            store,
            secrets,
            verifiers: BTreeMap::new(),
            clock,
            inner: Mutex::new(Inner::default()),
        }
    }

    /// provider（`credential_waits.provider`。例 `jev`）の検証を登録する。
    pub fn with_verifier(mut self, provider: &str, verifier: Arc<dyn CredentialVerifier>) -> Self {
        self.verifiers.insert(provider.to_string(), verifier);
        self
    }

    /// 開いている待ちがあるか（DB のみ。tick はこれが偽なら何もしない）。
    pub fn has_open_waits(&self) -> bool {
        self.store
            .credential_waits_list_waiting()
            .map(|w| !w.is_empty())
            .unwrap_or(false)
    }

    /// 1 回の処理: reconcile → 検証 → 再開。検証は秘密ごと・fingerprint ごとに 1 回だけ呼ぶ。
    pub fn process(&self) -> Result<ResumeReport, StoreError> {
        let now = (self.clock)();
        let mut report = ResumeReport::default();
        let waits = self.store.credential_waits_list_waiting()?;
        let mut by_secret: BTreeMap<String, Vec<CredentialWait>> = BTreeMap::new();
        for w in waits {
            by_secret.entry(w.secret_id.clone()).or_default().push(w);
        }
        for (secret_id, waits) in by_secret {
            self.process_secret(&secret_id, waits, now, &mut report)?;
        }
        Ok(report)
    }

    fn process_secret(
        &self,
        secret_id: &str,
        waits: Vec<CredentialWait>,
        now: OffsetDateTime,
        report: &mut ResumeReport,
    ) -> Result<(), StoreError> {
        // verified のまま残った待ち（claim・再キューの失敗の続き）は、検証せずに再開を試みる。
        for w in waits
            .iter()
            .filter(|w| w.state == CredentialWaitState::Verified)
        {
            self.claim_and_requeue(w, now, report)?;
        }
        if !waits.iter().any(|w| {
            matches!(
                w.state,
                CredentialWaitState::Waiting | CredentialWaitState::Rejected
            )
        }) {
            return Ok(());
        }
        // reconcile: 保存済みの秘密と記録を突き合わせる（取り逃した `secret_stored` を足す。重複は何もしない）。
        let Some(value) = self.secrets.read(secret_id) else {
            return Ok(());
        };
        let fp = secret_fingerprint(&value);
        self.store
            .credential_record_secret_saved(secret_id, &fp, now)?;
        let current = self.store.credential_waits_list_waiting()?;
        let open: Vec<&CredentialWait> = current
            .iter()
            .filter(|w| {
                w.secret_id == secret_id
                    && matches!(
                        w.state,
                        CredentialWaitState::Waiting | CredentialWaitState::Rejected
                    )
                    && w.seen_fingerprint.as_deref() == Some(fp.as_str())
            })
            .collect();
        if open.is_empty() {
            return Ok(());
        }
        let known = self.inner_lock().judged.get(secret_id).cloned();
        let outcome = match known {
            Some((jfp, outcome)) if jfp == fp && outcome != VerifyOutcome::Transient => outcome,
            _ => {
                {
                    let inner = self.inner_lock();
                    if let Some((_, not_before)) = inner.retry.get(secret_id)
                        && *not_before > now
                    {
                        return Ok(());
                    }
                }
                // 同じ provider の待ちは同じ検証器を使う（秘密 1 つにつき provider は 1 つ）。
                let Some(verifier) = open
                    .iter()
                    .find_map(|w| self.verifiers.get(&w.provider))
                    .cloned()
                else {
                    return Ok(());
                };
                report.verified_calls += 1;
                let outcome = verifier.verify(secret_id, &value);
                let mut inner = self.inner_lock();
                if outcome == VerifyOutcome::Transient {
                    let attempt = inner.retry.get(secret_id).map(|r| r.0).unwrap_or(0) + 1;
                    let delay = time::Duration::try_from(backoff_delay(attempt))
                        .unwrap_or(time::Duration::minutes(15));
                    inner
                        .retry
                        .insert(secret_id.to_string(), (attempt, now + delay));
                    inner.judged.remove(secret_id);
                } else {
                    inner.retry.remove(secret_id);
                    inner
                        .judged
                        .insert(secret_id.to_string(), (fp.clone(), outcome));
                }
                outcome
            }
        };
        for w in open {
            match outcome {
                VerifyOutcome::Transient => report.transient += 1,
                VerifyOutcome::Valid => {
                    if self.store.credential_wait_mark_verified(
                        &w.work_unit_id,
                        secret_id,
                        &fp,
                        now,
                    )? {
                        self.claim_and_requeue(w, now, report)?;
                    }
                }
                VerifyOutcome::InvalidKey | VerifyOutcome::InsufficientPermission => {
                    let kind = if outcome == VerifyOutcome::InvalidKey {
                        CredentialVerifyKind::InvalidKey
                    } else {
                        CredentialVerifyKind::InsufficientPermission
                    };
                    // 既に同じ判定の rejected なら状態は変わらず、件数にも数えない。
                    let again = w.state == CredentialWaitState::Rejected
                        && w.last_verify_kind == Some(kind);
                    if self.store.credential_wait_mark_rejected(
                        &w.work_unit_id,
                        secret_id,
                        &fp,
                        kind,
                        now,
                    )? && !again
                    {
                        report.rejected += 1;
                    }
                }
            }
        }
        Ok(())
    }

    fn inner_lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `verified → resumed` を一度だけ取り、取れたときだけ work unit を戻す。cancel 済みの task は戻さない。
    fn claim_and_requeue(
        &self,
        w: &CredentialWait,
        now: OffsetDateTime,
        report: &mut ResumeReport,
    ) -> Result<(), StoreError> {
        if !self
            .store
            .credential_wait_claim_resume(&w.work_unit_id, &w.secret_id, now)?
        {
            if self
                .store
                .credential_wait_get(&w.work_unit_id, &w.secret_id)?
                .is_some_and(|c| c.state == CredentialWaitState::Cancelled)
            {
                report.cancelled += 1;
            }
            return Ok(());
        }
        match requeue_work_unit(self.store.as_ref(), w, now) {
            Ok(()) => report.resumed += 1,
            Err(e) => {
                tracing::warn!(work_unit_id = %w.work_unit_id, error = %e, "could not requeue the work unit after credential verification; will retry");
                self.store
                    .credential_wait_release_claim(&w.work_unit_id, &w.secret_id, now)?;
            }
        }
        Ok(())
    }
}

/// 待っていた work unit を `blocked` から戻す（checkpoint があれば `needs_continuation`、無ければ `ready`）。
/// unit が既に `blocked` でなければ何もしない（二重に戻さない）。
fn requeue_work_unit(
    store: &dyn TaskStore,
    w: &CredentialWait,
    now: OffsetDateTime,
) -> Result<(), StoreError> {
    let units = store.work_units_for(w.task_id)?;
    let Some(unit) = units
        .into_iter()
        .find(|u| u.id == w.work_unit_id && u.status == WorkUnitStatus::Blocked)
    else {
        return Ok(());
    };
    let to = if unit.last_checkpoint_run_id.is_some() {
        WorkUnitStatus::NeedsContinuation
    } else {
        WorkUnitStatus::Ready
    };
    let mut row = unit.clone();
    row.status = to;
    row.blocked_reason = None;
    row.updated_at = now
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| StoreError::Invalid(e.to_string()))?;
    let event = Event::WorkUnitTransitioned {
        work_unit_id: unit.id.clone(),
        key: unit.key.clone(),
        from: unit.status,
        to,
        reason: "credential_resumed".to_string(),
        run_id: None,
    };
    store.work_unit_transition(w.task_id, row, event)
}

/// Jev（OpenRouter の Decisions API）の最小費用の検証。`state` 1 語・`noul` 質問 1 つだけを送る。
/// 401 → 誤キー、403 → 権限不足、200 → 有効、それ以外（402・429・5xx・通信失敗）は一時障害。
pub struct JevVerifier {
    pub endpoint: String,
    pub model: String,
    pub timeout: Duration,
}

impl JevVerifier {
    pub fn openrouter_default() -> Self {
        Self {
            endpoint: "https://openrouter.ai/api/alpha/decisions".to_string(),
            model: "typesafe/jev-1.14".to_string(),
            timeout: Duration::from_secs(20),
        }
    }
}

/// HTTP status の分類（実ネットワーク無しで試験できるよう分けてある）。
pub fn classify_http_status(status: u16) -> VerifyOutcome {
    match status {
        200..=299 => VerifyOutcome::Valid,
        401 => VerifyOutcome::InvalidKey,
        403 => VerifyOutcome::InsufficientPermission,
        _ => VerifyOutcome::Transient,
    }
}

impl CredentialVerifier for JevVerifier {
    fn verify(&self, _secret_id: &str, value: &str) -> VerifyOutcome {
        let Ok(client) = reqwest::blocking::Client::builder()
            .timeout(self.timeout)
            .build()
        else {
            return VerifyOutcome::Transient;
        };
        let body = serde_json::json!({
            "model": self.model,
            "state": "ping",
            "questions": {"ok": {"type": "noul", "instructions": "Is this a ping?", "criteria": "yes"}},
        });
        match client
            .post(&self.endpoint)
            .bearer_auth(value)
            .json(&body)
            .send()
        {
            Ok(resp) => classify_http_status(resp.status().as_u16()),
            // reqwest の Error は URL を含むので文面は使わない。
            Err(_) => VerifyOutcome::Transient,
        }
    }
}

#[cfg(test)]
#[path = "credential_resume_tests.rs"]
mod tests;
