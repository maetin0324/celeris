//! 全体台帳（測定 ID を跨ぐ消費・予約・provider 枠）。ADR 2026-10-19-experiment-sets D5/D7。
//! 別測定 ID（A/B のセットと B/C のセット）を順に動かしても、既消費・予約・provider 枠を持ち越す。
//!
//! 追記専用 JSONL（`schema` = `celeris.campaign-global-ledger/2`）。1 行は次のどちらか（`kind`）:
//! - `measurement`: 測定 ID の **確定消費**（runner の再集計）・投入時の取り分（allocation）・上限。
//!   再集計は単調（前の値より下げない）なので、空の publish や再集計が過去の消費を消せない。
//! - `attempt`: 試行 identity（測定 ID・試行・実行 attempt 番号）ごとの **未精算予約** と投入/精算状態。
//!   同じ identity の行は置換（最後の行が現在値）、別 attempt の消費は合算する。
//!
//! 承認上限（`Caps`）は台帳の全行（置換された古い行も含む）と manifest の最小で、どの書き込みでも増えない。
//! 読取・上限の最小化・資源検査・追記と fsync は 1 つの flock の中で行う（同時投入で両方通らない）。
//!
//! ## 投入の永続遷移とクラッシュ時の復旧（安全側）
//! `reserve`（Reserved を fsync）→ 実行器へ投入 → `mark_submitted`（Submitted）→ 実行器で終了を確認 →
//! `settle`（Settled = 確定消費。provider 枠を解放）。各書き込みの間で止まったとき:
//! 1. `reserve` の fsync 前: 予約も投入も無い。何も失わない。
//! 2. `reserve` 後・投入前、または投入後・`mark_submitted` 前: 行は Reserved のまま予約と枠を持ち続ける
//!    （黙って解放しない）。再開側は Reserved を「投入したかもしれない」とみなし、identity を冪等鍵に実行器へ
//!    問い合わせる。投入が無いと確かめたときだけ `Terminal::NotSubmitted` で解放し、あれば続きを追う。
//! 3. Submitted のまま停止: 予約と枠を持ち続ける。終了を確認してから `settle`（同じ identity の再精算は置換）。
//! 4. `settle` の後に runner の再集計（`record`）が古い・空でも、確定消費は下がらない。
//!
//! ## runner の配線（`runner::run`）
//! `admit_start` は開始の登録と検証（取り分の記録）だけで、投入の可否には使わない。毎投入（同期・非同期、
//! `stop_on_budget` に関係なく）の前に `admit_trial` で最新の全行を読み、最小の承認上限で tokens・試行数・
//! provider 枠（1 試行 1 枠）を検査して予約を fsync してから、ローカル台帳の予約 → lookup/submit に進む。
//! 実投入の `TrialRequest.budget` は case/arm 上限と共有予約額の小さい方。拒否は、未精算の予約が残れば待機、
//! 無ければ打ち切り（executor は呼ばない）。試行 identity は ローカル台帳の冪等キー、attempt 番号は台帳が決める
//! （未精算があれば同じ attempt を続け、無ければ精算済みの次の番号。前の attempt の確定消費は残る）。
//! 精算は記録の追記後で、cancel の停止未確認・追跡中は持ち続ける。未送信の解放は Reserved のまま
//! ローカル台帳が投入を示さないとき（Refused・routing 失敗・同期の Wait）と、再開時に冪等キーの照会で
//! 投入が無いと確かめたときだけ。同期 executor の Err と実行中の停止は予約額を消費として保守的に精算する。
//! tokens 欠測は予約額と既観測の大きい方で精算する（0 にしない）。
//!
//! 旧 `/1` 行は保守的に読む: tokens/trials は確定消費と予約の混在なので全部を確定消費として持ち（下げない）、
//! cap 欄が無い行は上限を与えない（manifest と他の行の最小が効くので承認は増えない）、`provider_slots` は
//! 解放の記録が無いので枠を占有し続ける。同じ測定 ID の持ち主が自分の未完了試行が無いことを確かめて
//! `confirm_legacy_idle` を呼んだときだけ解放する。読めない行・不明 schema は破損として拒否する（0 にしない）。
//!
//! ## 人の承認の記録（`ledger-init`、2026-10-26 決定）
//! runner の `admit_start`・予約・精算以外に、人の承認（誰が・いつ・範囲・期限・試行数）を台帳へ記録する入口。
//! - `celeris-campaign ledger-init --manifest <m> --global-ledger <dir> --approval <json>`。`--global-ledger` は
//!   manifest の宣言 path だけを受ける（`check_shared_ledger`）。
//! - 出典・範囲・期限・補足は台帳 dir の `approval.json`（schema `celeris.campaign-ledger-approval/1`）に置き、
//!   台帳の行（`deny_unknown_fields`）には入れない。固定済みの campaign binary も同じ台帳を読める。
//! - 承認の試行数（任意で tokens）は測定 ID の `measurement` 行の `caps` に入れる。上限は全行の最小なので増えない。
//!   違う承認の再記録は拒否する（引き上げは新しい台帳 path と manifest で行う）。同じ承認の再実行は冪等。
//! - 消費は書かない（runner の予約・精算だけが書く）。出力は台帳・manifest・実効の上限、確定消費・残り試行数。
//! - 入力の欠け（空の source/scope/expiry、`YYYY-MM-DD` でない期限、試行数 0）は何も書かずに失敗する。
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::manifest::{CampaignManifest, GlobalCap, Share, check_shares};

pub const GLOBAL_LEDGER_FILE: &str = "global-ledger.jsonl";
pub const GLOBAL_LEDGER_SCHEMA: &str = "celeris.campaign-global-ledger/2";
/// 旧形式（測定 ID ごとに確定消費 + 予約を 1 つの値で上書き）。読取のみ。
pub const GLOBAL_LEDGER_SCHEMA_V1: &str = "celeris.campaign-global-ledger/1";

const KIND_MEASUREMENT: &str = "measurement";
const KIND_ATTEMPT: &str = "attempt";

/// 承認単位の上限。台帳全体と manifest の最小を取り、増やさない。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caps {
    pub tokens: u64,
    pub trials: u64,
    pub provider_slots: u32,
}

impl Caps {
    pub const UNLIMITED: Caps = Caps {
        tokens: u64::MAX,
        trials: u64::MAX,
        provider_slots: u32::MAX,
    };

    pub fn min(self, o: Caps) -> Caps {
        Caps {
            tokens: self.tokens.min(o.tokens),
            trials: self.trials.min(o.trials),
            provider_slots: self.provider_slots.min(o.provider_slots),
        }
    }

    /// manifest が宣言する上限（tokens = budget.max_tokens、試行 = セットの approved_trials の最小、
    /// provider 枠 = budget.max_parallel）。
    pub fn of(m: &CampaignManifest) -> Caps {
        Caps {
            tokens: m.budget.max_tokens,
            trials: approved_trials(m),
            provider_slots: m.budget.max_parallel,
        }
    }
}

/// 旧 `/1` 行（読取のみ）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyRow {
    pub schema: String,
    pub measurement_id: String,
    pub manifest_sha256: String,
    /// 確定消費 + in-flight の予約の混在。
    pub tokens: u64,
    pub wall_secs: u64,
    pub trials: u64,
    /// 宣言 max_parallel（実際の占有ではない）。解放の記録が無い。
    pub provider_slots: u32,
    #[serde(default)]
    pub cap_tokens: Option<u64>,
    #[serde(default)]
    pub cap_trials: Option<u64>,
    #[serde(default)]
    pub cap_provider_slots: Option<u32>,
}

/// 確定消費（runner が自分の記録から再集計した値。in-flight の予約を含めない）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Consumed {
    pub tokens: u64,
    pub wall_secs: u64,
    /// 実際に動いた試行数（case × repetition。retry は 1 試行）。
    pub trials: u64,
}

impl Consumed {
    fn max(self, o: Consumed) -> Consumed {
        Consumed {
            tokens: self.tokens.max(o.tokens),
            wall_secs: self.wall_secs.max(o.wall_secs),
            trials: self.trials.max(o.trials),
        }
    }
}

/// 測定 ID の行（`kind` = measurement）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasurementRow {
    pub measurement_id: String,
    pub manifest_sha256: String,
    /// 確定消費（単調。下げない）。
    pub consumed: Consumed,
    /// 投入時に取った取り分（tokens・trials）。実消費がこれより少なくても `release_allocation` までは持つ。
    pub allocation: Share,
    /// この測定 ID が採った上限（台帳全体の最小に入る）。
    pub caps: Caps,
    /// 旧 `/1` 行から引き継いだ provider 枠（解放の記録が無いので占有のまま）。
    pub legacy_slots: u32,
}

/// 試行 identity。同じ identity の再予約・再精算は置換、別 attempt は別の消費。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptId {
    pub measurement_id: String,
    /// 試行（arm・case・repetition 等の安定な文字列）。試行数は異なる trial の数で数える。
    pub trial: String,
    /// 実行 attempt の番号（retry ごとに増やす）。
    pub attempt: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptState {
    /// 予約を fsync 済み。投入したかは未確定（再開では投入済みかもしれないとみなす）。
    Reserved,
    /// 実行器へ投入済み。終了は未確認。
    Submitted,
    /// 終了を確認して精算済み（provider 枠を解放、usage が確定消費）。
    Settled,
}

impl AttemptState {
    pub fn outstanding(self) -> bool {
        self != AttemptState::Settled
    }
}

/// 精算した消費。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub tokens: u64,
    pub wall_secs: u64,
    /// 試行が実際に動いたか（動いていなければ試行数に数えない）。
    pub ran: bool,
}

impl Usage {
    fn max(self, o: Usage) -> Usage {
        Usage {
            tokens: self.tokens.max(o.tokens),
            wall_secs: self.wall_secs.max(o.wall_secs),
            ran: self.ran || o.ran,
        }
    }
}

/// 試行 identity の行（`kind` = attempt）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptRow {
    pub id: AttemptId,
    pub manifest_sha256: String,
    pub state: AttemptState,
    /// 未精算の間に押さえる tokens（その試行の上限）。
    pub reserved_tokens: u64,
    /// 未精算の間に押さえる provider 枠。
    pub provider_slots: u32,
    /// 精算後の確定消費（未精算の間は 0）。
    pub usage: Usage,
    /// 予約時に採った上限（台帳全体の最小に入る）。
    pub caps: Caps,
}

/// 予約の要求。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReserveRequest {
    pub id: AttemptId,
    pub manifest_sha256: String,
    /// 投入する manifest の上限（台帳の既存上限より大きくても効かない）。
    pub caps: Caps,
    pub tokens: u64,
    pub provider_slots: u32,
}

/// `reserve` の結果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reserved {
    /// 新しく予約して fsync した。
    New,
    /// 同じ identity の未精算予約が既にある（冪等。資源を足さない）。再開側は状態に応じて続ける。
    Existing(AttemptState),
}

/// 予約を拒否した資源。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    Tokens,
    Trials,
    ProviderSlots,
}

impl Resource {
    pub fn name(self) -> &'static str {
        match self {
            Resource::Tokens => "tokens",
            Resource::Trials => "trials",
            Resource::ProviderSlots => "provider_slots",
        }
    }
}

/// 予約の拒否。`outstanding` は台帳全体に未精算の予約（または旧行の枠）が残っているか（待てば空く可能性）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rejection {
    pub resource: Resource,
    pub outstanding: bool,
    pub msg: String,
}

/// runner の 1 試行の投入許可の要求（attempt 番号は台帳が決める）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrialAdmission {
    pub measurement_id: String,
    pub trial: String,
    pub manifest_sha256: String,
    pub caps: Caps,
    pub tokens: u64,
    pub provider_slots: u32,
    /// 投入済みかもしれない試行を検査なしで Submitted として載せる。
    pub adopt: bool,
}

/// `admit_trial` の結果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admission {
    /// 予約がある（`existing` は既にあった未精算 attempt の状態。None は今回の新規予約）。`tokens` は予約額。
    Granted {
        id: AttemptId,
        existing: Option<AttemptState>,
        tokens: u64,
    },
    Rejected(Rejection),
}

/// 実行器で確かめた終了。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Terminal {
    /// 投入されていないと確かめた（Reserved のときだけ。消費 0 で解放）。
    NotSubmitted,
    /// 終了を確かめた。消費を確定する。
    Finished(Usage),
}

/// 測定 ID ごとの現在の占有（確定消費 + 未精算予約。取り分より小さければ取り分）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeasurementView {
    pub measurement_id: String,
    /// 全体台帳で押さえている tokens（max(取り分, 確定消費 + 未精算予約)）。
    pub tokens: u64,
    pub wall_secs: u64,
    /// 全体台帳で押さえている試行数（max(取り分, 確定 + 未精算の新しい試行)）。
    pub trials: u64,
    /// 未終了予約の provider 枠 + 旧行の枠。
    pub provider_slots: u32,
    /// 確定消費（publish の単調値と精算済み attempt の合計の大きい方）。
    pub consumed: Consumed,
    pub allocation: Share,
    /// この測定 ID の行が採った上限（無ければ無制限）。
    pub caps: Caps,
    /// 未精算の attempt の数。
    pub outstanding: usize,
}

/// 台帳を畳み込んだ現在値。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerState {
    /// 全行（置換された古い行を含む）の上限の最小。
    pub caps: Caps,
    pub measurements: BTreeMap<String, MeasurementRow>,
    pub attempts: BTreeMap<AttemptId, AttemptRow>,
    /// ファイルが改行で終わっているか（途中で切れた行の後ろに追記しない）。
    clean_tail: bool,
}

impl LedgerState {
    fn empty() -> Self {
        Self {
            caps: Caps::UNLIMITED,
            measurements: BTreeMap::new(),
            attempts: BTreeMap::new(),
            clean_tail: true,
        }
    }

    fn ids(&self) -> BTreeSet<String> {
        self.measurements
            .keys()
            .cloned()
            .chain(self.attempts.keys().map(|a| a.measurement_id.clone()))
            .collect()
    }

    /// 測定 ID の占有。`extra` は検査用に足す未精算予約、`allocation` は取り分の差し替え。
    fn view_with(
        &self,
        id: &str,
        extra: Option<&AttemptRow>,
        allocation: Option<Share>,
    ) -> MeasurementView {
        let row = self.measurements.get(id);
        let mut settled = Consumed::default();
        let (mut held_tokens, mut slots, mut outstanding) = (0u64, 0u32, 0usize);
        let (mut ran, mut open) = (BTreeSet::new(), BTreeSet::new());
        let attempts = self
            .attempts
            .values()
            .filter(|a| a.id.measurement_id == id)
            .filter(|a| extra.is_none_or(|e| e.id != a.id))
            .chain(extra);
        for a in attempts {
            if a.state.outstanding() {
                held_tokens = held_tokens.saturating_add(a.reserved_tokens);
                slots = slots.saturating_add(a.provider_slots);
                outstanding += 1;
                open.insert(a.id.trial.as_str());
            } else {
                settled.tokens = settled.tokens.saturating_add(a.usage.tokens);
                settled.wall_secs = settled.wall_secs.saturating_add(a.usage.wall_secs);
                if a.usage.ran {
                    ran.insert(a.id.trial.as_str());
                }
            }
        }
        settled.trials = ran.len() as u64;
        let published = row.map(|r| r.consumed).unwrap_or_default();
        let consumed = published.max(settled);
        let new_trials = open.difference(&ran).count() as u64;
        let allocation =
            allocation.unwrap_or_else(|| row.map(|r| r.allocation).unwrap_or_default());
        MeasurementView {
            measurement_id: id.to_string(),
            tokens: allocation
                .tokens
                .max(consumed.tokens.saturating_add(held_tokens)),
            wall_secs: consumed.wall_secs,
            trials: allocation
                .trials
                .max(consumed.trials.saturating_add(new_trials)),
            provider_slots: slots.saturating_add(row.map_or(0, |r| r.legacy_slots)),
            consumed,
            allocation,
            caps: row.map_or(Caps::UNLIMITED, |r| r.caps),
            outstanding,
        }
    }

    pub fn view(&self, id: &str) -> MeasurementView {
        self.view_with(id, None, None)
    }

    /// `id` 以外の測定 ID の占有の合計（provider 枠も合算）。
    pub fn others(&self, id: &str) -> Others {
        let mut o = Others::default();
        for other in self.ids().iter().filter(|x| x.as_str() != id) {
            let v = self.view(other);
            o.tokens = o.tokens.saturating_add(v.tokens);
            o.wall_secs = o.wall_secs.saturating_add(v.wall_secs);
            o.trials = o.trials.saturating_add(v.trials);
            o.provider_slots = o.provider_slots.saturating_add(v.provider_slots);
        }
        o
    }
}

#[derive(Clone, Debug)]
pub struct GlobalLedger {
    path: PathBuf,
}

/// 自分以外の測定 ID の合計（確定消費 + 未精算予約。provider 枠は未終了予約の合計）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Others {
    pub tokens: u64,
    pub wall_secs: u64,
    pub trials: u64,
    pub provider_slots: u32,
}

/// runner が公開する確定消費（`record` の引数）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Published {
    pub measurement_id: String,
    pub manifest_sha256: String,
    pub consumed: Consumed,
    pub caps: Caps,
}

/// 選択したセットの取り分の合計（宣言したセットだけ。無ければ 0）。
fn own_allocation(m: &CampaignManifest) -> Share {
    let mut t = Share::default();
    for (name, set) in &m.experiment_sets {
        if !m.selected_sets.is_empty() && !m.selected_sets.contains(name) {
            continue;
        }
        if let Some(a) = set.fixed.allocation {
            t.tokens = t.tokens.saturating_add(a.tokens);
            t.trials = t.trials.saturating_add(a.trials);
        }
    }
    t
}

/// 承認単位の試行数上限。宣言が無ければ無制限。
fn approved_trials(m: &CampaignManifest) -> u64 {
    m.experiment_sets
        .values()
        .filter_map(|s| s.fixed.approved_trials)
        .min()
        .unwrap_or(u64::MAX)
}

fn parse<T: DeserializeOwned>(mut v: Value, at: &str) -> Result<T, String> {
    if let Some(o) = v.as_object_mut() {
        o.remove("schema");
        o.remove("kind");
    }
    serde_json::from_value(v).map_err(|e| format!("{at}: {e}"))
}

fn line_of<T: Serialize>(kind: &str, row: &T) -> Result<String, String> {
    let mut v = serde_json::to_value(row).map_err(|e| e.to_string())?;
    let o = v.as_object_mut().ok_or("ledger row is not an object")?;
    o.insert("schema".into(), GLOBAL_LEDGER_SCHEMA.into());
    o.insert("kind".into(), kind.into());
    let mut line = serde_json::to_string(&v).map_err(|e| e.to_string())?;
    line.push('\n');
    Ok(line)
}

impl GlobalLedger {
    pub fn open(dir: &Path) -> Result<Self, String> {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(Self {
            path: dir.join(GLOBAL_LEDGER_FILE),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 台帳を全部読んで畳み込む。読めない行・不明 schema・不明 kind は破損として拒否する（黙って 0 にしない）。
    pub fn state(&self) -> Result<LedgerState, String> {
        let text = match fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(LedgerState::empty()),
            Err(e) => return Err(format!("{}: {e}", self.path.display())),
        };
        let mut s = LedgerState::empty();
        s.clean_tail = text.is_empty() || text.ends_with('\n');
        for (i, line) in text.lines().enumerate() {
            let at = format!("{}:{}", self.path.display(), i + 1);
            let v: Value = serde_json::from_str(line)
                .map_err(|e| format!("{at}: corrupt global ledger line: {e}"))?;
            match (v["schema"].as_str(), v["kind"].as_str()) {
                (Some(GLOBAL_LEDGER_SCHEMA_V1), _) => {
                    let r: LegacyRow =
                        serde_json::from_value(v).map_err(|e| format!("{at}: {e}"))?;
                    let caps = Caps {
                        tokens: r.cap_tokens.unwrap_or(u64::MAX),
                        trials: r.cap_trials.unwrap_or(u64::MAX),
                        provider_slots: r.cap_provider_slots.unwrap_or(u32::MAX),
                    };
                    s.caps = s.caps.min(caps);
                    s.measurements.insert(
                        r.measurement_id.clone(),
                        MeasurementRow {
                            measurement_id: r.measurement_id,
                            manifest_sha256: r.manifest_sha256,
                            consumed: Consumed {
                                tokens: r.tokens,
                                wall_secs: r.wall_secs,
                                trials: r.trials,
                            },
                            allocation: Share::default(),
                            caps,
                            legacy_slots: r.provider_slots,
                        },
                    );
                }
                (Some(GLOBAL_LEDGER_SCHEMA), Some(KIND_MEASUREMENT)) => {
                    let r: MeasurementRow = parse(v, &at)?;
                    s.caps = s.caps.min(r.caps);
                    s.measurements.insert(r.measurement_id.clone(), r);
                }
                (Some(GLOBAL_LEDGER_SCHEMA), Some(KIND_ATTEMPT)) => {
                    let r: AttemptRow = parse(v, &at)?;
                    s.caps = s.caps.min(r.caps);
                    s.attempts.insert(r.id.clone(), r);
                }
                (schema, kind) => {
                    return Err(format!(
                        "{at}: unknown global ledger schema/kind {schema:?}/{kind:?}"
                    ));
                }
            }
        }
        Ok(s)
    }

    /// 測定 ID → 現在の占有。
    pub fn current(&self) -> Result<BTreeMap<String, MeasurementView>, String> {
        let s = self.state()?;
        Ok(s.ids()
            .into_iter()
            .map(|id| (id.clone(), s.view(&id)))
            .collect())
    }

    pub fn others(&self, measurement_id: &str) -> Result<Others, String> {
        Ok(self.state()?.others(measurement_id))
    }

    /// 台帳全体の排他（flock。process が落ちても残らない）。
    fn lock(&self) -> Result<fs::File, String> {
        let p = self.path.with_extension("lock");
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&p)
            .map_err(|e| format!("{}: {e}", p.display()))?;
        f.lock().map_err(|e| format!("{}: {e}", p.display()))?;
        Ok(f)
    }

    /// 排他の中で 1 行追記して fsync する。途中で切れた行の後ろには書かない。
    fn append_locked(&self, s: &LedgerState, line: &str) -> Result<(), String> {
        if !s.clean_tail {
            return Err(format!(
                "{}: global ledger ends with a torn line; repair it before writing",
                self.path.display()
            ));
        }
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("{}: {e}", self.path.display()))?;
        f.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())
    }

    fn write_measurement(&self, s: &LedgerState, row: &MeasurementRow) -> Result<(), String> {
        if s.measurements.get(&row.measurement_id) == Some(row) {
            return Ok(());
        }
        self.append_locked(s, &line_of(KIND_MEASUREMENT, row)?)
    }

    fn write_attempt(&self, s: &LedgerState, row: &AttemptRow) -> Result<(), String> {
        if s.attempts.get(&row.id) == Some(row) {
            return Ok(());
        }
        self.append_locked(s, &line_of(KIND_ATTEMPT, row)?)
    }

    fn measurement_or_new(s: &LedgerState, id: &str, hash: &str) -> MeasurementRow {
        s.measurements
            .get(id)
            .cloned()
            .unwrap_or_else(|| MeasurementRow {
                measurement_id: id.to_string(),
                manifest_sha256: hash.to_string(),
                consumed: Consumed::default(),
                allocation: Share::default(),
                caps: Caps::UNLIMITED,
                legacy_slots: 0,
            })
    }

    /// 自分の確定消費を公開する。前の値より下げない（空 publish・古い再集計で消えない）。上限は増やさない。
    /// 未精算予約・精算済み attempt には触らない。
    pub fn record(&self, p: &Published) -> Result<(), String> {
        let _g = self.lock()?;
        let s = self.state()?;
        let mut row = Self::measurement_or_new(&s, &p.measurement_id, &p.manifest_sha256);
        row.manifest_sha256 = p.manifest_sha256.clone();
        row.consumed = row.consumed.max(p.consumed);
        row.caps = row.caps.min(p.caps);
        self.write_measurement(&s, &row)
    }

    pub fn published(m: &CampaignManifest, hash: &str, consumed: Consumed) -> Published {
        Published {
            measurement_id: m.campaign_id.clone(),
            manifest_sha256: hash.into(),
            consumed,
            caps: Caps::of(m),
        }
    }

    /// 1 試行 attempt の予約。最新の全行読取 → 上限の最小化 → tokens・試行数・provider 枠の検査 → 予約の追記と
    /// fsync を 1 つの排他で行う。同じ identity の未精算予約があれば何も足さず `Existing`。精算済みの identity は
    /// 拒否する（retry は attempt 番号を変える）。拒否したときは何も書かない（上限の最小化も残さない）。
    pub fn reserve(&self, req: &ReserveRequest) -> Result<Reserved, String> {
        let _g = self.lock()?;
        let s = self.state()?;
        let id = &req.id;
        if let Some(prev) = s.attempts.get(id) {
            if prev.state.outstanding() {
                return Ok(Reserved::Existing(prev.state));
            }
            return Err(format!(
                "global ledger: attempt {}/{}#{} is already settled; retry with a new attempt number",
                id.measurement_id, id.trial, id.attempt
            ));
        }
        let caps = s.caps.min(req.caps);
        let row = AttemptRow {
            id: id.clone(),
            manifest_sha256: req.manifest_sha256.clone(),
            state: AttemptState::Reserved,
            reserved_tokens: req.tokens,
            provider_slots: req.provider_slots,
            usage: Usage::default(),
            caps,
        };
        if let Err(r) = Self::check(&s, &row) {
            return Err(r.msg);
        }
        self.write_attempt(&s, &row)?;
        Ok(Reserved::New)
    }

    /// `row` を足したときの tokens・試行数・provider 枠の検査（排他の中で最新の全行に対して呼ぶ）。
    fn check(s: &LedgerState, row: &AttemptRow) -> Result<(), Rejection> {
        let id = &row.id;
        let caps = row.caps;
        let o = s.others(&id.measurement_id);
        let mine = s.view_with(&id.measurement_id, Some(row), None);
        let outstanding = s.attempts.values().any(|a| a.state.outstanding())
            || s.measurements.values().any(|m| m.legacy_slots > 0);
        let reject = |resource: Resource, used: u128, cap: u128| {
            Err(Rejection {
                resource,
                outstanding,
                msg: format!(
                    "global ledger rejects {}/{}#{}: {} {used} would exceed approved cap {cap}",
                    id.measurement_id,
                    id.trial,
                    id.attempt,
                    resource.name()
                ),
            })
        };
        let tokens = o.tokens as u128 + mine.tokens as u128;
        if tokens > caps.tokens as u128 {
            return reject(Resource::Tokens, tokens, caps.tokens as u128);
        }
        let trials = o.trials as u128 + mine.trials as u128;
        if trials > caps.trials as u128 {
            return reject(Resource::Trials, trials, caps.trials as u128);
        }
        let slots = o.provider_slots as u128 + mine.provider_slots as u128;
        if slots > caps.provider_slots as u128 {
            return reject(Resource::ProviderSlots, slots, caps.provider_slots as u128);
        }
        Ok(())
    }

    /// runner の 1 試行の投入許可（毎投入の前に呼ぶ）。最新の全行読取 → attempt 番号の決定 → 上限の最小化 →
    /// 検査 → 予約の追記と fsync を 1 つの排他で行う。
    /// - 同じ試行の未精算 attempt があればそれを返す（冪等。資源を足さない。同一冪等キーの続き）。
    /// - 無ければ精算済み attempt の次の番号で新しく予約する（retry。前の attempt の確定消費は残る）。
    /// - `adopt` は投入済みかもしれない試行（ローカル台帳に task がある・投入結果不明）を、上限検査で落とさず
    ///   Submitted として台帳に載せる（実在する消費を数えないより安全）。
    ///
    /// 拒否は `Admission::Rejected`（何も書かない）。I/O・破損は Err。
    pub fn admit_trial(&self, a: &TrialAdmission) -> Result<Admission, String> {
        let _g = self.lock()?;
        let s = self.state()?;
        let mine = s
            .attempts
            .values()
            .filter(|r| r.id.measurement_id == a.measurement_id && r.id.trial == a.trial);
        let mut next = 1u32;
        let mut settled: Option<&AttemptRow> = None;
        for r in mine {
            if r.state.outstanding() {
                return Ok(Admission::Granted {
                    id: r.id.clone(),
                    existing: Some(r.state),
                    tokens: r.reserved_tokens,
                });
            }
            next = next.max(r.id.attempt.saturating_add(1));
            if settled.is_none_or(|p| p.id.attempt < r.id.attempt) {
                settled = Some(r);
            }
        }
        // 投入済みの試行（adopt）を追っている最中に、その attempt だけが先に精算されていた（全体 → ローカルの順で
        // 止まった）。新しい attempt を切ると同じ実行を二重に予約・計上するので、精算済みの attempt を続ける
        // （確定消費は精算済みの値のまま。後の精算は置換で増えない）。
        if a.adopt
            && let Some(r) = settled
        {
            return Ok(Admission::Granted {
                id: r.id.clone(),
                existing: Some(r.state),
                tokens: r.reserved_tokens,
            });
        }
        let row = AttemptRow {
            id: AttemptId {
                measurement_id: a.measurement_id.clone(),
                trial: a.trial.clone(),
                attempt: next,
            },
            manifest_sha256: a.manifest_sha256.clone(),
            state: if a.adopt {
                AttemptState::Submitted
            } else {
                AttemptState::Reserved
            },
            reserved_tokens: a.tokens,
            provider_slots: a.provider_slots,
            usage: Usage::default(),
            caps: s.caps.min(a.caps),
        };
        if !a.adopt
            && let Err(r) = Self::check(&s, &row)
        {
            return Ok(Admission::Rejected(r));
        }
        self.write_attempt(&s, &row)?;
        Ok(Admission::Granted {
            id: row.id,
            existing: None,
            tokens: row.reserved_tokens,
        })
    }

    /// attempt の現在の行（無ければ None）。
    pub fn attempt(&self, id: &AttemptId) -> Result<Option<AttemptRow>, String> {
        Ok(self.state()?.attempts.get(id).cloned())
    }

    /// 測定 ID の未精算 attempt（再開時の照合用）。
    pub fn outstanding(&self, measurement_id: &str) -> Result<Vec<AttemptRow>, String> {
        Ok(self
            .state()?
            .attempts
            .into_values()
            .filter(|r| r.id.measurement_id == measurement_id && r.state.outstanding())
            .collect())
    }

    /// 実行器へ投入したことを記録する（Reserved → Submitted。Submitted なら何もしない）。
    pub fn mark_submitted(&self, id: &AttemptId) -> Result<(), String> {
        let _g = self.lock()?;
        let s = self.state()?;
        let mut row = Self::attempt_of(&s, id)?;
        match row.state {
            AttemptState::Reserved => {
                row.state = AttemptState::Submitted;
                self.write_attempt(&s, &row)
            }
            AttemptState::Submitted => Ok(()),
            AttemptState::Settled => Err(format!(
                "global ledger: attempt {}/{}#{} is already settled",
                id.measurement_id, id.trial, id.attempt
            )),
        }
    }

    /// 終了を確かめた attempt を精算する（provider 枠と予約 tokens を解放し、usage を確定消費にする）。
    /// 同じ identity の再精算は置換（各欄の大きい方。二重加算せず、空の再精算で消費を消さない）。
    /// `NotSubmitted` は Reserved のときだけ（投入済みの attempt を消費 0 で解放しない）。
    pub fn settle(&self, id: &AttemptId, t: Terminal) -> Result<(), String> {
        let _g = self.lock()?;
        let s = self.state()?;
        let mut row = Self::attempt_of(&s, id)?;
        let usage = match (row.state, t) {
            (AttemptState::Reserved, Terminal::NotSubmitted) => Usage::default(),
            (_, Terminal::NotSubmitted) => {
                return Err(format!(
                    "global ledger: attempt {}/{}#{} is {:?}; cannot release it as not submitted",
                    id.measurement_id, id.trial, id.attempt, row.state
                ));
            }
            (AttemptState::Settled, Terminal::Finished(u)) => row.usage.max(u),
            (_, Terminal::Finished(u)) => u,
        };
        row.state = AttemptState::Settled;
        row.usage = usage;
        self.write_attempt(&s, &row)
    }

    fn attempt_of(s: &LedgerState, id: &AttemptId) -> Result<AttemptRow, String> {
        s.attempts.get(id).cloned().ok_or_else(|| {
            format!(
                "global ledger: unknown attempt {}/{}#{} (reserve it first)",
                id.measurement_id, id.trial, id.attempt
            )
        })
    }

    /// 測定 ID の取り分を確定消費まで戻す（測定の終わり）。未精算の attempt が残っていれば拒否する。
    pub fn release_allocation(&self, measurement_id: &str) -> Result<(), String> {
        let _g = self.lock()?;
        let s = self.state()?;
        let v = s.view(measurement_id);
        if v.outstanding > 0 {
            return Err(format!(
                "global ledger: {measurement_id} has {} unsettled attempts",
                v.outstanding
            ));
        }
        let Some(mut row) = s.measurements.get(measurement_id).cloned() else {
            return Ok(());
        };
        row.allocation = Share::default();
        self.write_measurement(&s, &row)
    }

    /// 旧 `/1` 行の provider 枠を解放する。呼ぶ側（同じ測定 ID の持ち主）が自分の run 台帳に未完了の試行が
    /// 無いこと（終了の確認）を確かめた後だけ呼ぶ。確定消費は引き継ぐ。
    pub fn confirm_legacy_idle(&self, measurement_id: &str) -> Result<(), String> {
        let _g = self.lock()?;
        let s = self.state()?;
        let Some(mut row) = s.measurements.get(measurement_id).cloned() else {
            return Ok(());
        };
        row.legacy_slots = 0;
        self.write_measurement(&s, &row)
    }

    /// 投入前の拒否と測定単位の取り分の予約。全体上限は manifest と台帳の全行の上限の最小（別測定 ID の
    /// manifest で増やせない）。他の測定 ID の占有（確定消費 + 未精算予約・取り分）と自分の取り分（セットの
    /// `allocation`。宣言が無ければ tokens 0、trials は期待試行数）が収まらない・宣言 provider 枠が全体を超える
    /// 場合は Err。判定と取り分の記録は排他の中で行う。取り分は publish で消えない。
    pub fn admit_start(&self, m: &CampaignManifest) -> Result<Others, String> {
        let _g = self.lock()?;
        let s = self.state()?;
        let caps = s.caps.min(Caps::of(m));
        let o = s.others(&m.campaign_id);
        let mut own = own_allocation(m);
        if own.trials == 0 {
            own.trials = m.expected_trials();
        }
        let hash = m.compute_sha256().unwrap_or_default();
        let mut row = Self::measurement_or_new(&s, &m.campaign_id, &hash);
        row.allocation = Share {
            tokens: row.allocation.tokens.max(own.tokens),
            trials: row.allocation.trials.max(own.trials),
            provider_slots: 0,
        };
        row.caps = row.caps.min(caps);
        let mine = s.view_with(&m.campaign_id, None, Some(row.allocation));
        let consumed = Share {
            tokens: o.tokens,
            trials: o.trials,
            provider_slots: 0,
        };
        let shares = BTreeMap::from([(
            m.campaign_id.clone(),
            Share {
                tokens: mine.tokens,
                trials: mine.trials,
                provider_slots: m.budget.max_parallel,
            },
        )]);
        let total = Share {
            tokens: caps.tokens,
            trials: caps.trials,
            provider_slots: caps.provider_slots,
        };
        check_shares(&GlobalCap { total, consumed }, &shares)
            .map_err(|e| format!("global ledger rejects {}: {e}", m.campaign_id))?;
        if o.tokens >= caps.tokens {
            return Err(format!(
                "global ledger rejects {}: other measurements already used {} of {} tokens",
                m.campaign_id, o.tokens, caps.tokens
            ));
        }
        self.write_measurement(&s, &row)?;
        Ok(o)
    }
}

/// 承認の記録の file 名（台帳 dir の中。台帳の行には入れない: 行の schema は承認の出典を持たない）。
pub const APPROVAL_FILE: &str = "approval.json";
pub const APPROVAL_SCHEMA: &str = "celeris.campaign-ledger-approval/1";

/// 人の承認（`ledger-init` の入力）。秘密値は持たない。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    /// 誰がいつ承認したか。
    pub source: String,
    /// 承認の範囲（case・arm・model 等）。
    pub scope: String,
    /// 期限（`YYYY-MM-DD`）。
    pub expiry: String,
    /// 承認した試行数（台帳の上限 `caps.trials` になる。増やせない）。
    pub approved_trials: u64,
    /// 承認した tokens の上限。無ければ台帳は tokens を制限しない（manifest の budget が効く）。
    #[serde(default)]
    pub max_tokens: Option<u64>,
    /// 停止条件などの補足。
    #[serde(default)]
    pub note: Option<String>,
}

impl Approval {
    pub fn validate(&self) -> Result<(), String> {
        for (name, v) in [
            ("source", &self.source),
            ("scope", &self.scope),
            ("expiry", &self.expiry),
        ] {
            if v.trim().is_empty() {
                return Err(format!("approval: {name} is empty"));
            }
        }
        let bad = || format!("approval: expiry {:?} is not YYYY-MM-DD", self.expiry);
        let parts: Vec<&str> = self.expiry.split('-').collect();
        let [y, mo, d] = parts.as_slice() else {
            return Err(bad());
        };
        if (y.len(), mo.len(), d.len()) != (4, 2, 2) {
            return Err(bad());
        }
        let n = |x: &str| x.parse::<u8>().map_err(|_| bad());
        let month = time::Month::try_from(n(mo)?).map_err(|_| bad())?;
        let year = y.parse::<i32>().map_err(|_| bad())?;
        time::Date::from_calendar_date(year, month, n(d)?).map_err(|_| bad())?;
        if self.approved_trials == 0 {
            return Err("approval: approved_trials must be > 0".into());
        }
        Ok(())
    }

    fn caps(&self) -> Caps {
        Caps {
            tokens: self.max_tokens.unwrap_or(u64::MAX),
            trials: self.approved_trials,
            provider_slots: u32::MAX,
        }
    }
}

/// `ledger-init` の結果。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LedgerInit {
    pub ledger_path: PathBuf,
    pub approval_path: PathBuf,
    pub measurement_id: String,
    /// 台帳全体の上限（全行の最小）。
    pub ledger_caps: Caps,
    /// manifest が宣言する上限。
    pub manifest_caps: Caps,
    /// 投入で効く上限（台帳と manifest の最小）。
    pub effective_caps: Caps,
    /// 台帳の全測定 ID の確定消費の合計。
    pub consumed: Consumed,
    /// 承認の残り試行数（`approved_trials` − 確定消費）。
    pub remaining_trials_approval: u64,
    /// 投入で効く残り試行数（`effective_caps.trials` − 確定消費）。
    pub remaining_trials_effective: u64,
    /// この manifest の期待試行数。
    pub expected_trials: u64,
}

impl GlobalLedger {
    /// 承認を台帳に記録する（冪等）。承認の出典・範囲・期限は台帳 dir の `approval.json`、上限は測定 ID の行の
    /// `caps` に入れる。既に違う承認が記録されていれば拒否する（上限は増えないので、引き上げは新しい台帳で行う）。
    /// 消費は書かない（試行の消費は runner の予約・精算だけが書く）。
    pub fn init_approval(&self, m: &CampaignManifest, a: &Approval) -> Result<LedgerInit, String> {
        a.validate()?;
        let _g = self.lock()?;
        let dir = self
            .path
            .parent()
            .ok_or("global ledger path has no parent")?;
        let approval_path = dir.join(APPROVAL_FILE);
        let mut doc = serde_json::to_value(a).map_err(|e| e.to_string())?;
        if let Some(o) = doc.as_object_mut() {
            o.insert("schema".into(), APPROVAL_SCHEMA.into());
        }
        match fs::read(&approval_path) {
            Ok(b) => {
                let prev: Value = serde_json::from_slice(&b)
                    .map_err(|e| format!("{}: {e}", approval_path.display()))?;
                if prev != doc {
                    return Err(format!(
                        "{}: a different approval is already recorded; caps never increase, use a new ledger",
                        approval_path.display()
                    ));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let tmp = approval_path.with_extension("json.tmp");
                let body = serde_json::to_vec_pretty(&doc).map_err(|e| e.to_string())?;
                let mut f =
                    fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
                f.write_all(&body).map_err(|e| e.to_string())?;
                f.sync_all().map_err(|e| e.to_string())?;
                fs::rename(&tmp, &approval_path)
                    .map_err(|e| format!("{}: {e}", approval_path.display()))?;
            }
            Err(e) => return Err(format!("{}: {e}", approval_path.display())),
        }
        let s = self.state()?;
        let hash = m.compute_sha256()?;
        let mut row = Self::measurement_or_new(&s, &m.campaign_id, &hash);
        row.caps = row.caps.min(a.caps());
        self.write_measurement(&s, &row)?;
        let s = self.state()?;
        let mut consumed = Consumed::default();
        for id in s.ids() {
            let c = s.view(&id).consumed;
            consumed.tokens = consumed.tokens.saturating_add(c.tokens);
            consumed.wall_secs = consumed.wall_secs.saturating_add(c.wall_secs);
            consumed.trials = consumed.trials.saturating_add(c.trials);
        }
        let manifest_caps = Caps::of(m);
        let effective_caps = s.caps.min(manifest_caps);
        Ok(LedgerInit {
            ledger_path: self.path.clone(),
            approval_path,
            measurement_id: m.campaign_id.clone(),
            ledger_caps: s.caps,
            manifest_caps,
            effective_caps,
            consumed,
            remaining_trials_approval: a.approved_trials.saturating_sub(consumed.trials),
            remaining_trials_effective: effective_caps.trials.saturating_sub(consumed.trials),
            expected_trials: m.expected_trials(),
        })
    }
}
