//! 全体台帳（測定 ID を跨ぐ消費・予約・provider 枠）。ADR 2026-10-19-experiment-sets D5/D7。
//! 別測定 ID（A/B のセットと B/C のセット）を順に動かしても、既消費・予約・provider 枠を持ち越す。
//! 追記専用 JSONL。測定 ID ごとに最後の行が現在値（加算でなく上書きなので、retry の再集計で二重加算しない）。
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::manifest::{CampaignManifest, GlobalCap, Share, check_shares};

pub const GLOBAL_LEDGER_FILE: &str = "global-ledger.jsonl";
pub const GLOBAL_LEDGER_SCHEMA: &str = "celeris.campaign-global-ledger/1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalRow {
    pub schema: String,
    pub measurement_id: String,
    pub manifest_sha256: String,
    /// 確定消費 + in-flight の予約（この測定 ID 全体の現在値）。
    pub tokens: u64,
    pub wall_secs: u64,
    /// 実際に動いた試行数（case × repetition。retry は 1 試行）。
    pub trials: u64,
    pub provider_slots: u32,
    /// この測定 ID が投入時に採った上限（全体の最小を取り、別測定 ID で増やせない）。旧行は無制限。
    #[serde(default = "unlimited_u64")]
    pub cap_tokens: u64,
    #[serde(default = "unlimited_u64")]
    pub cap_trials: u64,
    #[serde(default = "unlimited_u32")]
    pub cap_provider_slots: u32,
}

fn unlimited_u64() -> u64 {
    u64::MAX
}

fn unlimited_u32() -> u32 {
    u32::MAX
}

#[derive(Clone, Debug)]
pub struct GlobalLedger {
    path: PathBuf,
}

/// 自分以外の測定 ID の合計（既消費 + 予約）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Others {
    pub tokens: u64,
    pub wall_secs: u64,
    pub trials: u64,
    pub provider_slots: u32,
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

impl GlobalLedger {
    pub fn open(dir: &Path) -> Result<Self, String> {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(Self {
            path: dir.join(GLOBAL_LEDGER_FILE),
        })
    }

    /// 測定 ID → 現在値（最後の行）。読めない行は台帳の破損として拒否する（黙って 0 にしない）。
    pub fn current(&self) -> Result<BTreeMap<String, GlobalRow>, String> {
        let text = match fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(e) => return Err(format!("{}: {e}", self.path.display())),
        };
        let mut out = BTreeMap::new();
        for (i, line) in text.lines().enumerate() {
            let r: GlobalRow = serde_json::from_str(line)
                .map_err(|e| format!("{}:{}: {e}", self.path.display(), i + 1))?;
            out.insert(r.measurement_id.clone(), r);
        }
        Ok(out)
    }

    pub fn others(&self, measurement_id: &str) -> Result<Others, String> {
        let mut o = Others::default();
        for r in self
            .current()?
            .values()
            .filter(|r| r.measurement_id != measurement_id)
        {
            o.tokens = o.tokens.saturating_add(r.tokens);
            o.wall_secs = o.wall_secs.saturating_add(r.wall_secs);
            o.trials = o.trials.saturating_add(r.trials);
            o.provider_slots = o.provider_slots.max(r.provider_slots);
        }
        Ok(o)
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

    /// 自分の現在値を記録する。前回と同じなら書かない。上限は既存行より増やさない。
    pub fn record(&self, row: &GlobalRow) -> Result<(), String> {
        let _g = self.lock()?;
        self.record_locked(row)
    }

    fn record_locked(&self, row: &GlobalRow) -> Result<(), String> {
        let cur = self.current()?;
        let mut row = row.clone();
        if let Some(prev) = cur.get(&row.measurement_id) {
            row.cap_tokens = row.cap_tokens.min(prev.cap_tokens);
            row.cap_trials = row.cap_trials.min(prev.cap_trials);
            row.cap_provider_slots = row.cap_provider_slots.min(prev.cap_provider_slots);
        }
        if cur.get(&row.measurement_id) == Some(&row) {
            return Ok(());
        }
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("{}: {e}", self.path.display()))?;
        let mut line = serde_json::to_string(&row).map_err(|e| e.to_string())?;
        line.push('\n');
        f.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())
    }

    pub fn row(m: &CampaignManifest, hash: &str, tokens: u64, wall: u64, trials: u64) -> GlobalRow {
        GlobalRow {
            schema: GLOBAL_LEDGER_SCHEMA.into(),
            measurement_id: m.campaign_id.clone(),
            manifest_sha256: hash.into(),
            tokens,
            wall_secs: wall,
            trials,
            provider_slots: m.budget.max_parallel,
            cap_tokens: m.budget.max_tokens,
            cap_trials: approved_trials(m),
            cap_provider_slots: m.budget.max_parallel,
        }
    }

    /// 投入前の拒否と予約。全体上限は manifest の budget と台帳に既にある上限の最小（別測定 ID の manifest で
    /// 増やせない）。他の測定 ID の既消費 + 予約と自分の取り分（セットの `allocation`。宣言が無ければ tokens 0、
    /// trials は期待試行数）が収まらない・provider 枠が全体を超える場合は Err。
    /// 判定と予約の記録は排他の中で行う（同時投入で両方通らない）。
    pub fn admit_start(&self, m: &CampaignManifest) -> Result<Others, String> {
        let _g = self.lock()?;
        let cur = self.current()?;
        let (mut cap_t, mut cap_n, mut cap_s) = (
            m.budget.max_tokens,
            approved_trials(m),
            m.budget.max_parallel,
        );
        for r in cur.values() {
            cap_t = cap_t.min(r.cap_tokens);
            cap_n = cap_n.min(r.cap_trials);
            cap_s = cap_s.min(r.cap_provider_slots);
        }
        let o = self.others(&m.campaign_id)?;
        let mut own = own_allocation(m);
        if own.trials == 0 {
            own.trials = m.expected_trials();
        }
        let consumed = Share {
            tokens: o.tokens,
            trials: o.trials,
            provider_slots: 0,
        };
        let shares = BTreeMap::from([(
            m.campaign_id.clone(),
            Share {
                provider_slots: m.budget.max_parallel,
                ..own
            },
        )]);
        let total = Share {
            tokens: cap_t,
            trials: cap_n,
            provider_slots: cap_s,
        };
        check_shares(&GlobalCap { total, consumed }, &shares)
            .map_err(|e| format!("global ledger rejects {}: {e}", m.campaign_id))?;
        if o.tokens >= cap_t {
            return Err(format!(
                "global ledger rejects {}: other measurements already used {} of {} tokens",
                m.campaign_id, o.tokens, cap_t
            ));
        }
        // 予約: 自分の現在値を取り分まで引き上げる（確定消費は下げない。次の記録で実測値に置き換わる）。
        let mut row = cur
            .get(&m.campaign_id)
            .cloned()
            .unwrap_or_else(|| Self::row(m, &m.compute_sha256().unwrap_or_default(), 0, 0, 0));
        row.cap_tokens = cap_t;
        row.cap_trials = cap_n;
        row.cap_provider_slots = cap_s;
        row.tokens = row.tokens.max(own.tokens);
        row.trials = row.trials.max(own.trials);
        self.record_locked(&row)?;
        Ok(o)
    }
}
