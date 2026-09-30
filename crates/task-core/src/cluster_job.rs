//! ADR-0090: クラスタ job（PBS / Slurm）の durable wait。
//!
//! worker の run が `result.json` の `{"type": "wait", "kind": "cluster_job", ...}` で終わると、dispatcher は
//! その run を閉じ（`runs.status = waiting`）、task（または WorkUnit）を止めて `ClusterJobWaitStarted` を残す。
//! daemon の tick がクラスタの ssh master 越しに `qstat -xf`（PBS）/ `sacct`（Slurm）を `poll_secs` ごとに
//! 高々 1 回だけ流し、すべての job が終わったら `ClusterJobWaitFinished{satisfied}` と一緒に task を
//! continuation の run に戻す。
//!
//! - **events が正本**。`cluster_job_waits` 表は event と同じトランザクションで書く派生の索引
//!   （[`apply_event_tx`]）。状態の変わらない poll は event を出さず、`last_polled_at` / `last_status` だけを
//!   直接書く（[`ClusterJobWaitStore::cluster_job_wait_touch`]）。
//! - このモジュールの parser・検証は純粋関数（I/O なし・LLM なし）。
//! - task の中止・終端では wait を `cancelled` に閉じるだけで、**job は `qdel` しない**（ADR-0090 D6）。

use rusqlite::{Connection, OptionalExtension, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::store::{SqliteStore, StoreError};
use crate::{Event, TaskId};

/// 1 つの wait が持てる job の数の上限。
pub const MAX_JOBS: usize = 64;
/// job id の最大長。
pub const JOB_ID_MAX_LEN: usize = 64;
/// `summary` の最大文字数。
pub const SUMMARY_MAX_CHARS: usize = 500;
/// poll の間隔の下限（秒）。設定・worker の申告のどちらもこれより短くできない。
pub const MIN_POLL_SECS: u64 = 30;
/// `[[clusters]] job_wait.poll_secs` の既定（秒）。
pub const DEFAULT_POLL_SECS: u64 = 300;
/// `[[clusters]] job_wait.max_wait_secs` の既定（秒。24 時間）。
pub const DEFAULT_MAX_WAIT_SECS: u64 = 86_400;
/// `[[clusters]] job_wait.max_wait_secs` の上限（秒。14 日）。
pub const MAX_WAIT_SECS_CEILING: u64 = 14 * 86_400;
/// `result.json` の `kind`。
pub const WAIT_KIND_CLUSTER_JOB: &str = "cluster_job";
/// `Trigger::ClusterJobWait` の `Transitioned.reason`。
pub const REASON_WAITING: &str = "waiting_for_cluster_jobs";
/// `Trigger::ClusterJobResume` の `Transitioned.reason`。
pub const REASON_RESUME: &str = "cluster_job_resume";

/// job scheduler の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClusterScheduler {
    Pbs,
    Slurm,
}

impl ClusterScheduler {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pbs => "pbs",
            Self::Slurm => "slurm",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pbs" => Some(Self::Pbs),
            "slurm" => Some(Self::Slurm),
            _ => None,
        }
    }
}

/// 1 つの job の状態（scheduler の文字を正規化したもの）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClusterJobState {
    /// 待ち行列にいる（PBS `Q` / `W` / `T`、Slurm `PENDING`）。
    Queued,
    /// 保留・一時停止（PBS `H` / `S` / `U`、Slurm `SUSPENDED` / `REQUEUE_HOLD`）。
    Held,
    /// 実行中（PBS `R` / `B`、Slurm `RUNNING`）。
    Running,
    /// 終了処理中（PBS `E`、Slurm `COMPLETING`）。
    Exiting,
    /// 終わった（PBS `F` / `X` / `C`、Slurm の終端の状態）。
    Finished,
    /// scheduler が「その job は知らない」と明示した（履歴が消えた・id の誤り）。終わったものとして扱う。
    Gone,
    /// 出力に現れなかった（poll の失敗かもしれないので終わったとは扱わない）。
    Unknown,
}

impl ClusterJobState {
    /// wait を満たす（これ以上待っても変わらない）状態か。
    pub fn is_finished(self) -> bool {
        matches!(self, Self::Finished | Self::Gone)
    }

    /// GUI・前置きの 1 文字（`Q` / `H` / `R` / `E` / `F` / `?`）。
    pub fn letter(self) -> &'static str {
        match self {
            Self::Queued => "Q",
            Self::Held => "H",
            Self::Running => "R",
            Self::Exiting => "E",
            Self::Finished => "F",
            Self::Gone => "gone",
            Self::Unknown => "?",
        }
    }
}

/// 1 つの job の poll の結果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClusterJobStatus {
    /// worker が申告した id（`42634`。scheduler の server 名の接尾辞は付けない）。
    pub job_id: String,
    pub state: ClusterJobState,
    /// 終わった job の終了コード（PBS `Exit_status`、Slurm `ExitCode` の前半）。未確定なら省略。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_status: Option<i32>,
    /// scheduler が返した生の状態（PBS の `job_state` の文字、Slurm の `State`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_state: Option<String>,
}

impl ClusterJobStatus {
    fn unknown(job_id: &str) -> Self {
        Self {
            job_id: job_id.to_string(),
            state: ClusterJobState::Unknown,
            exit_status: None,
            raw_state: None,
        }
    }
}

/// wait の状態。`waiting` だけが「開いている」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClusterJobWaitState {
    Waiting,
    Satisfied,
    TimedOut,
    Cancelled,
}

impl ClusterJobWaitState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Satisfied => "satisfied",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "waiting" => Some(Self::Waiting),
            "satisfied" => Some(Self::Satisfied),
            "timed_out" => Some(Self::TimedOut),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

/// `result.json` の wait の申告（[`parse_wait_request`] が検証した後の形）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClusterJobWaitRequest {
    /// `[[clusters]] id`。省略時は task のクラスタ（dispatcher が埋める）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster: Option<String>,
    pub scheduler: ClusterScheduler,
    pub jobs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
}

/// job id として受け付ける文字（`42634`、`42634.sirius-pbs`、`1234[]`、`5678_1`）。シェルに渡すので厳しく。
pub fn valid_job_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= JOB_ID_MAX_LEN
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '[' | ']'))
        && id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
}

/// `[[clusters]] id` として受け付ける文字（ログ・event に出すだけだが念のため）。
fn valid_cluster_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// `result.json` の値から wait の申告を読む。
///
/// 受け付ける形は 2 つ（どちらも同じ欄）:
/// - 最上位: `{"type": "wait", "kind": "cluster_job", "cluster": ..., "jobs": [...], ...}`
/// - 入れ子: `{"summary": ..., "wait": {"kind": "cluster_job", "jobs": [...], ...}}`（`yield` と同じ置き方）
///
/// wait の申告が無ければ `None`。申告はあるが不正なら `Some(Err(理由))`（呼び出し側は run を
/// `error(retryable)` にする）。`checkpoint` は生の JSON のまま返す（`yield` と同じく
/// dispatcher の `merge_checkpoint` が検証・合成する）。
pub fn parse_wait_request(
    value: &serde_json::Value,
) -> Option<Result<(ClusterJobWaitRequest, Option<serde_json::Value>), String>> {
    let obj = value.as_object()?;
    let (spec, outer) = if obj.get("type").and_then(|t| t.as_str()) == Some("wait") {
        (obj, None)
    } else {
        match obj.get("wait") {
            Some(serde_json::Value::Object(inner)) => (inner, Some(obj)),
            Some(serde_json::Value::Null) | None => return None,
            Some(_) => return Some(Err("`wait` must be an object".to_string())),
        }
    };
    Some(parse_wait_fields(spec, outer))
}

fn parse_wait_fields(
    spec: &serde_json::Map<String, serde_json::Value>,
    outer: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Result<(ClusterJobWaitRequest, Option<serde_json::Value>), String> {
    let field = |name: &str| spec.get(name).or_else(|| outer.and_then(|o| o.get(name)));
    let kind = spec
        .get("kind")
        .and_then(|k| k.as_str())
        .unwrap_or(WAIT_KIND_CLUSTER_JOB);
    if kind != WAIT_KIND_CLUSTER_JOB {
        return Err(format!(
            "unsupported wait kind {kind:?} (only \"{WAIT_KIND_CLUSTER_JOB}\")"
        ));
    }
    let scheduler = match spec.get("scheduler").and_then(|s| s.as_str()) {
        Some(s) => ClusterScheduler::parse(&s.to_ascii_lowercase())
            .ok_or_else(|| format!("unknown scheduler {s:?} (pbs | slurm)"))?,
        None => ClusterScheduler::Pbs,
    };
    let cluster = match spec.get("cluster") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(c)) if valid_cluster_id(c) => Some(c.clone()),
        Some(other) => return Err(format!("invalid cluster {other}")),
    };
    let raw_jobs = spec
        .get("jobs")
        .or_else(|| spec.get("job_ids"))
        .and_then(|j| j.as_array())
        .ok_or_else(|| "`jobs` must be a non-empty array of job ids".to_string())?;
    let mut jobs: Vec<String> = Vec::new();
    for j in raw_jobs {
        let id = match j {
            serde_json::Value::String(s) => s.trim().to_string(),
            serde_json::Value::Number(n) => n.to_string(),
            other => return Err(format!("invalid job id {other}")),
        };
        if !valid_job_id(&id) {
            return Err(format!("invalid job id {id:?}"));
        }
        if !jobs.contains(&id) {
            jobs.push(id);
        }
    }
    if jobs.is_empty() {
        return Err("`jobs` must be a non-empty array of job ids".to_string());
    }
    if jobs.len() > MAX_JOBS {
        return Err(format!(
            "too many jobs ({}; at most {MAX_JOBS} per wait)",
            jobs.len()
        ));
    }
    let secs = |name: &str| -> Result<Option<u64>, String> {
        match spec.get(name) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(v) => v
                .as_u64()
                .filter(|n| *n > 0)
                .map(Some)
                .ok_or_else(|| format!("`{name}` must be a positive integer")),
        }
    };
    let poll_secs = secs("poll_secs")?;
    let timeout_secs = secs("timeout_secs")?;
    let summary = field("summary")
        .and_then(|s| s.as_str())
        .map(|s| truncate_chars(s.trim(), SUMMARY_MAX_CHARS))
        .unwrap_or_default();
    let checkpoint = field("checkpoint").filter(|c| c.is_object()).cloned();
    Ok((
        ClusterJobWaitRequest {
            cluster,
            scheduler,
            jobs,
            poll_secs,
            timeout_secs,
            summary,
        },
        checkpoint,
    ))
}

/// `[[clusters]] job_wait`（ADR-0090 D7）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClusterJobWaitLimits {
    /// poll の既定の間隔（秒、>= [`MIN_POLL_SECS`]）。worker の `poll_secs` はこれより短くできない。
    pub poll_secs: u64,
    /// 待ちの上限（秒、`poll_secs` 以上 [`MAX_WAIT_SECS_CEILING`] 以下）。worker の `timeout_secs` の上限と既定。
    pub max_wait_secs: u64,
}

impl Default for ClusterJobWaitLimits {
    fn default() -> Self {
        Self {
            poll_secs: DEFAULT_POLL_SECS,
            max_wait_secs: DEFAULT_MAX_WAIT_SECS,
        }
    }
}

impl ClusterJobWaitLimits {
    /// 設定の検証（`celeris` の設定の読み込みが呼ぶ）。
    pub fn validate(&self) -> Result<(), String> {
        if self.poll_secs < MIN_POLL_SECS {
            return Err(format!(
                "job_wait.poll_secs must be >= {MIN_POLL_SECS} (got {})",
                self.poll_secs
            ));
        }
        if self.max_wait_secs < self.poll_secs {
            return Err(format!(
                "job_wait.max_wait_secs ({}) must be >= job_wait.poll_secs ({})",
                self.max_wait_secs, self.poll_secs
            ));
        }
        if self.max_wait_secs > MAX_WAIT_SECS_CEILING {
            return Err(format!(
                "job_wait.max_wait_secs must be <= {MAX_WAIT_SECS_CEILING} (got {})",
                self.max_wait_secs
            ));
        }
        Ok(())
    }

    /// worker の申告を設定の範囲に丸める: `(poll_secs, timeout_secs)`。
    pub fn clamp(&self, req: &ClusterJobWaitRequest) -> (u64, u64) {
        let poll = req
            .poll_secs
            .unwrap_or(self.poll_secs)
            .max(self.poll_secs)
            .max(MIN_POLL_SECS);
        let timeout = req
            .timeout_secs
            .unwrap_or(self.max_wait_secs)
            .min(self.max_wait_secs)
            .max(poll);
        (poll, timeout)
    }
}

/// `cluster_job_waits` の 1 行（`ClusterJobWaitStarted` の中身と同じ形）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ClusterJobWait {
    pub wait_id: String,
    pub task_id: TaskId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_unit_id: Option<String>,
    pub run_id: String,
    pub cluster: String,
    pub scheduler: ClusterScheduler,
    pub jobs: Vec<String>,
    pub poll_secs: u64,
    pub timeout_secs: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// worker が添えた checkpoint の申告（生の JSON）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<serde_json::Value>,
    /// RFC 3339。
    pub created_at: String,
    /// RFC 3339（`created_at + timeout_secs`）。
    pub deadline: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_polled_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    pub state: ClusterJobWaitState,
    #[serde(default)]
    pub last_status: Vec<ClusterJobStatus>,
}

impl ClusterJobWait {
    /// job ごとの直近の状態（まだ poll していない job は `Unknown`）。申告の順。
    pub fn job_statuses(&self) -> Vec<ClusterJobStatus> {
        self.jobs
            .iter()
            .map(|j| {
                self.last_status
                    .iter()
                    .find(|s| &s.job_id == j)
                    .cloned()
                    .unwrap_or_else(|| ClusterJobStatus::unknown(j))
            })
            .collect()
    }
}

/// `42634 (R) 42635 (Q)` の形（GUI の 1 行・ログ用）。まだ poll していない job は `(?)`。
pub fn status_line(statuses: &[ClusterJobStatus]) -> String {
    statuses
        .iter()
        .map(|s| match (s.state, s.exit_status) {
            (ClusterJobState::Finished, Some(code)) => format!("{} (F, exit {code})", s.job_id),
            (state, _) => format!("{} ({})", s.job_id, state.letter()),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// すべての job が終わったか（`statuses` が申告の job をすべて含み、どれも [`ClusterJobState::is_finished`]）。
pub fn all_finished(jobs: &[String], statuses: &[ClusterJobStatus]) -> bool {
    !jobs.is_empty()
        && jobs.iter().all(|j| {
            statuses
                .iter()
                .any(|s| &s.job_id == j && s.state.is_finished())
        })
}

/// クラスタで流す poll のコマンド（シェルの 1 行）。job id は [`valid_job_id`] を通ったものだけ。
///
/// - PBS: `qstat -xf <ids>`（`-x` で終わった job の履歴も返す。PBS Pro の job history が前提）。
/// - Slurm: `sacct -n -P -X -o JobID,State,ExitCode -j <ids>`（`squeue` は終わった job を返さない）。
pub fn poll_command(scheduler: ClusterScheduler, jobs: &[String]) -> String {
    let ids: Vec<&str> = jobs
        .iter()
        .map(String::as_str)
        .filter(|j| valid_job_id(j))
        .collect();
    match scheduler {
        ClusterScheduler::Pbs => format!("qstat -xf {}", ids.join(" ")),
        ClusterScheduler::Slurm => format!(
            "sacct -n -P -X -o JobID,State,ExitCode -j {}",
            ids.join(",")
        ),
    }
}

/// poll の出力を scheduler ごとに読む。
pub fn parse_poll_output(
    scheduler: ClusterScheduler,
    stdout: &str,
    stderr: &str,
    jobs: &[String],
) -> Vec<ClusterJobStatus> {
    match scheduler {
        ClusterScheduler::Pbs => parse_pbs_qstat_xf(stdout, stderr, jobs),
        ClusterScheduler::Slurm => parse_slurm_sacct(stdout, jobs),
    }
}

/// `42634.sirius-pbs` → `42634`（server 名の接尾辞を落とす。配列 job の `[]` は残す）。
fn short_job_id(full: &str) -> &str {
    full.split('.').next().unwrap_or(full)
}

fn job_matches(requested: &str, reported: &str) -> bool {
    requested == reported || short_job_id(requested) == short_job_id(reported)
}

fn pbs_state(letter: &str) -> ClusterJobState {
    match letter {
        "Q" | "W" | "T" | "M" => ClusterJobState::Queued,
        "H" | "S" | "U" => ClusterJobState::Held,
        "R" | "B" => ClusterJobState::Running,
        "E" => ClusterJobState::Exiting,
        "F" | "X" | "C" => ClusterJobState::Finished,
        _ => ClusterJobState::Unknown,
    }
}

/// PBS の `qstat -xf <ids>` の出力を読む。
///
/// 各 job は `Job Id: 42634.server` で始まる段落で、`job_state = F` と（終わった job なら）
/// `Exit_status = 0` を持つ。長い値は次の行にタブで折り返されるが、読むのは 1 行の欄だけ。
/// stderr の `qstat: Unknown Job Id 42634.server` は [`ClusterJobState::Gone`]（終わったものとして扱う）。
/// 出力に現れなかった job は [`ClusterJobState::Unknown`]（終わったとは扱わない）。
pub fn parse_pbs_qstat_xf(stdout: &str, stderr: &str, jobs: &[String]) -> Vec<ClusterJobStatus> {
    // (報告された id, job_state, Exit_status)
    let mut blocks: Vec<(String, Option<String>, Option<i32>)> = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some(id) = trimmed.strip_prefix("Job Id:") {
            blocks.push((id.trim().to_string(), None, None));
            continue;
        }
        let Some(current) = blocks.last_mut() else {
            continue;
        };
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        match key.trim() {
            "job_state" => current.1 = Some(value.trim().to_string()),
            "Exit_status" | "exit_status" => current.2 = value.trim().parse::<i32>().ok(),
            _ => {}
        }
    }
    let gone: Vec<&str> = stderr
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            l.split_once("Unknown Job Id")
                .map(|(_, rest)| rest.trim().trim_start_matches(':').trim())
                .and_then(|rest| rest.split_whitespace().next())
        })
        .collect();
    jobs.iter()
        .map(|j| {
            if let Some((_, state, exit)) = blocks.iter().find(|(id, _, _)| job_matches(j, id)) {
                let raw = state.clone();
                let st = raw
                    .as_deref()
                    .map(pbs_state)
                    .unwrap_or(ClusterJobState::Unknown);
                return ClusterJobStatus {
                    job_id: j.clone(),
                    state: st,
                    exit_status: if st == ClusterJobState::Finished {
                        *exit
                    } else {
                        None
                    },
                    raw_state: raw,
                };
            }
            if gone.iter().any(|g| job_matches(j, g)) {
                return ClusterJobStatus {
                    job_id: j.clone(),
                    state: ClusterJobState::Gone,
                    exit_status: None,
                    raw_state: Some("unknown to the scheduler".to_string()),
                };
            }
            ClusterJobStatus::unknown(j)
        })
        .collect()
}

fn slurm_state(state: &str) -> ClusterJobState {
    // `CANCELLED by 123` のような接尾辞は先頭の語だけを見る。
    let word = state.split_whitespace().next().unwrap_or("");
    match word {
        "PENDING" | "CONFIGURING" | "REQUEUED" | "RESIZING" => ClusterJobState::Queued,
        "SUSPENDED" | "REQUEUE_HOLD" | "REQUEUE_FED" | "STOPPED" => ClusterJobState::Held,
        "RUNNING" | "SIGNALING" | "STAGE_OUT" => ClusterJobState::Running,
        "COMPLETING" => ClusterJobState::Exiting,
        "COMPLETED" | "FAILED" | "CANCELLED" | "TIMEOUT" | "OUT_OF_MEMORY" | "NODE_FAIL"
        | "PREEMPTED" | "BOOT_FAIL" | "DEADLINE" | "REVOKED" | "SPECIAL_EXIT" => {
            ClusterJobState::Finished
        }
        _ => ClusterJobState::Unknown,
    }
}

/// Slurm の `sacct -n -P -X -o JobID,State,ExitCode -j <ids>` の出力（`123|COMPLETED|0:0`）を読む。
/// `ExitCode` は `<exit>:<signal>`。出力に無い job は [`ClusterJobState::Unknown`]。
pub fn parse_slurm_sacct(stdout: &str, jobs: &[String]) -> Vec<ClusterJobStatus> {
    let rows: Vec<(String, String, Option<i32>)> = stdout
        .lines()
        .filter_map(|l| {
            let mut cols = l.trim().split('|');
            let id = cols.next()?.trim().to_string();
            let state = cols.next()?.trim().to_string();
            let exit = cols
                .next()
                .and_then(|c| c.trim().split(':').next())
                .and_then(|c| c.parse::<i32>().ok());
            (!id.is_empty()).then_some((id, state, exit))
        })
        .collect();
    jobs.iter()
        .map(|j| match rows.iter().find(|(id, _, _)| id == j) {
            Some((_, state, exit)) => {
                let st = slurm_state(state);
                ClusterJobStatus {
                    job_id: j.clone(),
                    state: st,
                    exit_status: if st == ClusterJobState::Finished {
                        *exit
                    } else {
                        None
                    },
                    raw_state: Some(state.clone()),
                }
            }
            None => ClusterJobStatus::unknown(j),
        })
        .collect()
}

/// continuation の前置きに入れる、job の最終状態の行（`- 42634: F, Exit_status 0` の形）。
pub fn preamble_lines(statuses: &[ClusterJobStatus]) -> Vec<String> {
    statuses
        .iter()
        .map(|s| {
            let raw = s
                .raw_state
                .as_deref()
                .map(|r| format!(" / scheduler: {r}"))
                .unwrap_or_default();
            match (s.state, s.exit_status) {
                (ClusterJobState::Finished, Some(code)) => {
                    format!("{}: finished (Exit_status {code}{raw})", s.job_id)
                }
                (ClusterJobState::Finished, None) => {
                    format!("{}: finished (exit status unknown{raw})", s.job_id)
                }
                (ClusterJobState::Gone, _) => format!(
                    "{}: no longer known to the scheduler (history expired?; exit status unknown)",
                    s.job_id
                ),
                (state, _) => format!("{}: {:?} ({}{raw})", s.job_id, state, state.letter()),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// store（`SqliteStore` だけが実装する。`TaskStore` の supertrait）
// ---------------------------------------------------------------------------

/// `cluster_job_waits` の読み取りと、event を出さない poll の記録。開く・閉じるは event の追記で行う
/// （`ClusterJobWaitStarted` / `ClusterJobWaitFinished` を `apply_transition_with_events` などに渡す）。
pub trait ClusterJobWaitStore: Send + Sync {
    fn cluster_job_wait_get(&self, wait_id: &str) -> Result<Option<ClusterJobWait>, StoreError>;
    /// task の wait（作成順）。
    fn cluster_job_waits_for_task(
        &self,
        task_id: TaskId,
    ) -> Result<Vec<ClusterJobWait>, StoreError>;
    /// `waiting` の全件（作成順）。
    fn cluster_job_waits_waiting(&self) -> Result<Vec<ClusterJobWait>, StoreError>;
    /// 状態の変わらない poll: `last_polled_at` と `last_status` だけを書く（`waiting` の行だけ。event なし）。
    fn cluster_job_wait_touch(
        &self,
        wait_id: &str,
        polled_at: &str,
        statuses: &[ClusterJobStatus],
    ) -> Result<(), StoreError>;
}

const SELECT_WAIT: &str = "SELECT wait_id, task_id, work_unit_id, run_id, cluster, scheduler, jobs_json, \
     poll_secs, timeout_secs, summary, checkpoint_json, created_at, deadline, last_polled_at, finished_at, \
     state, last_status_json FROM cluster_job_waits";

fn row_to_wait(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawWait> {
    Ok(RawWait {
        wait_id: row.get(0)?,
        task_id: row.get(1)?,
        work_unit_id: row.get(2)?,
        run_id: row.get(3)?,
        cluster: row.get(4)?,
        scheduler: row.get(5)?,
        jobs_json: row.get(6)?,
        poll_secs: row.get(7)?,
        timeout_secs: row.get(8)?,
        summary: row.get(9)?,
        checkpoint_json: row.get(10)?,
        created_at: row.get(11)?,
        deadline: row.get(12)?,
        last_polled_at: row.get(13)?,
        finished_at: row.get(14)?,
        state: row.get(15)?,
        last_status_json: row.get(16)?,
    })
}

struct RawWait {
    wait_id: String,
    task_id: String,
    work_unit_id: Option<String>,
    run_id: String,
    cluster: String,
    scheduler: String,
    jobs_json: String,
    poll_secs: i64,
    timeout_secs: i64,
    summary: String,
    checkpoint_json: Option<String>,
    created_at: String,
    deadline: String,
    last_polled_at: Option<String>,
    finished_at: Option<String>,
    state: String,
    last_status_json: String,
}

fn wait_from_raw(raw: RawWait) -> Result<ClusterJobWait, StoreError> {
    let task_id = raw
        .task_id
        .parse::<TaskId>()
        .map_err(|e| StoreError::Invalid(format!("cluster_job_waits.task_id: {e}")))?;
    let scheduler = ClusterScheduler::parse(&raw.scheduler).ok_or_else(|| {
        StoreError::Invalid(format!("cluster_job_waits.scheduler: {}", raw.scheduler))
    })?;
    let state = ClusterJobWaitState::parse(&raw.state)
        .ok_or_else(|| StoreError::Invalid(format!("cluster_job_waits.state: {}", raw.state)))?;
    Ok(ClusterJobWait {
        wait_id: raw.wait_id,
        task_id,
        work_unit_id: raw.work_unit_id,
        run_id: raw.run_id,
        cluster: raw.cluster,
        scheduler,
        jobs: serde_json::from_str(&raw.jobs_json)?,
        poll_secs: u64::try_from(raw.poll_secs).unwrap_or(DEFAULT_POLL_SECS),
        timeout_secs: u64::try_from(raw.timeout_secs).unwrap_or(DEFAULT_MAX_WAIT_SECS),
        summary: raw.summary,
        checkpoint: raw
            .checkpoint_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?,
        created_at: raw.created_at,
        deadline: raw.deadline,
        last_polled_at: raw.last_polled_at,
        finished_at: raw.finished_at,
        state,
        last_status: serde_json::from_str(&raw.last_status_json).unwrap_or_default(),
    })
}

fn query_waits(
    conn: &Connection,
    where_sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Vec<ClusterJobWait>, StoreError> {
    let mut stmt = conn.prepare(&format!("{SELECT_WAIT} {where_sql}"))?;
    let rows = stmt
        .query_map(args, row_to_wait)?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(wait_from_raw).collect()
}

fn to_i64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// event の追記と同じトランザクションで `cluster_job_waits` を書く（`SqliteStore::append_event_tx` と
/// `apply_transition_tx` の extra events から呼ぶ）。
pub(crate) fn apply_event_tx(
    conn: &Connection,
    task_id: TaskId,
    event: &Event,
    ts: &str,
) -> Result<(), StoreError> {
    match event {
        Event::ClusterJobWaitStarted { wait } => {
            conn.execute(
                "INSERT OR IGNORE INTO cluster_job_waits (wait_id, task_id, work_unit_id, run_id, cluster, \
                 scheduler, jobs_json, poll_secs, timeout_secs, summary, checkpoint_json, created_at, deadline, \
                 last_polled_at, finished_at, state, last_status_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, \
                 ?9, ?10, ?11, ?12, ?13, NULL, NULL, 'waiting', ?14)",
                params![
                    wait.wait_id,
                    task_id.to_string(),
                    wait.work_unit_id,
                    wait.run_id,
                    wait.cluster,
                    wait.scheduler.as_str(),
                    serde_json::to_string(&wait.jobs)?,
                    to_i64(wait.poll_secs),
                    to_i64(wait.timeout_secs),
                    wait.summary,
                    wait.checkpoint
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()?,
                    wait.created_at,
                    wait.deadline,
                    serde_json::to_string(&wait.last_status)?,
                ],
            )?;
        }
        Event::ClusterJobWaitPolled { wait_id, jobs } => {
            conn.execute(
                "UPDATE cluster_job_waits SET last_status_json = ?1, last_polled_at = ?2 \
                 WHERE wait_id = ?3 AND state = 'waiting'",
                params![serde_json::to_string(jobs)?, ts, wait_id],
            )?;
        }
        Event::ClusterJobWaitFinished {
            wait_id,
            state,
            jobs,
            ..
        } => {
            if jobs.is_empty() {
                conn.execute(
                    "UPDATE cluster_job_waits SET state = ?1, finished_at = ?2 \
                     WHERE wait_id = ?3 AND state = 'waiting'",
                    params![state.as_str(), ts, wait_id],
                )?;
            } else {
                conn.execute(
                    "UPDATE cluster_job_waits SET state = ?1, finished_at = ?2, last_status_json = ?3 \
                     WHERE wait_id = ?4 AND state = 'waiting'",
                    params![state.as_str(), ts, serde_json::to_string(jobs)?, wait_id],
                )?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// task が終端になったら、`waiting` の wait を `cancelled` に閉じ、`ClusterJobWaitFinished{cancelled}` を残す
/// （同じトランザクション）。**job は `qdel` しない**（ADR-0090 D6: 人が投げ直す・結果を拾う余地を残す）。
pub(crate) fn cancel_open_for_task_tx(
    conn: &Connection,
    task_id: TaskId,
) -> Result<(), StoreError> {
    let open = query_waits(
        conn,
        "WHERE task_id = ?1 AND state = 'waiting' ORDER BY created_at, wait_id",
        &[&task_id.to_string()],
    )?;
    for wait in open {
        let event = Event::ClusterJobWaitFinished {
            wait_id: wait.wait_id.clone(),
            state: ClusterJobWaitState::Cancelled,
            jobs: wait.last_status.clone(),
            detail: "task_terminal: the jobs were not cancelled (no qdel)".to_string(),
        };
        SqliteStore::append_event_tx(conn, task_id, &event)?;
    }
    Ok(())
}

/// `apply_transition_tx` の迂回防止: `waiting` の wait があるか（一般の回答・途中確認の再開で `ready` に戻さない）。
pub(crate) fn has_waiting_tx(conn: &Connection, task_id: TaskId) -> Result<bool, StoreError> {
    let n: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM cluster_job_waits WHERE task_id = ?1 AND state = 'waiting' LIMIT 1",
            params![task_id.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    Ok(n.is_some())
}

impl ClusterJobWaitStore for SqliteStore {
    fn cluster_job_wait_get(&self, wait_id: &str) -> Result<Option<ClusterJobWait>, StoreError> {
        let conn = self.lock()?;
        Ok(query_waits(&conn, "WHERE wait_id = ?1", &[&wait_id])?
            .into_iter()
            .next())
    }

    fn cluster_job_waits_for_task(
        &self,
        task_id: TaskId,
    ) -> Result<Vec<ClusterJobWait>, StoreError> {
        let conn = self.lock()?;
        query_waits(
            &conn,
            "WHERE task_id = ?1 ORDER BY created_at, wait_id",
            &[&task_id.to_string()],
        )
    }

    fn cluster_job_waits_waiting(&self) -> Result<Vec<ClusterJobWait>, StoreError> {
        let conn = self.lock()?;
        query_waits(
            &conn,
            "WHERE state = 'waiting' ORDER BY created_at, wait_id",
            &[],
        )
    }

    fn cluster_job_wait_touch(
        &self,
        wait_id: &str,
        polled_at: &str,
        statuses: &[ClusterJobStatus],
    ) -> Result<(), StoreError> {
        let conn = self.lock()?;
        conn.execute(
            "UPDATE cluster_job_waits SET last_polled_at = ?1, last_status_json = ?2 \
             WHERE wait_id = ?3 AND state = 'waiting'",
            params![polled_at, serde_json::to_string(statuses)?, wait_id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "cluster_job/tests.rs"]
mod tests;
