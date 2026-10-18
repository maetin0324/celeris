//! 測定環境の検査（ADR 2026-10-18-measure-env-launcher D2）: `--api-url` の先で答えている daemon が
//! 「測定 config で起動した、その daemon」であることを、daemon 自身の答え（`GET /api/v1/config` の
//! `identity`）と測定 config の突き合わせで確かめる。読むだけ（GET 1 回。有料実行・task 作成は 0 件）。
//! 食い違い（別 daemon・起動後の config 書き換え・CLI 上書き・実効構成の相違・版）は不足の一覧にして返す。
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{
    manifest::{CampaignManifest, ConfigId},
    measure_env::{self, resolve, tget},
    preflight::{Host, MeasureEnv},
};

pub const ENV_CHECK_SCHEMA: &str = "celeris.campaign-env-check/1";

/// 通常運用の既定（`crates/celeris/src/config/selfdeploy.rs`）。測定 config が書かなければ daemon はこれで動く。
const DEFAULT_RELEASES_DIR: &str = "~/.local/celeris/releases";
const DEFAULT_SELFDEPLOY_REPO: &str = "~/workspace/agent-platform";
const DEFAULT_KNOWLEDGE_ROOT: &str = "~/.local/share/celeris/knowledge";

/// `GET /api/v1/config` のうち照合に使う部分（正式 DTO `task_api::ConfigView` の部分集合。
/// 試験は実 router の本文をそのまま通す）。
#[derive(Debug, Deserialize)]
struct ConfigBody {
    config_path: String,
    identity: Identity,
}

#[derive(Debug, Deserialize)]
struct Identity {
    instance_id: String,
    pid: u32,
    release: String,
    config_sha256: Option<String>,
    overrides: Vec<String>,
    effective: Effective,
}

#[derive(Debug, Deserialize, PartialEq)]
struct Effective {
    db: String,
    workspace_root: String,
    knowledge_root: String,
    knowledge_default_mounts: Vec<String>,
    memory_dir: Option<String>,
    langmem_enabled: bool,
    knowledge_gc_enabled: bool,
    cron_seed_count: usize,
    selfdeploy_releases_dir: String,
    selfdeploy_repo: String,
    selfdeploy_push: bool,
    api_listen: String,
    llm_proxy_listen: Option<String>,
}

/// 測定 config の `[api] listen` から導く origin。`0.0.0.0` / `::` は loopback に読み替える。
pub fn derive_api_url(config: &Path) -> Result<String, String> {
    let c = measure_env::load_toml(config)?;
    let listen = tget(&c, &["api", "listen"])
        .and_then(|x| x.as_str())
        .ok_or_else(|| format!("{}: [api] listen is not set", config.display()))?;
    let (host, port) = listen
        .rsplit_once(':')
        .filter(|(_, p)| p.parse::<u16>().is_ok())
        .ok_or_else(|| {
            format!(
                "{}: [api] listen {listen:?} is not host:port",
                config.display()
            )
        })?;
    let host = match host.trim_matches(['[', ']']) {
        "0.0.0.0" => "127.0.0.1".to_string(),
        "::" => "[::1]".to_string(),
        h if h.contains(':') => format!("[{h}]"),
        h => h.to_string(),
    };
    Ok(format!("http://{host}:{port}"))
}

fn norm_url(u: &str) -> &str {
    u.trim_end_matches('/')
}

/// 測定 config から導いた期待の実効値。config が書いていない項目で、通常運用の既定に落ちると
/// 危険なもの（接続・proxy）は照合しない（`measure_env::check_config` の担当）。
fn expected_effective(c: &toml::Value, cfg: &Path) -> Result<Effective, String> {
    let path = |keys: &[&str], default: Option<&str>| -> Option<String> {
        let mut cur = c;
        for k in keys {
            match cur.get(k) {
                Some(x) => cur = x,
                None => {
                    return default.map(|d| resolve(d, cfg).to_string_lossy().into_owned());
                }
            }
        }
        cur.as_str()
            .map(|s| resolve(s, cfg).to_string_lossy().into_owned())
    };
    let db = match c.get("db") {
        Some(toml::Value::String(s)) => Some(resolve(s, cfg).to_string_lossy().into_owned()),
        Some(t) => t
            .get("path")
            .and_then(|x| x.as_str())
            .map(|s| resolve(s, cfg).to_string_lossy().into_owned()),
        None => None,
    }
    .ok_or("config sets no db")?;
    let flag = |keys: &[&str], d: bool| tget(c, keys).and_then(|x| x.as_bool()).unwrap_or(d);
    let mounts = tget(c, &["knowledge", "default_mounts"])
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    Ok(Effective {
        db,
        workspace_root: path(&["workspace_root"], None).ok_or("config sets no workspace_root")?,
        knowledge_root: path(&["knowledge", "root"], Some(DEFAULT_KNOWLEDGE_ROOT))
            .unwrap_or_default(),
        knowledge_default_mounts: mounts,
        memory_dir: c.get("memory").and_then(|_| {
            path(
                &["memory", "dir"],
                Some(crate::measure_env::DEFAULT_MEMORY_DIR),
            )
        }),
        langmem_enabled: flag(&["knowledge", "langmem", "enabled"], false),
        knowledge_gc_enabled: flag(&["knowledge", "gc", "enabled"], false),
        cron_seed_count: tget(c, &["cron", "seed"])
            .and_then(|s| s.as_array())
            .map_or(0, Vec::len),
        selfdeploy_releases_dir: path(&["selfdeploy", "releases_dir"], Some(DEFAULT_RELEASES_DIR))
            .unwrap_or_default(),
        selfdeploy_repo: path(&["selfdeploy", "repo"], Some(DEFAULT_SELFDEPLOY_REPO))
            .unwrap_or_default(),
        // 既定は true（本番へ push する）。測定 config が false と書く。
        selfdeploy_push: flag(&["selfdeploy", "push"], true),
        api_listen: tget(c, &["api", "listen"])
            .and_then(|x| x.as_str())
            .ok_or("config sets no [api] listen")?
            .to_string(),
        // 書いていなければ空（daemon 側は既定で起動し得る。proxy の port は check_config の担当）。
        llm_proxy_listen: None,
    })
}

/// B/C の harness_version（12 桁 hex の release）。揃っていなければ None（版の固定は env-lock の担当）。
fn manifest_release(m: &CampaignManifest) -> Option<String> {
    let mut it = [ConfigId::B, ConfigId::C].into_iter().filter_map(|id| {
        m.configs
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.harness_version.to_ascii_lowercase())
    });
    let first = it.next()?;
    it.all(|v| v == first).then_some(first)
}

/// 接続先 daemon の同一性照合。返すのは不足の一覧（空なら一致）。api_url が測定 config から導いた値と
/// 違うときは、token を送らず・接続もしないで拒否する。
pub fn check_identity(
    me: &MeasureEnv,
    m: &CampaignManifest,
    api_url: &str,
    token_env: &str,
    host: &dyn Host,
) -> Vec<String> {
    verify(me, m, api_url, token_env, host)
        .err()
        .unwrap_or_default()
}

/// 同一性照合に通った接続先 daemon の識別（pid・instance・release）。照合は 1 回の GET で行う。
/// B/C の能力 probe は、この接続先 daemon の worker 起動経路で検査する（検査先 identity として報告に残す）。
pub fn verify(
    me: &MeasureEnv,
    m: &CampaignManifest,
    api_url: &str,
    token_env: &str,
    host: &dyn Host,
) -> Result<crate::cap_probe::DaemonRef, Vec<String>> {
    let derived = derive_api_url(&me.config).map_err(|e| vec![e])?;
    if norm_url(api_url) != derived {
        return Err(vec![format!(
            "--api-url {api_url} is not the measurement daemon's api origin {derived} (derived from [api] listen of {})",
            me.config.display()
        )]);
    }
    let body = host
        .api_get(api_url, token_env, "/api/v1/config")
        .map_err(|e| vec![format!("GET /api/v1/config: {e}")])?;
    let cfg: ConfigBody = serde_json::from_value(body).map_err(|e| {
        vec![format!(
            "GET /api/v1/config has no daemon identity (a daemon without ADR 2026-10-18 D2?): {e}"
        )]
    })?;
    let missing = check_body(me, m, &cfg);
    if !missing.is_empty() {
        return Err(missing);
    }
    Ok(crate::cap_probe::DaemonRef {
        pid: cfg.identity.pid,
        probe_task_id: None,
        instance_id: Some(cfg.identity.instance_id.clone()),
        release: Some(cfg.identity.release.clone()),
    })
}

fn check_body(me: &MeasureEnv, m: &CampaignManifest, cfg: &ConfigBody) -> Vec<String> {
    let mut v = Vec::new();
    let id = &cfg.identity;
    if id.instance_id.is_empty() {
        v.push("identity.instance_id is empty".into());
    }
    // launcher が起動した process（その PID）が答えていること。別 daemon が同じ port で答えるのを弾く。
    match me.daemon_pid {
        None => v.push("--daemon-pid (the launcher-owned daemon pid) is required".into()),
        Some(p) if p != id.pid => v.push(format!(
            "identity.pid {} != launcher-owned daemon pid {p} (another process answers the api)",
            id.pid
        )),
        Some(_) => {}
    }
    // 起動後の config 書き換え・別 file の config で起動した daemon。
    let abs = |p: &Path| -> PathBuf {
        let p = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        p.canonicalize().unwrap_or(p)
    };
    if abs(Path::new(&cfg.config_path)) != abs(&me.config) {
        v.push(format!(
            "daemon runs config {} but the measurement config is {}",
            cfg.config_path,
            me.config.display()
        ));
    }
    match std::fs::read(&me.config) {
        Err(e) => v.push(format!("{}: {e}", me.config.display())),
        Ok(b) => {
            let want = format!("{:x}", Sha256::digest(b));
            if id.config_sha256.as_deref() != Some(want.as_str()) {
                v.push(format!(
                    "identity.config_sha256 {:?} != sha256 of the measurement config {want} (the file changed since the daemon started, or another config)",
                    id.config_sha256
                ));
            }
        }
    }
    if !id.overrides.is_empty() {
        v.push(format!(
            "daemon started with CLI overrides {:?} (only --config is allowed)",
            id.overrides
        ));
    }
    // 版: dev は不可。manifest の B/C release と一致。
    if id.release == "dev" || id.release.is_empty() {
        v.push("identity.release is `dev` (a measurement needs a pinned release)".into());
    } else if let Some(want) = manifest_release(m)
        && id.release.to_ascii_lowercase() != want
    {
        v.push(format!(
            "identity.release {} != manifest B/C harness_version {want}",
            id.release
        ));
    }
    // 実効構成。
    match measure_env::load_toml(&me.config).and_then(|c| expected_effective(&c, &me.config)) {
        Err(e) => v.push(e),
        Ok(want) => {
            let got = &id.effective;
            let norm = |s: &str| resolve(s, &me.config).to_string_lossy().into_owned();
            let p = |s: &String| norm(s);
            let mut diff = |name: &str, same: bool, w: String, g: String| {
                if !same {
                    v.push(format!("effective.{name}: daemon {g} != config {w}"));
                }
            };
            for (name, w, g) in [
                ("db", &want.db, &got.db),
                ("workspace_root", &want.workspace_root, &got.workspace_root),
                ("knowledge_root", &want.knowledge_root, &got.knowledge_root),
                (
                    "selfdeploy_releases_dir",
                    &want.selfdeploy_releases_dir,
                    &got.selfdeploy_releases_dir,
                ),
                (
                    "selfdeploy_repo",
                    &want.selfdeploy_repo,
                    &got.selfdeploy_repo,
                ),
            ] {
                diff(name, p(w) == p(g), w.clone(), g.clone());
            }
            diff(
                "memory_dir",
                want.memory_dir.as_deref().map(norm) == got.memory_dir.as_deref().map(norm),
                format!("{:?}", want.memory_dir),
                format!("{:?}", got.memory_dir),
            );
            diff(
                "knowledge_default_mounts",
                want.knowledge_default_mounts == got.knowledge_default_mounts,
                format!("{:?}", want.knowledge_default_mounts),
                format!("{:?}", got.knowledge_default_mounts),
            );
            for (name, w, g) in [
                ("langmem_enabled", want.langmem_enabled, got.langmem_enabled),
                (
                    "knowledge_gc_enabled",
                    want.knowledge_gc_enabled,
                    got.knowledge_gc_enabled,
                ),
                ("selfdeploy_push", want.selfdeploy_push, got.selfdeploy_push),
            ] {
                diff(name, w == g, w.to_string(), g.to_string());
            }
            diff(
                "cron_seed_count",
                want.cron_seed_count == got.cron_seed_count,
                want.cron_seed_count.to_string(),
                got.cron_seed_count.to_string(),
            );
            diff(
                "api_listen",
                want.api_listen == got.api_listen,
                want.api_listen.clone(),
                got.api_listen.clone(),
            );
        }
    }
    v
}

#[derive(Debug, serde::Serialize)]
pub struct EnvCheckReport {
    pub schema: &'static str,
    pub ok: bool,
    /// 常に 0（env-check は GET 1 回だけで、有料実行も task 作成もしない）。
    pub paid_calls_made: u32,
    pub api_url: String,
    pub missing: Vec<String>,
}

/// 単独の `env-check`: 測定 config の検査（通常運用との分離・canary 定義）と接続先 daemon の同一性。
pub fn run(
    me: &MeasureEnv,
    m: &CampaignManifest,
    kb_src: Option<&Path>,
    api_url: &str,
    token_env: &str,
    host: &dyn Host,
) -> EnvCheckReport {
    let mut missing = measure_env::check_config(&me.config, &me.normal_config, kb_src);
    if let Err(e) = measure_env::Canaries::load(&me.canaries) {
        missing.push(e);
    }
    if !host.env_nonempty(token_env) {
        missing.push(format!(
            "environment variable {token_env} is not set or empty"
        ));
    }
    if let Err(e) = crate::curl_transport::validate_target(api_url, token_env) {
        missing.push(e);
    } else if host.env_nonempty(token_env) {
        missing.extend(check_identity(me, m, api_url, token_env, host));
    }
    EnvCheckReport {
        schema: ENV_CHECK_SCHEMA,
        ok: missing.is_empty(),
        paid_calls_made: 0,
        api_url: api_url.to_string(),
        missing,
    }
}
