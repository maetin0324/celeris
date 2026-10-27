//! 測定専用 daemon の構成検査（P0-3）。通常運用の KB・記憶・状態・履歴が試行に入らないことを、
//! 測定 daemon の config と通常運用の config の突き合わせで確かめる。読むだけ（config の値は secret を含まない
//! 項目だけ見る）。クラスタ / task-api の接続設定は壊れていないこと（通常運用と同じ `clusters`、`[api]` の
//! listen と token_file がある）も見る。
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::snapshot::hash_tree;

/// 通常運用の既定値（`crates/celeris/src/config` の `default_*`）。測定 config がこれに落ちるのを拒否する。
const DEFAULT_KNOWLEDGE_ROOT: &str = "~/.local/share/celeris/knowledge";
pub(crate) const DEFAULT_MEMORY_DIR: &str = "~/.local/celeris/memory";
const DEFAULT_WORKSPACE_ROOT: &str = "~/.local/celeris/workspaces";

/// canary 識別文字列（秘密ではない目印）。`forbidden` は通常 KB・他試行・grader 私有領域にだけ置いたもの、
/// `required` は試行用 KB にだけ置いたもの。
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Canaries {
    #[serde(default)]
    pub forbidden: Vec<String>,
    #[serde(default)]
    pub required: Vec<String>,
}

impl Canaries {
    pub fn load(p: &Path) -> Result<Self, String> {
        let b = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
        let c: Self = serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", p.display()))?;
        if c.forbidden.iter().chain(&c.required).any(String::is_empty) {
            return Err(format!("{}: empty canary string", p.display()));
        }
        if c.forbidden.is_empty() || c.required.is_empty() {
            return Err(format!(
                "{}: needs at least one forbidden and one required canary",
                p.display()
            ));
        }
        Ok(c)
    }
}

pub(crate) fn load_toml(p: &Path) -> Result<toml::Value, String> {
    let t = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
    toml::from_str::<toml::Value>(&t).map_err(|e| format!("{}: {e}", p.display()))
}

fn lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

/// `~` 展開、相対は config 基準。存在すれば symlink を解いた実 path。
pub(crate) fn resolve(raw: &str, cfg: &Path) -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let p = match raw.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if raw == "~" => home,
        None => PathBuf::from(raw),
    };
    let p = if p.is_absolute() {
        p
    } else {
        cfg.parent().unwrap_or(Path::new(".")).join(p)
    };
    let p = lexical(&p);
    p.canonicalize().unwrap_or(p)
}

fn get_path(v: &toml::Value, keys: &[&str], cfg: &Path, default: Option<&str>) -> Option<PathBuf> {
    let mut cur = v;
    for k in keys {
        match cur.get(k) {
            Some(x) => cur = x,
            None => return default.map(|d| resolve(d, cfg)),
        }
    }
    cur.as_str().map(|s| resolve(s, cfg))
}

pub(crate) fn tget<'a>(v: &'a toml::Value, keys: &[&str]) -> Option<&'a toml::Value> {
    keys.iter().try_fold(v, |c, k| c.get(k))
}

fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

fn is_empty_tree(p: &Path) -> bool {
    match std::fs::read_dir(p) {
        Err(_) => true, // 無い = 空
        Ok(mut it) => it.next().is_none(),
    }
}

/// 測定 daemon の config を通常運用の config と突き合わせる。返すのは不足の一覧。
pub fn check_config(measure: &Path, normal: &Path, fixed_kb: Option<&Path>) -> Vec<String> {
    let (m, n) = match (load_toml(measure), load_toml(normal)) {
        (Ok(m), Ok(n)) => (m, n),
        (m, n) => return [m.err(), n.err()].into_iter().flatten().collect(),
    };
    let mut v = Vec::new();
    let apart = |what: &str, mp: Option<PathBuf>, np: PathBuf, v: &mut Vec<String>| {
        match mp {
            None => v.push(format!(
                "measurement config must set {what} explicitly (the default is the normal operation's path)"
            )),
            Some(mp) if overlaps(&mp, &np) => v.push(format!(
                "measurement {what} {} overlaps the normal operation's {}",
                mp.display(),
                np.display()
            )),
            Some(_) => {}
        }
    };
    // (1) knowledge.root: 別 path で、空か固定初期 KB。
    let kroot = get_path(&m, &["knowledge", "root"], measure, None);
    apart(
        "[knowledge] root",
        kroot.clone(),
        get_path(
            &n,
            &["knowledge", "root"],
            normal,
            Some(DEFAULT_KNOWLEDGE_ROOT),
        )
        .unwrap_or_default(),
        &mut v,
    );
    if let Some(r) = &kroot
        && !is_empty_tree(r)
    {
        match (fixed_kb.map(hash_tree), hash_tree(r)) {
            (Some(Ok(f)), Ok(h)) if f == h => {}
            _ => v.push(format!(
                "measurement knowledge root {} is neither empty nor identical to the fixed initial KB (--kb-src)",
                r.display()
            )),
        }
    }
    // default_mounts: 明示し、kb: だけ（memory / repo / dir は通常運用の中身へ届く）。
    match tget(&m, &["knowledge", "default_mounts"]).and_then(|x| x.as_array()) {
        None => v.push(
            "measurement config must set [knowledge] default_mounts explicitly (the default mounts kb:user, kb:environment)"
                .into(),
        ),
        Some(a) => {
            for x in a {
                let s = x.as_str().unwrap_or("");
                if !s.starts_with("kb:") {
                    v.push(format!(
                        "[knowledge] default_mounts {s:?}: only kb:<scope> of the measurement root is allowed"
                    ));
                }
            }
        }
    }
    // 記憶: 無い（読み書きとも止まる）か、別 path で空。
    if let Some(dir) = m.get("memory").map(|_| {
        get_path(&m, &["memory", "dir"], measure, Some(DEFAULT_MEMORY_DIR)).unwrap_or_default()
    }) {
        let np =
            get_path(&n, &["memory", "dir"], normal, Some(DEFAULT_MEMORY_DIR)).unwrap_or_default();
        if overlaps(&dir, &np) {
            v.push(format!(
                "measurement [memory] dir {} overlaps the normal operation's",
                dir.display()
            ));
        } else if !is_empty_tree(&dir) {
            v.push(format!(
                "measurement [memory] dir {} is not empty",
                dir.display()
            ));
        }
    }
    // state: DB と作業場所。
    let db_of = |c: &toml::Value, p: &Path| match c.get("db") {
        Some(toml::Value::String(s)) => Some(resolve(s, p)),
        Some(t) => t
            .get("path")
            .and_then(|x| x.as_str())
            .map(|s| resolve(s, p)),
        None => None,
    };
    match (db_of(&m, measure), db_of(&n, normal)) {
        (None, _) => v.push("measurement config must set db explicitly".into()),
        (Some(a), Some(b)) if a == b => v.push(format!(
            "measurement db {} is the normal operation's db",
            a.display()
        )),
        _ => {}
    }
    apart(
        "workspace_root",
        get_path(&m, &["workspace_root"], measure, None),
        get_path(
            &n,
            &["workspace_root"],
            normal,
            Some(DEFAULT_WORKSPACE_ROOT),
        )
        .unwrap_or_default(),
        &mut v,
    );
    // 試行後に KB を書き換える自動整理は無効。
    for (what, ok) in [
        (
            "[knowledge.langmem] enabled",
            tget(&m, &["knowledge", "langmem", "enabled"]).and_then(|x| x.as_bool()) != Some(true),
        ),
        (
            "[knowledge.gc] enabled",
            tget(&m, &["knowledge", "gc", "enabled"]).and_then(|x| x.as_bool()) != Some(true),
        ),
        (
            "[[cron.seed]]",
            tget(&m, &["cron", "seed"])
                .and_then(|s| s.as_array())
                .is_none_or(|a| a.is_empty()),
        ),
    ] {
        if !ok {
            v.push(format!(
                "measurement config must not enable {what} (it would rewrite the KB between trials)"
            ));
        }
    }
    // 接続設定は壊さない: クラスタは通常運用と同じ、task-api は listen と [[api.tokens]] を持つ。
    if n.get("clusters") != m.get("clusters") {
        v.push("measurement [[clusters]] differs from the normal operation's (real cluster connections must be kept)".into());
    }
    if tget(&m, &["api", "listen"]).is_none() {
        v.push("measurement config lacks [api] listen (task-api authentication)".into());
    }
    // ADR 2026-10-27-api-principals: token は `[[api.tokens]]`（launcher は最初の 1 本の token_file を使う）。
    if tget(&m, &["api", "tokens"])
        .and_then(|t| t.as_array())
        .and_then(|t| t.first())
        .and_then(|t| t.get("token_file"))
        .is_none()
    {
        v.push(
            "measurement config lacks [[api.tokens]] token_file (task-api authentication)".into(),
        );
    }
    v
}
