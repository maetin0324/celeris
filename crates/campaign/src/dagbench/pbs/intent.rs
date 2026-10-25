//! recovery 比較の submit intent 台帳（ADR 2026-10-25-recovery-scheduler-backend D4・実装付記 I3）。
//!
//! - 単一 submit owner は identity・資源/payload hash・決定的 `submit_intent_id` を `submit-intents.jsonl`（追記専用、
//!   1 行 = 1 遷移、書くたびに `sync_data`）へ確定してから qsub を 1 回だけ送る。
//! - 同じ dir の `submit-intents.lock` の排他 lock（`File::lock`）の中で台帳を読み直し、qsub の前後の記録を
//!   同じ lock 区間で書く。別 process・別 thread から同じ intent を投入しても物理 qsub は 1 回に収まる。
//! - 送信前後どちらで止まっても台帳から再開する。最後が `intent_recorded`/`sent` なら送ったか分からないので
//!   unknown_submit へ進め、qsub を送り直さない。
//! - unknown_submit は [`PbsTransport::find_by_marker`] で候補 1 件、かつ `qstat -x -f` の owner/cluster/account/queue/
//!   資源/script が intent と一致したときだけ job ID を追記して再追跡する。複数・不一致・履歴欠落・権限不足・Unsupported は
//!   unresolved_stop（終端）。0 件と照会の timeout・切断は poll 間隔ごとに試行 deadline 内だけ再照合し、deadline で
//!   unresolved_stop。未投入にも成功にも変換せず、自動再送・qdel をしない。時刻は [`IntentClock`] で注入する。
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions, TryLockError},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::transport::{
    MarkerClass, MarkerQuery, PbsReply, PbsTransport, QstatClass, QsubClass, QsubRequest,
    QueryFailure, SUBMIT_INTENT_VAR, UnknownReason, classify_marker, classify_qstat, classify_qsub,
};
use crate::{
    dagbench::ext::drop_torn_tail,
    observe::{TRIAL_MARKER_VAR, job_variable},
};

/// 台帳の file 名。
pub const INTENT_LEDGER: &str = "submit-intents.jsonl";
/// 排他 lock の file 名（台帳と同じ dir）。
pub const INTENT_LOCK: &str = "submit-intents.lock";
/// 台帳 1 行の schema。
pub const INTENT_ROW_SCHEMA: &str = "celeris.submit-intent/1";
/// `submit_intent_id` の元にする canonical JSON の schema。
pub const INTENT_ID_SCHEMA: &str = "celeris.submit-intent-id/1";

/// 1 つの外部投入の identity。`submit_intent_id` はこれだけから決まる（再起動・controller epoch で変わらない）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmitIdentity {
    pub measurement_id: String,
    pub arm: String,
    pub trial_key: String,
    pub logical_job_id: String,
    pub node: String,
    pub workload_attempt: u32,
}

/// canonical JSON（key の辞書順に並べた struct）。
#[derive(Serialize)]
struct IdCanon<'a> {
    arm: &'a str,
    logical_job_id: &'a str,
    measurement_id: &'a str,
    node: &'a str,
    schema: &'a str,
    trial_key: &'a str,
    workload_attempt: u32,
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl SubmitIdentity {
    /// `"si-" + hex(sha256(canonical JSON))`（64 桁全体）。
    pub fn submit_intent_id(&self) -> String {
        let canon = IdCanon {
            arm: &self.arm,
            logical_job_id: &self.logical_job_id,
            measurement_id: &self.measurement_id,
            node: &self.node,
            schema: INTENT_ID_SCHEMA,
            trial_key: &self.trial_key,
            workload_attempt: self.workload_attempt,
        };
        let json = serde_json::to_vec(&canon).unwrap_or_default();
        format!("si-{}", sha256_hex(&json))
    }
}

/// 投入の全条件。資源・投入先・payload hash・poll 間隔・試行 deadline を intent に固定する。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntentSpec {
    pub identity: SubmitIdentity,
    /// PBS 上の投入者（`Job_Owner` の user 部）。marker 検索もこの user の job に限る。
    pub owner: String,
    /// cluster 名。`server`（無ければ job ID の server 部）の先頭 label と一致を求める。
    pub cluster: String,
    pub queue: String,
    pub account: String,
    pub select: String,
    pub place: String,
    pub walltime: String,
    /// `-N`。表示用で照合には使わない。
    pub job_name: String,
    /// cluster 上の job script の絶対 path。
    pub script: String,
    /// job script と入力の sha256（呼び出し側が spec から渡す）。
    pub payload_sha256: String,
    pub poll_secs: u64,
    pub trial_deadline_secs: u64,
}

/// 資源 hash の元（canonical、key の辞書順）。
#[derive(Serialize)]
struct ResourceCanon<'a> {
    account: &'a str,
    cluster: &'a str,
    owner: &'a str,
    place: &'a str,
    queue: &'a str,
    rerunnable: bool,
    script: &'a str,
    select: &'a str,
    walltime: &'a str,
}

impl IntentSpec {
    pub fn submit_intent_id(&self) -> String {
        self.identity.submit_intent_id()
    }

    /// 投入先・資源・script の sha256。
    pub fn resource_sha256(&self) -> String {
        let canon = ResourceCanon {
            account: &self.account,
            cluster: &self.cluster,
            owner: &self.owner,
            place: &self.place,
            queue: &self.queue,
            rerunnable: false,
            script: &self.script,
            select: &self.select,
            walltime: &self.walltime,
        };
        sha256_hex(&serde_json::to_vec(&canon).unwrap_or_default())
    }

    /// qsub の要求（両 marker は identity から埋める）。
    pub fn request(&self) -> QsubRequest {
        QsubRequest {
            queue: self.queue.clone(),
            account: self.account.clone(),
            select: self.select.clone(),
            place: self.place.clone(),
            walltime: self.walltime.clone(),
            job_name: self.job_name.clone(),
            trial_key: self.identity.trial_key.clone(),
            submit_intent: self.submit_intent_id(),
            script: self.script.clone(),
        }
    }

    pub fn marker_query(&self) -> MarkerQuery {
        MarkerQuery {
            owner: self.owner.clone(),
            trial_key: self.identity.trial_key.clone(),
            submit_intent: self.submit_intent_id(),
        }
    }

    pub fn job_ref(&self, job_id: &str) -> String {
        format!("pbs:{}:{job_id}", self.cluster)
    }

    /// `qstat -x -f` の原票が intent と一致するか。欠けた欄も不一致（推定で補わない）。
    pub fn check_record(
        &self,
        job_id: &str,
        attrs: &BTreeMap<String, String>,
    ) -> Result<(), String> {
        let get = |k: &str| attrs.get(k).map(String::as_str);
        let want = |k: &str, v: &str| match get(k) {
            Some(got) if got == v => Ok(()),
            Some(got) => Err(format!("{k} is {got:?}, intent has {v:?}")),
            None => Err(format!("{k} is missing from the record")),
        };
        match get("Job_Owner").map(|o| o.split('@').next().unwrap_or_default()) {
            Some(o) if o == self.owner => {}
            got => return Err(format!("Job_Owner is {got:?}, intent has {:?}", self.owner)),
        }
        let server = get("server")
            .map(str::to_string)
            .or_else(|| job_id.split_once('.').map(|(_, h)| h.to_string()))
            .unwrap_or_default();
        if server.split('.').next() != Some(self.cluster.as_str()) {
            return Err(format!(
                "server {server:?} is not cluster {:?}",
                self.cluster
            ));
        }
        want("Account_Name", &self.account)?;
        want("queue", &self.queue)?;
        want("Resource_List.select", &self.select)?;
        want("Resource_List.place", &self.place)?;
        want("Resource_List.walltime", &self.walltime)?;
        want("Rerunable", "False")?;
        let intent = self.submit_intent_id();
        for (var, v) in [
            (TRIAL_MARKER_VAR, self.identity.trial_key.as_str()),
            (SUBMIT_INTENT_VAR, intent.as_str()),
        ] {
            if job_variable(attrs, var).as_deref() != Some(v) {
                return Err(format!("Variable_List {var} is not {v:?}"));
            }
        }
        match get("Submit_arguments").and_then(|a| a.split_whitespace().last()) {
            Some(s) if s == self.script => Ok(()),
            got => Err(format!(
                "Submit_arguments script is {got:?}, intent has {:?}",
                self.script
            )),
        }
    }
}

/// unresolved_stop の理由。どれも未投入・成功の根拠にしない。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnresolvedReason {
    MultipleCandidates,
    Mismatch,
    /// 候補の原票を scheduler が知らない（履歴が消えた）。
    HistoryMissing,
    /// 照会が非 0 で終わった（権限不足など）。
    QueryRefused,
    /// 検索手段が未確認（D4.4）。
    Unsupported,
    /// 照会の応答が期待した形でない。
    InvalidReply,
    /// 試行 deadline まで照合できなかった（0 件・timeout・切断の続き）。
    DeadlineReached,
    /// 送信前拒否（D4.6 によりこの接続確認では停止）。
    RejectedBeforeSend,
}

/// 再照合を待つ原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryCause {
    /// 候補 0 件（未投入の証拠ではない）。
    NoCandidate,
    /// 照会の timeout・切断・起動失敗。
    QueryUnavailable,
}

/// unknown_submit の理由。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownCause {
    Qsub(UnknownReason),
    /// 前の owner が `intent_recorded`/`sent` の後で止まった（送ったか分からない）。
    Interrupted,
}

/// 台帳の遷移。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum IntentEvent {
    IntentRecorded {
        spec: Box<IntentSpec>,
        resource_sha256: String,
        deadline_unix: u64,
    },
    /// qsub を起動する直前。
    Sent,
    Accepted {
        job_id: String,
        job_ref: String,
        reply: Box<PbsReply>,
    },
    RejectedBeforeSend {
        reply: Box<PbsReply>,
    },
    UnknownSubmit {
        cause: UnknownCause,
        detail: String,
        reply: Option<PbsReply>,
    },
    /// 照合できず、`next_check_unix` に再照合する。
    ReconcileRetry {
        cause: RetryCause,
        detail: String,
        next_check_unix: u64,
        replies: Vec<PbsReply>,
    },
    Reconciled {
        job_id: String,
        job_ref: String,
        payload_sha256: String,
        replies: Vec<PbsReply>,
    },
    UnresolvedStop {
        reason: UnresolvedReason,
        detail: String,
        replies: Vec<PbsReply>,
    },
}

/// 台帳 1 行。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentRow {
    pub schema: String,
    pub seq: u64,
    pub at_utc: String,
    pub epoch: String,
    pub submit_intent_id: String,
    #[serde(flatten)]
    pub event: IntentEvent,
}

/// 台帳を再生した 1 intent の状態。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IntentState {
    /// `intent_recorded` の後（送ったか分からない）。
    Recorded,
    /// `sent` の後で結果の行が無い（送ったか分からない）。
    Sent,
    Accepted {
        job_id: String,
    },
    RejectedBeforeSend,
    Unknown {
        next_check_unix: u64,
    },
    Reconciled {
        job_id: String,
    },
    Unresolved {
        reason: UnresolvedReason,
    },
}

/// 1 intent の再生結果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntentStatus {
    pub spec: IntentSpec,
    pub resource_sha256: String,
    pub deadline_unix: u64,
    pub state: IntentState,
}

/// owner に返す結果。`Pending` は unknown_submit のまま次の照合待ち。未投入を表す variant は持たない。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubmitOutcome {
    Accepted {
        job_id: String,
        job_ref: String,
    },
    Reconciled {
        job_id: String,
        job_ref: String,
    },
    Pending {
        next_check_unix: u64,
        cause: RetryCause,
    },
    Unresolved {
        reason: UnresolvedReason,
        detail: String,
    },
}

#[derive(Debug)]
pub enum IntentError {
    Io(std::io::Error),
    /// 同じ `submit_intent_id` が別の条件で記録済み。qsub は送らない。
    Conflict(String),
    /// 台帳の行が遷移規則に合わない・未知の intent。
    Ledger(String),
}

impl std::fmt::Display for IntentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "submit intent ledger: {e}"),
            Self::Conflict(s) => write!(f, "submit intent conflict: {s}"),
            Self::Ledger(s) => write!(f, "submit intent ledger: {s}"),
        }
    }
}

impl std::error::Error for IntentError {}

impl From<std::io::Error> for IntentError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// 注入時計（UTC の unix 秒）。試験は sleep せず時刻を進める実装を渡す。
pub trait IntentClock {
    fn now_unix(&self) -> u64;
    /// `t` まで待つ。
    fn sleep_until(&self, t: u64);
}

/// 実時計。
pub struct SystemClock;

impl IntentClock for SystemClock {
    fn now_unix(&self) -> u64 {
        OffsetDateTime::now_utc().unix_timestamp().max(0) as u64
    }
    fn sleep_until(&self, t: u64) {
        let now = self.now_unix();
        if t > now {
            std::thread::sleep(Duration::from_secs(t - now));
        }
    }
}

fn utc(t: u64) -> String {
    i64::try_from(t)
        .ok()
        .and_then(|s| OffsetDateTime::from_unix_timestamp(s).ok())
        .and_then(|d| d.format(&Rfc3339).ok())
        .unwrap_or_default()
}

/// lock を待つ直前に呼ぶ hook（試験で lock 待ちを出来事として同期する）。
pub type LockWaitHook = Arc<dyn Fn() + Send + Sync>;

/// submit intent 台帳。状態は持たず、操作ごとに lock の中で file を読み直す。
pub struct IntentLedger {
    dir: PathBuf,
    epoch: String,
    lock_wait_hook: Option<LockWaitHook>,
}

/// lock 区間。drop で file を閉じて lock を放す。
struct Held<'a> {
    ledger: &'a IntentLedger,
    _lock: File,
    rows: Vec<IntentRow>,
}

impl IntentLedger {
    /// `dir` に台帳を置く（無ければ作る）。`epoch` は observer epoch として各行に残す。
    pub fn open(dir: &Path, epoch: &str) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        Ok(Self {
            dir: dir.to_path_buf(),
            epoch: epoch.into(),
            lock_wait_hook: None,
        })
    }

    pub fn with_lock_wait_hook(mut self, hook: LockWaitHook) -> Self {
        self.lock_wait_hook = Some(hook);
        self
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join(INTENT_LEDGER)
    }

    fn lock(&self) -> Result<Held<'_>, IntentError> {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.dir.join(INTENT_LOCK))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                if let Some(h) = &self.lock_wait_hook {
                    h();
                }
                lock.lock()?;
            }
            Err(TryLockError::Error(e)) => return Err(e.into()),
        }
        let path = self.path();
        drop_torn_tail(&path)?;
        let rows = read_rows(&path)?;
        Ok(Held {
            ledger: self,
            _lock: lock,
            rows,
        })
    }

    /// 全行（lock の中で読む）。
    pub fn rows(&self) -> Result<Vec<IntentRow>, IntentError> {
        Ok(self.lock()?.rows)
    }

    /// 全 intent の再生結果。
    pub fn replay(&self) -> Result<BTreeMap<String, IntentStatus>, IntentError> {
        replay(&self.lock()?.rows)
    }

    /// intent を確定してから qsub を 1 回だけ送る。既に記録された intent なら送らずに再生・再照合へ回す。
    pub fn submit(
        &self,
        spec: &IntentSpec,
        transport: &dyn PbsTransport,
        clock: &dyn IntentClock,
    ) -> Result<SubmitOutcome, IntentError> {
        let id = spec.submit_intent_id();
        let mut held = self.lock()?;
        let status = replay(&held.rows)?.remove(&id);
        let Some(status) = status else {
            let now = clock.now_unix();
            held.append(
                &id,
                now,
                IntentEvent::IntentRecorded {
                    spec: Box::new(spec.clone()),
                    resource_sha256: spec.resource_sha256(),
                    deadline_unix: now.saturating_add(spec.trial_deadline_secs),
                },
            )?;
            held.append(&id, now, IntentEvent::Sent)?;
            let reply = transport.qsub(&spec.request());
            let now = clock.now_unix();
            return match classify_qsub(&reply) {
                QsubClass::Accepted { job_id } => {
                    let job_ref = spec.job_ref(&job_id);
                    // 書けなければ Err。台帳は `sent` で終わり、次の owner が unknown_submit へ進める。
                    held.append(
                        &id,
                        now,
                        IntentEvent::Accepted {
                            job_id: job_id.clone(),
                            job_ref: job_ref.clone(),
                            reply: Box::new(reply),
                        },
                    )?;
                    Ok(SubmitOutcome::Accepted { job_id, job_ref })
                }
                QsubClass::RejectedBeforeSend { detail } => {
                    held.append(
                        &id,
                        now,
                        IntentEvent::RejectedBeforeSend {
                            reply: Box::new(reply),
                        },
                    )?;
                    Ok(SubmitOutcome::Unresolved {
                        reason: UnresolvedReason::RejectedBeforeSend,
                        detail,
                    })
                }
                QsubClass::Unknown { reason, detail } => {
                    held.append(
                        &id,
                        now,
                        IntentEvent::UnknownSubmit {
                            cause: UnknownCause::Qsub(reason),
                            detail,
                            reply: Some(reply),
                        },
                    )?;
                    held.reconcile_pass(&id, transport, clock)
                }
            };
        };
        if status.spec != *spec || status.resource_sha256 != spec.resource_sha256() {
            return Err(IntentError::Conflict(format!(
                "{id} is recorded with different resources or payload"
            )));
        }
        held.resume(&id, status, transport, clock)
    }

    /// 記録済み intent の照合を 1 回だけ進める（期限前なら照会しない）。
    pub fn reconcile(
        &self,
        submit_intent_id: &str,
        transport: &dyn PbsTransport,
        clock: &dyn IntentClock,
    ) -> Result<SubmitOutcome, IntentError> {
        let mut held = self.lock()?;
        let status = replay(&held.rows)?
            .remove(submit_intent_id)
            .ok_or_else(|| IntentError::Ledger(format!("unknown intent {submit_intent_id}")))?;
        held.resume(submit_intent_id, status, transport, clock)
    }

    /// unknown_submit が解決（reconciled）か停止（unresolved_stop）するまで、poll 間隔ごとに照合する。
    /// 待ちは lock の外で [`IntentClock::sleep_until`] を使う。
    pub fn resolve(
        &self,
        submit_intent_id: &str,
        transport: &dyn PbsTransport,
        clock: &dyn IntentClock,
    ) -> Result<SubmitOutcome, IntentError> {
        loop {
            match self.reconcile(submit_intent_id, transport, clock)? {
                SubmitOutcome::Pending {
                    next_check_unix, ..
                } => clock.sleep_until(next_check_unix),
                done => return Ok(done),
            }
        }
    }
}

impl Held<'_> {
    fn append(&mut self, id: &str, now: u64, event: IntentEvent) -> Result<(), IntentError> {
        let row = IntentRow {
            schema: INTENT_ROW_SCHEMA.into(),
            seq: self.rows.last().map_or(1, |r| r.seq + 1),
            at_utc: utc(now),
            epoch: self.ledger.epoch.clone(),
            submit_intent_id: id.into(),
            event,
        };
        let mut line = serde_json::to_vec(&row).map_err(std::io::Error::other)?;
        line.push(b'\n');
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.ledger.path())?;
        f.write_all(&line)?;
        f.sync_data()?;
        self.rows.push(row);
        Ok(())
    }

    fn resume(
        &mut self,
        id: &str,
        status: IntentStatus,
        transport: &dyn PbsTransport,
        clock: &dyn IntentClock,
    ) -> Result<SubmitOutcome, IntentError> {
        let spec = &status.spec;
        match status.state {
            IntentState::Accepted { job_id } => Ok(SubmitOutcome::Accepted {
                job_ref: spec.job_ref(&job_id),
                job_id,
            }),
            IntentState::Reconciled { job_id } => Ok(SubmitOutcome::Reconciled {
                job_ref: spec.job_ref(&job_id),
                job_id,
            }),
            IntentState::Unresolved { reason } => Ok(SubmitOutcome::Unresolved {
                reason,
                detail: "recorded as unresolved_stop".into(),
            }),
            IntentState::RejectedBeforeSend => Ok(SubmitOutcome::Unresolved {
                reason: UnresolvedReason::RejectedBeforeSend,
                detail: "recorded as rejected_before_send".into(),
            }),
            IntentState::Recorded | IntentState::Sent => {
                self.append(
                    id,
                    clock.now_unix(),
                    IntentEvent::UnknownSubmit {
                        cause: UnknownCause::Interrupted,
                        detail: format!("previous owner stopped after {:?}", status.state),
                        reply: None,
                    },
                )?;
                self.reconcile_pass(id, transport, clock)
            }
            IntentState::Unknown { .. } => self.reconcile_pass(id, transport, clock),
        }
    }

    /// unknown_submit の照合 1 回。
    fn reconcile_pass(
        &mut self,
        id: &str,
        transport: &dyn PbsTransport,
        clock: &dyn IntentClock,
    ) -> Result<SubmitOutcome, IntentError> {
        let status = replay(&self.rows)?
            .remove(id)
            .ok_or_else(|| IntentError::Ledger(format!("unknown intent {id}")))?;
        let IntentState::Unknown { next_check_unix } = status.state else {
            return Err(IntentError::Ledger(format!(
                "{id} is not unknown_submit: {:?}",
                status.state
            )));
        };
        let spec = status.spec;
        let now = clock.now_unix();
        if now >= status.deadline_unix {
            return self.stop(
                id,
                now,
                UnresolvedReason::DeadlineReached,
                format!("trial deadline {} reached", utc(status.deadline_unix)),
                Vec::new(),
            );
        }
        if now < next_check_unix {
            let cause = self
                .rows
                .iter()
                .rev()
                .find_map(|r| match &r.event {
                    IntentEvent::ReconcileRetry { cause, .. } if r.submit_intent_id == id => {
                        Some(*cause)
                    }
                    _ => None,
                })
                .unwrap_or(RetryCause::QueryUnavailable);
            return Ok(SubmitOutcome::Pending {
                next_check_unix,
                cause,
            });
        }
        let find = transport.find_by_marker(&spec.marker_query());
        let mut replies = vec![find.clone()];
        let retry = |held: &mut Self, cause, detail: String, replies: Vec<PbsReply>| {
            let now = clock.now_unix();
            let next = now.saturating_add(spec.poll_secs.max(1));
            held.append(
                id,
                now,
                IntentEvent::ReconcileRetry {
                    cause,
                    detail,
                    next_check_unix: next,
                    replies,
                },
            )?;
            Ok(SubmitOutcome::Pending {
                next_check_unix: next,
                cause,
            })
        };
        let job_id = match classify_marker(&find, &spec.marker_query()) {
            MarkerClass::Failed { reason, detail } => {
                return match failure_stop(reason) {
                    Some(r) => self.stop(id, clock.now_unix(), r, detail, replies),
                    None => retry(self, RetryCause::QueryUnavailable, detail, replies),
                };
            }
            MarkerClass::Candidates(c) if c.is_empty() => {
                return retry(
                    self,
                    RetryCause::NoCandidate,
                    "no job carries both markers".into(),
                    replies,
                );
            }
            MarkerClass::Candidates(c) if c.len() > 1 => {
                return self.stop(
                    id,
                    clock.now_unix(),
                    UnresolvedReason::MultipleCandidates,
                    format!("candidates {c:?}"),
                    replies,
                );
            }
            MarkerClass::Candidates(mut c) => c.remove(0),
        };
        let qstat = transport.qstat_full(&job_id);
        replies.push(qstat.clone());
        let now = clock.now_unix();
        match classify_qstat(&qstat, &job_id) {
            QstatClass::Record { attrs } => match spec.check_record(&job_id, &attrs) {
                Ok(()) => {
                    let job_ref = spec.job_ref(&job_id);
                    self.append(
                        id,
                        now,
                        IntentEvent::Reconciled {
                            job_id: job_id.clone(),
                            job_ref: job_ref.clone(),
                            payload_sha256: spec.payload_sha256.clone(),
                            replies,
                        },
                    )?;
                    Ok(SubmitOutcome::Reconciled { job_id, job_ref })
                }
                Err(e) => self.stop(id, now, UnresolvedReason::Mismatch, e, replies),
            },
            QstatClass::UnknownJob { detail } => {
                self.stop(id, now, UnresolvedReason::HistoryMissing, detail, replies)
            }
            QstatClass::Failed { reason, detail } => match failure_stop(reason) {
                Some(r) => self.stop(id, now, r, detail, replies),
                None => retry(self, RetryCause::QueryUnavailable, detail, replies),
            },
        }
    }

    fn stop(
        &mut self,
        id: &str,
        now: u64,
        reason: UnresolvedReason,
        detail: String,
        replies: Vec<PbsReply>,
    ) -> Result<SubmitOutcome, IntentError> {
        self.append(
            id,
            now,
            IntentEvent::UnresolvedStop {
                reason,
                detail: detail.clone(),
                replies,
            },
        )?;
        Ok(SubmitOutcome::Unresolved { reason, detail })
    }
}

/// 照会失敗のうち停止させるもの（None は deadline 内で再照合）。
fn failure_stop(f: QueryFailure) -> Option<UnresolvedReason> {
    match f {
        QueryFailure::NotStarted
        | QueryFailure::Unknown(UnknownReason::Timeout | UnknownReason::Disconnected) => None,
        QueryFailure::Unknown(UnknownReason::NonZero) => Some(UnresolvedReason::QueryRefused),
        QueryFailure::Unknown(UnknownReason::Unsupported) => Some(UnresolvedReason::Unsupported),
        QueryFailure::Unknown(UnknownReason::InvalidId) => Some(UnresolvedReason::InvalidReply),
    }
}

fn read_rows(path: &Path) -> std::io::Result<Vec<IntentRow>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for line in BufReader::new(File::open(path)?).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        out.push(serde_json::from_str(&line).map_err(std::io::Error::other)?);
    }
    Ok(out)
}

/// 行を順に再生する。遷移規則に合わない行（記録前の遷移・終端の後の行・2 本目の intent_recorded）は Err。
pub fn replay(rows: &[IntentRow]) -> Result<BTreeMap<String, IntentStatus>, IntentError> {
    let mut out: BTreeMap<String, IntentStatus> = BTreeMap::new();
    for r in rows {
        let bad = |why: &str| {
            IntentError::Ledger(format!(
                "row seq {} for {}: {why}",
                r.seq, r.submit_intent_id
            ))
        };
        if r.schema != INTENT_ROW_SCHEMA {
            return Err(bad("unknown schema"));
        }
        if let IntentEvent::IntentRecorded {
            spec,
            resource_sha256,
            deadline_unix,
        } = &r.event
        {
            if out.contains_key(&r.submit_intent_id) {
                return Err(bad("second intent_recorded"));
            }
            if spec.submit_intent_id() != r.submit_intent_id {
                return Err(bad("submit_intent_id does not match the identity"));
            }
            out.insert(
                r.submit_intent_id.clone(),
                IntentStatus {
                    spec: (**spec).clone(),
                    resource_sha256: resource_sha256.clone(),
                    deadline_unix: *deadline_unix,
                    state: IntentState::Recorded,
                },
            );
            continue;
        }
        let st = out
            .get_mut(&r.submit_intent_id)
            .ok_or_else(|| bad("transition before intent_recorded"))?;
        let unknown = matches!(st.state, IntentState::Unknown { .. });
        st.state = match (&st.state, &r.event) {
            (IntentState::Recorded, IntentEvent::Sent) => IntentState::Sent,
            (IntentState::Sent, IntentEvent::Accepted { job_id, .. }) => IntentState::Accepted {
                job_id: job_id.clone(),
            },
            (IntentState::Sent, IntentEvent::RejectedBeforeSend { .. }) => {
                IntentState::RejectedBeforeSend
            }
            (IntentState::Recorded | IntentState::Sent, IntentEvent::UnknownSubmit { .. }) => {
                IntentState::Unknown { next_check_unix: 0 }
            }
            (
                _,
                IntentEvent::ReconcileRetry {
                    next_check_unix, ..
                },
            ) if unknown => IntentState::Unknown {
                next_check_unix: *next_check_unix,
            },
            (_, IntentEvent::Reconciled { job_id, .. }) if unknown => IntentState::Reconciled {
                job_id: job_id.clone(),
            },
            (_, IntentEvent::UnresolvedStop { reason, .. }) if unknown => {
                IntentState::Unresolved { reason: *reason }
            }
            (s, e) => {
                let name = serde_json::to_value(e)
                    .ok()
                    .and_then(|v| v.get("state").and_then(|s| s.as_str()).map(String::from))
                    .unwrap_or_default();
                return Err(bad(&format!("{name} is not allowed after {s:?}")));
            }
        };
    }
    Ok(out)
}
