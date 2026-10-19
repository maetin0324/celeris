//! 多軸評価の生記録（human-ops JSONL）。形式・identity・時計の規則は
//! `agent-docs/adr/2026-10-19-eval-raw-human-ops-record.md` と `docs/ops/eval-raw-record-definitions.md`。
//!
//! - 未知の時刻・時間は `None`（JSON null）で、理由を `missing` に書く。0 に変換しない。
//! - `record_id` は材料欄の正規化 JSON の sha256。同じ事実の再送は行を増やさない。
//! - 時計は注入される（`recorded_at` は呼び出し側が渡す）。本体は host の時計を読まない。
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{
    ledger::TrialKey,
    manifest::{ConfigId, canonical},
};

pub const HUMAN_OP_SCHEMA: &str = "celeris.eval-human-op/1";
pub const HUMAN_OPS_FILE: &str = "human-ops.jsonl";

/// 欄が JSON に無いことを形式エラーにする（`Option` でも省略を許さない）。
fn req<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d)
}

macro_rules! str_enum {
    ($(#[$m:meta])* $name:ident { $($var:ident = $s:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
        pub enum $name { $(#[serde(rename = $s)] $var),+ }
        impl $name {
            pub fn as_str(&self) -> &'static str { match self { $(Self::$var => $s),+ } }
        }
    };
}

str_enum!(RecordType {
    OpStart = "op_start", OpEnd = "op_end", WaitStart = "wait_start", WaitEnd = "wait_end",
    JobObservation = "job_observation", Attempt = "attempt", Cutoff = "cutoff", Correction = "correction",
});
str_enum!(ExperimentSet { MainAb = "main-ab", JevBc = "jev-bc" });
str_enum!(ClockSource { SystemUtc = "system_utc", SystemUtcNtpSynced = "system_utc_ntp_synced", Unknown = "unknown" });
str_enum!(TimeBasis {
    EvaluatorLive = "evaluator_live", EvaluatorReported = "evaluator_reported",
    HarnessTranscript = "harness_transcript", SchedulerAccounting = "scheduler_accounting",
    CelerisEvent = "celeris_event", Unknown = "unknown",
});
str_enum!(TimePrecision { Second = "second", Minute = "minute", Approximate = "approximate" });
str_enum!(MissingReason {
    Unknown = "unknown", NotRecorded = "not_recorded", NotApplicable = "not_applicable",
    OpenAtCutoff = "open_at_cutoff",
});
str_enum!(OpKind {
    ManualTranscription = "manual_transcription", StatusCheck = "status_check", Resubmit = "resubmit",
    Resume = "resume", ResearchDirection = "research_direction", SafetyApproval = "safety_approval",
    SystemAdjustment = "system_adjustment", SystemRecovery = "system_recovery",
    ManualHandoff = "manual_handoff", ProtocolStep = "protocol_step", Unclassified = "unclassified",
});
str_enum!(Mode { Manual = "manual", Automatic = "automatic" });
str_enum!(Intent {
    Research = "research", Safety = "safety", System = "system", Protocol = "protocol", Monitoring = "monitoring",
});
str_enum!(Trigger {
    SystemRequest = "system_request", Failure = "failure", Scheduled = "scheduled", SelfInitiated = "self_initiated",
});
str_enum!(Outcome { Ok = "ok", Failed = "failed", Abandoned = "abandoned" });
str_enum!(AttemptResult {
    Ok = "ok", Failed = "failed", Cancelled = "cancelled", Timeout = "timeout", Abandoned = "abandoned",
});
str_enum!(ActorKind { Human = "human", Script = "script" });
str_enum!(TargetKind {
    Artifact = "artifact", Job = "job", Task = "task", Run = "run", Session = "session", Config = "config",
});

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Actor {
    pub kind: ActorKind,
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub kind: TargetKind,
    #[serde(rename = "ref")]
    pub target_ref: String,
    #[serde(deserialize_with = "req")]
    pub sha256: Option<String>,
}

/// 1 行 1 record。型ごとに使わない欄は null / 空（検証で強制する）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanOpRecord {
    pub schema: String,
    pub record_id: String,
    pub record_type: RecordType,
    pub experiment_set: ExperimentSet,
    pub campaign_id: String,
    pub manifest_sha256: String,
    #[serde(deserialize_with = "req")]
    pub case_id: Option<String>,
    #[serde(deserialize_with = "req")]
    pub arm: Option<ConfigId>,
    #[serde(deserialize_with = "req")]
    pub repetition: Option<u32>,
    #[serde(deserialize_with = "req")]
    pub trial_no: Option<u32>,
    #[serde(deserialize_with = "req")]
    pub trial_key: Option<String>,
    #[serde(deserialize_with = "req")]
    pub task_id: Option<String>,
    #[serde(deserialize_with = "req")]
    pub run_id: Option<String>,
    #[serde(deserialize_with = "req")]
    pub job_ref: Option<String>,
    #[serde(deserialize_with = "req")]
    pub logical_op_id: Option<String>,
    #[serde(deserialize_with = "req")]
    pub attempt: Option<u32>,
    // 時計
    pub recorded_at: String,
    pub clock_source: ClockSource,
    pub host: String,
    // 事実時刻（分からなければ null＋missing）
    #[serde(deserialize_with = "req")]
    pub start: Option<String>,
    #[serde(deserialize_with = "req")]
    pub end: Option<String>,
    #[serde(deserialize_with = "req")]
    pub occurred_at: Option<String>,
    #[serde(deserialize_with = "req")]
    pub job_submit: Option<String>,
    #[serde(deserialize_with = "req")]
    pub job_start: Option<String>,
    #[serde(deserialize_with = "req")]
    pub job_end: Option<String>,
    #[serde(deserialize_with = "req")]
    pub time_basis: Option<TimeBasis>,
    #[serde(deserialize_with = "req")]
    pub time_precision: Option<TimePrecision>,
    pub missing: BTreeMap<String, MissingReason>,
    // 操作
    #[serde(deserialize_with = "req")]
    pub op_kind: Option<OpKind>,
    #[serde(deserialize_with = "req")]
    pub mode: Option<Mode>,
    #[serde(deserialize_with = "req")]
    pub intent: Option<Intent>,
    #[serde(deserialize_with = "req")]
    pub actor: Option<Actor>,
    #[serde(deserialize_with = "req")]
    pub reason: Option<String>,
    #[serde(deserialize_with = "req")]
    pub trigger: Option<Trigger>,
    pub targets: Vec<Target>,
    #[serde(deserialize_with = "req")]
    pub celeris_event_ref: Option<String>,
    #[serde(deserialize_with = "req")]
    pub outcome: Option<Outcome>,
    #[serde(deserialize_with = "req")]
    pub wait_kind: Option<String>,
    #[serde(deserialize_with = "req")]
    pub result: Option<AttemptResult>,
    // 訂正
    #[serde(deserialize_with = "req")]
    pub corrects: Option<String>,
    #[serde(deserialize_with = "req")]
    pub set: Option<Map<String, Value>>,
    #[serde(deserialize_with = "req")]
    pub note: Option<String>,
}

/// 区間。端が None なら長さも None（0 にしない）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimingSpan {
    pub start: Option<String>,
    pub end: Option<String>,
}

impl TimingSpan {
    /// 両端が分かるときだけ秒数。解釈できない・end<start も None。
    pub fn duration_secs(&self) -> Option<u64> {
        let s = parse_utc(self.start.as_deref()?).ok()?;
        let e = parse_utc(self.end.as_deref()?).ok()?;
        u64::try_from((e - s).whole_seconds()).ok()
    }
}

/// `attempt` 行の読み取り用 view。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttemptRecord {
    pub logical_op_id: String,
    pub attempt: u32,
    pub span: TimingSpan,
    pub result: Option<AttemptResult>,
}

/// 所要時間の集計。欠測が 1 件でもあれば合計は出さず `observed_sum` と欠測数を並べる。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurationStat {
    pub observed_sum: Option<u64>,
    pub observed: usize,
    pub missing: usize,
}

impl DurationStat {
    pub fn from_spans(spans: &[TimingSpan]) -> Self {
        let mut observed_sum = None;
        let (mut observed, mut missing) = (0, 0);
        for d in spans.iter().map(TimingSpan::duration_secs) {
            match d {
                Some(v) => {
                    observed += 1;
                    observed_sum = Some(observed_sum.unwrap_or(0) + v);
                }
                None => missing += 1,
            }
        }
        Self {
            observed_sum,
            observed,
            missing,
        }
    }

    /// 欠測が無く観測が 1 件以上あるときだけ合計。
    pub fn total(&self) -> Option<u64> {
        if self.missing == 0 {
            self.observed_sum
        } else {
            None
        }
    }
}

fn parse_utc(s: &str) -> Result<OffsetDateTime, String> {
    if !s.ends_with('Z') {
        return Err(format!("{s:?}: UTC（Z 終わり）の RFC 3339 でない"));
    }
    OffsetDateTime::parse(s, &Rfc3339).map_err(|e| format!("{s:?}: {e}"))
}

/// 材料に入らず、同一性の比較（本体の同一）からも外す欄。
const BODY_EXCLUDED: [&str; 3] = ["recorded_at", "clock_source", "host"];

impl HumanOpRecord {
    /// 材料欄から決定的に作る id。
    pub fn compute_record_id(&self) -> String {
        let v = json!({
            "schema": self.schema, "record_type": self.record_type, "experiment_set": self.experiment_set,
            "campaign_id": self.campaign_id, "manifest_sha256": self.manifest_sha256,
            "case_id": self.case_id, "arm": self.arm, "repetition": self.repetition,
            "trial_no": self.trial_no, "logical_op_id": self.logical_op_id, "attempt": self.attempt,
            "op_kind": self.op_kind, "wait_kind": self.wait_kind, "job_ref": self.job_ref,
            "corrects": self.corrects,
        });
        let text = canonical(&v).to_string();
        let hex: String = Sha256::digest(text)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        format!("hop-{hex}")
    }

    /// `record_id` を材料から埋める。
    pub fn with_record_id(mut self) -> Self {
        self.record_id = self.compute_record_id();
        self
    }

    /// 時計と本体の同一性（recorded_at・clock_source・host を除く）を比べる。
    pub fn same_body(&self, other: &Self) -> bool {
        let strip = |r: &Self| -> Option<Value> {
            let mut v = serde_json::to_value(r).ok()?;
            let m = v.as_object_mut()?;
            for k in BODY_EXCLUDED {
                m.remove(k);
            }
            Some(v)
        };
        matches!((strip(self), strip(other)), (Some(a), Some(b)) if a == b)
    }

    pub fn timing_span(&self) -> TimingSpan {
        TimingSpan {
            start: self.start.clone(),
            end: self.end.clone(),
        }
    }

    /// `attempt` 行の view。
    pub fn as_attempt(&self) -> Option<AttemptRecord> {
        if self.record_type != RecordType::Attempt {
            return None;
        }
        Some(AttemptRecord {
            logical_op_id: self.logical_op_id.clone()?,
            attempt: self.attempt?,
            span: self.timing_span(),
            result: self.result,
        })
    }

    /// 表示名（A0/B0/B1/C1）。
    pub fn arm_label(&self) -> Option<&'static str> {
        match (self.experiment_set, self.arm?) {
            (ExperimentSet::MainAb, ConfigId::A) => Some("A0"),
            (ExperimentSet::MainAb, ConfigId::B) => Some("B0"),
            (ExperimentSet::JevBc, ConfigId::B) => Some("B1"),
            (ExperimentSet::JevBc, ConfigId::C) => Some("C1"),
            _ => None,
        }
    }

    /// 形式の検証。最初に見つけた違反を返す。
    pub fn validate(&self) -> Result<(), String> {
        use RecordType as T;
        if self.schema != HUMAN_OP_SCHEMA {
            return Err(format!(
                "schema は {HUMAN_OP_SCHEMA:?}（{:?}）",
                self.schema
            ));
        }
        if self.record_id != self.compute_record_id() {
            return Err("record_id が材料欄から計算した値と違う".into());
        }
        nonempty("campaign_id", &self.campaign_id)?;
        nonempty("host", &self.host)?;
        if self.manifest_sha256.len() != 64
            || !self.manifest_sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("manifest_sha256 は 64 桁の hex".into());
        }
        parse_utc(&self.recorded_at).map_err(|e| format!("recorded_at: {e}"))?;
        self.validate_arm_trial()?;

        // 型ごとの時刻欄と、その型で使う欄。
        let (time_fields, ids): (&[&str], bool) = match self.record_type {
            T::OpStart | T::WaitStart => (&["start"], true),
            T::OpEnd | T::WaitEnd => (&["end"], true),
            T::Attempt => (&["start", "end"], true),
            T::JobObservation => (&["job_submit", "job_start", "job_end"], false),
            T::Cutoff => (&["occurred_at"], false),
            T::Correction => (&[], false),
        };
        let all_times = [
            "start",
            "end",
            "occurred_at",
            "job_submit",
            "job_start",
            "job_end",
        ];
        let mut any_time = false;
        for f in all_times {
            let val = self.time_field(f);
            if time_fields.contains(&f) {
                match val {
                    Some(s) => {
                        parse_utc(s).map_err(|e| format!("{f}: {e}"))?;
                        any_time = true;
                        if self.missing.contains_key(f) {
                            return Err(format!("{f} に値があるのに missing.{f} がある"));
                        }
                    }
                    None => {
                        let r = self.missing.get(f).ok_or_else(|| {
                            format!("{f} が null なのに missing.{f} が無い（未知の時間は理由付き）")
                        })?;
                        if *r == MissingReason::OpenAtCutoff
                            && !matches!(f, "end" | "job_start" | "job_end")
                        {
                            return Err(format!("missing.{f}: open_at_cutoff は終端側の欄だけ"));
                        }
                    }
                }
            } else if val.is_some() || self.missing.contains_key(f) {
                return Err(format!("{f} は {} では使えない", self.record_type.as_str()));
            }
        }
        for k in self.missing.keys() {
            let known = all_times.contains(&k.as_str())
                || ["task_id", "run_id", "job_ref", "case_id"].contains(&k.as_str());
            if !known {
                return Err(format!("missing.{k}: 未知の欄名"));
            }
            if !all_times.contains(&k.as_str()) && self.identity_field(k).is_some() {
                return Err(format!("{k} に値があるのに missing.{k} がある"));
            }
        }
        if any_time != (self.time_basis.is_some() && self.time_precision.is_some()) {
            return Err("事実時刻がある行だけ time_basis と time_precision を持つ".into());
        }
        if self.time_basis.is_some() != self.time_precision.is_some() {
            return Err("time_basis と time_precision は両方か無しか".into());
        }
        // 順序: end >= start、job_submit <= job_start <= job_end
        let ordered = |a: &str, b: &str| -> Result<(), String> {
            if let (Some(x), Some(y)) = (self.time_field(a), self.time_field(b)) {
                let (x, y) = (parse_utc(x)?, parse_utc(y)?);
                if y < x {
                    return Err(format!("{b} が {a} より前"));
                }
            }
            Ok(())
        };
        ordered("start", "end")?;
        ordered("job_submit", "job_start")?;
        ordered("job_start", "job_end")?;
        ordered("job_submit", "job_end")?;

        // logical_op / attempt
        match (ids, &self.logical_op_id, self.attempt) {
            (true, Some(l), Some(a)) => {
                nonempty("logical_op_id", l)?;
                if a == 0 {
                    return Err("attempt は 1 始まり".into());
                }
            }
            (true, _, _) => return Err("logical_op_id と attempt が必須".into()),
            (false, None, None) => {}
            (false, _, _) => {
                return Err(format!(
                    "logical_op_id/attempt は {} では使えない",
                    self.record_type.as_str()
                ));
            }
        }
        // 型固有の欄
        let is_op = matches!(self.record_type, T::OpStart | T::OpEnd);
        let is_wait = matches!(self.record_type, T::WaitStart | T::WaitEnd);
        let op_set = self.op_kind.is_some()
            || self.mode.is_some()
            || self.intent.is_some()
            || self.actor.is_some()
            || self.trigger.is_some()
            || self.reason.is_some()
            || !self.targets.is_empty()
            || self.celeris_event_ref.is_some()
            || self.outcome.is_some();
        if is_op {
            let (Some(kind), Some(_), Some(_), Some(actor), Some(_)) = (
                self.op_kind,
                self.mode,
                self.intent,
                &self.actor,
                self.trigger,
            ) else {
                return Err("op には op_kind・mode・intent・actor・trigger が必須".into());
            };
            nonempty("actor.id", &actor.id)?;
            if actor.id.contains('@') {
                return Err("actor.id にメールアドレスを書かない（仮名か script 名）".into());
            }
            let reason_needed = matches!(
                kind,
                OpKind::Resubmit
                    | OpKind::Resume
                    | OpKind::SystemAdjustment
                    | OpKind::SystemRecovery
                    | OpKind::ManualHandoff
                    | OpKind::Unclassified
            );
            match &self.reason {
                Some(r) => nonempty("reason", r)?,
                None if reason_needed => {
                    return Err(format!("{} には reason が必須", kind.as_str()));
                }
                None => {}
            }
            if (self.record_type == T::OpEnd) != self.outcome.is_some() {
                return Err("outcome は op_end だけが持ち、op_end では必須".into());
            }
            for t in &self.targets {
                nonempty("targets.ref", &t.target_ref)?;
            }
        } else if op_set {
            return Err(format!(
                "op 欄は {} では使えない",
                self.record_type.as_str()
            ));
        }
        match (&self.wait_kind, is_wait) {
            (Some(w), true) => nonempty("wait_kind", w)?,
            (None, true) => return Err("wait には wait_kind が必須".into()),
            (Some(_), false) => {
                return Err(format!(
                    "wait_kind は {} では使えない",
                    self.record_type.as_str()
                ));
            }
            (None, false) => {}
        }
        match (self.record_type, &self.job_ref) {
            (T::JobObservation, Some(j)) => {
                if j.splitn(3, ':').filter(|p| !p.is_empty()).count() != 3 {
                    return Err("job_ref は <scheduler>:<cluster>:<job_id>".into());
                }
            }
            (T::JobObservation, None) => return Err("job_observation には job_ref が必須".into()),
            _ => {}
        }
        if (self.record_type == T::Attempt) != self.result.is_some() {
            return Err("result は attempt だけが持ち、attempt では必須".into());
        }
        if self.record_type == T::Correction {
            let c = self
                .corrects
                .as_deref()
                .ok_or("correction には corrects が必須")?;
            if !is_record_id(c) {
                return Err("corrects は hop-<sha256 hex>".into());
            }
            let set = self.set.as_ref().ok_or("correction には set が必須")?;
            if set.is_empty() {
                return Err("set が空".into());
            }
            for k in set.keys() {
                if !CORRECTABLE.contains(&k.as_str()) {
                    return Err(format!(
                        "set.{k}: 訂正できない欄（identity・材料欄は変えない）"
                    ));
                }
            }
        } else if self.corrects.is_some() || self.set.is_some() {
            return Err("corrects/set は correction だけ".into());
        }
        Ok(())
    }

    fn validate_arm_trial(&self) -> Result<(), String> {
        let any = self.case_id.is_some()
            || self.arm.is_some()
            || self.repetition.is_some()
            || self.trial_no.is_some()
            || self.trial_key.is_some();
        if !any {
            return Ok(());
        }
        let (Some(case), Some(arm), Some(rep), Some(no), Some(key)) = (
            &self.case_id,
            self.arm,
            self.repetition,
            self.trial_no,
            &self.trial_key,
        ) else {
            return Err(
                "case_id・arm・repetition・trial_no・trial_key は揃って書くか全て null".into(),
            );
        };
        nonempty("case_id", case)?;
        let ok = match self.experiment_set {
            ExperimentSet::MainAb => matches!(arm, ConfigId::A | ConfigId::B),
            ExperimentSet::JevBc => matches!(arm, ConfigId::B | ConfigId::C),
        };
        if !ok {
            return Err(format!(
                "{} に arm {arm:?} は無い",
                self.experiment_set.as_str()
            ));
        }
        if rep == 0 {
            return Err("repetition は 1 始まり".into());
        }
        let expect = TrialKey {
            manifest_sha256: self.manifest_sha256.clone(),
            problem_id: case.clone(),
            arm,
            trial_no: no,
            rep,
        }
        .idempotency_key();
        if *key != expect {
            return Err(format!("trial_key が identity 欄と矛盾（期待 {expect}）"));
        }
        Ok(())
    }

    fn time_field(&self, f: &str) -> Option<&str> {
        match f {
            "start" => self.start.as_deref(),
            "end" => self.end.as_deref(),
            "occurred_at" => self.occurred_at.as_deref(),
            "job_submit" => self.job_submit.as_deref(),
            "job_start" => self.job_start.as_deref(),
            "job_end" => self.job_end.as_deref(),
            _ => None,
        }
    }

    fn identity_field(&self, f: &str) -> Option<&str> {
        match f {
            "task_id" => self.task_id.as_deref(),
            "run_id" => self.run_id.as_deref(),
            "job_ref" => self.job_ref.as_deref(),
            "case_id" => self.case_id.as_deref(),
            _ => None,
        }
    }
}

/// correction で置き換えられる欄。
const CORRECTABLE: [&str; 18] = [
    "start",
    "end",
    "occurred_at",
    "job_submit",
    "job_start",
    "job_end",
    "time_basis",
    "time_precision",
    "missing",
    "mode",
    "intent",
    "reason",
    "trigger",
    "outcome",
    "result",
    "targets",
    "celeris_event_ref",
    "note",
];

fn nonempty(name: &str, v: &str) -> Result<(), String> {
    if v.trim().is_empty() {
        Err(format!("{name} が空"))
    } else {
        Ok(())
    }
}

fn is_record_id(s: &str) -> bool {
    s.strip_prefix("hop-")
        .is_some_and(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[derive(Debug, PartialEq, Eq)]
pub enum AppendOutcome {
    Appended,
    /// 同じ record_id・同じ本体が既にある。行は増えない。
    Duplicate,
}

#[derive(Debug, PartialEq, Eq)]
pub enum HumanOpError {
    Invalid(String),
    /// 同じ record_id で本体が食い違う。上書きしない（直すなら correction）。
    Conflict {
        record_id: String,
    },
    /// 行番号（1 始まり）付きの読み込みエラー。
    Line {
        line: usize,
        msg: String,
    },
    Io(String),
}

impl std::fmt::Display for HumanOpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(m) => write!(f, "invalid human-op record: {m}"),
            Self::Conflict { record_id } => write!(
                f,
                "{record_id}: 同じ record_id で内容が食い違う（上書きしない。訂正は correction 行で）"
            ),
            Self::Line { line, msg } => write!(f, "line {line}: {msg}"),
            Self::Io(m) => write!(f, "io: {m}"),
        }
    }
}

impl std::error::Error for HumanOpError {}

/// 読み込み: 壊れた行（JSON・形式・検証違反）は行番号付きで拒否する。重複は潰さず file の順で返す。
pub fn read_records(path: &Path) -> Result<Vec<HumanOpRecord>, HumanOpError> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(HumanOpError::Io(format!("{}: {e}", path.display()))),
    };
    let mut out: Vec<HumanOpRecord> = Vec::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for (i, l) in text.lines().enumerate() {
        let line = i + 1;
        if l.trim().is_empty() {
            continue;
        }
        let err = |msg: String| HumanOpError::Line { line, msg };
        let r: HumanOpRecord = serde_json::from_str(l).map_err(|e| err(e.to_string()))?;
        r.validate().map_err(err)?;
        if let Some(&j) = seen.get(&r.record_id) {
            if !out[j].same_body(&r) {
                return Err(err(format!("{}: 先の行と本体が食い違う", r.record_id)));
            }
            continue;
        }
        seen.insert(r.record_id.clone(), out.len());
        out.push(r);
    }
    Ok(out)
}

/// correction を適用した、record_id で一意の読み出し。correction 行自体は含めない。
pub fn resolve(records: &[HumanOpRecord]) -> Result<Vec<HumanOpRecord>, HumanOpError> {
    let mut by_id: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, r) in records.iter().enumerate() {
        by_id.entry(r.record_id.as_str()).or_insert(i);
    }
    // correction の訂正は、最後の行が勝つ: 対象ごとの有効な set を行順に重ねる。
    let mut eff: Vec<Option<Map<String, Value>>> = records.iter().map(|r| r.set.clone()).collect();
    for (i, r) in records
        .iter()
        .enumerate()
        .filter(|(_, r)| r.record_type == RecordType::Correction)
    {
        let target = r.corrects.as_deref().and_then(|c| by_id.get(c)).copied();
        let Some(t) = target else {
            return Err(HumanOpError::Invalid(format!(
                "{}: corrects の対象が無い",
                r.record_id
            )));
        };
        let tr = &records[t];
        if tr.experiment_set != r.experiment_set
            || tr.campaign_id != r.campaign_id
            || tr.manifest_sha256 != r.manifest_sha256
        {
            return Err(HumanOpError::Invalid(format!(
                "{}: 訂正が対象と別の campaign",
                r.record_id
            )));
        }
        if tr.record_type == RecordType::Correction {
            // 訂正の訂正: 対象の set を、この行の set で欄ごとに置き換える。
            let add = eff[i].clone();
            if let (Some(base), Some(add)) = (eff[t].as_mut(), add) {
                for (k, v) in add {
                    base.insert(k, v);
                }
            }
        }
    }
    let mut out = Vec::new();
    for r in records {
        if r.record_type == RecordType::Correction {
            continue;
        }
        let mut v = serde_json::to_value(r).map_err(|e| HumanOpError::Invalid(e.to_string()))?;
        let mut touched = false;
        for (ci, c) in records.iter().enumerate() {
            if c.record_type == RecordType::Correction
                && c.corrects.as_deref() == Some(r.record_id.as_str())
                && let (Some(set), Some(m)) = (&eff[ci], v.as_object_mut())
            {
                for (k, x) in set {
                    m.insert(k.clone(), x.clone());
                }
                touched = true;
            }
        }
        let rr: HumanOpRecord = if touched {
            serde_json::from_value(v)
                .map_err(|e| HumanOpError::Invalid(format!("{}: 訂正後 {e}", r.record_id)))?
        } else {
            r.clone()
        };
        rr.validate()
            .map_err(|e| HumanOpError::Invalid(format!("{}: 訂正後 {e}", r.record_id)))?;
        out.push(rr);
    }
    Ok(out)
}

/// 追記専用の JSONL。
pub struct HumanOpLog {
    path: PathBuf,
}

impl HumanOpLog {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// campaign の out ディレクトリ配下の既定位置。
    pub fn in_out_dir(out: &Path) -> Self {
        Self::new(out.join(HUMAN_OPS_FILE))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn read(&self) -> Result<Vec<HumanOpRecord>, HumanOpError> {
        read_records(&self.path)
    }

    /// 冪等な追記。検証 → 既存と照合（同一なら no-op、食い違いは拒否）→ 1 write + fsync。
    pub fn append(&self, rec: &HumanOpRecord) -> Result<AppendOutcome, HumanOpError> {
        rec.validate().map_err(HumanOpError::Invalid)?;
        let existing = self.read()?;
        if let Some(e) = existing.iter().find(|e| e.record_id == rec.record_id) {
            return if e.same_body(rec) {
                Ok(AppendOutcome::Duplicate)
            } else {
                Err(HumanOpError::Conflict {
                    record_id: rec.record_id.clone(),
                })
            };
        }
        if rec.record_type == RecordType::Correction
            && !rec
                .corrects
                .as_deref()
                .is_some_and(|c| existing.iter().any(|e| e.record_id == c))
        {
            return Err(HumanOpError::Invalid(
                "corrects の対象が file に無い".into(),
            ));
        }
        let mut line = serde_json::to_string(rec).map_err(|e| HumanOpError::Io(e.to_string()))?;
        line.push('\n');
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(dir)
                .map_err(|e| HumanOpError::Io(format!("{}: {e}", dir.display())))?;
        }
        // 末尾が改行で終わらない file には改行を足して、行が繋がらないようにする。
        let needs_nl = fs::read(&self.path)
            .map(|b| b.last().is_some_and(|c| *c != b'\n'))
            .unwrap_or(false);
        if needs_nl {
            line.insert(0, '\n');
        }
        let io = |e: std::io::Error| HumanOpError::Io(format!("{}: {e}", self.path.display()));
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(io)?;
        f.write_all(line.as_bytes()).map_err(io)?;
        f.sync_all().map_err(io)?;
        Ok(AppendOutcome::Appended)
    }
}

/// op_start/op_end を (campaign, trial_key, logical_op_id, attempt, op_kind) で対にした区間。
/// 片方しか無い op も捨てない（無い側は None）。
pub fn op_spans(records: &[HumanOpRecord]) -> Vec<(HumanOpRecord, TimingSpan)> {
    let mut m: BTreeMap<String, (HumanOpRecord, TimingSpan)> = BTreeMap::new();
    for r in records {
        let key = match r.record_type {
            RecordType::OpStart | RecordType::OpEnd => format!(
                "{}|{:?}|{:?}|{:?}|{:?}",
                r.campaign_id, r.trial_key, r.logical_op_id, r.attempt, r.op_kind
            ),
            _ => continue,
        };
        let e = m
            .entry(key)
            .or_insert_with(|| (r.clone(), TimingSpan::default()));
        match r.record_type {
            RecordType::OpStart => {
                e.1.start = r.start.clone();
                e.0 = r.clone();
            }
            _ => e.1.end = r.end.clone(),
        }
    }
    m.into_values().collect()
}

/// 同じ論理作業のやり直し回数（異なる attempt 数 − 1）。
pub fn redo_counts(records: &[HumanOpRecord]) -> BTreeMap<(Option<String>, String), u32> {
    let mut m: BTreeMap<(Option<String>, String), std::collections::BTreeSet<u32>> =
        BTreeMap::new();
    for r in records {
        if let (Some(l), Some(a)) = (&r.logical_op_id, r.attempt) {
            m.entry((r.trial_key.clone(), l.clone()))
                .or_default()
                .insert(a);
        }
    }
    m.into_iter()
        .map(|(k, v)| (k, v.len().saturating_sub(1) as u32))
        .collect()
}

// 評価者用 CLI（campaign human-op ...）。
pub mod cli {
    //! `celeris-campaign human-op <record|span|validate|list>`: 評価者用の生記録 CLI。A/B/C 同じ command 列で同形式の行を書く。
    //! 形式は `docs/ops/eval-raw-record-definitions.md`。時計は `--now` / `CELERIS_HUMAN_OPS_NOW`（試験用）で差し替えられる。
    use std::{collections::BTreeMap, path::Path};

    use serde_json::{Value, json};
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    use crate::{
        human_ops::*,
        ledger::TrialKey,
        manifest::{CampaignManifest, ConfigId},
    };

    const USAGE: &str = "usage: celeris-campaign human-op <record|span|validate|list> --file <jsonl> ...\n\
record --file F --campaign ID (--manifest-sha256 H | --manifest M) --case ID --arm A0|B0|B1|C1 --rep N --op-kind K --actor NAME --mode manual|automatic --intent I \
[--actor-kind human|script] [--trigger T] [--logical-op ID] [--attempt N] [--started-at T|now] [--ended-at T|now] [--outcome ok|failed|abandoned] [--reason S] [--target kind:ref]... \
[--task-id ID] [--run-id ID] [--celeris-event-ref R] [--trial-no N] [--missing-reason R] [--time-basis B] [--time-precision P] [--note S]\n\
span --kind wait|attempt|job|cutoff (same identity flags) [--wait-kind K] [--logical-op ID] [--attempt N] [--result R] [--job-ref S:C:ID] [--job-submit|--job-start|--job-end T] [--started-at T] [--ended-at T] [--occurred-at T]\n\
validate --file F\nlist --file F [--campaign ID] [--arm A0|B0|B1|C1] [--case ID]\n\
clock: [--now T] [--clock-source system_utc|system_utc_ntp_synced|unknown] [--host H] (env CELERIS_HUMAN_OPS_NOW / CELERIS_HUMAN_OPS_HOST)";

    fn opt(args: &[String], name: &str) -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    }

    fn opts(args: &[String], name: &str) -> Vec<String> {
        args.windows(2)
            .filter(|w| w[0] == name)
            .map(|w| w[1].clone())
            .collect()
    }

    fn req(args: &[String], name: &str) -> Result<String, String> {
        opt(args, name).ok_or_else(|| format!("{name} is required\n{USAGE}"))
    }

    /// 文字列を enum 名（serde の名前）で解く。
    fn parse_enum<T: serde::de::DeserializeOwned>(name: &str, v: &str) -> Result<T, String> {
        serde_json::from_value(Value::String(v.to_string()))
            .map_err(|_| format!("{name}: 不正な値 {v:?}"))
    }

    fn parse_num(name: &str, v: &str) -> Result<u32, String> {
        v.parse().map_err(|_| format!("{name}: 数値でない {v:?}"))
    }

    struct Clock {
        now: String,
        source: ClockSource,
        host: String,
    }

    fn clock(args: &[String]) -> Result<Clock, String> {
        let now = match opt(args, "--now").or_else(|| std::env::var("CELERIS_HUMAN_OPS_NOW").ok()) {
            Some(n) => n,
            None => OffsetDateTime::now_utc()
                .replace_nanosecond(0)
                .map_err(|e| e.to_string())?
                .format(&Rfc3339)
                .map_err(|e| e.to_string())?,
        };
        let source = match opt(args, "--clock-source") {
            Some(s) => parse_enum("--clock-source", &s)?,
            None => ClockSource::SystemUtc,
        };
        let host = opt(args, "--host")
            .or_else(|| std::env::var("CELERIS_HUMAN_OPS_HOST").ok())
            .or_else(|| std::env::var("HOSTNAME").ok())
            .or_else(|| {
                std::fs::read_to_string("/etc/hostname")
                    .ok()
                    .map(|s| s.trim().to_string())
            })
            .filter(|h| !h.trim().is_empty())
            .unwrap_or_else(|| "unknown".into());
        Ok(Clock { now, source, host })
    }

    /// 事実時刻の指定。`now` は時計の値（evaluator_live）。
    struct Fact {
        value: Option<String>,
        live: bool,
    }

    fn fact(args: &[String], name: &str, clock: &Clock) -> Fact {
        match opt(args, name) {
            Some(v) if v == "now" => Fact {
                value: Some(clock.now.clone()),
                live: true,
            },
            Some(v) => Fact {
                value: Some(v),
                live: false,
            },
            None => Fact {
                value: None,
                live: false,
            },
        }
    }

    struct Ctx {
        base: HumanOpRecord,
        clock: Clock,
        missing_reason: MissingReason,
        basis: Option<(TimeBasis, TimePrecision)>,
    }

    fn arm_of(label: &str) -> Result<(ExperimentSet, ConfigId), String> {
        Ok(match label {
            "A0" => (ExperimentSet::MainAb, ConfigId::A),
            "B0" => (ExperimentSet::MainAb, ConfigId::B),
            "B1" => (ExperimentSet::JevBc, ConfigId::B),
            "C1" => (ExperimentSet::JevBc, ConfigId::C),
            o => return Err(format!("--arm: A0|B0|B1|C1 のどれか（{o:?}）")),
        })
    }

    fn ctx(args: &[String]) -> Result<Ctx, String> {
        let clock = clock(args)?;
        let (set, arm) = arm_of(&req(args, "--arm")?)?;
        let campaign_id = req(args, "--campaign")?;
        let manifest_sha256 = match (opt(args, "--manifest-sha256"), opt(args, "--manifest")) {
            (Some(h), _) => h,
            (None, Some(p)) => CampaignManifest::load(Path::new(&p))?.sha256,
            _ => return Err(format!("--manifest-sha256 か --manifest が必要\n{USAGE}")),
        };
        let case = req(args, "--case")?;
        let rep = parse_num("--rep", &req(args, "--rep")?)?;
        let trial_no = match opt(args, "--trial-no") {
            Some(n) => parse_num("--trial-no", &n)?,
            None => 1,
        };
        let key = TrialKey {
            manifest_sha256: manifest_sha256.clone(),
            problem_id: case.clone(),
            arm,
            trial_no,
            rep,
        }
        .idempotency_key();
        let mut missing = BTreeMap::new();
        // A に Celeris の task / run は無い（定義書 §2.1）。
        let a_arm = arm == ConfigId::A;
        let task_id = opt(args, "--task-id");
        let run_id = opt(args, "--run-id");
        if a_arm {
            for (f, v) in [("task_id", &task_id), ("run_id", &run_id)] {
                if v.is_none() {
                    missing.insert(f.to_string(), MissingReason::NotApplicable);
                }
            }
        }
        let basis = match (opt(args, "--time-basis"), opt(args, "--time-precision")) {
            (None, None) => None,
            (b, p) => Some((
                b.map(|b| parse_enum("--time-basis", &b))
                    .transpose()?
                    .unwrap_or(TimeBasis::EvaluatorReported),
                p.map(|p| parse_enum("--time-precision", &p))
                    .transpose()?
                    .unwrap_or(TimePrecision::Second),
            )),
        };
        let missing_reason = match opt(args, "--missing-reason") {
            Some(r) => parse_enum("--missing-reason", &r)?,
            None => MissingReason::Unknown,
        };
        let base = HumanOpRecord {
            schema: HUMAN_OP_SCHEMA.into(),
            record_id: String::new(),
            record_type: RecordType::OpStart,
            experiment_set: set,
            campaign_id,
            manifest_sha256,
            case_id: Some(case),
            arm: Some(arm),
            repetition: Some(rep),
            trial_no: Some(trial_no),
            trial_key: Some(key),
            task_id,
            run_id,
            job_ref: None,
            logical_op_id: None,
            attempt: None,
            recorded_at: clock.now.clone(),
            clock_source: clock.source,
            host: clock.host.clone(),
            start: None,
            end: None,
            occurred_at: None,
            job_submit: None,
            job_start: None,
            job_end: None,
            time_basis: None,
            time_precision: None,
            missing,
            op_kind: None,
            mode: None,
            intent: None,
            actor: None,
            reason: None,
            trigger: None,
            targets: vec![],
            celeris_event_ref: opt(args, "--celeris-event-ref"),
            outcome: None,
            wait_kind: None,
            result: None,
            corrects: None,
            set: None,
            note: opt(args, "--note"),
        };
        Ok(Ctx {
            base,
            clock,
            missing_reason,
            basis,
        })
    }

    impl Ctx {
        /// 事実時刻を欄に入れ、null なら missing に理由を書き、time_basis/precision を決める。
        fn times(&self, r: &mut HumanOpRecord, facts: &[(&str, Fact)]) {
            let mut any = false;
            let mut live = true;
            for (name, f) in facts {
                let slot = match *name {
                    "start" => &mut r.start,
                    "end" => &mut r.end,
                    "occurred_at" => &mut r.occurred_at,
                    "job_submit" => &mut r.job_submit,
                    "job_start" => &mut r.job_start,
                    _ => &mut r.job_end,
                };
                match &f.value {
                    Some(v) => {
                        *slot = Some(v.clone());
                        any = true;
                        live &= f.live;
                    }
                    None => {
                        r.missing.insert((*name).to_string(), self.missing_reason);
                    }
                }
            }
            if any {
                let (b, p) = self.basis.unwrap_or(if live {
                    (TimeBasis::EvaluatorLive, TimePrecision::Second)
                } else {
                    (TimeBasis::EvaluatorReported, TimePrecision::Second)
                });
                r.time_basis = Some(b);
                r.time_precision = Some(p);
            }
        }
    }

    fn append_all(file: &Path, recs: Vec<HumanOpRecord>) -> Result<String, String> {
        let log = HumanOpLog::new(file);
        let mut out = Vec::new();
        for r in recs {
            let r = r.with_record_id();
            let st = log.append(&r).map_err(|e| e.to_string())?;
            out.push(json!({
            "record_id": r.record_id,
            "record_type": r.record_type.as_str(),
            "status": match st { AppendOutcome::Appended => "appended", AppendOutcome::Duplicate => "duplicate" },
        }));
        }
        Ok(serde_json::to_string_pretty(&out).unwrap_or_default())
    }

    fn logical_and_attempt(args: &[String], default_op: &str) -> Result<(String, u32), String> {
        let l = opt(args, "--logical-op").unwrap_or_else(|| default_op.to_string());
        let a = match opt(args, "--attempt") {
            Some(a) => parse_num("--attempt", &a)?,
            None => 1,
        };
        Ok((l, a))
    }

    fn record(args: &[String]) -> Result<String, String> {
        let file = req(args, "--file")?;
        let c = ctx(args)?;
        let kind: OpKind = parse_enum("--op-kind", &req(args, "--op-kind")?)?;
        let (logical, attempt) = logical_and_attempt(args, kind.as_str())?;
        let mut targets = Vec::new();
        for t in opts(args, "--target") {
            let (k, r) = t.split_once(':').ok_or("--target は <kind>:<ref>")?;
            targets.push(Target {
                kind: parse_enum("--target kind", k)?,
                target_ref: r.to_string(),
                sha256: None,
            });
        }
        let mut r = c.base.clone();
        r.logical_op_id = Some(logical);
        r.attempt = Some(attempt);
        r.op_kind = Some(kind);
        r.mode = Some(parse_enum("--mode", &req(args, "--mode")?)?);
        r.intent = Some(parse_enum("--intent", &req(args, "--intent")?)?);
        r.actor = Some(Actor {
            kind: match opt(args, "--actor-kind") {
                Some(k) => parse_enum("--actor-kind", &k)?,
                None => ActorKind::Human,
            },
            id: req(args, "--actor")?,
        });
        r.trigger = Some(match opt(args, "--trigger") {
            Some(t) => parse_enum("--trigger", &t)?,
            None => Trigger::SelfInitiated,
        });
        r.reason = opt(args, "--reason");
        r.targets = targets;
        let start = fact(args, "--started-at", &c.clock);
        let end = fact(args, "--ended-at", &c.clock);
        let outcome = opt(args, "--outcome")
            .map(|o| parse_enum::<Outcome>("--outcome", &o))
            .transpose()?;
        // 終了が分かる（時刻か outcome がある）ときだけ op_end を書く。分からない終了は書かず missing のまま。
        let close = end.value.is_some() || outcome.is_some();
        let mut s = r.clone();
        s.record_type = RecordType::OpStart;
        c.times(&mut s, &[("start", start)]);
        let mut recs = vec![s];
        if close {
            let mut e = r;
            e.record_type = RecordType::OpEnd;
            e.outcome = Some(outcome.unwrap_or(Outcome::Ok));
            c.times(&mut e, &[("end", end)]);
            recs.push(e);
        }
        append_all(Path::new(&file), recs)
    }

    fn span(args: &[String]) -> Result<String, String> {
        let file = req(args, "--file")?;
        let c = ctx(args)?;
        let kind = req(args, "--kind")?;
        let start = fact(args, "--started-at", &c.clock);
        let end = fact(args, "--ended-at", &c.clock);
        let mut r = c.base.clone();
        let recs = match kind.as_str() {
            "wait" => {
                let (l, a) = logical_and_attempt(args, "wait")?;
                r.logical_op_id = Some(l);
                r.attempt = Some(a);
                r.wait_kind = Some(req(args, "--wait-kind")?);
                let close = end.value.is_some();
                let mut s = r.clone();
                s.record_type = RecordType::WaitStart;
                c.times(&mut s, &[("start", start)]);
                let mut v = vec![s];
                if close {
                    r.record_type = RecordType::WaitEnd;
                    c.times(&mut r, &[("end", end)]);
                    v.push(r);
                }
                v
            }
            "attempt" => {
                let l = req(args, "--logical-op")?;
                let (l, a) = logical_and_attempt(args, &l)?;
                r.record_type = RecordType::Attempt;
                r.logical_op_id = Some(l);
                r.attempt = Some(a);
                r.result = Some(parse_enum("--result", &req(args, "--result")?)?);
                c.times(&mut r, &[("start", start), ("end", end)]);
                vec![r]
            }
            "job" => {
                r.record_type = RecordType::JobObservation;
                r.job_ref = Some(req(args, "--job-ref")?);
                let f = |n| fact(args, n, &c.clock);
                c.times(
                    &mut r,
                    &[
                        ("job_submit", f("--job-submit")),
                        ("job_start", f("--job-start")),
                        ("job_end", f("--job-end")),
                    ],
                );
                vec![r]
            }
            "cutoff" => {
                r.record_type = RecordType::Cutoff;
                c.times(
                    &mut r,
                    &[("occurred_at", fact(args, "--occurred-at", &c.clock))],
                );
                vec![r]
            }
            o => return Err(format!("--kind: wait|attempt|job|cutoff（{o:?}）")),
        };
        append_all(Path::new(&file), recs)
    }

    fn read(args: &[String]) -> Result<Vec<HumanOpRecord>, String> {
        let file = req(args, "--file")?;
        if !Path::new(&file).exists() {
            return Err(format!("{file}: file が無い"));
        }
        read_records(Path::new(&file)).map_err(|e| e.to_string())
    }

    fn validate(args: &[String]) -> Result<String, String> {
        let recs = read(args)?;
        let n = recs.len();
        resolve(&recs).map_err(|e| e.to_string())?;
        Ok(format!("ok: {n} records"))
    }

    fn list(args: &[String]) -> Result<String, String> {
        let recs = resolve(&read(args)?).map_err(|e| e.to_string())?;
        let arm = opt(args, "--arm");
        let (campaign, case) = (opt(args, "--campaign"), opt(args, "--case"));
        let keep: Vec<&HumanOpRecord> = recs
            .iter()
            .filter(|r| arm.as_deref().is_none_or(|a| r.arm_label() == Some(a)))
            .filter(|r| campaign.as_deref().is_none_or(|c| r.campaign_id == c))
            .filter(|r| {
                case.as_deref()
                    .is_none_or(|c| r.case_id.as_deref() == Some(c))
            })
            .collect();
        let owned: Vec<HumanOpRecord> = keep.iter().map(|r| (*r).clone()).collect();
        let records: Vec<Value> = keep
            .iter()
            .map(|r| {
                let mut v = serde_json::to_value(r).unwrap_or(Value::Null);
                if let Some(m) = v.as_object_mut() {
                    m.insert("arm_label".into(), json!(r.arm_label()));
                }
                v
            })
            .collect();
        // 所要時間: 端が欠ければ null（0 にしない）。
        let ops: Vec<Value> = op_spans(&owned)
        .into_iter()
        .map(|(r, s)| {
            json!({
                "arm_label": r.arm_label(), "case_id": r.case_id, "logical_op_id": r.logical_op_id,
                "attempt": r.attempt, "op_kind": r.op_kind, "start": s.start, "end": s.end,
                "duration_secs": s.duration_secs(),
            })
        })
        .collect();
        let redo: Vec<Value> = redo_counts(&owned)
            .into_iter()
            .map(|((t, l), n)| json!({"trial_key": t, "logical_op_id": l, "redo": n}))
            .collect();
        serde_json::to_string_pretty(&json!({"records": records, "ops": ops, "redo_counts": redo}))
            .map_err(|e| e.to_string())
    }

    /// `args[0] == "human-op"`。
    pub fn cli(args: &[String]) -> Result<String, String> {
        let rest = args.get(2..).unwrap_or(&[]);
        match args.get(1).map(String::as_str) {
            Some("record") => record(rest),
            Some("span") => span(rest),
            Some("validate") => validate(rest),
            Some("list") => list(rest),
            _ => Err(USAGE.into()),
        }
    }
}
