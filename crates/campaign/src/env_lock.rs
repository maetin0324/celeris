//! 測定開始時の固定記録 `env-lock.json`（ADR 2026-10-18-measure-env-launcher D3）。
//! A の実行ファイル（解決済み絶対 path・sha256・`--version`）、B/C の Celeris release（sha12）、arm ごとの
//! 候補集合・下段方策・根拠表の版、worker harness の実体を、campaign hash に結び付けて `<out>/env-lock.json` に固定する。
//! 再開時は現物から作り直した期待値と保存値を突き合わせ、違う key 名を挙げて拒否する（有料実行より前）。
//! 秘密値は持たない（path・版・sha256・id だけ。環境変数・credential・token は読まない）。
use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    cap_probe::CapEnvPin,
    grader::sha256_file,
    manifest::{CampaignManifest, ConfigId},
    preflight::{Host, resolve_program_in},
};

pub const ENV_LOCK_FILE: &str = "env-lock.json";
pub const ENV_LOCK_SCHEMA: &str = "celeris-env-lock/3";

/// 実行ファイルの実体。同じ path でも binary が変われば `sha256` が変わる。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProgramPin {
    pub path: String,
    pub sha256: String,
    pub version: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerProgram {
    pub name: String,
    #[serde(flatten)]
    pub pin: ProgramPin,
}

/// daemon の `GET /api/v1/llm/models`（正式 DTO `ModelCatalogView`）の 1 行。除外（`override.disabled`）でない候補だけ。
/// `tier` は人の上書き（`override.tier`）で、無ければ `unset`。source・model_id・tier 以外（capabilities 等）は取らない。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CatalogRow {
    pub model_id: String,
    pub tier: String,
    pub source_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArmLock {
    pub id: String,
    pub harness_version: String,
    pub lower_policy: Option<String>,
    pub evidence_table_id: Option<String>,
    pub no_evidence_table: bool,
    pub online_update: bool,
    pub models: Vec<String>,
    pub run_harnesses: Vec<String>,
    #[serde(default)]
    pub catalog: Vec<CatalogRow>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ALock {
    pub program: ProgramPin,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BcLock {
    pub release: String,
    pub worker_programs: Vec<WorkerProgram>,
    pub arms: Vec<ArmLock>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnvLock {
    pub schema: String,
    pub campaign_id: String,
    pub campaign_sha256: String,
    /// A が有効なときだけ（B/C のみの測定は A の実行ファイルを固定しない）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub a: Option<ALock>,
    /// 実験セット宣言（選択を含む）の hash。宣言の無い測定は空。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sets_sha256: String,
    pub bc: BcLock,
    /// 正規化した CAP_ENV と mount の実体。CAP_ENV を使わない測定は `null`（後から足す・外すも不一致）。
    pub cap_env: Option<CapEnvPin>,
}

/// 現物の観測。`a_program` は preflight と同じ解決（`resolve_program_in`）の結果から作る。
pub struct Observed {
    /// A が有効なときだけ要る。
    pub a_program: Option<ProgramPin>,
    pub worker_programs: Vec<WorkerProgram>,
    pub catalog: Vec<CatalogRow>,
    pub cap_env: Option<CapEnvPin>,
}

/// `GET /api/v1/llm/models` の本文から候補集合・階層を取り出す（順序を整えて返す）。空は Err（空固定の禁止）。
pub fn catalog_from_models(body: &Value) -> Result<Vec<CatalogRow>, String> {
    #[derive(Deserialize)]
    struct Over {
        #[serde(default)]
        disabled: bool,
        tier: Option<String>,
    }
    #[derive(Deserialize)]
    struct Item {
        source: String,
        model_id: String,
        #[serde(rename = "override")]
        over: Option<Over>,
    }
    #[derive(Deserialize)]
    struct View {
        items: Vec<Item>,
    }
    let v: View = serde_json::from_value(body.clone())
        .map_err(|e| format!("GET /api/v1/llm/models: unexpected body: {e}"))?;
    let mut rows: Vec<CatalogRow> = v
        .items
        .into_iter()
        .filter(|i| !i.over.as_ref().is_some_and(|o| o.disabled))
        .map(|i| CatalogRow {
            tier: i
                .over
                .and_then(|o| o.tier)
                .unwrap_or_else(|| "unset".into()),
            model_id: i.model_id,
            source_id: i.source,
        })
        .collect();
    rows.sort_by(|a, b| (&a.source_id, &a.model_id).cmp(&(&b.source_id, &b.model_id)));
    rows.dedup_by(|a, b| a.source_id == b.source_id && a.model_id == b.model_id);
    if rows.is_empty() {
        return Err("GET /api/v1/llm/models: the daemon's model catalog has no enabled candidate (refusing to lock an empty catalog)".into());
    }
    Ok(rows)
}

/// 実 daemon の読取 API から catalog を取る（GET 1 回。token は環境変数名で渡す）。
pub fn fetch_catalog(
    host: &dyn Host,
    api_url: &str,
    token_env: &str,
) -> Result<Vec<CatalogRow>, String> {
    catalog_from_models(&host.api_get(api_url, token_env, "/api/v1/llm/models")?)
}

/// 実行ファイルを解決し、sha256 と `--version` を取る。
pub fn pin_program(
    program: &Path,
    path_var: Option<&std::ffi::OsStr>,
) -> Result<ProgramPin, String> {
    let r = resolve_program_in(program, path_var)?;
    Ok(ProgramPin {
        sha256: sha256_file(&r.path)?,
        path: r.path.to_string_lossy().into_owned(),
        version: r.version,
    })
}

fn is_sha12(s: &str) -> bool {
    s.len() == 12 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// manifest と現物から期待値を作る。必須入力の欠落・版の不一致はここで Err（全件を返す）。
pub fn expected(m: &CampaignManifest, obs: &Observed) -> Result<EnvLock, Vec<String>> {
    let mut errs = Vec::new();
    let campaign_sha256 = match m.compute_sha256() {
        Ok(h) if m.sha256.is_empty() || m.sha256 == h => h,
        Ok(h) => {
            errs.push(format!(
                "campaign_sha256: manifest sha256 field {} != computed {h}",
                m.sha256
            ));
            h
        }
        Err(e) => {
            errs.push(format!("campaign_sha256: {e}"));
            String::new()
        }
    };
    let cfg = |id| m.configs.iter().find(|c| c.id == id);
    // 有効 arm だけを固定する。未使用 arm の config・実行ファイルは要求しない。
    let a = if m.is_active(ConfigId::A) {
        match (cfg(ConfigId::A), obs.a_program.as_ref()) {
            (None, _) => {
                errs.push("manifest has no config A".into());
                None
            }
            (Some(_), None) => {
                errs.push("a.program: path and sha256 are required".into());
                None
            }
            (Some(a), Some(prog)) => {
                if !a.harness_version.is_empty() && !prog.version.contains(&a.harness_version) {
                    errs.push(format!(
                        "a.program.version: {:?} does not match manifest A harness_version {:?}",
                        prog.version, a.harness_version
                    ));
                }
                if prog.sha256.is_empty() || prog.path.is_empty() {
                    errs.push("a.program: path and sha256 are required".into());
                }
                Some(ALock {
                    program: prog.clone(),
                })
            }
        }
    } else {
        None
    };
    let mut release = String::new();
    let mut arms = Vec::new();
    for id in [ConfigId::B, ConfigId::C]
        .into_iter()
        .filter(|id| m.is_active(*id))
    {
        let Some(c) = cfg(id) else {
            errs.push(format!("manifest has no config {id:?}"));
            continue;
        };
        if !is_sha12(&c.harness_version) {
            errs.push(format!(
                "bc.release: config {id:?} harness_version {:?} is not a 12-hex release sha",
                c.harness_version
            ));
        } else if release.is_empty() {
            release = c.harness_version.to_ascii_lowercase();
        } else if release != c.harness_version.to_ascii_lowercase() {
            errs.push("bc.release: B and C harness_version differ".into());
        }
        let r = c.routing.clone().unwrap_or_default();
        arms.push(ArmLock {
            id: format!("{id:?}"),
            harness_version: c.harness_version.to_ascii_lowercase(),
            lower_policy: r.lower_policy,
            evidence_table_id: r.evidence_table_id,
            no_evidence_table: r.no_evidence_table == Some(true),
            online_update: r.online_update == Some(true),
            models: r.models.unwrap_or_default(),
            run_harnesses: r.run_harnesses.unwrap_or_default(),
            catalog: obs.catalog.clone(),
        });
    }
    if obs.catalog.is_empty() {
        errs.push("bc.arms.catalog: candidate set is empty (must come from the daemon)".into());
    }
    let mut workers = obs.worker_programs.clone();
    workers.sort_by(|a, b| a.name.cmp(&b.name));
    for w in &workers {
        if w.pin.version.is_empty() || w.pin.sha256.is_empty() {
            errs.push(format!(
                "bc.worker_programs: {} lacks version/sha256",
                w.name
            ));
        }
    }
    if !errs.is_empty() {
        return Err(errs);
    }
    Ok(EnvLock {
        schema: ENV_LOCK_SCHEMA.into(),
        campaign_id: m.campaign_id.clone(),
        campaign_sha256,
        a,
        sets_sha256: if m.experiment_sets.is_empty() {
            String::new()
        } else {
            m.sets_sha256().unwrap_or_default()
        },
        bc: BcLock {
            release,
            worker_programs: workers,
            arms,
        },
        cap_env: obs.cap_env.clone(),
    })
}

/// 保存値と期待値の食い違う key（`a.program.sha256` のような path）を返す。空なら一致。
pub fn diff(stored: &EnvLock, want: &EnvLock) -> Vec<String> {
    fn walk(prefix: &str, a: &Value, b: &Value, out: &mut Vec<String>) {
        match (a, b) {
            (Value::Object(x), Value::Object(y)) => {
                let keys: std::collections::BTreeSet<_> = x.keys().chain(y.keys()).collect();
                for k in keys {
                    let p = if prefix.is_empty() {
                        k.clone()
                    } else {
                        format!("{prefix}.{k}")
                    };
                    walk(
                        &p,
                        x.get(k).unwrap_or(&Value::Null),
                        y.get(k).unwrap_or(&Value::Null),
                        out,
                    );
                }
            }
            _ if a != b => out.push(prefix.to_string()),
            _ => {}
        }
    }
    // arms は id、catalog は `source/model_id` を key にした object へ直し、追加・削除・階層移動を key 名で挙げる。
    fn keyed(mut v: Value) -> Value {
        let Some(arms) = v.pointer_mut("/bc/arms").and_then(Value::as_array_mut) else {
            return v;
        };
        let mut by_id = serde_json::Map::new();
        for mut arm in std::mem::take(arms) {
            if let Some(rows) = arm.get_mut("catalog").and_then(Value::as_array_mut) {
                let m: serde_json::Map<String, Value> = std::mem::take(rows)
                    .into_iter()
                    .map(|r| {
                        let k = format!(
                            "{}/{}",
                            r["source_id"].as_str().unwrap_or_default(),
                            r["model_id"].as_str().unwrap_or_default()
                        );
                        (k, r["tier"].clone())
                    })
                    .collect();
                arm["catalog"] = Value::Object(m);
            }
            let id = arm["id"].as_str().unwrap_or_default().to_string();
            by_id.insert(id, arm);
        }
        v["bc"]["arms"] = Value::Object(by_id);
        v
    }
    let mut out = Vec::new();
    let (Ok(a), Ok(b)) = (serde_json::to_value(stored), serde_json::to_value(want)) else {
        return vec!["serialize".into()];
    };
    walk("", &keyed(a), &keyed(b), &mut out);
    out
}

pub fn load(out: &Path) -> Result<Option<EnvLock>, String> {
    let p = out.join(ENV_LOCK_FILE);
    match fs::read(&p) {
        Ok(b) => {
            // 旧記録（名前一覧 pin の /2、cap_env の無い /1 など）は拒否する。黙って新形式に読み替えない（ADR campaign-lock 付記）。
            let v: Value =
                serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", p.display()))?;
            if v["schema"] != ENV_LOCK_SCHEMA {
                return Err(format!(
                    "{}: env-lock schema {} is not {ENV_LOCK_SCHEMA} (cap_env content pin); refusing to resume. \
                     Migrate explicitly: only before any trial data exists, remove it and re-run env-lock",
                    p.display(),
                    v["schema"]
                ));
            }
            serde_json::from_value(v)
                .map(Some)
                .map_err(|e| format!("{}: {e}", p.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", p.display())),
    }
}

fn write_atomic(out: &Path, l: &EnvLock) -> Result<PathBuf, String> {
    fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let tmp = out.join(format!("{ENV_LOCK_FILE}.tmp"));
    let dst = out.join(ENV_LOCK_FILE);
    let body = serde_json::to_vec_pretty(l).map_err(|e| e.to_string())?;
    fs::write(&tmp, body).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, &dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    Ok(dst)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Created,
    Verified,
}

/// 無ければ作り（`verify_only` でなければ）、あれば照合する。不一致・欠落は Err（違う key 名を含む）。
pub fn establish(
    m: &CampaignManifest,
    obs: &Observed,
    out: &Path,
    verify_only: bool,
) -> Result<Outcome, String> {
    let want = expected(m, obs).map_err(|e| e.join("\n"))?;
    match load(out)? {
        Some(stored) => {
            let d = diff(&stored, &want);
            if d.is_empty() {
                Ok(Outcome::Verified)
            } else {
                Err(format!(
                    "env-lock mismatch (refusing to run): {}",
                    d.join(", ")
                ))
            }
        }
        None if verify_only => Err(format!(
            "{}: no env-lock to verify",
            out.join(ENV_LOCK_FILE).display()
        )),
        None => {
            write_atomic(out, &want)?;
            Ok(Outcome::Created)
        }
    }
}

/// watch（`env-lock --verify`）・`env-lock`・`offline-lock`・preflight が共有する現物の観測の組み立て
/// （ADR 2026-10-21-preflight-env-lock-check）。`worker_programs` は `<name>=<program>`。
pub fn observe(
    m: &CampaignManifest,
    a_program: &Path,
    worker_programs: &[String],
    cap_env: Option<&Path>,
    catalog: impl FnOnce() -> Result<Vec<CatalogRow>, String>,
) -> Result<Observed, String> {
    let path_var = std::env::var_os("PATH");
    // A が有効なときだけ A の実行ファイルを固定する（B/C のみの測定は A を探さない）。
    let a_program = if m.is_active(ConfigId::A) {
        Some(pin_program(a_program, path_var.as_deref())?)
    } else {
        None
    };
    let mut workers = Vec::new();
    for w in worker_programs {
        let (name, p) = w
            .split_once('=')
            .ok_or("--worker-program expects <name>=<program>")?;
        workers.push(WorkerProgram {
            name: name.into(),
            pin: pin_program(Path::new(p), path_var.as_deref())?,
        });
    }
    // catalog は A の解決の後に取る（A の欠落を daemon 接続の失敗より先に報告する）。
    let catalog = catalog()?;
    let cap_env = match cap_env {
        Some(p) => Some(crate::cap_probe::CapEnv::load(p)?.pin()?),
        None => None,
    };
    Ok(Observed {
        a_program,
        worker_programs: workers,
        catalog,
        cap_env,
    })
}

/// `<out>/env-lock.json` が在れば watch と同じ照合（`establish` の verify_only）をする。無ければ `Ok(false)`。
pub fn verify_existing(m: &CampaignManifest, obs: &Observed, out: &Path) -> Result<bool, String> {
    if !out.join(ENV_LOCK_FILE).exists() {
        return Ok(false);
    }
    establish(m, obs, out, true).map(|_| true)
}
