//! ADR 2026-11-16-scratch-stale-build-outputs: target の位置を変えた直後（引き継ぎの rename・seed の写し）に、
//! cargo が付け替えない build script の出力（`root-output` の OUT_DIR の外を指す `rustc-link-search`）が pool の
//! 別の場所（`targets/`・`seeds/` の下。引き継ぎ元・seed の世代）を指す package の build の出力を消す。
//! 次の build で cargo は build script を再実行し、その package を作り直す（`cargo clean -p <pkg>` と同じ効果）。
//! 判定は path だけで行い、dir が今在るかは見ない。cargo の profile の dir の見つけ方（`profile_dirs`）もここに置く。
//! **LLM は呼ばない**。

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::reflink::{remove_any, unit_hash};
use super::{DELETING_PREFIX, Pool, seeds_dir};

/// `rustc-link-search` の値の前置き（cargo の `-L KIND=PATH`）。
const LINK_SEARCH_KINDS: [&str; 5] = ["native=", "dependency=", "crate=", "framework=", "all="];

/// 消した package 1 つ（`target-origin.json` の `removed_build_outputs` の要素）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovedBuildOutput {
    /// target からの相対の profile の dir（`debug`、`<triple>/release` など）。
    pub profile: String,
    pub package: String,
    /// 元の場所を指していた `rustc-link-search` の値（前置きを外したもの）。
    pub link_search: Vec<String>,
    /// 消した `build/`・`.fingerprint/` の dir と `deps/` の file の数。
    pub removed_entries: usize,
}

/// cargo が profile の dir に置き、build の間 flock する file（`workspace_targets` の lock もこれを見る）。
pub const CARGO_LOCK: &str = ".cargo-lock";

/// `<target>/<profile>` と `<target>/<triple>/<profile>` のうち `.cargo-lock` を持つ実 dir（名前順。symlink は辿らず、
/// `.deleting-*` は見ない）。scratch の写しの掃除（`prune_workspace_artifacts`・`remove_stale_build_outputs`）と
/// 作業場所の target の掃除（`workspace_targets`）が使う。
pub fn profile_dirs(target: &Path) -> Vec<PathBuf> {
    fn real(path: &Path, dir: bool) -> bool {
        std::fs::symlink_metadata(path)
            .map(|m| {
                let t = m.file_type();
                if dir { t.is_dir() } else { t.is_file() }
            })
            .unwrap_or(false)
    }
    fn children(dir: &Path) -> Vec<PathBuf> {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut out: Vec<PathBuf> = rd
            .flatten()
            .filter(|e| !e.file_name().to_string_lossy().starts_with(DELETING_PREFIX))
            .map(|e| e.path())
            .filter(|p| real(p, true))
            .collect();
        out.sort();
        out
    }
    let mut out = Vec::new();
    for p in children(target) {
        if real(&p.join(CARGO_LOCK), false) {
            out.push(p);
            continue;
        }
        out.extend(
            children(&p)
                .into_iter()
                .filter(|q| real(&q.join(CARGO_LOCK), false)),
        );
    }
    out
}

/// 元の場所の根（pool の `targets/` と `seeds/`。設定の path と実体の path の両方）。
fn foreign_roots(pool: &Pool) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for root in [pool.targets_dir(), seeds_dir(pool)] {
        if let Ok(real) = root.canonicalize()
            && real != root
        {
            roots.push(real);
        }
        roots.push(root);
    }
    roots
}

/// 引き継ぎ・写しの直後の target（`scan`。写しでは rename 前の一時 dir）から、pool の別の場所を指す
/// build script の出力を持つ package の build の出力を消す。`own` は rename の後の自分の `target/`。
pub fn remove_stale_build_outputs(
    pool: &Pool,
    scan: &Path,
    own: &Path,
) -> io::Result<Vec<RemovedBuildOutput>> {
    let foreign = foreign_roots(pool);
    let mut removed = Vec::new();
    for profile in profile_dirs(scan) {
        let stale = stale_packages(&profile, own, &foreign)?;
        let rel = profile
            .strip_prefix(scan)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| profile.display().to_string());
        for (package, link_search) in stale {
            let removed_entries = remove_package(&profile, &package)?;
            removed.push(RemovedBuildOutput {
                profile: rel.clone(),
                package,
                link_search,
                removed_entries,
            });
        }
    }
    Ok(removed)
}

/// profile の中で、元の場所を指す `rustc-link-search` を持つ package と、その値。
fn stale_packages(
    profile: &Path,
    own: &Path,
    foreign: &[PathBuf],
) -> io::Result<BTreeMap<String, Vec<String>>> {
    let mut stale: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in read_dir_if_exists(&profile.join("build"))? {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some((package, _)) = name.rsplit_once('-') else {
            continue;
        };
        let unit = e.path();
        let Ok(output) = std::fs::read(unit.join("output")) else {
            continue;
        };
        let root = match std::fs::read(unit.join("root-output")) {
            Ok(b) => PathBuf::from(String::from_utf8_lossy(&b).trim_end_matches(['\n', '\r'])),
            Err(_) => unit.join("out"),
        };
        for value in link_search_values(&String::from_utf8_lossy(&output)) {
            let path = Path::new(value);
            if path.starts_with(&root)
                || path.starts_with(own)
                || !foreign.iter().any(|r| path.starts_with(r))
            {
                continue;
            }
            stale
                .entry(package.to_string())
                .or_default()
                .push(value.to_string());
        }
    }
    Ok(stale)
}

/// build script の出力の `rustc-link-search` の値（前置きを外したもの）。
fn link_search_values(output: &str) -> impl Iterator<Item = &str> {
    output.lines().filter_map(|line| {
        let line = line.trim_end_matches('\r');
        let value = line
            .strip_prefix("cargo::rustc-link-search=")
            .or_else(|| line.strip_prefix("cargo:rustc-link-search="))?;
        Some(
            LINK_SEARCH_KINDS
                .iter()
                .find_map(|k| value.strip_prefix(k))
                .unwrap_or(value),
        )
    })
}

/// profile の中の package の build の出力（`build/<pkg>-*`・`.fingerprint/<pkg>-*` と、その hash の `deps/` の
/// file）を消し、消した数を返す。
fn remove_package(profile: &Path, package: &str) -> io::Result<usize> {
    let mut hashes = BTreeSet::new();
    let mut count = 0;
    for sub in ["build", ".fingerprint"] {
        for e in read_dir_if_exists(&profile.join(sub))? {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some((pkg, hash)) = name.rsplit_once('-')
                && pkg == package
            {
                hashes.insert(hash.to_string());
                remove_any(&e.path())?;
                count += 1;
            }
        }
    }
    for e in read_dir_if_exists(&profile.join("deps"))? {
        let name = e.file_name().to_string_lossy().into_owned();
        let base = name.split('.').next().unwrap_or(&name);
        if unit_hash(base).is_some_and(|h| hashes.contains(h)) {
            remove_any(&e.path())?;
            count += 1;
        }
    }
    Ok(count)
}

/// dir の entry（無い dir は空。読めない dir は error）。
fn read_dir_if_exists(dir: &Path) -> io::Result<Vec<std::fs::DirEntry>> {
    match std::fs::read_dir(dir) {
        Ok(rd) => rd.collect(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}
