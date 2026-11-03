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
    ScriptGenerationState, ScriptMode, ScriptModeChange,
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

/// `control-ts/`（source と既定の dist の置き場）。`CELERIS_SCRIPT_CONTROL_TS_DIR` が在ればその dir
/// （`scripts/dev/tests/script-bundle-fresh.sh` が source の写しと偽の dist を置き、作業ツリーの dist に触れずに試す）。
pub fn control_ts_dir() -> PathBuf {
    std::env::var_os(CONTROL_TS_DIR_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root().join("control-ts"))
}

/// [`control_ts_dir`] を差し替える変数（試験の台本だけが使う）。
pub const CONTROL_TS_DIR_ENV: &str = "CELERIS_SCRIPT_CONTROL_TS_DIR";

/// bundle を読む試験が使う dir。`CELERIS_SCRIPT_BUNDLE_DIR` が在ればそこ、無ければ `control-ts/dist`。
/// [`dist_bundle`] で使えると判定した dir と、配備（`deploy_bundle`）に渡す dir を揃える。
pub fn dist_dir() -> PathBuf {
    bundle_dir_override().unwrap_or_else(|| control_ts_dir().join("dist"))
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

/// 試験に使う bundle の dir を外から渡す変数（付記 S5-9 (1)）。在れば `control-ts/dist` を読まず、使えないなら失敗にする。
pub const BUNDLE_DIR_ENV: &str = "CELERIS_SCRIPT_BUNDLE_DIR";
/// SKIP を 1 行ずつ足す file（付記 S5-9 (5)）。`scripts/dev/test-affected.sh` が数える。
pub const SKIP_LOG_ENV: &str = "CELERIS_SCRIPT_SKIP_LOG";
/// `build-bundle.mjs` が dist に書く source の内容 hash の file（付記 S5-9 (2)）。
pub const INPUTS_FILE: &str = "inputs.sha256";
/// 内容 hash に入れる `control-ts/` 直下の file（`src/` の下の全 file に加える）。
const INPUT_FILES: [&str; 3] = ["bundle.config.json", "package.json", "pnpm-lock.yaml"];

/// `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` なら、使えない bundle を SKIP でなく失敗にする。
pub fn require_bundle() -> bool {
    std::env::var("CELERIS_SCRIPT_REQUIRE_BUNDLE").as_deref() == Ok("1")
}

/// 試験に使う bundle のうち、`domain` を含み、いまの `bundle.config.json` と同じ領域の集合を持つもの。
///
/// `CELERIS_SCRIPT_BUNDLE_DIR` が在ればその dir を読み、使えないなら失敗にする。無ければ `control-ts/dist` を読み、
/// 無い・読めない・古い（[`check_dist`]）なら SKIP（`None`）。`CELERIS_SCRIPT_REQUIRE_BUNDLE=1` なら SKIP せず失敗にする。
pub fn dist_bundle(test: &str, domain: &str) -> Option<VerifiedBundle> {
    match bundle_dir_override() {
        Some(dir) => {
            let checked = check_bundle(&dir, &bundle_config_path(), domain);
            take_bundle(checked, &dir, test, true)
        }
        None => take_bundle(
            check_dist(&control_ts_dir(), domain),
            &control_ts_dir().join("dist"),
            test,
            require_bundle(),
        ),
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

/// `control_ts` の `dist` の bundle を使えるか判定する。[`check_bundle`] に加えて、dist の `inputs.sha256` が
/// 作業ツリーの source の内容 hash（[`control_ts_inputs_sha256`]）と合うことを確かめる。
///
/// `control-ts/dist` は git 管理外で、task の根に古い物が残る。領域と capability が config と揃っていても、
/// source を変える前の bundle を正として比べると、TS の実装でなく bundle の古さで落ちる（付記 S5-9）。
pub fn check_dist(control_ts: &Path, domain: &str) -> Result<VerifiedBundle, String> {
    let dist = control_ts.join("dist");
    let bundle = check_bundle(&dist, &control_ts.join("bundle.config.json"), domain)?;
    let recorded = std::fs::read_to_string(dist.join(INPUTS_FILE))
        .map_err(|e| format!("stale bundle: no {INPUTS_FILE} ({e}); rebuild control-ts/dist"))?;
    let actual = control_ts_inputs_sha256(control_ts)?;
    if recorded.trim() != actual {
        return Err(format!(
            "stale bundle: {INPUTS_FILE} {} (source_commit {}) does not match the sources {actual}; \
             rebuild control-ts/dist",
            recorded.trim(),
            bundle.manifest.source_commit
        ));
    }
    Ok(bundle)
}

/// `control-ts/scripts/build-bundle.mjs` の `inputsSha256` と同じ値: `src/` の下の全 file と `bundle.config.json`・
/// `package.json`・`pnpm-lock.yaml` を、1 行 = `<file の sha256>  <control_ts からの相対 path>\n` で path の昇順に並べた
/// 本文の sha256。
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
    for rel in &files {
        let path = control_ts.join(rel);
        let bytes = std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        text.push_str(&format!("{}  {rel}\n", sha256_hex(&bytes)));
    }
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
             {} domains {expected:?} or lack {domain:?}; rebuild control-ts/dist",
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
             {} capabilities {expected_caps:?}; rebuild control-ts/dist",
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

    pub fn set_mode(&self, mode: ScriptMode, generation: Option<String>) {
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
