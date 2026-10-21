use super::*;
use crate::daemon::secrets::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use task_api::types::GenreConfigView;
use task_core::{DaemonMode, InstanceRole, SharedRole, SqliteStore, TaskStore};
use time::OffsetDateTime;

#[test]
fn seeds_the_org_once_into_an_empty_db_and_never_again() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../config/org.example.toml"),
        dir.path().join("org.toml"),
    )
    .unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        format!(
            "db = \"celeris.sqlite3\"\nworkspace_root = \"ws\"\norg_include = \"org.toml\"\n{}",
            r#"

[[roles]]
id = "implementer"

[[roles]]
id = "literature-reader"

[[roles]]
id = "cos-role"

[[genres]]
id = "conversation"
description = "人と話す"
default_role = "cos-role"
roles = ["cos-role"]

[[genres]]
id = "coding"
description = "コードを書く"
default_role = "implementer"
roles = ["implementer"]

[[genres]]
id = "literature"
description = "関連研究の調査"
default_role = "literature-reader"
roles = ["literature-reader"]

[[roles]]
id = "web-researcher"

[[genres]]
id = "web-research"
description = "一般 Web の調査"
default_role = "web-researcher"
roles = ["web-researcher"]

[[roles]]
id = "data-analyst"

[[genres]]
id = "data-analysis"
description = "データを整える"
default_role = "data-analyst"
roles = ["data-analyst"]

[[roles]]
id = "writer"

[[genres]]
id = "writing"
description = "書く"
default_role = "writer"
roles = ["writer"]

[conversation]
genre = "conversation"
"#
        ),
    )
    .unwrap();
    let config = Config::load(&path).unwrap();
    let store = SqliteStore::open(&config.db.path).unwrap();

    assert_eq!(seed_org_if_empty(&store, &config).unwrap(), 15);
    let nodes = store.org_list().unwrap();
    assert_eq!(nodes.len(), 15);
    let cos = nodes.iter().find(|n| n.id == "cos").unwrap();
    assert_eq!(cos.kind, task_core::OrgKind::Secretary);
    assert_eq!(cos.parent_id, None);
    assert_eq!(
        nodes
            .iter()
            .find(|n| n.id == "software-engineering")
            .unwrap()
            .genre
            .as_deref(),
        Some("coding")
    );

    // 人が GUI で名前を変えても、次の起動で設定に戻されない。
    let mut renamed = cos.clone();
    renamed.name = "本人".into();
    store.org_upsert(&renamed).unwrap();
    assert_eq!(seed_org_if_empty(&store, &config).unwrap(), 0);
    assert_eq!(store.org_get("cos").unwrap().unwrap().name, "本人");
    assert_eq!(store.org_list().unwrap().len(), 15);
}

/// 監査 D-4: `org_include` の並びに木としての不整合（種類の順序。`Config::load` は循環・順序までは
/// 見ない。org.rs のコメント参照）があれば、`seed_org_if_empty` は 1 件も書かずにエラーを返す
/// （部分的に蒔かれた組織が残ると、次回起動時は `org_list` が空でなくなり二度と補完されない）。
#[test]
fn seed_org_if_empty_writes_nothing_when_one_node_breaks_the_tree() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("org.toml"),
        r#"
[[org]]
id = "secretary"
name = "秘書"
kind = "secretary"

[[org]]
id = "research"
name = "研究部"
kind = "department"
parent_id = "secretary"

[[org]]
id = "research-survey"
name = "調査課"
kind = "section"
parent_id = "research"

[[org]]
id = "research-survey-sub"
name = "壊れた子"
kind = "section"
parent_id = "research-survey"
"#,
    )
    .unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "db = \"celeris.sqlite3\"\nworkspace_root = \"ws\"\norg_include = \"org.toml\"\n",
    )
    .unwrap();
    let config = Config::load(&path).unwrap();
    let store = SqliteStore::open(&config.db.path).unwrap();

    let err = seed_org_if_empty(&store, &config).unwrap_err();
    assert!(err.to_string().contains("placed under"), "{err}");
    assert!(
        store.org_list().unwrap().is_empty(),
        "nothing is written on failure"
    );
}

/// `org_include` が無い設定では何も蒔かない。
#[test]
fn without_org_include_nothing_is_seeded() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("celeris.sqlite3");
    let config: Config = toml::from_str(&format!("db = \"{}\"\n", db.display())).unwrap();
    let store = SqliteStore::open(&config.db.path).unwrap();
    assert_eq!(seed_org_if_empty(&store, &config).unwrap(), 0);
    assert!(store.org_list().unwrap().is_empty());
}

/// ADR-0012 D1: 同じ claude-code を使う 2 アカウントが、それぞれの env と model を持つアダプタになる。
#[test]
fn config_view_exposes_genres() {
    let text = r#"
[[roles]]
id = "lead"

[[roles]]
id = "implementer"

[[genres]]
id = "coding"
description = "write and fix code"
default_role = "implementer"
roles = ["lead", "implementer"]

"#;
    let cfg: Config = toml::from_str(text).unwrap();
    cfg.validate().unwrap();
    let view = config_view(&cfg, "127.0.0.1:7710".parse().unwrap(), "i", "dev", "t");
    assert_eq!(
        view.genres,
        vec![GenreConfigView {
            id: "coding".into(),
            description: "write and fix code".into(),
            capabilities: vec![],
            input_artifacts: vec![],
            output_artifacts: vec![],
            default_role: Some("implementer".into()),
            roles: vec!["lead".into(), "implementer".into()],
        }]
    );
}

/// ADR-0028 D1: `capabilities` / `input_artifacts` / `output_artifacts` を書けば `GET /config` の
/// `genres[]` にそのまま出る。
#[test]
fn config_view_exposes_genre_capabilities_and_artifacts() {
    let text = r#"
[[roles]]
id = "literature-reader"

[[genres]]
id = "literature"
description = "related work survey"
capabilities = ["academic literature search", "citation graph traversal"]
input_artifacts = ["question", "pdf"]
output_artifacts = ["answer.md: 引用付きの答え", "citations.json"]
default_role = "literature-reader"
roles = ["literature-reader"]

"#;
    let cfg: Config = toml::from_str(text).unwrap();
    cfg.validate().unwrap();
    let view = config_view(&cfg, "127.0.0.1:7710".parse().unwrap(), "i", "dev", "t");
    assert_eq!(
        view.genres[0].capabilities,
        vec![
            "academic literature search".to_string(),
            "citation graph traversal".to_string()
        ]
    );
    assert_eq!(
        view.genres[0].input_artifacts,
        vec!["question".to_string(), "pdf".to_string()]
    );
    // Phase 38（ADR-0028 追記）: `名前: 説明` を書いても `GenreConfigView` の型は変わらず、値の文字列に
    // 説明が付くだけ（GUI は `:` の前を名前として扱う。`docs/api/v1/api.md`）。
    assert_eq!(
        view.genres[0].output_artifacts,
        vec![
            "answer.md: 引用付きの答え".to_string(),
            "citations.json".to_string()
        ]
    );
    let json = serde_json::to_value(&view.genres[0]).unwrap();
    assert_eq!(json["capabilities"][0], "academic literature search");
    assert_eq!(json["output_artifacts"][0], "answer.md: 引用付きの答え");
    assert_eq!(json["output_artifacts"][1], "citations.json");
}

/// ADR-0026 D2/D3: `acp` プロバイダの行は `[adapters.acp]` の env に重ね、`command`/`args` は行の値が
/// 優先し、`model` は行の値がそのまま（`[adapters.acp]` にモデルの既定値は無い）。
#[test]
fn langmem_knowledge_config_defaults_to_disabled() {
    let cfg: Config = toml::from_str("").unwrap();
    assert!(!cfg.knowledge.langmem.enabled);
    assert_eq!(
        cfg.knowledge.langmem.provider,
        task_worker::LangMemProvider::OpenaiCompatible
    );
    assert!(cfg.knowledge.langmem.base_url.is_none());
    assert!(cfg.knowledge.langmem.model.is_none());
    assert!(cfg.knowledge.langmem.api_key_secret.is_none());
    assert_eq!(cfg.knowledge.langmem.max_related_pages, 10);
    assert_eq!(cfg.adapters.langmem.command, "python3");
    assert!(cfg.adapters.langmem.idle_timeout_secs.is_none());
}

/// 未知のアダプタは `langmem` を含めた既知の一覧で拒否される。
#[test]
fn unknown_adapter_message_lists_langmem() {
    let cfg: Config = toml::from_str("[[harnesses]]\nid = \"x\"\nadapter = \"bogus\"\n").unwrap();
    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("langmem"), "{err}");
}

// ---- ADR-0030 D2: `env_from_secrets` の優先順と欠落時の扱い ----

/// 優先順は `[adapters.*].env` < `[adapters.*].env_from_secrets` < 行の `env` < 行の `env_from_secrets`
/// （celeris 自身の環境はプロセス継承なのでここでは扱わない）。秘密が見つからない層はそのキーに触れず、
/// 下の層の値が残る。
#[test]
fn merged_env_with_secrets_follows_the_precedence_order_and_falls_back_when_a_secret_is_missing() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let secrets_dir = dir.path().join("secrets");
    std::fs::create_dir_all(&secrets_dir).unwrap_or_else(|e| panic!("mkdir: {e}"));
    std::fs::write(secrets_dir.join("id-base"), "base-secret\n")
        .unwrap_or_else(|e| panic!("write: {e}"));
    std::fs::write(secrets_dir.join("id-row"), "row-secret")
        .unwrap_or_else(|e| panic!("write: {e}"));
    // `id-row-missing` はわざと作らない（欠落を再現する）。

    let base_env = HashMap::from([
        ("K".to_string(), "base-env".to_string()),
        ("ONLY_BASE".to_string(), "b".to_string()),
    ]);
    let base_secrets = HashMap::from([("K".to_string(), "id-base".to_string())]);
    let row_env = HashMap::from([("K".to_string(), "row-env".to_string())]);
    let row_secrets = HashMap::from([("K".to_string(), "id-row".to_string())]);

    // 全層が揃っていれば行の env_from_secrets が勝つ。
    let merged = merged_env_with_secrets(
        &base_env,
        &base_secrets,
        &row_env,
        &row_secrets,
        Some(&secrets_dir),
    );
    let map: HashMap<String, String> = merged.into_iter().collect();
    assert_eq!(map.get("K"), Some(&"row-secret".to_string()));
    assert_eq!(map.get("ONLY_BASE"), Some(&"b".to_string()));

    // 行の env_from_secrets の秘密が無ければ、そのキーには触れず 1 段下（行の env）が残る。
    let row_secrets_missing = HashMap::from([("K".to_string(), "id-row-missing".to_string())]);
    let merged = merged_env_with_secrets(
        &base_env,
        &base_secrets,
        &row_env,
        &row_secrets_missing,
        Some(&secrets_dir),
    );
    let map: HashMap<String, String> = merged.into_iter().collect();
    assert_eq!(map.get("K"), Some(&"row-env".to_string()));

    // 行の env も無ければ、その下（`[adapters.*].env_from_secrets`）が残る。
    let empty: HashMap<String, String> = HashMap::new();
    let merged = merged_env_with_secrets(
        &base_env,
        &base_secrets,
        &empty,
        &row_secrets_missing,
        Some(&secrets_dir),
    );
    let map: HashMap<String, String> = merged.into_iter().collect();
    assert_eq!(map.get("K"), Some(&"base-secret".to_string()));

    // `[secrets]` 自体が未設定（`secrets_dir: None`）なら env_from_secrets は何も足さない（設定エラーにしない）。
    let merged = merged_env_with_secrets(&base_env, &base_secrets, &row_env, &row_secrets, None);
    let map: HashMap<String, String> = merged.into_iter().collect();
    assert_eq!(
        map.get("K"),
        Some(&"row-env".to_string()),
        "missing [secrets] falls back to the env layer, not an error"
    );

    // 末尾の改行は読み取り時に落ちる。
    let base_secrets_only = HashMap::from([("K".to_string(), "id-base".to_string())]);
    let merged = merged_env_with_secrets(
        &empty,
        &base_secrets_only,
        &empty,
        &empty,
        Some(&secrets_dir),
    );
    let map: HashMap<String, String> = merged.into_iter().collect();
    assert_eq!(map.get("K"), Some(&"base-secret".to_string()));
}

/// API settings exposes secret references without exposing secret values.
#[test]
fn api_settings_carries_secrets_dir_and_usage() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let path = dir.path().join("config.toml");
    std::fs::write(
            &path,
            "[secrets]\ndir = \"secrets\"\n\n[adapters.local_deep_research]\nenv_from_secrets = { TAVILY = \"tavily\" }\n\n",
        )
        .unwrap_or_else(|e| panic!("{e}"));
    let cfg = Config::load(&path).unwrap_or_else(|e| panic!("{e}"));
    let settings = api_settings(
        &cfg,
        "127.0.0.1:7710".parse().unwrap_or_else(|e| panic!("{e}")),
        None,
        "i".into(),
        "t".into(),
        None,
        "sha12sha12ab".into(),
        DaemonMode::Verify,
        SharedRole::new(InstanceRole::Standby),
        None,
    );
    assert_eq!(
        settings.secrets_dir,
        cfg.secrets.as_ref().map(|s| s.dir.clone())
    );
    assert!(settings.secret_usage.contains_key("tavily"));
    // ADR-0040 D3 / D4: `release` / `mode` / `role` はそのまま API へ渡る（`GET /health` に出る）。
    assert_eq!(settings.release, "sha12sha12ab");
    assert_eq!(settings.mode, DaemonMode::Verify);
    assert_eq!(settings.role.get(), InstanceRole::Standby);
}

/// S7: `[accounts]` は reload の対象外。`claude_dir` / `max_runs_per_account` / `check_model` のどれかが
/// 変わっていたら `reload` はエラー（400 に写る文字列）を返し、稼働中の状態には触れない。
#[test]
fn reload_providers_rejects_changes_to_the_accounts_section() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let accounts_dir = dir.path().join("accounts");
    std::fs::create_dir_all(&accounts_dir).unwrap_or_else(|e| panic!("{e}"));
    let config_path = dir.path().join("config.toml");
    let db = dir.path().join("celeris.db");
    let ws = dir.path().join("ws");
    let write_config = |max_runs: u32| {
        std::fs::write(
                &config_path,
                format!(
                    "db = {db:?}\nworkspace_root = {ws:?}\n[accounts]\nclaude_dir = {accounts_dir:?}\nmax_runs_per_account = {max_runs}\n"
                ),
            )
            .unwrap_or_else(|e| panic!("{e}"));
    };
    write_config(2);
    let mut config = Config::load(&config_path).unwrap_or_else(|e| panic!("{e}"));
    let mut dispatcher =
        build_dispatcher(&config, Default::default()).unwrap_or_else(|e| panic!("{e}"));

    // Individual settings survive reload, and reload reads authoritative DB values.
    dispatcher
        .set_account_limit("claude-code", "a", Some(3))
        .unwrap();
    assert!(reload_providers(&mut dispatcher, &mut config).is_ok());
    assert_eq!(dispatcher.account_max_runs("claude-code", "a"), Some(3));
    let store = SqliteStore::open(&db).unwrap();
    store
        .set_account_limit("claude-code", "a", Some(4))
        .unwrap();
    assert!(reload_providers(&mut dispatcher, &mut config).is_ok());
    assert_eq!(dispatcher.account_max_runs("claude-code", "a"), Some(4));

    // max_runs_per_account を変えると、次の reload はエラーになる。
    write_config(3);
    let err = reload_providers(&mut dispatcher, &mut config).unwrap_err();
    assert!(err.contains("[accounts]"), "{err}");
    assert!(err.contains("restart"), "{err}");
}

/// ADR 2026-10-08 account-concurrency-limits §(d)(e)（人の決定 implicit-account）: a declared LLM source
/// without an account pool gets one implicit account (`implicit/<llm source id>`). A stored override
/// survives `POST /reload` and a restart (new `build_dispatcher` on the same DB), and is pruned once
/// the source is no longer declared.
#[test]
fn implicit_account_limit_survives_reload_and_is_pruned_with_its_source() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let config_path = dir.path().join("config.toml");
    let db = dir.path().join("celeris.db");
    let ws = dir.path().join("ws");
    let write_config = |sources: &str| {
        std::fs::write(
            &config_path,
            format!(
                "db = {db:?}\nworkspace_root = {ws:?}\n\
                 [llm_sources.x]\nkind = \"fake\"\nmodels = [\"fake\"]\n{sources}"
            ),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    };
    let qwen = "[llm_sources.qwen]\nkind = \"fake\"\nmodels = [\"fake\"]\n";
    write_config(qwen);
    let mut config = Config::load(&config_path).unwrap_or_else(|e| panic!("{e}"));
    let mut dispatcher =
        build_dispatcher(&config, Default::default()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        dispatcher.implicit_account_ids(),
        vec!["qwen".to_string(), "x".to_string()]
    );
    assert_eq!(
        dispatcher.implicit_account_limit("qwen"),
        Some((2, task_dispatch::capacity::MaxRunsSource::Default))
    );
    assert_eq!(dispatcher.implicit_account_limit("none"), None);

    dispatcher
        .set_account_limit("implicit", "qwen", Some(1))
        .unwrap();
    assert!(reload_providers(&mut dispatcher, &mut config).is_ok());
    assert_eq!(
        dispatcher.implicit_account_limit("qwen"),
        Some((1, task_dispatch::capacity::MaxRunsSource::Account))
    );
    drop(dispatcher);
    let mut dispatcher =
        build_dispatcher(&config, Default::default()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(dispatcher.account_max_runs("implicit", "qwen"), Some(1));
    let store = SqliteStore::open(&db).unwrap();
    assert_eq!(store.account_limits().unwrap().len(), 1);

    // source の宣言を消すと、その暗黙アカウントの保存値は reload で prune される。
    write_config("");
    assert!(reload_providers(&mut dispatcher, &mut config).is_ok());
    assert_eq!(dispatcher.implicit_account_ids(), vec!["x".to_string()]);
    assert_eq!(dispatcher.account_max_runs("implicit", "qwen"), None);
    assert!(store.account_limits().unwrap().is_empty());
}

/// Phase 44（実機 2026-09-18）: `[[roles]]` の `max_turns` を変えて `reload` すると、次に作られる子の
/// budget が新しい値になる（`Dispatcher::config().roles` に反映される。委譲の子は `spawn_worker` の
/// 時点でこの写しを使う）。`[reports]` / `[notify]` / `[conversation]` も同様に `Config` 自身へ反映する。
#[test]
fn reload_rereads_roles_genres_delegation_and_reports_notify_conversation() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let config_path = dir.path().join("config.toml");
    let db = dir.path().join("celeris.db");
    let ws = dir.path().join("ws");
    let write_config = |max_turns: u32, notify_interval: u64| {
        std::fs::write(
            &config_path,
            format!(
                "db = {db:?}\nworkspace_root = {ws:?}\n\
                     [[roles]]\nid = \"implementer\"\nmax_turns = {max_turns}\n\
                     [delegation]\nmax_delegate_per_run = 3\n\
                     [notify]\ninterval_secs = {notify_interval}\n"
            ),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    };
    write_config(20, 30);
    let mut config = Config::load(&config_path).unwrap_or_else(|e| panic!("{e}"));
    let mut dispatcher =
        build_dispatcher(&config, Default::default()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(dispatcher.config().roles[0].max_turns, Some(20));
    assert_eq!(dispatcher.config().delegation.max_delegate_per_run, 3);

    // `[[roles]] implementer` の `max_turns` を 20 → 60、`[delegation]` と `[notify]` も変える。
    write_config(60, 90);
    assert!(reload_providers(&mut dispatcher, &mut config).is_ok());

    // ディスパッチャ側（次に作られる子の budget が読む先）。
    assert_eq!(dispatcher.config().roles[0].max_turns, Some(60));
    assert_eq!(config.role_specs()[0].max_turns, Some(60));
    // celeris の tick ループが直接読む `[notify]`。
    assert_eq!(config.notify.interval_secs, 90);
}
// ---- ADR-0053 D3 追記 / Phase 66b（本番 2026-09-21 の観測）: `[[clusters.forwards]]` を持つ ----
// ---- クラスタが設定されていても、tick がマルチスレッド tokio ランタイムの中から panic しないこと ----

/// 本番で観測した panic（`crates/celeris/src/lib.rs:621`、「Cannot start a runtime from within a
/// runtime」）の再現・回帰テスト。`main`/`--mode verify` と同じ配線（`build_dispatcher`）で
/// `auth = "totp"` かつ `[[clusters.forwards]]` を持つクラスタを 1 つ作り、master が死んでいる状態
/// （`cluster_connected` が空）で `dispatcher.tick()` を **`#[tokio::test(flavor = "multi_thread")]`**
/// （celeris の実行時と同じマルチスレッド・ランタイム）の中から直接呼ぶ。
///
/// 実 ssh は起こさない: `cluster_connector` だけ、実物（`cluster_connector` 関数）と**同じ形**
/// （現在のスレッドで新しいネストした current_thread ランタイムを作って `block_on` する）の偽物に
/// 差し替える。これは Phase 66b の修正前なら panic した形そのものなので、この形が panic しなくなった
/// ことが「呼び出し元が async ワーカーから逃がしている」ことの直接の証拠になる（`tunnel_forward_ensurer`
/// / `tunnel_probe` は ssh・HTTP を呼ぶだけで元々 panic しないので偽物で十分）。
///
/// Phase 84b: 「master が死んでいる」ことをテストの前提にするため、`set_cluster_liveness_probe` で
/// `ssh -O check` を偽物（常に false）に差し替える。以前はここを本物の `control_master_alive_blocking`
/// に任せていたため、テストを動かすマシン自身が（人の別作業で）`pegasus` へ実際に ssh ControlMaster
/// を張っていると「master 生存」と誤判定され、`cluster_connector` が一度も呼ばれずに落ちた
/// （観測: 2026-09-21 21:07 UTC の release gate）。クラスタの id/host も、`~/.ssh/config` に実在
/// しうる名前（`pegasus`/`sirius`/`fern03`）を避け、テスト専用の `test-cluster` にした。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_totp_cluster_with_a_forward_does_not_panic_the_first_tick_phase_66b() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    std::fs::create_dir_all(dir.path().join("ws")).unwrap_or_else(|e| panic!("ws: {e}"));
    let config_path = dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        r#"
db = "celeris.sqlite3"
workspace_root = "ws"

[[clusters]]
id = "test-cluster"
host = "test-cluster"
auth = "totp"

[[clusters.forwards]]
listen = "127.0.0.1:0"
target = "bnode150:18000"
"#,
    )
    .unwrap_or_else(|e| panic!("config: {e}"));
    let config = Config::load(&config_path).unwrap_or_else(|e| panic!("{e}"));
    let masters: ClusterMasters = Default::default();
    let mut dispatcher =
        build_dispatcher(&config, masters).unwrap_or_else(|e| panic!("build_dispatcher: {e}"));

    // Phase 84b: 実機の ssh 状態に依存しないよう、master は常に死んでいる扱いにする。
    dispatcher.set_cluster_liveness_probe(Arc::new(|_ssh_command: &[String], _host: &str| false));

    let connector_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let connector_calls_hook = connector_calls.clone();
    dispatcher.set_cluster_connector(Arc::new(move |_cluster_id: &str, _host: &str| {
        connector_calls_hook.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // Phase 66b の契約: ここに来た時点で、呼び出し元はすでに tokio の文脈を持たない OS
        // スレッドへ逃がしているはず（さもなければこのテストの意味が無い）。
        assert!(
            tokio::runtime::Handle::try_current().is_err(),
            "the cluster connector hook must run off any tokio runtime context (Phase 66b)"
        );
        // 実物の `cluster_connector`（本ファイルの上のほう）と同じ形: ネストした current_thread
        // ランタイムを作って `block_on` する。修正前はこの形が「Cannot start a runtime from within
        // a runtime」で panic した。実 ssh は起こさない。
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|e| panic!("nested runtime: {e}"));
        rt.block_on(async { Err::<(), String>("test: no real ssh".to_string()) })
    }));
    dispatcher
        .set_tunnel_forward_ensurer(Arc::new(|_host: &str, _listen: &str, _target: &str| Ok(())));
    dispatcher.set_tunnel_probe(Arc::new(|_listen: &str| {
        Err("unreachable in test".to_string())
    }));
    dispatcher.set_accepting_new_work(true);

    // celeris の tick ループ（`tick_loop`）がしているのと同じこと: マルチスレッド tokio ランタイムの
    // 中から、同期の `dispatcher.tick()` を直接呼ぶ。修正前はここで panic した。
    let report = dispatcher.tick();
    assert!(
        report.is_ok(),
        "the first tick must complete without panicking: {report:?}"
    );
    assert!(
        connector_calls.load(std::sync::atomic::Ordering::SeqCst) >= 1,
        "the totp cluster with a forward must still reach the cluster connector (key auth \
             before TOTP, ADR-0053 D3)"
    );
}

// ---- ADR-0053 Phase 66b の未解決事項 / Phase 81: `try_auto_connect_cluster`（`auth =
// "publickey"`、`dispatch_ready` の中から呼ぶ）も同じ形で async ワーカーから退避させたことの回帰
// テスト ----

/// `a_totp_cluster_with_a_forward_does_not_panic_the_first_tick_phase_66b` と同じ配線
/// （`build_dispatcher`、`#[tokio::test(flavor = "multi_thread")]`、実物と同じ形（ネストした
/// current_thread ランタイム + `block_on`）の偽の `cluster_connector`）だが、`auth = "publickey"`
/// のクラスタに ready なタスクを 1 件置き、`dispatch_ready` から `try_auto_connect_cluster` を
/// 実際に通す（66b の時点では「本番に publickey クラスタが無いので未検証」として scope 外に
/// されていた経路）。
///
/// 偽の `cluster_connector` は `Err` を返す: 成功させると `SshWorkspace::prepare` が実際の
/// ssh/rsync を試みてテストが外部ネットワークに出てしまうため（CLAUDE.md の禁止事項）。自動接続が
/// 失敗する経路でも、`try_auto_connect_cluster` 自身が tokio の文脈を持たない OS スレッドの中から
/// 呼ばれることと、tick がパニックしないことは変わらず検証できる。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_publickey_cluster_with_a_ready_task_does_not_panic_the_first_tick_phase_81() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    std::fs::create_dir_all(dir.path().join("ws")).unwrap_or_else(|e| panic!("ws: {e}"));
    let config_path = dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        r#"
db = "celeris.sqlite3"
workspace_root = "ws"

[[clusters]]
id = "auto"
host = "celeris-no-such-host-for-tests-auto-phase81"
auth = "publickey"
"#,
    )
    .unwrap_or_else(|e| panic!("config: {e}"));
    let config = Config::load(&config_path).unwrap_or_else(|e| panic!("{e}"));
    let store = SqliteStore::open(&config.db.path).unwrap_or_else(|e| panic!("open store: {e}"));
    let now = OffsetDateTime::now_utc();
    let task = task_core::Task {
        harness_id: None,
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        repos: Vec::new(),
        id: task_core::TaskId::new(),
        parent_id: None,
        kind: task_core::TaskKind::Execute,
        title: "phase 81 publickey auto-connect".into(),
        objective: "no-op".into(),
        acceptance: vec![task_core::Criterion {
            text: "ok".into(),
            check: task_core::Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
        }],
        inputs: vec![],
        depends_on: vec![],
        status: task_core::Status::Ready,
        priority: 0,
        worker_hint: task_core::WorkerHint {
            tier: task_core::Tier::Standard,
        },
        workspace: task_core::WorkspaceSpec::Remote {
            cluster: "auto".into(),
            path: PathBuf::from("/remote/project"),
            mode: None,
        },
        budget: task_core::Budget {
            max_turns: 3,
            max_wall_secs: 30,
            max_retries: 0,
            max_tokens: None,
        },
        attempts: 0,
        lease: None,
        created_at: now,
        updated_at: now,
        role: None,
        genre: None,
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: None,
        conversation: None,
        skills: Vec::new(),
        mode: task_core::TaskMode::default(),
        labels: Vec::new(),
        category: Default::default(),
    };
    store
        .insert(&task)
        .unwrap_or_else(|e| panic!("insert: {e}"));

    let masters: ClusterMasters = Default::default();
    let mut dispatcher =
        build_dispatcher(&config, masters).unwrap_or_else(|e| panic!("build_dispatcher: {e}"));

    // Phase 84b: 実機の ssh 状態に依存しないよう、master は常に死んでいる扱いにする
    // （`try_auto_connect_cluster` は `cluster_connected` を見ないが、`refresh_cluster_liveness`
    // が同じ tick で先に呼ばれるので、ここも決定的な偽物に揃えておく）。
    dispatcher.set_cluster_liveness_probe(Arc::new(|_ssh_command: &[String], _host: &str| false));

    let connector_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let connector_calls_hook = connector_calls.clone();
    dispatcher.set_cluster_connector(Arc::new(move |_cluster_id: &str, _host: &str| {
        connector_calls_hook.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // Phase 81 の契約: `try_auto_connect_cluster` も `run_cluster_hooks_off_async` 経由で
        // 呼ばれ、ここに来た時点で呼び出し元はすでに tokio の文脈を持たない OS スレッドへ
        // 逃がしているはず（さもなければこのテストの意味が無い）。
        assert!(
            tokio::runtime::Handle::try_current().is_err(),
            "the cluster connector hook must run off any tokio runtime context (Phase 81)"
        );
        // 実物の `cluster_connector`（本ファイルの上のほう）と同じ形: ネストした current_thread
        // ランタイムを作って `block_on` する。修正前ならこの形は「Cannot start a runtime from
        // within a runtime」で panic した経路。実 ssh は起こさない。
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|e| panic!("nested runtime: {e}"));
        rt.block_on(async { Err::<(), String>("test: no real ssh".to_string()) })
    }));
    dispatcher.set_accepting_new_work(true);

    // celeris の tick ループがしているのと同じこと: マルチスレッド tokio ランタイムの中から、
    // 同期の `dispatcher.tick()` を直接呼ぶ。退避していなければここで panic した。
    let report = dispatcher.tick();
    assert!(
        report.is_ok(),
        "the first tick must complete without panicking: {report:?}"
    );
    assert_eq!(
        connector_calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the publickey cluster must reach the cluster connector exactly once (ADR-0032 D3)"
    );
    // 自動接続が失敗したので cooldown に落ち、この tick では dispatch されない（実
    // ssh/rsync には一切触れていない）。
    assert_eq!(report.unwrap().dispatched, 0);
}

/// ADR-0053 D3 Phase 84b 追記: `a_totp_cluster_with_a_forward_does_not_panic_the_first_tick_phase_66b`
/// と同じ配線（`build_dispatcher`、`[[clusters.forwards]]` を持つ `auth = "totp"` クラスタ）だが、
/// `set_cluster_liveness_probe` が「master 生存」を返す点だけが違う。D3 の設計どおり、master が
/// 生きていれば `cluster_connector`（鍵認証での再接続）は要らず、`tunnel_forward_ensurer`
/// （forward の(再)確立）だけが呼ばれることを確認する。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_totp_cluster_with_a_live_master_skips_the_connector_but_ensures_the_forward_phase_84b() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    std::fs::create_dir_all(dir.path().join("ws")).unwrap_or_else(|e| panic!("ws: {e}"));
    let config_path = dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        r#"
db = "celeris.sqlite3"
workspace_root = "ws"

[[clusters]]
id = "test-cluster"
host = "test-cluster"
auth = "totp"

[[clusters.forwards]]
listen = "127.0.0.1:0"
target = "bnode150:18000"
"#,
    )
    .unwrap_or_else(|e| panic!("config: {e}"));
    let config = Config::load(&config_path).unwrap_or_else(|e| panic!("{e}"));
    let masters: ClusterMasters = Default::default();
    let mut dispatcher =
        build_dispatcher(&config, masters).unwrap_or_else(|e| panic!("build_dispatcher: {e}"));

    // master は常に生存している扱い（実機の ssh 状態には依存しない偽物）。
    dispatcher.set_cluster_liveness_probe(Arc::new(|_ssh_command: &[String], _host: &str| true));

    let connector_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let connector_calls_hook = connector_calls.clone();
    dispatcher.set_cluster_connector(Arc::new(move |_cluster_id: &str, _host: &str| {
        connector_calls_hook.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err::<(), String>("must not be called while the master is alive (Phase 84b)".to_string())
    }));
    let ensure_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let ensure_calls_hook = ensure_calls.clone();
    dispatcher.set_tunnel_forward_ensurer(Arc::new(
        move |_host: &str, _listen: &str, _target: &str| {
            ensure_calls_hook.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        },
    ));
    dispatcher.set_tunnel_probe(Arc::new(|_listen: &str| {
        Err("unreachable in test".to_string())
    }));
    dispatcher.set_accepting_new_work(true);

    let report = dispatcher.tick();
    assert!(
        report.is_ok(),
        "the first tick must complete without panicking: {report:?}"
    );
    assert_eq!(
        connector_calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the cluster connector must not run while the ssh master is alive (ADR-0053 D3)"
    );
    assert!(
        ensure_calls.load(std::sync::atomic::Ordering::SeqCst) >= 1,
        "the forward must still be (re-)established while the master is alive (ADR-0053 D3)"
    );
}
