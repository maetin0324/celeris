//! 付記 S2・S4: 本物の deno_core host で compare を確かめる試験（`script_compare_lane_rules.rs`・
//! `script_compare_route_priority.rs`・`script_compare_cos_digest.rs`）の共通部分。userns を使わない。外部ネットワークに出ない。
//!
//! 待ちの方針: dispatcher の deadline は 50 ms の固定で、host が忙しいと正しい bundle でも `timeout` になりうる。
//! `timeout` は環境の遅さであって比較の結果ではないので、`timeout` のときだけ新しい task で比較をやり直す。
//! 他の outcome はやり直さない。

// 試験 file ごとに使う helper が違う。
#![allow(dead_code)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use celeris::{ClusterMasters, Config, ScriptPolicy, build_dispatcher};
use celeris_script::manifest::{VerifiedBundle, load_bundle_dir, sha256_hex};
use celeris_script::{ScriptRuntimeHandle, ScriptSource};
use task_core::script_compare::{
    CompareOutcome, NewScriptGeneration, ScriptCompareRecord, ScriptGeneration,
    ScriptGenerationState, ScriptGenerationTransition, ScriptMode, ScriptModeChange,
};
use task_core::{
    Budget, Check, Criterion, Event, RoutingRecord, SqliteStore, Status, Task, TaskId, TaskKind,
    TaskRouting, TaskStore, Tier, TierSource, WorkerHint, WorkspaceSpec,
};
use task_dispatch::Dispatcher;
use time::OffsetDateTime;

/// 全件比較と replay の deadline。本番の 50 ms ではなく、遅い host でも一致を比べられる長さにする。
pub const LONG_DEADLINE: Duration = Duration::from_secs(30);
/// `timeout` のときにやり直す上限。
pub const TIMEOUT_RETRIES: usize = 20;

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `control-ts/`（bundle の source の置き場）。`CELERIS_SCRIPT_CONTROL_TS_DIR` が在ればその dir
/// （`scripts/dev/tests/script-bundle-fresh.sh` が source の写しと偽の dist を置き、作業ツリーに触れずに試す）。
pub fn control_ts_dir() -> PathBuf {
    std::env::var_os(CONTROL_TS_DIR_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root().join("control-ts"))
}

/// [`control_ts_dir`] を差し替える変数（試験の台本だけが使う）。
pub const CONTROL_TS_DIR_ENV: &str = "CELERIS_SCRIPT_CONTROL_TS_DIR";

/// bundle を読む試験が使う dir。`CELERIS_SCRIPT_BUNDLE_DIR` が在ればそこ、無ければ source から作った bundle の dir
/// （[`fresh_bundle_dir`]）。[`dist_bundle`] で使えると判定した dir と、配備（`deploy_bundle`）に渡す dir を揃える。
/// [`dist_bundle`] が `Some` を返した後に呼ぶ。
pub fn dist_dir() -> PathBuf {
    match bundle_dir_override() {
        Some(dir) => dir,
        None => fresh_bundle_dir()
            .unwrap_or_else(|why| panic!("no bundle (call dist_bundle first): {why}")),
    }
}

fn bundle_dir_override() -> Option<PathBuf> {
    std::env::var_os(BUNDLE_DIR_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// bundle を作る設定（`domains` の key が bundle の manifest の `domains` になる）。
pub fn bundle_config_path() -> PathBuf {
    control_ts_dir().join("bundle.config.json")
}

/// 試験に使う bundle の dir を外から渡す変数（付記 S5-9 (1)）。在れば source から作らず、使えないなら失敗にする。
pub const BUNDLE_DIR_ENV: &str = "CELERIS_SCRIPT_BUNDLE_DIR";
/// SKIP を 1 行ずつ足す file（付記 S5-9 (5)）。`scripts/dev/test-affected.sh` が数える。
pub const SKIP_LOG_ENV: &str = "CELERIS_SCRIPT_SKIP_LOG";
/// 内容 hash に入れる `control-ts/` の file（`src/` の下の全 file に加える）。`build-bundle.mjs` 自身も入れる。
const INPUT_FILES: [&str; 4] = [
    "bundle.config.json",
    "package.json",
    "pnpm-lock.yaml",
    "scripts/build-bundle.mjs",
];
/// 内容 hash に入れる host の schema（`control-ts/` の親からの相対 path。build-bundle が manifest に hash を書く）。
const SCHEMA_FILE: &str = "docs/api/script-host/host-v1.schema.json";

/// `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` なら、使えない bundle を SKIP でなく失敗にする。
pub fn require_bundle() -> bool {
    std::env::var("CELERIS_SCRIPT_REQUIRE_BUNDLE").as_deref() == Ok("1")
}

/// 試験に使う bundle のうち、`domain` を含み、いまの `bundle.config.json` と同じ領域の集合を持つもの。
///
/// `CELERIS_SCRIPT_BUNDLE_DIR` が在ればその dir を読み、使えないなら失敗にする。無ければ control-ts の source から
/// `$TMPDIR` に bundle を作って読む（[`fresh_bundle_dir`]）。`control-ts/dist` は読まない（task の根に古い物が残り、
/// 古い本文で試験が誤って落ちる。付記 S5-9）。作れない（node・node_modules が無い等）なら SKIP（`None`）。
/// `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` なら SKIP せず失敗にする。
pub fn dist_bundle(test: &str, domain: &str) -> Option<VerifiedBundle> {
    match bundle_dir_override() {
        Some(dir) => {
            let checked = check_bundle(&dir, &bundle_config_path(), domain);
            take_bundle(checked, &dir, test, true)
        }
        None => {
            let (checked, dir) = match fresh_bundle_dir() {
                Ok(dir) => (check_bundle(&dir, &bundle_config_path(), domain), dir),
                Err(why) => (Err(why), control_ts_dir()),
            };
            take_bundle(checked, &dir, test, require_bundle())
        }
    }
}

/// [`check_bundle`] の結果を試験に渡す。使えなければ、`require` なら panic し、そうでなければ SKIP を stderr と
/// `CELERIS_SCRIPT_SKIP_LOG` の file（在れば。1 行 = 試験名・tab・理由）に残して `None` を返す。
pub fn take_bundle(
    checked: Result<VerifiedBundle, String>,
    dir: &Path,
    test: &str,
    require: bool,
) -> Option<VerifiedBundle> {
    let log = std::env::var_os(SKIP_LOG_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    take_bundle_logged(checked, dir, test, require, log.as_deref())
}

/// [`take_bundle`] の本体。`log` は SKIP の行を足す file。
pub fn take_bundle_logged(
    checked: Result<VerifiedBundle, String>,
    dir: &Path,
    test: &str,
    require: bool,
    log: Option<&Path>,
) -> Option<VerifiedBundle> {
    let why = match checked {
        Ok(bundle) => return Some(bundle),
        Err(why) => why,
    };
    assert!(
        !require,
        "CELERIS_SCRIPT_REQUIRE_BUNDLE=1 or {BUNDLE_DIR_ENV} is set but {} is not usable: {why}",
        dir.display()
    );
    eprintln!("SKIP {test}: {} is not usable: {why}", dir.display());
    if let Some(log) = log {
        use std::io::Write as _;
        let line = format!("{test}\t{}: {}\n", dir.display(), why.replace('\n', " "));
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
            .and_then(|mut f| f.write_all(line.as_bytes()))
            .unwrap_or_else(|e| panic!("append {}: {e}", log.display()));
    }
    None
}

/// control-ts の source から作った bundle の dir（`$TMPDIR/celeris-script-bundle/<内容 hash>`）。
/// 同じ process の中では 1 回だけ判定する。別の process（nextest は試験ごとに process を分ける）とは、同じ内容 hash の
/// dir を使い回す。
pub fn fresh_bundle_dir() -> Result<PathBuf, String> {
    static DIR: std::sync::OnceLock<Result<PathBuf, String>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        build_bundle_cached(
            &control_ts_dir(),
            &std::env::temp_dir().join("celeris-script-bundle"),
        )
    })
    .clone()
}

/// `control_ts` の source から `cache/<内容 hash>` に bundle を作る（在れば作らない）。
///
/// 内容 hash（[`control_ts_inputs_sha256`]）が同じ dir は同じ source から作った物なので、古い bundle を読むことはない。
/// 並んで走る process が同時に作らないよう `<hash>.lock` を `create_new` で取り、取れない process は出来上がるのを待つ。
/// 作るのは `control_ts/scripts/build-bundle.mjs` の `buildBundle`（`ts-verify.sh`・`test-affected.sh` と同じ）。
pub fn build_bundle_cached(control_ts: &Path, cache: &Path) -> Result<PathBuf, String> {
    let hash = control_ts_inputs_sha256(control_ts)?;
    let dir = cache.join(&hash);
    let ready = |d: &Path| d.join("manifest.json").is_file();
    if ready(&dir) {
        return Ok(dir);
    }
    std::fs::create_dir_all(cache).map_err(|e| format!("mkdir {}: {e}", cache.display()))?;
    let lock = cache.join(format!("{hash}.lock"));
    // 待つ上限。持ち主が殺されて lock が残っても、上限の後は自分で作る。
    let deadline = std::time::Instant::now() + Duration::from_secs(300);
    loop {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
        {
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if ready(&dir) {
                    return Ok(dir);
                }
                if std::time::Instant::now() > deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(format!("create {}: {e}", lock.display())),
        }
    }
    let result = (|| {
        if ready(&dir) {
            return Ok(dir.clone());
        }
        let tmp = cache.join(format!("{hash}.tmp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        run_build_bundle(control_ts, &tmp)?;
        std::fs::rename(&tmp, &dir).map_err(|e| format!("rename {}: {e}", tmp.display()))?;
        Ok(dir.clone())
    })();
    let _ = std::fs::remove_file(&lock);
    result
}

/// `node` で `control_ts/scripts/build-bundle.mjs` の `buildBundle({ outDir: out })` を呼ぶ。
fn run_build_bundle(control_ts: &Path, out: &Path) -> Result<(), String> {
    let script = control_ts.join("scripts/build-bundle.mjs");
    let output = std::process::Command::new("node")
        .args([
            "--input-type=module",
            "-e",
            "const { buildBundle } = await import(process.env.CELERIS_BUILD_BUNDLE);\n\
             await buildBundle({ outDir: process.env.CELERIS_BUILD_BUNDLE_OUT });",
        ])
        .env(
            "CELERIS_BUILD_BUNDLE",
            url_of(&script).ok_or_else(|| format!("{} is not absolute", script.display()))?,
        )
        .env("CELERIS_BUILD_BUNDLE_OUT", out)
        .current_dir(control_ts)
        .output()
        .map_err(|e| format!("cannot run node to build the bundle: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(3).collect();
        return Err(format!(
            "build-bundle.mjs failed ({}): {}",
            output.status,
            tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
        ));
    }
    if !out.join("manifest.json").is_file() {
        return Err(format!(
            "build-bundle.mjs wrote no {}/manifest.json",
            out.display()
        ));
    }
    Ok(())
}

/// `import()` に渡す file URL（絶対 path のときだけ）。
fn url_of(path: &Path) -> Option<String> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    Some(format!("file://{}", abs.display()))
}

/// bundle の source の内容 hash: `src/` の下の全 file と `bundle.config.json`・`package.json`・`pnpm-lock.yaml`・
/// `scripts/build-bundle.mjs` を、1 行 = `<file の sha256>  <control_ts からの相対 path>\n` で path の昇順に並べ、
/// 最後に host の schema（`control_ts` の親の `docs/api/script-host/`）の行を足した本文の sha256。[`build_bundle_cached`] の dir の名前に使う。
pub fn control_ts_inputs_sha256(control_ts: &Path) -> Result<String, String> {
    fn walk(dir: &Path, rel: &str, out: &mut Vec<String>) -> Result<(), String> {
        let entries = std::fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("read {}: {e}", dir.display()))?;
            let kind = entry
                .file_type()
                .map_err(|e| format!("stat {}: {e}", entry.path().display()))?;
            let child = format!("{rel}/{}", entry.file_name().to_string_lossy());
            if kind.is_dir() {
                walk(&entry.path(), &child, out)?;
            } else if kind.is_file() {
                out.push(child);
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(&control_ts.join("src"), "src", &mut files)?;
    files.extend(INPUT_FILES.iter().map(|f| (*f).to_owned()));
    files.sort();
    let mut text = String::new();
    let mut add = |path: PathBuf, rel: &str| -> Result<(), String> {
        let bytes = std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        text.push_str(&format!("{}  {rel}\n", sha256_hex(&bytes)));
        Ok(())
    };
    for rel in &files {
        add(control_ts.join(rel), rel)?;
    }
    add(control_ts.join("..").join(SCHEMA_FILE), SCHEMA_FILE)?;
    Ok(sha256_hex(text.as_bytes()))
}

/// `dir` の bundle を使えるか判定する。`config` は `bundle.config.json` の path。
///
/// 古い bundle（manifest の `domains` が config の `domains` の key 集合と違う、`domain` を含まない、または
/// manifest の `capabilities` が config の各領域の capability の和集合と違う）と、読めない bundle は `Err`（理由）。
pub fn check_bundle(dir: &Path, config: &Path, domain: &str) -> Result<VerifiedBundle, String> {
    let bundle = load_bundle_dir(dir).map_err(|e| e.to_string())?;
    let text = std::fs::read_to_string(config)
        .unwrap_or_else(|e| panic!("read {}: {e}", config.display()));
    let value: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", config.display()));
    let expected: BTreeSet<String> = value["domains"]
        .as_object()
        .unwrap_or_else(|| panic!("{} has no domains object", config.display()))
        .keys()
        .cloned()
        .collect();
    let actual: BTreeSet<String> = bundle.manifest.domains.iter().cloned().collect();
    if !actual.contains(domain) || actual != expected {
        return Err(format!(
            "stale bundle: manifest domains {actual:?} (source_commit {}) do not match \
             {} domains {expected:?} or lack {domain:?}",
            bundle.manifest.source_commit,
            config.display()
        ));
    }
    // 付記 S4-13: capability を足した config の前に作った bundle も古い（op が `denied` になる）。
    let expected_caps: BTreeSet<String> = value["domains"]
        .as_object()
        .into_iter()
        .flat_map(|d| d.values())
        .filter_map(|caps| caps.as_array())
        .flatten()
        .filter_map(|c| c.as_str().map(str::to_owned))
        .collect();
    let actual_caps: BTreeSet<String> = bundle
        .manifest
        .capabilities
        .iter()
        .map(|c| c.as_str().to_owned())
        .collect();
    if actual_caps != expected_caps {
        return Err(format!(
            "stale bundle: manifest capabilities {actual_caps:?} (source_commit {}) do not match \
             {} capabilities {expected_caps:?}",
            bundle.manifest.source_commit,
            config.display()
        ));
    }
    Ok(bundle)
}

/// 同じ host（deno_core の actor）で bundle を起こし、判定の口にする。
pub fn start_policy(
    source: ScriptSource,
    sha256: String,
) -> (ScriptRuntimeHandle, Arc<ScriptPolicy>) {
    let handle = ScriptRuntimeHandle::start(source).expect("start the script runtime");
    (handle.clone(), Arc::new(ScriptPolicy::new(handle, sha256)))
}

/// この file の中の本文（例外・違う答えの bundle）を同じ host で起こす。
pub fn start_code(code: &str) -> (ScriptRuntimeHandle, Arc<ScriptPolicy>, String) {
    let sha256 = sha256_hex(code.as_bytes());
    let source = ScriptSource {
        name: format!("celeris:bundle/{sha256}.mjs"),
        code: code.to_owned(),
        capabilities: BTreeSet::new(),
    };
    let (handle, policy) = start_policy(source, sha256.clone());
    (handle, policy, sha256)
}

/// 試験の config。`sources` は `[llm_sources.local]` の追加の行（機密除去の試験が base URL 等を足す）。
pub fn config_text(sources: &str, script: &str) -> String {
    format!(
        r#"
db = "celeris.sqlite3"
workspace_root = "ws"
tick_ms = 50

[llm_sources.local]
kind = "fake"
accounts = ["a1"]
models = ["fake"]
{sources}

[harnesses.fake]
kind = "fake"
api_formats = ["fake"]
source_kinds = ["fake"]
command = "sh"
args = ["-c", "cat >/dev/null; echo '{{\"type\":\"question\",\"text\":\"not needed\"}}'"]

[route_defaults.fake]
candidates = [{{ source = "local", model = "fake" }}]

[harness_selection]
execution_class_defaults = {{}}
rules = [{{ harness = "fake", required_capabilities = [] }}]
{script}
"#
    )
}

pub fn write_config(home: &Path, text: &str) -> Config {
    let path = home.join("config.toml");
    std::fs::write(&path, text).expect("config");
    Config::load(&path).expect("load config")
}

pub fn load_config(home: &Path, script: &str) -> Config {
    write_config(home, &config_text("", script))
}

pub struct Env {
    pub dir: tempfile::TempDir,
    pub store: SqliteStore,
    pub config: Config,
    /// compare する領域（mode の行と generation の manifest）。
    pub domain: &'static str,
}

impl Env {
    pub fn new(domain: &'static str) -> Self {
        Self::with_sources(domain, "")
    }

    pub fn with_sources(domain: &'static str, sources: &str) -> Self {
        Self::with_config(domain, sources, "")
    }

    /// `extra` は config の末尾に足す表（`[cos]` など）。
    pub fn with_config(domain: &'static str, sources: &str, extra: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("ws")).expect("ws");
        let config = write_config(dir.path(), &config_text(sources, extra));
        let store = SqliteStore::open(&config.db.path).expect("store");
        Self {
            dir,
            store,
            config,
            domain,
        }
    }

    pub fn dispatcher(&self) -> Dispatcher {
        build_dispatcher(&self.config, ClusterMasters::default()).expect("dispatcher")
    }

    pub fn generation(&self, sha256: &str) -> ScriptGeneration {
        self.store
            .script_generation_record(
                &NewScriptGeneration {
                    bundle_sha256: sha256.to_owned(),
                    host_api: celeris_script::dto::HOST_API_VERSION.to_owned(),
                    schema_sha256: "0".repeat(64),
                    manifest_json: serde_json::json!({ "domains": [self.domain] }).to_string(),
                    state: ScriptGenerationState::Loaded,
                },
                OffsetDateTime::now_utc(),
            )
            .expect("generation")
    }

    /// 配備した bundle の generation を、登録（付記 S5-1）と同じ形で `loaded` に記録する。daemon は mode の行が指せば
    /// 起動時にこの bundle を新しい isolate で load する。
    pub fn deployed_generation(&self, bundle: &VerifiedBundle) -> ScriptGeneration {
        self.store
            .script_generation_record(
                &NewScriptGeneration {
                    bundle_sha256: bundle.manifest.bundle_sha256.clone(),
                    host_api: celeris_script::dto::HOST_API_VERSION.to_owned(),
                    schema_sha256: bundle.manifest.schema_sha256.clone(),
                    manifest_json: serde_json::to_string(&bundle.manifest).expect("manifest"),
                    state: ScriptGenerationState::Loaded,
                },
                OffsetDateTime::now_utc(),
            )
            .expect("generation")
    }

    /// mode の行から外れて `draining` になった generation を、付記 S5-1・S5-3 の known-good の戻り道
    /// （`retired` にしてから同じ hash を登録し直して `loaded` に戻す）で再び指せるようにする。
    /// `draining` の generation へ直接戻すことは store が拒む（付記 S5-4）ので、その規則は弱めない。
    /// 試験では daemon は止まっていて in-flight は 0 なので、drain の終わりとして `retired` にしてよい。
    pub fn rearm_generation(&self, id: &str) -> ScriptGeneration {
        let current = self
            .store
            .script_generation_get(id)
            .expect("read")
            .expect("generation");
        // daemon の起動時の復元は、指されない `draining` を drain の終わりとして `retired` にする（付記 S5-2）。
        // daemon を通らずに戻した行は `draining` のまま。
        assert!(
            matches!(
                current.state,
                ScriptGenerationState::Draining | ScriptGenerationState::Retired
            ),
            "rearm expects a generation left draining or retired by a rust restore, got {:?}",
            current.state
        );
        if current.state == ScriptGenerationState::Draining {
            self.store
                .script_generation_transition(
                    &ScriptGenerationTransition {
                        id: id.to_owned(),
                        to: ScriptGenerationState::Retired,
                        actor: "human:test".into(),
                        reason: "drained (daemon stopped)".into(),
                        cancelled_in_flight: None,
                    },
                    OffsetDateTime::now_utc(),
                )
                .expect("retire");
        }
        let again = self
            .store
            .script_generation_record(
                &NewScriptGeneration {
                    bundle_sha256: current.bundle_sha256.clone(),
                    host_api: current.host_api.clone(),
                    schema_sha256: current.schema_sha256.clone(),
                    manifest_json: current.manifest_json.clone(),
                    state: ScriptGenerationState::Loaded,
                },
                OffsetDateTime::now_utc(),
            )
            .expect("re-register");
        assert_eq!(again.id, current.id, "re-registration keeps the row");
        assert_eq!(again.state, ScriptGenerationState::Loaded);
        again
    }

    /// mode を変える（人の actor で）。`ts` は付記 S5-4 のとおり同じ generation の `canary` を経る: `compare` からなら
    /// 昇格の前提の計数を script-guard に積み、`canary`（5000 bps）にしてから `ts` にする。
    pub fn set_mode(&self, mode: ScriptMode, generation: Option<String>) {
        let current = self.store.script_domain_mode(self.domain).expect("mode");
        if mode == ScriptMode::Ts
            && current.mode == ScriptMode::Compare
            && let Some(id) = generation.as_deref()
        {
            self.observe_clean(id, false);
            self.store
                .script_domain_set_canary(
                    &ScriptModeChange {
                        domain: self.domain.into(),
                        to_mode: ScriptMode::Canary,
                        to_generation: generation.clone(),
                        expected_version: current.version,
                        actor: "human:test".into(),
                        reason: "script_compare test".into(),
                    },
                    5000,
                    OffsetDateTime::now_utc(),
                )
                .expect("set canary");
            self.observe_clean(id, true);
        }
        let version = self
            .store
            .script_domain_mode(self.domain)
            .expect("mode")
            .version;
        self.store
            .script_domain_set_mode(
                &ScriptModeChange {
                    domain: self.domain.into(),
                    to_mode: mode,
                    to_generation: generation,
                    expected_version: version,
                    actor: "human:test".into(),
                    reason: "script_compare test".into(),
                },
                OffsetDateTime::now_utc(),
            )
            .expect("set mode");
    }

    /// 昇格の前提（(f) の「進む条件」）の件数だけ、成功の呼び出しを数える。
    fn observe_clean(&self, generation_id: &str, ts_side: bool) {
        use task_core::script_guard::{
            SCRIPT_GUARD_READY_COS, SCRIPT_GUARD_READY_PURE, ScriptGuardCall,
            ScriptGuardObservation,
        };
        let domain = task_core::script_compare::ScriptDomain::parse(self.domain).expect("domain");
        let required = match domain {
            task_core::script_compare::ScriptDomain::CosDigest => SCRIPT_GUARD_READY_COS,
            _ => SCRIPT_GUARD_READY_PURE,
        };
        for _ in 0..required {
            self.store
                .script_guard_observe(
                    domain,
                    generation_id,
                    ScriptGuardObservation {
                        call: ScriptGuardCall::Ok,
                        safety_violation: false,
                        ts_side,
                    },
                    OffsetDateTime::now_utc(),
                )
                .expect("observe");
        }
    }

    pub fn mode(&self) -> ScriptMode {
        self.store
            .script_domain_mode(self.domain)
            .expect("mode")
            .mode
    }

    pub fn insert_task(&self) -> Task {
        let task = hinted_task(self.dir.path());
        self.store.insert(&task).expect("insert");
        task
    }

    pub fn events(&self, task: &Task) -> Vec<Event> {
        self.store
            .events_for(task.id)
            .expect("events")
            .into_iter()
            .map(|(_, e)| e)
            .collect()
    }

    pub fn records(&self, task: &Task) -> Vec<ScriptCompareRecord> {
        self.events(task)
            .into_iter()
            .filter_map(|e| match e {
                Event::ScriptCompareRecorded { record } => Some(*record),
                _ => None,
            })
            .collect()
    }

    pub fn routing(&self, task: &Task) -> RoutingRecord {
        self.events(task)
            .into_iter()
            .find_map(|e| match e {
                Event::RoutingDecided { record, .. } => Some(*record),
                _ => None,
            })
            .expect("routing_decided")
    }

    /// 1 件の task を 1 tick で dispatch し、走った compare を待つ。
    pub async fn dispatch(&self, d: &mut Dispatcher) -> (Task, Vec<ScriptCompareRecord>) {
        let task = self.insert_task();
        d.tick().expect("tick");
        d.wait_script_compares().await;
        let records = self.records(&task);
        (task, records)
    }

    /// `timeout` 以外の結果が出るまで dispatch する（file 先頭の待ちの方針）。
    pub async fn dispatch_until_compared(&self, d: &mut Dispatcher) -> (Task, ScriptCompareRecord) {
        for _ in 0..TIMEOUT_RETRIES {
            let (task, records) = self.dispatch(d).await;
            assert_eq!(records.len(), 1, "{records:?}");
            let record = records.into_iter().next().expect("one record");
            if !is_timeout(&record) {
                return (task, record);
            }
        }
        panic!("every compare timed out ({TIMEOUT_RETRIES} attempts)");
    }

    /// mode rust・判定の口なしの判定（比べる基準）の task。
    pub async fn baseline(&self) -> Task {
        let mut d = self.dispatcher();
        let (task, records) = self.dispatch(&mut d).await;
        assert!(records.is_empty(), "{records:?}");
        task
    }
}

pub fn is_timeout(record: &ScriptCompareRecord) -> bool {
    record.outcome == CompareOutcome::TsError
        && record.error.as_ref().is_some_and(|e| e.kind == "timeout")
}

/// `lane_assist` の試験と同じ形の task（`TierSource::Hint` で規則表が決める）。
pub fn hinted_task(dir: &Path) -> Task {
    let id = TaskId::new();
    let now = OffsetDateTime::now_utc();
    Task {
        harness_id: None,
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: Some(TaskRouting {
            tier_source: TierSource::Hint,
            ..Default::default()
        }),
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id,
        parent_id: None,
        kind: TaskKind::Execute,
        title: "typo".into(),
        objective: "crates/task-core/src/model.rs の typo を直す".into(),
        acceptance: vec![Criterion {
            text: "試験が通る".into(),
            check: Check::Command {
                cmd: "cargo test".into(),
                expect_exit: 0,
            },
        }],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Ready,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
        },
        workspace: WorkspaceSpec::Local {
            path: dir.join(id.to_string()),
            mode: None,
        },
        budget: Budget {
            max_turns: 10,
            max_wall_secs: 600,
            max_retries: 2,
            max_tokens: None,
        },
        attempts: 0,
        lease: None,
        created_at: now,
        updated_at: now,
        role: None,
        genre: Some("coding".into()),
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: None,
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}
