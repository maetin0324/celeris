//! 付記 S2: 本物の deno_core host で compare を確かめる試験（`script_compare_lane_rules.rs`・
//! `script_compare_route_priority.rs`）の共通部分。userns を使わない。外部ネットワークに出ない。
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

pub fn dist_dir() -> PathBuf {
    repo_root().join("control-ts/dist")
}

/// bundle を作る設定（`domains` の key が bundle の manifest の `domains` になる）。
pub fn bundle_config_path() -> PathBuf {
    repo_root().join("control-ts/bundle.config.json")
}

/// `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` なら、使えない bundle を SKIP でなく失敗にする。
pub fn require_bundle() -> bool {
    std::env::var("CELERIS_SCRIPT_REQUIRE_BUNDLE").as_deref() == Ok("1")
}

/// `control-ts/dist` の bundle のうち、`domain` を含み、いまの `bundle.config.json` と同じ領域の集合を持つもの。
/// 無い・読めない・古いなら SKIP（`None`）。`CELERIS_SCRIPT_REQUIRE_BUNDLE=1` なら失敗にする。
pub fn dist_bundle(test: &str, domain: &str) -> Option<VerifiedBundle> {
    usable_bundle(
        &dist_dir(),
        &bundle_config_path(),
        test,
        domain,
        require_bundle(),
    )
}

/// `dir` の bundle を使えるか判定する。`config` は `bundle.config.json` の path。
///
/// `control-ts/dist` は git 管理外なので、領域を足す前の source で作った古い bundle が作業ツリーに残りうる。
/// 古い bundle（manifest の `domains` が config の `domains` の key 集合と違う、または `domain` を含まない）を
/// 正として比べると、TS の実装でなく bundle の古さで落ちる。だから使えない bundle と同じく SKIP にする。
/// `require` が真なら SKIP せず panic する。
pub fn usable_bundle(
    dir: &Path,
    config: &Path,
    test: &str,
    domain: &str,
    require: bool,
) -> Option<VerifiedBundle> {
    let skip = |why: String| {
        assert!(
            !require,
            "CELERIS_SCRIPT_REQUIRE_BUNDLE=1 but {} is not usable: {why}",
            dir.display()
        );
        eprintln!("SKIP {test}: {} is not usable: {why}", dir.display());
    };
    let bundle = match load_bundle_dir(dir) {
        Ok(bundle) => bundle,
        Err(e) => {
            skip(e.to_string());
            return None;
        }
    };
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
        skip(format!(
            "stale bundle: manifest domains {actual:?} (source_commit {}) do not match \
             {} domains {expected:?} or lack {domain:?}; rebuild control-ts/dist",
            bundle.manifest.source_commit,
            config.display()
        ));
        return None;
    }
    Some(bundle)
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
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("ws")).expect("ws");
        let config = write_config(dir.path(), &config_text(sources, ""));
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
