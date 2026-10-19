//! 副比較（B1/C1）の系列台帳。承認総枠は測定 ID ごとに 1 つで、既消費は測定 ID ごとに固定した 1 か所の台帳へ
//! 追記し、そこから合算する。出力先（out）・manifest hash・版を変えても台帳の位置は変わらず、消費を失わない。
//!
//! 配置: `<series_root>/<measurement_id>/{series.json, series-ledger.jsonl, series.lock, runs/}`。
//! - `series.json`: 測定 ID・campaign_id・許可 out root（`runs/`）を固定する（`series-init` が作る。run は作らない）。
//! - `series-ledger.jsonl`: 追記専用。out ごとの消費（確定 + 予約）の snapshot で、同じ out は新しい行が勝つ（再開・重複で二重計上しない）。
//! - `series.lock`: run 中の排他 lock。同じ系列の run が同時に動くと枠を二重に使えるので、2 本目は拒否する。
//!
//! 未設定・欠損・破損・読取不能は既消費 0 にせず拒否する。out は `runs/` の下（`..`・symlink の逸脱を含めて外は不可）だけ。
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    budget::{self, Spend},
    ledger::Ledger,
    manifest::{Budget, CampaignManifest},
    record::{META_FILE, TRIALS_FILE},
};

pub const SERIES_SCHEMA: &str = "celeris.campaign-series/1";
pub const ENTRY_SCHEMA: &str = "celeris.campaign-series-entry/1";
pub const SERIES_FILE: &str = "series.json";
pub const LEDGER_FILE: &str = "series-ledger.jsonl";
pub const LOCK_FILE: &str = "series.lock";
pub const RUNS_DIR: &str = "runs";

/// 測定 ID ごとに固定した系列台帳の位置。campaign lock（`series` 欄）に封印され、launcher.env の `SERIES_ROOT` から作る。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeriesPin {
    pub measurement_id: String,
    /// 系列 dir の絶対 path（`<series_root>/<measurement_id>`）。
    pub dir: String,
    /// 許可する出力 root の絶対 path（`<dir>/runs`）。
    pub out_root: String,
}

impl SeriesPin {
    pub fn new(series_root: &Path, measurement_id: &str) -> Result<Self, String> {
        if !series_root.is_absolute() {
            return Err(format!(
                "series root must be an absolute path: {}",
                series_root.display()
            ));
        }
        if series_root.components().any(|c| c == Component::ParentDir) {
            return Err(format!(
                "series root must not contain '..': {}",
                series_root.display()
            ));
        }
        let id = measurement_id;
        if id.is_empty() || id == "." || id == ".." || id.contains(['/', '\\', '\0']) {
            return Err(format!(
                "measurement id {id:?} is not a usable path component"
            ));
        }
        let dir = series_root.join(id);
        Ok(Self {
            measurement_id: id.into(),
            out_root: dir.join(RUNS_DIR).to_string_lossy().into_owned(),
            dir: dir.to_string_lossy().into_owned(),
        })
    }

    fn dir(&self) -> PathBuf {
        PathBuf::from(&self.dir)
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct SeriesMeta {
    schema: String,
    measurement_id: String,
    campaign_id: String,
    dir: String,
    out_root: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    schema: String,
    out: String,
    campaign_sha256: String,
    tokens: u64,
    wall_secs: u64,
    held_tokens: u64,
    held_wall_secs: u64,
    in_flight: usize,
}

/// 開いた系列台帳。保持中は同じ系列の他の run を拒否する（drop で解放）。
#[derive(Debug)]
pub struct Series {
    pin: SeriesPin,
    entries: BTreeMap<String, Entry>,
    _lock: File,
}

/// 系列の作成（冪等）。`imports` は引継ぎ元の既存 out dir（hash 変更前の run など）で、その消費を台帳へ追記する。
pub fn init(
    pin: &SeriesPin,
    campaign_id: &str,
    imports: &[PathBuf],
    b: &Budget,
) -> Result<(), String> {
    let dir = pin.dir();
    fs::create_dir_all(PathBuf::from(&pin.out_root))
        .map_err(|e| format!("{}: {e}", pin.out_root))?;
    let meta = SeriesMeta {
        schema: SERIES_SCHEMA.into(),
        measurement_id: pin.measurement_id.clone(),
        campaign_id: campaign_id.into(),
        dir: pin.dir.clone(),
        out_root: pin.out_root.clone(),
    };
    let path = dir.join(SERIES_FILE);
    if path.exists() {
        check_meta(pin, campaign_id)?;
    } else {
        let mut text = serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?;
        text.push('\n');
        write_new(&path, text.as_bytes())?;
    }
    let lp = dir.join(LEDGER_FILE);
    if !lp.exists() {
        write_new(&lp, b"")?;
    }
    let mut s = open(pin, campaign_id)?;
    for out in imports {
        let (hash, sp) = live_spend(out, b)?
            .ok_or_else(|| format!("{}: no run-meta.json; nothing to carry over", out.display()))?;
        let meta: serde_json::Value = read_json(&out.join(META_FILE))?;
        if meta["campaign_id"].as_str() != Some(campaign_id) {
            return Err(format!(
                "{}: campaign_id differs from the series {campaign_id}",
                out.display()
            ));
        }
        let canon = fs::canonicalize(out).map_err(|e| format!("{}: {e}", out.display()))?;
        s.append(&canon, &hash, &sp)?;
    }
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(bytes).map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

fn check_meta(pin: &SeriesPin, campaign_id: &str) -> Result<(), String> {
    let path = pin.dir().join(SERIES_FILE);
    let m: SeriesMeta = read_json(&path).map_err(|e| format!("series ledger unreadable: {e}"))?;
    if m.schema != SERIES_SCHEMA
        || m.measurement_id != pin.measurement_id
        || m.campaign_id != campaign_id
        || m.dir != pin.dir
        || m.out_root != pin.out_root
    {
        return Err(format!(
            "{} does not match the pinned series (measurement {}, campaign {campaign_id}, {}); refusing",
            path.display(),
            pin.measurement_id,
            pin.dir
        ));
    }
    Ok(())
}

/// 系列台帳を開いて排他 lock を取る。無い・壊れている・読めない・別の系列を指すときは Err（既消費 0 にしない）。
pub fn open(pin: &SeriesPin, campaign_id: &str) -> Result<Series, String> {
    let dir = pin.dir();
    if !dir.join(SERIES_FILE).is_file() {
        return Err(format!(
            "series ledger {} is missing (run series-init for measurement {}); refusing to treat prior spend as 0",
            dir.display(),
            pin.measurement_id
        ));
    }
    check_meta(pin, campaign_id)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(LOCK_FILE))
        .map_err(|e| format!("{}: {e}", dir.join(LOCK_FILE).display()))?;
    lock.try_lock().map_err(|e| {
        format!(
            "another run of series {} is active or its lock is unusable ({e}); refusing",
            pin.measurement_id
        )
    })?;
    let lp = dir.join(LEDGER_FILE);
    let text = fs::read_to_string(&lp)
        .map_err(|e| format!("series ledger {} unreadable: {e}", lp.display()))?;
    if !text.is_empty() && !text.ends_with('\n') {
        return Err(format!(
            "series ledger {} has a torn last line",
            lp.display()
        ));
    }
    let mut entries = BTreeMap::new();
    for (i, l) in text.lines().enumerate() {
        let e: Entry = serde_json::from_str(l)
            .map_err(|e| format!("series ledger {} line {}: {e}", lp.display(), i + 1))?;
        if e.schema != ENTRY_SCHEMA {
            return Err(format!(
                "series ledger {} line {}: bad schema",
                lp.display(),
                i + 1
            ));
        }
        entries.insert(e.out.clone(), e);
    }
    Ok(Series {
        pin: pin.clone(),
        entries,
        _lock: lock,
    })
}

/// out が許可 root の真下か。`..`・symlink での逸脱、root 自身、root の外は Err。副作用なし。
pub fn confine(out: &Path, pin: &SeriesPin) -> Result<PathBuf, String> {
    let abs = if out.is_absolute() {
        out.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(out)
    };
    if abs.components().any(|c| c == Component::ParentDir) {
        return Err(format!("out {} must not contain '..'", out.display()));
    }
    let root = fs::canonicalize(&pin.out_root).map_err(|e| {
        format!(
            "allowed out root {} is unusable ({e}); run series-init",
            pin.out_root
        )
    })?;
    // 存在する最も深い祖先を実体化し（symlink を解く）、残りを後ろへ付ける。
    let mut base = abs.clone();
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    while !base.exists() {
        let Some(name) = base.file_name().map(|n| n.to_os_string()) else {
            return Err(format!("out {} has no existing ancestor", out.display()));
        };
        rest.push(name);
        if !base.pop() {
            break;
        }
    }
    let mut real = fs::canonicalize(&base).map_err(|e| format!("{}: {e}", base.display()))?;
    real.extend(rest.iter().rev());
    if real == root || !real.starts_with(&root) {
        return Err(format!(
            "out {} is outside the allowed output root {} for measurement {}; refusing before any effect",
            out.display(),
            root.display(),
            pin.measurement_id
        ));
    }
    Ok(real)
}

/// out の現在の消費（確定 + 予約）を、その out の記録・台帳から読む（読取専用）。run-meta が無ければ None。
fn live_spend(out: &Path, b: &Budget) -> Result<Option<(String, Spend)>, String> {
    let mp = out.join(META_FILE);
    if !mp.exists() {
        return Ok(None);
    }
    let meta: serde_json::Value = read_json(&mp)?;
    let hash = meta["campaign_sha256"]
        .as_str()
        .ok_or_else(|| format!("{}: campaign_sha256 missing", mp.display()))?
        .to_string();
    let records = budget::read_records(&out.join(TRIALS_FILE))?;
    let ledger = Ledger::open_readonly(out)?;
    let s = budget::spend(&records, &ledger, b, &hash, None);
    Ok(Some((hash, s)))
}

impl Series {
    /// 自分の out 以外の既消費。台帳の snapshot と、その out が今も読めるならその実測の大きい方（crash で snapshot が
    /// 古くても予約を落とさない。out が消えても snapshot が残る）。
    pub fn prior(&self, self_out: &Path, b: &Budget) -> Result<Spend, String> {
        let mut total = Spend::default();
        for (out, e) in &self.entries {
            if Path::new(out) == self_out {
                continue;
            }
            let (mut t, mut w, mut ht, mut hw, mut n) = (
                e.tokens,
                e.wall_secs,
                e.held_tokens,
                e.held_wall_secs,
                e.in_flight,
            );
            if let Some((_, l)) = live_spend(Path::new(out), b)? {
                t = t.max(l.tokens);
                w = w.max(l.wall_secs);
                ht = ht.max(l.held_tokens);
                hw = hw.max(l.held_wall_secs);
                n = n.max(l.in_flight);
            }
            total.tokens += t;
            total.wall_secs += w;
            total.held_tokens += ht;
            total.held_wall_secs += hw;
            total.in_flight += n;
        }
        Ok(total)
    }

    /// 自分の out の現在の消費を台帳へ追記する（out の実体が無ければ何もしない）。
    pub fn record(&mut self, out: &Path, b: &Budget) -> Result<(), String> {
        match live_spend(out, b)? {
            Some((hash, s)) => self.append(out, &hash, &s),
            None => Ok(()),
        }
    }

    fn append(&mut self, out: &Path, hash: &str, s: &Spend) -> Result<(), String> {
        let e = Entry {
            schema: ENTRY_SCHEMA.into(),
            out: out.to_string_lossy().into_owned(),
            campaign_sha256: hash.into(),
            tokens: s.tokens,
            wall_secs: s.wall_secs,
            held_tokens: s.held_tokens,
            held_wall_secs: s.held_wall_secs,
            in_flight: s.in_flight,
        };
        let mut line = serde_json::to_string(&e).map_err(|e| e.to_string())?;
        line.push('\n');
        let lp = self.pin.dir().join(LEDGER_FILE);
        let mut f = OpenOptions::new()
            .append(true)
            .open(&lp)
            .map_err(|e| format!("{}: {e}", lp.display()))?;
        f.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
        self.entries.insert(e.out.clone(), e);
        Ok(())
    }
}

/// 副比較の run 入口: 出力先の確認 → 台帳を開く（排他）→ 既消費。副作用（out 作成・executor）より前に呼ぶ。
pub fn begin(
    m: &CampaignManifest,
    pin: Option<&SeriesPin>,
    out: &Path,
) -> Result<Option<(Series, PathBuf, Spend)>, String> {
    if !m.is_side() {
        return Ok(None);
    }
    let pin = pin.ok_or(
        "side comparison (B1/C1) requires the pinned series ledger (--series-root and --measurement-id; \
SERIES_ROOT in launcher.env); refusing to treat prior spend as 0",
    )?;
    let real = confine(out, pin)?;
    let series = open(pin, &m.campaign_id)?;
    let prior = series.prior(&real, &m.budget)?;
    Ok(Some((series, real, prior)))
}
