//! 実 cluster（ssh）の承認 gate（ADR 2026-10-25-recovery-scheduler-backend 付記 I15 (1)）。
//!
//! - [`load`] は人の承認記録（`celeris.recovery-smoke-approval/1`、`deny_unknown_fields`）を読み、spec の
//!   cluster・queue・account・資源・measurement_id と完全一致、`1 ≤ max_jobs ≤ spec.limits.max_jobs`、起動時の UTC が
//!   `valid_until` より前であることを確かめる。外れれば Err（呼び出し側は ssh・qsub を起こさない）。
//! - [`pin`] は承認記録を測定 dir の [`APPROVAL_PIN`] に hard link で 1 回だけ作り、2 回目以降は sha256 の一致を求める。
//! - [`ApprovalGate`] は qsub の直前に期限と件数（intent 台帳で `sent` 以降に進んだ intent の数）を確かめ、外れれば
//!   qsub を起こさず `Unsupported` の原票を返す。qstat・marker 検索は素通し（期限切れの後も観測・回収は続けてよい）。
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::{
    backend::RecoverySpec,
    intent::{INTENT_LEDGER, IntentEvent, IntentRow},
    transport::{CallOutcome, MarkerQuery, PbsCall, PbsReply, PbsTransport, QsubRequest},
};
use crate::observe::Origin;

pub const APPROVAL_SCHEMA: &str = "celeris.recovery-smoke-approval/1";
/// 測定 dir の承認 pin。
pub const APPROVAL_PIN: &str = "recovery-approval.json";

/// 人の承認記録。欄の欠落・未知の欄は読み込みエラー。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    pub schema: String,
    /// 人の決定の参照（解釈しない）。
    pub source: String,
    pub cluster: String,
    pub queue: String,
    pub account: String,
    pub select: String,
    pub place: String,
    pub walltime_secs: u64,
    pub max_jobs: u32,
    /// UTC RFC 3339（`Z`）。
    pub valid_until: String,
    pub measurement_id: String,
}

/// 検査を通った承認。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckedApproval {
    pub approval: Approval,
    pub bytes: Vec<u8>,
    pub sha256: String,
    pub valid_until_unix: i64,
    /// spec と承認の `max_jobs` の小さい方。
    pub max_jobs: u32,
}

fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

/// 承認記録を読み、spec と `now` で検査する。
pub fn load(
    path: &Path,
    spec: &RecoverySpec,
    now: OffsetDateTime,
) -> Result<CheckedApproval, String> {
    if !path.is_absolute() {
        return Err(format!(
            "DAGBENCH_PBS_APPROVAL {} must be an absolute path",
            path.display()
        ));
    }
    let bytes = fs::read(path).map_err(|e| format!("approval {}: {e}", path.display()))?;
    let a: Approval =
        serde_json::from_slice(&bytes).map_err(|e| format!("approval {}: {e}", path.display()))?;
    let fail = |m: String| Err(format!("approval {}: {m}", path.display()));
    if a.schema != APPROVAL_SCHEMA {
        return fail(format!("schema {:?} is not {APPROVAL_SCHEMA}", a.schema));
    }
    if a.source.trim().is_empty() {
        return fail("source is empty".into());
    }
    let r = &spec.resources;
    for (k, got, want) in [
        ("cluster", a.cluster.as_str(), spec.cluster.as_str()),
        ("queue", a.queue.as_str(), spec.queue.as_str()),
        ("account", a.account.as_str(), spec.account.as_str()),
        ("select", a.select.as_str(), r.select.as_str()),
        ("place", a.place.as_str(), r.place.as_str()),
        (
            "measurement_id",
            a.measurement_id.as_str(),
            spec.measurement_id.as_str(),
        ),
    ] {
        if got != want {
            return fail(format!("{k} {got:?} differs from the spec {want:?}"));
        }
    }
    if a.walltime_secs != r.walltime_secs {
        return fail(format!(
            "walltime_secs {} differs from the spec {}",
            a.walltime_secs, r.walltime_secs
        ));
    }
    if a.max_jobs == 0 || a.max_jobs > spec.limits.max_jobs {
        return fail(format!(
            "max_jobs {} is outside 1..={} (spec limits.max_jobs)",
            a.max_jobs, spec.limits.max_jobs
        ));
    }
    if !a.valid_until.ends_with('Z') {
        return fail(format!("valid_until {:?} is not UTC (Z)", a.valid_until));
    }
    let until = OffsetDateTime::parse(&a.valid_until, &Rfc3339)
        .map_err(|e| format!("approval {}: valid_until: {e}", path.display()))?;
    if now >= until {
        return fail(format!("expired at {} (now {now})", a.valid_until));
    }
    Ok(CheckedApproval {
        max_jobs: a.max_jobs.min(spec.limits.max_jobs),
        valid_until_unix: until.unix_timestamp(),
        sha256: sha256_hex(&bytes),
        bytes,
        approval: a,
    })
}

/// 承認を測定 dir に pin する（無ければ hard link で 1 回だけ作る。あれば sha256 の一致を求める）。
pub fn pin(dir: &Path, a: &CheckedApproval) -> Result<(), String> {
    let path = dir.join(APPROVAL_PIN);
    if !path.exists() {
        let tmp = dir.join(format!(".{APPROVAL_PIN}.{}.tmp", std::process::id()));
        File::create(&tmp)
            .and_then(|mut f| {
                f.write_all(&a.bytes)?;
                f.sync_all()
            })
            .map_err(|e| format!("{}: {e}", tmp.display()))?;
        let linked = fs::hard_link(&tmp, &path);
        let _ = fs::remove_file(&tmp);
        match linked {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    check_pinned(dir, a)
}

/// 測定 dir の pin が `a` と同じ承認であること（子 process も自分でやり直す）。
pub fn check_pinned(dir: &Path, a: &CheckedApproval) -> Result<(), String> {
    let path = dir.join(APPROVAL_PIN);
    let pinned = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let got = sha256_hex(&pinned);
    if got != a.sha256 {
        return Err(format!(
            "approval sha256 {} differs from the pinned {got} ({APPROVAL_PIN}; the approval cannot change within a measurement)",
            a.sha256
        ));
    }
    Ok(())
}

/// 測定 dir の intent 台帳で `sent` 以降に進んだ intent（`except` を除く）の数。lock は取らない
/// （qsub は台帳の lock の中で呼ばれる）。
pub fn sent_intents(dir: &Path, except: &str) -> Result<usize, String> {
    let path = dir.join(INTENT_LEDGER);
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let mut ids = BTreeSet::new();
    for l in text.lines().filter(|l| !l.trim().is_empty()) {
        // 末尾の壊れ行（書きかけ）は数えない。
        let Ok(row) = serde_json::from_str::<IntentRow>(l) else {
            continue;
        };
        if !matches!(row.event, IntentEvent::IntentRecorded { .. })
            && row.submit_intent_id != except
        {
            ids.insert(row.submit_intent_id);
        }
    }
    Ok(ids.len())
}

/// qsub の直前に期限と件数を確かめる transport（qsub 以外は素通し）。
pub struct ApprovalGate<T> {
    inner: T,
    dir: PathBuf,
    valid_until_unix: i64,
    max_jobs: u32,
}

impl<T: PbsTransport> ApprovalGate<T> {
    pub fn new(inner: T, dir: &Path, a: &CheckedApproval) -> Self {
        Self {
            inner,
            dir: dir.to_path_buf(),
            valid_until_unix: a.valid_until_unix,
            max_jobs: a.max_jobs,
        }
    }

    fn refusal(&self, req: &QsubRequest) -> Option<String> {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        if now >= self.valid_until_unix {
            return Some(format!(
                "approval expired (valid_until unix {})",
                self.valid_until_unix
            ));
        }
        match sent_intents(&self.dir, &req.submit_intent) {
            Ok(n) if n < self.max_jobs as usize => None,
            Ok(n) => Some(format!(
                "approval max_jobs {} reached ({n} intents already sent)",
                self.max_jobs
            )),
            Err(e) => Some(format!("intent ledger unreadable: {e}")),
        }
    }
}

impl<T: PbsTransport> PbsTransport for ApprovalGate<T> {
    fn origin(&self) -> Origin {
        self.inner.origin()
    }

    fn qsub(&self, req: &QsubRequest) -> PbsReply {
        let Some(why) = self.refusal(req) else {
            return self.inner.qsub(req);
        };
        let t = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_default();
        PbsReply {
            call: PbsCall::Qsub,
            argv: req.argv().unwrap_or_else(|_| vec!["qsub".into()]),
            started_utc: t.clone(),
            finished_utc: t,
            exit: None,
            stdout: String::new(),
            stderr: format!("not started: approval gate: {why}"),
            outcome: CallOutcome::Unsupported,
        }
    }

    fn qstat_full(&self, job_id: &str) -> PbsReply {
        self.inner.qstat_full(job_id)
    }

    fn find_by_marker(&self, m: &MarkerQuery) -> PbsReply {
        self.inner.find_by_marker(m)
    }
}
