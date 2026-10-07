//! ADR 2026-10-07-build-tmp-hygiene D1.1〜D1.3: 共有 cargo target の掃除の**純粋な計画**。
//!
//! 走査（`stat`）・`.cargo-lock` の flock・rename・削除は I/O 層（`task_dispatch::target_sweep`）が持つ。
//! ここは「走査済みの項目」と「lock が取れなかった profile dir」と「今の時刻」と既定値を受け取り、
//! 消す項目と skip の理由を返すだけで、file system にも時計にも触らない（試験は決定的）。
//!
//! 規則（D1.2）の順:
//! 1. 古さ: 使用時刻が `max_age_days` より古い項目を消す。
//! 2. 上限: root の合計が `max_bytes_per_root` を超えたら、残りを使用時刻の古い順（同時刻は path の辞書順）に
//!    上限 × `target_ratio` 以下まで消す。
//! 3. 放置 target: どの profile も lock が取れ、全項目が `stale_target_days` より古い target dir を丸ごと消す。
//! 4. lock の取れない profile dir の項目は 1 つも消さない（`build_in_progress`）。
//! 5. それで下がりきらなければ `over_cap_unresolved`（消さないことを優先する）。
//! 6. symlink と root の外の path は計画に入れない（`symlink` / `outside_root`）。
//!
//! `deps`・`fingerprint`・`build` は同じ key（`<name>-<hash>`）のものを 1 つの単位として一緒に消す。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Serialize;

const DAY_SECS: u64 = 24 * 60 * 60;
const GIB: u64 = 1024 * 1024 * 1024;

/// 掃除の単位になる項目の種類（D1.1 の表）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// `deps/<name>-<hash>*`（file 1 つが 1 件。同じ key で束ねる）。
    Deps,
    /// `.fingerprint/<name>-<hash>/`。
    Fingerprint,
    /// `build/<name>-<hash>/`。
    Build,
    /// `incremental/<name>-<hash>/`。
    Incremental,
}

impl ItemKind {
    /// `deps`・`fingerprint`・`build` は同じ key を一緒に消す。`incremental` は単独。
    fn group(self) -> &'static str {
        match self {
            ItemKind::Deps | ItemKind::Fingerprint | ItemKind::Build => "crate",
            ItemKind::Incremental => "incremental",
        }
    }
}

/// 走査済みの項目 1 件（I/O 層が作る）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TargetSnapshot {
    /// 設定の root（`[maintenance.target_sweep] roots` の 1 つ）。
    pub root: PathBuf,
    /// root の直下（深さ 1）か `<root>/<repo-key>/wu-*`（深さ 2）の target dir。
    pub target_dir: PathBuf,
    /// `.cargo-lock` を持つ profile dir（`<target>/<profile>` か `<target>/<triple>/<profile>`）。
    pub profile_dir: PathBuf,
    /// 項目の path（`deps` は file、他は dir）。
    pub path: PathBuf,
    pub kind: ItemKind,
    /// `<name>-<hash>`。
    pub key: String,
    /// 実使用量（`st_blocks × 512` の合計）。
    pub bytes: u64,
    /// 項目に属する file・dir の `max(mtime, atime)`。
    pub used_at: SystemTime,
    /// 項目そのものが symlink（辿らない）。
    pub symlink: bool,
}

/// 規則の既定値（D1.3）。`roots` は呼び出し側（config）が渡す。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SweepParams {
    pub roots: Vec<PathBuf>,
    pub max_age_days: u64,
    pub max_bytes_per_root: u64,
    pub target_ratio: f64,
    pub stale_target_days: u64,
}

impl Default for SweepParams {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            max_age_days: 7,
            max_bytes_per_root: 120 * GIB,
            target_ratio: 0.8,
            stale_target_days: 14,
        }
    }
}

impl SweepParams {
    /// 上限の段で下げる先（上限 × ratio）。ratio は 0〜1 に丸める。
    pub fn cap_target_bytes(&self) -> u64 {
        let ratio = self.target_ratio.clamp(0.0, 1.0);
        (self.max_bytes_per_root as f64 * ratio) as u64
    }
}

/// 消す理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeleteReason {
    Age,
    Cap,
    StaleTarget,
}

/// 消さない理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    BuildInProgress,
    OutsideRoot,
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlannedDelete {
    pub root: PathBuf,
    /// 項目の path か、`stale_target` なら target dir。
    pub path: PathBuf,
    /// この削除で新たに減る量（先に計画した項目の分は数えない）。
    pub bytes: u64,
    pub reason: DeleteReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Skip {
    /// 項目の path。`build_in_progress` は profile dir 1 件にまとめる。
    pub path: PathBuf,
    pub reason: SkipReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RootSummary {
    pub root: PathBuf,
    pub before_bytes: u64,
    pub after_bytes: u64,
    pub over_cap_unresolved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct SweepPlan {
    pub delete: Vec<PlannedDelete>,
    pub skip: Vec<Skip>,
    pub roots: Vec<RootSummary>,
    /// どれかの root が上限 × ratio まで下がらなかった。
    pub over_cap_unresolved: bool,
}

impl SweepPlan {
    pub fn deleted_bytes(&self) -> u64 {
        self.delete.iter().map(|d| d.bytes).sum()
    }
}

/// `deps/` の file 名から項目の key（`<name>-<hash>`）を出す。
///
/// `libfoo-0123abcd.rlib`・`libfoo-0123abcd.rmeta`・`foo-0123abcd.d`・`foo-0123abcd`（試験 binary）・
/// `foo-0123abcd.foo.1a2b-cgu.0.rcgu.o` はどれも `foo-0123abcd`。library の成果物（`.rlib`・`.rmeta`・
/// `.so`・`.a`・`.dylib`）だけ先頭の `lib` を落とす（`.d` と binary には付かない）。
/// hash が 16 進でなければ `None`（掃除の単位にしない）。
pub fn deps_key(file_name: &str) -> Option<String> {
    let (stem, ext) = match file_name.split_once('.') {
        Some((stem, rest)) => (stem, rest.rsplit('.').next().unwrap_or(rest)),
        None => (file_name, ""),
    };
    let lib_artifact = matches!(ext, "rlib" | "rmeta" | "so" | "a" | "dylib");
    let stem = if lib_artifact {
        stem.strip_prefix("lib").unwrap_or(stem)
    } else {
        stem
    };
    item_key(stem).map(str::to_string)
}

/// dir 名（`.fingerprint/`・`build/`・`incremental/` の直下）が `<name>-<hash>` ならその key。
pub fn dir_key(dir_name: &str) -> Option<String> {
    item_key(dir_name).map(str::to_string)
}

fn item_key(s: &str) -> Option<&str> {
    let (name, hash) = s.rsplit_once('-')?;
    let valid = !name.is_empty() && !hash.is_empty() && hash.bytes().all(|b| b.is_ascii_hexdigit());
    valid.then_some(s)
}

/// `deps/` の file 名を key ごとに束ねる。key の出ない file は落とす。
pub fn group_deps_by_key<'a, I>(file_names: I) -> BTreeMap<String, Vec<&'a str>>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut groups: BTreeMap<String, Vec<&'a str>> = BTreeMap::new();
    for name in file_names {
        if let Some(key) = deps_key(name) {
            groups.entry(key).or_default().push(name);
        }
    }
    groups
}

/// `..` を含まず `root` の下にある（`root` そのものは含まない）。
fn strictly_inside(path: &Path, root: &Path) -> bool {
    !path.components().any(|c| matches!(c, Component::ParentDir))
        && path != root
        && path.starts_with(root)
}

/// 入力の検証（規則 6）: root が設定にあり、target dir ⊂ root、profile ⊂ target、項目 ⊂ profile。
fn within_root(item: &TargetSnapshot, roots: &BTreeSet<&Path>) -> bool {
    let root = item.root.as_path();
    (roots.is_empty() || roots.contains(root))
        && !root.components().any(|c| matches!(c, Component::ParentDir))
        && strictly_inside(&item.target_dir, root)
        && strictly_inside(&item.profile_dir, &item.target_dir)
        && strictly_inside(&item.path, &item.profile_dir)
}

/// 一緒に消す単位（profile dir × 種類の群 × key）。
struct Unit<'a> {
    root: &'a Path,
    target_dir: &'a Path,
    items: Vec<&'a TargetSnapshot>,
    bytes: u64,
    used_at: SystemTime,
    /// 同時刻の並びに使う（単位の中で最小の path）。
    first_path: &'a Path,
    planned: bool,
}

/// D1.2 の規則で消す項目を決める。I/O をしない。
///
/// `locked` は `.cargo-lock` の flock が取れなかった profile dir。`params.roots` が空なら
/// 入力の `root` をそのまま信じる（root の集合での絞り込みをしない）。
pub fn plan(
    snapshot: &[TargetSnapshot],
    locked: &BTreeSet<PathBuf>,
    now: SystemTime,
    params: &SweepParams,
) -> SweepPlan {
    let roots: BTreeSet<&Path> = params.roots.iter().map(PathBuf::as_path).collect();
    let mut out = SweepPlan::default();

    // 規則 6 と 4: 検証と lock の振り分け。
    let mut valid: Vec<&TargetSnapshot> = Vec::new();
    let mut locked_seen: BTreeSet<&Path> = BTreeSet::new();
    let mut before: BTreeMap<&Path, u64> = BTreeMap::new();
    for item in snapshot {
        if !within_root(item, &roots) {
            out.skip.push(Skip {
                path: item.path.clone(),
                reason: SkipReason::OutsideRoot,
            });
            continue;
        }
        if item.symlink {
            out.skip.push(Skip {
                path: item.path.clone(),
                reason: SkipReason::Symlink,
            });
            continue;
        }
        *before.entry(item.root.as_path()).or_default() += item.bytes;
        if locked.contains(&item.profile_dir) {
            locked_seen.insert(item.profile_dir.as_path());
            continue;
        }
        valid.push(item);
    }
    for profile in &locked_seen {
        out.skip.push(Skip {
            path: profile.to_path_buf(),
            reason: SkipReason::BuildInProgress,
        });
    }

    // 単位に束ねる（deps・fingerprint・build は同じ key で 1 単位）。
    let mut by_unit: BTreeMap<(&Path, &str, &str), Vec<&TargetSnapshot>> = BTreeMap::new();
    for item in &valid {
        by_unit
            .entry((
                item.profile_dir.as_path(),
                item.kind.group(),
                item.key.as_str(),
            ))
            .or_default()
            .push(item);
    }
    let mut units: Vec<Unit> = by_unit
        .into_values()
        .filter_map(|mut items| {
            items.sort_by(|a, b| a.path.cmp(&b.path));
            let first = *items.first()?;
            Some(Unit {
                root: first.root.as_path(),
                target_dir: first.target_dir.as_path(),
                bytes: items.iter().map(|i| i.bytes).sum(),
                used_at: items
                    .iter()
                    .map(|i| i.used_at)
                    .max()
                    .unwrap_or(first.used_at),
                first_path: first.path.as_path(),
                items,
                planned: false,
            })
        })
        .collect();
    units.sort_by(|a, b| a.first_path.cmp(b.first_path));

    let older_than = |used_at: SystemTime, days: u64| {
        now.duration_since(used_at)
            .map(|age| age > Duration::from_secs(days.saturating_mul(DAY_SECS)))
            .unwrap_or(false)
    };
    let mut current: BTreeMap<&Path, u64> = before.clone();
    let push_unit = |out: &mut SweepPlan, unit: &Unit, reason: DeleteReason| {
        for item in &unit.items {
            out.delete.push(PlannedDelete {
                root: item.root.clone(),
                path: item.path.clone(),
                bytes: item.bytes,
                reason,
            });
        }
    };

    // 規則 1: 古さ。
    for unit in units.iter_mut() {
        if older_than(unit.used_at, params.max_age_days) {
            unit.planned = true;
            push_unit(&mut out, unit, DeleteReason::Age);
            if let Some(c) = current.get_mut(unit.root) {
                *c = c.saturating_sub(unit.bytes);
            }
        }
    }

    // 規則 2: 上限。
    let cap_target = params.cap_target_bytes();
    let mut over_cap: BTreeSet<&Path> = BTreeSet::new();
    for (&root, &bytes) in &current {
        if bytes > params.max_bytes_per_root {
            over_cap.insert(root);
        }
    }
    let mut order: Vec<usize> = (0..units.len())
        .filter(|&i| !units[i].planned && over_cap.contains(units[i].root))
        .collect();
    order.sort_by(|&a, &b| {
        (units[a].used_at, units[a].first_path).cmp(&(units[b].used_at, units[b].first_path))
    });
    for i in order {
        let root = units[i].root;
        let Some(c) = current.get_mut(root) else {
            continue;
        };
        if *c <= cap_target {
            continue;
        }
        *c = c.saturating_sub(units[i].bytes);
        units[i].planned = true;
        push_unit(&mut out, &units[i], DeleteReason::Cap);
    }

    // 規則 3: 放置 target dir 全体。lock の取れない profile を持つ target は対象外。
    let mut targets: BTreeMap<&Path, Vec<usize>> = BTreeMap::new();
    for (i, unit) in units.iter().enumerate() {
        targets.entry(unit.target_dir).or_default().push(i);
    }
    let target_is_locked = |t: &Path| locked.iter().any(|p| p.starts_with(t));
    let skipped_targets: BTreeSet<&Path> = snapshot
        .iter()
        .filter(|item| item.symlink || !within_root(item, &roots))
        .map(|item| item.target_dir.as_path())
        .collect();
    for (target_dir, idxs) in targets {
        if target_is_locked(target_dir) || skipped_targets.contains(target_dir) {
            continue;
        }
        if !idxs
            .iter()
            .all(|&i| older_than(units[i].used_at, params.stale_target_days))
        {
            continue;
        }
        let root = units[idxs[0]].root;
        let remaining: u64 = idxs
            .iter()
            .filter(|&&i| !units[i].planned)
            .map(|&i| units[i].bytes)
            .sum();
        for &i in &idxs {
            units[i].planned = true;
        }
        if let Some(c) = current.get_mut(root) {
            *c = c.saturating_sub(remaining);
        }
        out.delete.push(PlannedDelete {
            root: root.to_path_buf(),
            path: target_dir.to_path_buf(),
            bytes: remaining,
            reason: DeleteReason::StaleTarget,
        });
    }

    // 規則 5 と root ごとの量。
    for (root, before_bytes) in before {
        let after_bytes = current.get(root).copied().unwrap_or(before_bytes);
        let unresolved = over_cap.contains(root) && after_bytes > cap_target;
        out.over_cap_unresolved |= unresolved;
        out.roots.push(RootSummary {
            root: root.to_path_buf(),
            before_bytes,
            after_bytes,
            over_cap_unresolved: unresolved,
        });
    }
    out
}

#[cfg(test)]
mod tests;
