use super::*;

#[test]
fn provider_kind_legacy_production_inference_warnings_and_cheap_tier() {
    use task_core::{LlmSourceRef as Source, SourceOrigin, Tier};
    let fixture = include_str!("fixtures/provider_kind_legacy_production.toml");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, fixture).unwrap();
    let cfg = Config::load(&path).unwrap();
    for (id, expected) in [
        ("opencode-qwen", Source::OpenaiCompatible("qwen".into())),
        ("ldr-qwen", Source::Celeris),
        ("paperqa-qwen", Source::Celeris),
        ("langmem-main", Source::Celeris),
        ("claude-pool", Source::ClaudeOauth),
        ("codex-pool", Source::CodexOauth),
        ("unclassified-tool", Source::Unknown),
    ] {
        let resolved = cfg.provider_llm_source(id).unwrap();
        assert_eq!(resolved.source, expected, "{id}");
        assert_eq!(resolved.origin, SourceOrigin::Derived, "{id}");
        assert_eq!(
            cfg.provider_kind(id),
            Some(task_core::ProviderKind::Adapter)
        );
    }
    assert_eq!(
        cfg.provider_specs()
            .iter()
            .find(|p| p.id == "opencode-qwen")
            .unwrap()
            .tiers,
        vec![Tier::Cheap]
    );
    assert_eq!(
        cfg.providers
            .iter()
            .find(|p| p.id == "opencode-qwen")
            .unwrap()
            .tiers
            .len(),
        3
    );
    let warnings = cfg.provider_kind_warnings();
    for code in [
        "deprecated_qwen_provider_id",
        "direct_qwen_model",
        "unknown_llm_source",
        "qwen_fixed_acp_noncheap_tier",
    ] {
        assert!(
            warnings.iter().any(|warning| warning.starts_with(code)),
            "{code}"
        );
    }
    assert!(!warnings.join(" ").contains("fixture-secret-sentinel"));
    assert!(!warnings.join(" ").contains("/fixture/qwen.json"));
}

#[test]
fn provider_kind_explicit_source_rejects_adapter_model_and_missing_reference() {
    let head = "[[llm_proxy.sources.openai_compatible]]\nid = \"qwen\"\nbase_url = \"http://127.0.0.1:9/v1\"\n";
    for row in [
        "id = \"bad\"\nadapter = \"fake\"\nllm_source = \"celeris\"",
        "id = \"bad\"\nadapter = \"acp\"\nmodel = \"celeris/cheap\"\nllm_source = \"codex_oauth\"",
        "id = \"bad\"\nadapter = \"acp\"\nllm_source = \"celeris\"",
        "id = \"bad\"\nadapter = \"claude-code\"\nmodel = \"celeris/cheap\"\nllm_source = \"claude_oauth\"",
        "id = \"bad\"\nadapter = \"acp\"\nllm_source = \"unknown\"",
        "id = \"bad\"\nadapter = \"acp\"\nllm_source = \"openai_compatible:missing\"",
    ] {
        let cfg: Config = toml::from_str(&format!("{head}\n[[providers]]\n{row}\n")).unwrap();
        assert!(
            matches!(cfg.validate(), Err(ConfigError::Invalid(_))),
            "{row}"
        );
    }
    let cfg: Config = toml::from_str(&format!("{head}\n[[providers]]\nid = \"ok\"\nadapter = \"acp\"\nkind = \"adapter\"\nllm_source = \"openai_compatible:qwen\"\n")).unwrap();
    cfg.validate().unwrap();
    let resolved = cfg.provider_llm_source("ok").unwrap();
    assert_eq!(resolved.origin, task_core::SourceOrigin::Explicit);
    assert_eq!(
        cfg.provider_kind("ok"),
        Some(task_core::ProviderKind::Adapter)
    );
    assert_eq!(
        serde_json::to_string(&resolved.source).unwrap(),
        "\"openai_compatible:qwen\""
    );
}

#[test]
fn provider_kind_qwen_id_without_model_does_not_guess_source() {
    let cfg: Config = toml::from_str("[[providers]]\nid = \"opencode-qwen\"\nadapter = \"acp\"\nenv = { OPENCODE_CONFIG = \"/fixture/private.json\" }\n").unwrap();
    cfg.validate().unwrap();
    assert_eq!(
        cfg.provider_llm_source("opencode-qwen").unwrap().source,
        task_core::LlmSourceRef::Unknown
    );
    assert_eq!(cfg.provider_specs()[0].tiers.len(), 3);
    assert!(
        cfg.provider_kind_warnings()
            .iter()
            .any(|warning| warning.starts_with("unknown_llm_source"))
    );
}

#[test]
fn provider_kind_qwen_direct_acp_without_opencode_config_is_cheap_only() {
    use task_core::Tier;
    let cfg: Config =
        toml::from_str("[[providers]]\nid = \"direct\"\nadapter = \"acp\"\nmodel = \"qwen3\"\n")
            .unwrap();
    cfg.validate().unwrap();
    assert_eq!(cfg.provider_specs()[0].tiers, vec![Tier::Cheap]);
    assert!(
        cfg.provider_kind_warnings()
            .iter()
            .any(|warning| warning.starts_with("qwen_fixed_acp_noncheap_tier"))
    );
}

#[test]
fn provider_kind_qwen_direct_acp_source_reference_is_cheap_only() {
    use task_core::Tier;
    let cfg: Config = toml::from_str("[[llm_proxy.sources.openai_compatible]]\nid = \"qwen\"\nbase_url = \"http://127.0.0.1:9/v1\"\n[[providers]]\nid = \"source\"\nadapter = \"acp\"\nllm_source = \"openai_compatible:qwen\"\n").unwrap();
    cfg.validate().unwrap();
    assert_eq!(cfg.provider_specs()[0].tiers, vec![Tier::Cheap]);
    assert!(
        cfg.provider_kind_warnings()
            .iter()
            .any(|warning| warning.starts_with("qwen_fixed_acp_noncheap_tier"))
    );
}

#[test]
fn provider_kind_qwen_direct_acp_proxy_model_keeps_all_tiers() {
    let cfg: Config = toml::from_str(
        "[[providers]]\nid = \"proxy\"\nadapter = \"acp\"\nmodel = \"celeris/cheap\"\nenv = { OPENCODE_CONFIG = \"/fixture/old-qwen.json\" }\n",
    )
    .unwrap();
    cfg.validate().unwrap();
    assert_eq!(cfg.provider_specs()[0].tiers.len(), 3);
    assert!(
        !cfg.provider_kind_warnings()
            .iter()
            .any(|warning| warning.starts_with("qwen_fixed_acp_noncheap_tier"))
    );
}

#[test]
fn browser_settings_default_to_unconfigured_and_site_policy_validates() {
    let cfg: Config = toml::from_str("").unwrap();
    assert!(cfg.browser.egress.resolver.is_none());
    assert!(cfg.api.browser_site_policies.is_empty());
    let cfg: Config = toml::from_str(
        r##"
[[api.browser_site_policies]]
policy_id = "pol-login"
exact_origin = "https://login.example.com"
login_url = "https://login.example.com/login"
password_selector = "#password"
submit_selector = "#submit"
"##,
    )
    .unwrap();
    assert!(cfg.api.browser_site_policies[0].validate().is_ok());
    let bad: Config = toml::from_str(
        r##"
[[api.browser_site_policies]]
policy_id = "pol-login"
exact_origin = "https://login.example.com"
login_url = "https://evil.example.com/login"
password_selector = "input:not(.x)"
"##,
    )
    .unwrap();
    assert!(bad.api.browser_site_policies[0].validate().is_err());
    assert!(toml::from_str::<Config>(
        "[[api.browser_site_policies]]\npolicy_id = \"p\"\nexact_origin = \"https://a.example\"\nlogin_url = \"https://a.example/\"\npassword_selector = \"#p\"\nbogus = 1\n"
    ).is_err());
    assert!(
        toml::from_str::<Config>("[browser.egress]\nresolver = \"127.0.0.1\"\nbogus = 1\n")
            .is_err()
    );
}

/// ADR-0116 D5: `[browser] runtime`。既定は `"daemon"`、`"launcher"` は `launcher_socket` 必須、
/// 未知の値と socket 欠落は設定検証で error。
#[test]
fn browser_runtime_defaults_to_daemon() {
    let cfg: Config = toml::from_str("").unwrap();
    assert_eq!(cfg.browser.runtime, "daemon");
    assert!(cfg.browser.launcher_socket.is_none());
    assert!(cfg.browser.validate().is_ok());
    assert_eq!(
        cfg.browser.runtime_kind(true),
        task_worker::browser::BrowserRuntimeKind::Daemon
    );
}

#[test]
fn browser_runtime_launcher_with_socket_validates() {
    let cfg: Config = toml::from_str(
        "[browser]\nruntime = \"launcher\"\nlauncher_socket = \"/run/celeris/browser-launcher.sock\"\n",
    )
    .unwrap();
    assert!(cfg.browser.validate().is_ok());
    assert_eq!(
        cfg.browser.runtime_kind(true),
        task_worker::browser::BrowserRuntimeKind::Launcher {
            socket: std::path::PathBuf::from("/run/celeris/browser-launcher.sock"),
            refuse_test_loopback: true,
        }
    );
}

#[test]
fn browser_runtime_launcher_without_socket_is_rejected() {
    let cfg: Config = toml::from_str("[browser]\nruntime = \"launcher\"\n").unwrap();
    let err = cfg.browser.validate().unwrap_err();
    assert!(err.to_string().contains("launcher_socket"));
}

#[test]
fn browser_runtime_unknown_value_is_rejected() {
    let cfg: Config = toml::from_str("[browser]\nruntime = \"bogus\"\n").unwrap();
    let err = cfg.browser.validate().unwrap_err();
    assert!(err.to_string().contains("runtime"));
}
use task_core::{AccountAdapter, DelegationLimits, OrgKind, Tier, WorkerHint};

/// ADR-0046 D3（Phase 59）: `config/org.example.toml` の `genre` が指す全ての harness を、
/// 互換の `[[genres]]`（`conversation` / `coding` / `literature` / `web-research` / `data-analysis` /
/// `writing`）として定義する（`config/celeris.example.toml` の `[[harnesses]]` の互換の射影と同じ集合）。
const ORG_TEST_GENRES: &str = r#"
[[providers]]
id = "x"
adapter = "fake"

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
"#;

// ---- ADR-0033 D1（Phase 23）: 組織図の種 ----

/// 例の設定（`config/org.example.toml`）が読め、ADR-0046 D7 の組織図（15 ノード。cos を根に
/// Engineering / Research / Operations の 3 部、それぞれの下に課）になる。`genre` は実在する
/// harness id（`conversation` / `coding` / `literature` / `web-research` / `data-analysis` /
/// `writing`）だけを指す。
#[test]
fn loads_the_org_example_and_maps_it_to_org_nodes() {
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
            "db = \"t.sqlite3\"\norg_include = \"org.toml\"\n{}",
            ORG_TEST_GENRES
        ),
    )
    .unwrap();

    let cfg = Config::load(&path).unwrap();
    let ids: Vec<&str> = cfg.org.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "cos",
            "engineering",
            "software-engineering",
            "ui-ux",
            "systems-performance",
            "browser-execution",
            "research",
            "literature-research",
            "web-research",
            "experiment-data",
            "scientific-writing",
            "operations",
            "cluster-hpc",
            "infrastructure",
            "monitoring-automation",
        ]
    );
    let nodes = cfg.org_nodes(time::OffsetDateTime::now_utc());
    assert_eq!(nodes.len(), 15);
    // 親が子より先に来る（cos → 部 → 課）。
    let order: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(order[0], "cos");
    assert!(
        order.iter().position(|id| *id == "engineering")
            < order.iter().position(|id| *id == "software-engineering")
    );
    // ADR-0046 D6: CoS（根）は対話用の harness を持つ。
    assert_eq!(
        nodes
            .iter()
            .find(|n| n.id == "cos")
            .unwrap()
            .genre
            .as_deref(),
        Some("conversation")
    );
    let literature = nodes
        .iter()
        .find(|n| n.id == "literature-research")
        .unwrap();
    assert_eq!(literature.kind, OrgKind::Section);
    assert_eq!(literature.genre.as_deref(), Some("literature"));
    assert_eq!(literature.parent_id.as_deref(), Some("research"));
    assert!(!literature.brief.is_empty());
    // 人間の決定（2026-09-18、ADR-0035 §1）: 学術文献は PaperQA2（literature）、一般 Web は LDR。
    let web = nodes.iter().find(|n| n.id == "web-research").unwrap();
    assert_eq!(web.genre.as_deref(), Some("web-research"));
    assert_eq!(web.parent_id.as_deref(), Some("research"));
    // ADR-0046 D7: 新しい harness `data-analysis` / `writing` はそれぞれの課の分野。
    assert_eq!(
        nodes
            .iter()
            .find(|n| n.id == "experiment-data")
            .unwrap()
            .genre
            .as_deref(),
        Some("data-analysis")
    );
    assert_eq!(
        nodes
            .iter()
            .find(|n| n.id == "scientific-writing")
            .unwrap()
            .genre
            .as_deref(),
        Some("writing")
    );
    assert_eq!(
        nodes
            .iter()
            .filter(|n| n.kind == OrgKind::Secretary)
            .count(),
        1
    );
}

/// `[[genres]]` に無い分野・重複 id・秘書が 0 か 2・知らない親は設定エラー。
#[test]
fn rejects_org_seeds_that_do_not_form_one_tree() {
    let base = "db = \"t.sqlite3\"\norg_include = \"org.toml\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n";
    let load = |org: &str| -> Result<Config, ConfigError> {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("org.toml"), org).unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, base).unwrap();
        Config::load(&path)
    };
    let secretary = "[[org]]\nid = \"secretary\"\nname = \"秘書\"\nkind = \"secretary\"\n";
    load(secretary).expect("a lone secretary is fine");

    let err = load(&format!(
        "{secretary}[[org]]\nid = \"coding\"\nname = \"部\"\nkind = \"department\"\n"
    ))
    .unwrap_err()
    .to_string();
    assert!(err.contains("parent_id is required"), "{err}");

    let err = load("[[org]]\nid = \"coding\"\nname = \"部\"\nkind = \"department\"\nparent_id = \"secretary\"\n")
            .unwrap_err()
            .to_string();
    assert!(err.contains("exactly one node"), "{err}");

    let err = load(&format!("{secretary}{secretary}"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("duplicate org id"), "{err}");

    let err = load(&format!(
            "{secretary}[[org]]\nid = \"coding\"\nname = \"部\"\nkind = \"department\"\nparent_id = \"nobody\"\n"
        ))
        .unwrap_err()
        .to_string();
    assert!(err.contains("is not one of the [[org]] entries"), "{err}");

    let err = load(&format!(
            "{secretary}[[org]]\nid = \"X\"\nname = \"部\"\nkind = \"department\"\nparent_id = \"secretary\"\n"
        ))
        .unwrap_err()
        .to_string();
    assert!(err.contains("kebab-case"), "{err}");

    let err = load(&format!(
            "{secretary}[[org]]\nid = \"c\"\nname = \"課\"\nkind = \"section\"\nparent_id = \"secretary\"\ngenre = \"nope\"\n"
        ))
        .unwrap_err()
        .to_string();
    assert!(err.contains("is not defined in [[genres]]"), "{err}");
}

/// `org_include` を書かなければ種は空、書いたのにファイルが無ければ設定エラー。
#[test]
fn org_include_is_optional_but_must_exist_when_written() {
    let cfg: Config = toml::from_str("[[providers]]\nid = \"x\"\nadapter = \"fake\"\n").unwrap();
    assert!(cfg.org.is_empty());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "db = \"t.sqlite3\"\norg_include = \"missing.toml\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n").unwrap();
    assert!(matches!(Config::load(&path), Err(ConfigError::Read { .. })));
}

#[test]
fn loads_example_config_and_resolves_relative_paths() {
    let path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config/celeris.example.toml"
    ));
    let cfg = Config::load(path).unwrap();
    assert!(cfg.db.path.is_absolute());
    assert!(cfg.workspace_root.is_absolute());
    assert_eq!(cfg.max_concurrency, 2);
    assert_eq!(cfg.providers[0].adapter, "fake");
    assert_eq!(cfg.tick(), Duration::from_millis(2000));
    assert_eq!(cfg.provider_specs()[0].concurrency, 2);
    assert!(!cfg.plan.auto_accept);
    assert!(!cfg.dispatch_config().plan_auto_accept);
    cfg.validate().unwrap();
    // 監査 M-1（Phase 59 追記）: `[[harnesses]]`（互換の射影で `[[genres]]` になる）は
    // `config/org.example.toml` の課が使うもの全部が揃っている（`[[harnesses]]` の宣言順）。
    assert!(
        cfg.roles.iter().all(|r| r.adapter.is_none()),
        "{:?}",
        cfg.roles
    );
    let mut genres: Vec<&str> = cfg.genres.iter().map(|g| g.id.as_str()).collect();
    genres.sort_unstable();
    assert_eq!(
        genres,
        vec![
            "coding",
            "conversation",
            "data-analysis",
            "knowledge-curation",
            "literature",
            "plan",
            "web-research",
            "writing"
        ]
    );
    // ADR-0046 D6（Phase 59）: `[conversation] genre = "conversation"` を明示している。
    assert_eq!(cfg.conversation_genre_id(), "conversation");
    // ADR-0052 D1 / D2（Phase 64）: `[[harnesses]]` に `knowledge` を書いていない例の設定でも、
    // 組み込みの `fallback = { tier = "cheap" }` が `DispatchConfig` に届く。
    let dispatch = cfg.dispatch_config();
    assert_eq!(dispatch.knowledge.fallback_tier, Some(Tier::Cheap));
    assert_eq!(dispatch.knowledge.langmem_base_url, None, "例は無効のまま");
}

/// ADR-0064 D1: `db` は従来どおり文字列（`db = "<path>"`）でも、`[db]` テーブル
/// （`path` / `busy_timeout_ms` / `checkpoint_interval_secs` / `backup_dir` /
/// `backup_interval_secs` / `backup_keep`）でも書ける。両方とも既定値は同じ。
#[test]
fn db_accepts_both_the_bare_path_string_and_the_table_form() {
    // 何も書かなければ既定（`~/.local/celeris/celeris.sqlite3`、busy_timeout 5000ms、
    // checkpoint 30s、backup_dir 無し、backup_interval 3600s、backup_keep 48）。
    let raw: Config = toml::from_str("[[providers]]\nid = \"x\"\nadapter = \"fake\"\n").unwrap();
    assert_eq!(raw.db, DbConfig::default());
    assert_eq!(raw.db.busy_timeout_ms, 5000);
    assert_eq!(raw.db.checkpoint_interval_secs, 30);
    assert_eq!(raw.db.backup_dir, None);
    assert_eq!(raw.db.backup_interval_secs, 3600);
    assert_eq!(raw.db.backup_keep, 48);

    // 文字列（従来どおり）。
    let raw: Config =
        toml::from_str("db = \"local.sqlite3\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n")
            .unwrap();
    assert_eq!(raw.db.path, PathBuf::from("local.sqlite3"));
    assert_eq!(raw.db.busy_timeout_ms, 5000, "still the default");

    // テーブル（新規、任意）: 一部だけ書けば残りは既定。
    let raw: Config = toml::from_str(
        "[db]\npath = \"/var/lib/celeris/celeris.sqlite3\"\nbusy_timeout_ms = 15000\n\
             checkpoint_interval_secs = 10\nbackup_dir = \"/var/backups/celeris\"\n\
             backup_interval_secs = 900\nbackup_keep = 12\n\
             [[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
    )
    .unwrap();
    assert_eq!(
        raw.db.path,
        PathBuf::from("/var/lib/celeris/celeris.sqlite3")
    );
    assert_eq!(raw.db.busy_timeout(), Duration::from_millis(15000));
    assert_eq!(raw.db.checkpoint_interval(), Duration::from_secs(10));
    assert_eq!(
        raw.db.backup_dir,
        Some(PathBuf::from("/var/backups/celeris"))
    );
    assert_eq!(raw.db.backup_interval(), Duration::from_secs(900));
    assert_eq!(raw.db.backup_keep, 12);
    assert!(raw.db.worker_read_only, "ADR-0095 D5: on unless opted out");

    // ADR-0095 D5: 既定（文字列の形も）は worker から読み取り専用。`false` は明示の opt-out。
    assert!(DbConfig::default().worker_read_only);
    let raw: Config = toml::from_str(
        "[db]\npath = \"x.sqlite3\"\nworker_read_only = false\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
    )
    .unwrap();
    assert!(!raw.db.worker_read_only);

    // 綴り間違いは `[db]` テーブルの中でも設定エラー（`deny_unknown_fields`。`Repr` が
    // untagged のため、メッセージは「どちらの形にも合わない」という一般的な文言になる）。
    assert!(
        toml::from_str::<Config>(
            "[db]\npath = \"x.sqlite3\"\nbusy_timeout_msx = 1\n\
                 [[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
        )
        .is_err()
    );

    // `Config::load` は `[db].backup_dir` の相対パス・`~` も他のパス設定と同じ規則で解決する。
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[db]\npath = \"d.sqlite3\"\nbackup_dir = \"backups\"\n\
             [[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
    )
    .unwrap();
    let cfg = Config::load(&path).unwrap();
    let base = dir.path().canonicalize().unwrap();
    assert_eq!(cfg.db.path, base.join("d.sqlite3"));
    assert_eq!(cfg.db.backup_dir, Some(base.join("backups")));
}

/// ADR-0052 D2（Phase 64）: `[[harnesses]] id = "knowledge"` の `fallback` は
/// `{ tier = … }` でも `false` でも書ける。書かなければ組み込みの既定（`cheap`）を継ぐ。
#[test]
fn the_knowledge_harness_fallback_is_configurable() {
    let base = r#"
db = "celeris.sqlite3"
workspace_root = "."

[knowledge.langmem]
enabled = true
base_url = "http://127.0.0.1:18000/v1"

[[providers]]
id = "p1"
adapter = "fake"
tiers = ["cheap", "standard", "frontier"]
"#;
    let write = |extra: &str| {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, format!("{base}{extra}")).unwrap();
        let cfg = Config::load(&path).unwrap();
        (dir, cfg.dispatch_config())
    };

    // 何も書かなければ組み込みの既定（tier cheap）。`base_url` も届く。
    let (_d, dispatch) = write("");
    assert_eq!(dispatch.knowledge.fallback_tier, Some(Tier::Cheap));
    assert_eq!(
        dispatch.knowledge.langmem_base_url.as_deref(),
        Some("http://127.0.0.1:18000/v1")
    );

    // tier を変えられる。
    let (_d, dispatch) = write(
        "\n[[harnesses]]\nid = \"knowledge\"\nadapter = \"langmem\"\nfallback = { tier = \"standard\" }\n",
    );
    assert_eq!(dispatch.knowledge.fallback_tier, Some(Tier::Standard));

    // `fallback = false` で無効。
    let (_d, dispatch) =
        write("\n[[harnesses]]\nid = \"knowledge\"\nadapter = \"langmem\"\nfallback = false\n");
    assert_eq!(dispatch.knowledge.fallback_tier, None);

    // 同じ id を書いても `fallback` を省けば組み込みの既定を継ぐ。
    let (_d, dispatch) = write("\n[[harnesses]]\nid = \"knowledge\"\nadapter = \"langmem\"\n");
    assert_eq!(dispatch.knowledge.fallback_tier, Some(Tier::Cheap));
}

/// 監査 M-1: 例の設定 2 つ（`celeris.example.toml` + `org.example.toml`）を**組み合わせて**読める。
/// 組織の `genre` が `[[genres]]` に無ければ `validate` が弾くので、これが噛み合いの回帰になる。
#[test]
fn the_two_example_files_load_together_through_org_include() {
    let config_dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../config"));
    let dir = tempfile::tempdir().unwrap();
    let example = std::fs::read_to_string(config_dir.join("celeris.example.toml")).unwrap();
    let enabled = example.replace("# org_include = \"org.toml\"", "org_include = \"org.toml\"");
    assert!(
        enabled.contains("\norg_include = \"org.toml\""),
        "org_include の行が見つからない"
    );
    std::fs::write(dir.path().join("config.toml"), enabled).unwrap();
    std::fs::copy(
        config_dir.join("org.example.toml"),
        dir.path().join("org.toml"),
    )
    .unwrap();

    let cfg = Config::load(&dir.path().join("config.toml")).unwrap();
    cfg.validate().unwrap();
    let ids: Vec<&str> = cfg.org.iter().map(|n| n.id.as_str()).collect();
    assert!(
        ids.contains(&"cos")
            && ids.contains(&"software-engineering")
            && ids.contains(&"literature-research")
    );
    assert_eq!(
        cfg.org
            .iter()
            .filter(|n| n.kind == task_core::OrgKind::Secretary)
            .count(),
        1
    );
    // 課の分野はすべて `[[genres]]` にある（`validate` が見ているのと同じ条件を明示しておく）。
    for node in &cfg.org {
        if let Some(genre) = &node.genre {
            assert!(
                cfg.genres.iter().any(|g| &g.id == genre),
                "{genre} が [[genres]] に無い"
            );
        }
    }
}

/// ADR-0073: 例の組織（`org.example.toml`）で matching がどの課を選ぶかの回帰試験。
/// `frontend` は `ui-ux` にだけあるので、画面の仕事は `ui-ux`、API / Rust は
/// `software-engineering` に行く。
#[test]
fn example_org_routes_ui_work_to_ui_ux_and_api_work_to_software_engineering() {
    use task_ops::matching::{Assignment, decide};

    let config_dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../config"));
    let dir = tempfile::tempdir().unwrap();
    let example = std::fs::read_to_string(config_dir.join("celeris.example.toml")).unwrap();
    let enabled = example.replace("# org_include = \"org.toml\"", "org_include = \"org.toml\"");
    std::fs::write(dir.path().join("config.toml"), enabled).unwrap();
    std::fs::copy(
        config_dir.join("org.example.toml"),
        dir.path().join("org.toml"),
    )
    .unwrap();
    let cfg = Config::load(&dir.path().join("config.toml")).unwrap();
    cfg.validate().unwrap();
    let nodes = cfg.org_nodes(time::OffsetDateTime::now_utc());

    let route = |skills: &[&str]| -> String {
        let mut task = routing_sample_task();
        task.genre = Some("coding".to_string());
        task.skills = skills.iter().map(|s| s.to_string()).collect();
        match decide(&nodes, &task) {
            Assignment::Assigned { node, .. } => node,
            other => panic!("{skills:?}: expected Assigned, got {other:?}"),
        }
    };

    assert_eq!(route(&["ui-design", "frontend"]), "ui-ux");
    // typescript / react は両方の課にあるが、frontend の 1 点で ui-ux が勝つ。
    assert_eq!(route(&["typescript", "react", "frontend"]), "ui-ux");
    assert_eq!(route(&["responsive", "css", "accessibility"]), "ui-ux");
    assert_eq!(route(&["rust", "api"]), "software-engineering");
    assert_eq!(
        route(&["typescript", "api", "sqlite"]),
        "software-engineering"
    );
    // typescript / react だけだと両課とも 2 点・同じ深さで並ぶ。同点は id の辞書順で
    // "software-engineering" < "ui-ux" となり software-engineering に行く。
    assert_eq!(route(&["typescript", "react"]), "software-engineering");
    assert_eq!(route(&["hpc", "perf"]), "systems-performance");
    // skill なし: browser 専用課は候補外。残る coding 課は 0 点・同じ深さで並び、
    // id の辞書順で先頭の cluster-hpc になる。
    assert_eq!(route(&[]), "cluster-hpc");
    assert_ne!(route(&[]), "browser-execution");
}

fn routing_sample_task() -> task_core::Task {
    use task_core::{
        Budget, Status, Task, TaskId, TaskKind, TaskMode, Tier, WorkerHint, WorkspaceSpec,
    };
    let now = time::OffsetDateTime::now_utc();
    Task {
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "t".into(),
        objective: "o".into(),
        acceptance: vec![],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Ready,
        priority: 10,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::local("/tmp"),
        repos: vec![],
        budget: Budget {
            max_turns: 1,
            max_wall_secs: 1,
            max_retries: 0,
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
        labels: vec![],
        category: Default::default(),
        skills: vec![],
        mode: TaskMode::Production,
        conversation: None,
    }
}

#[test]
fn plan_auto_accept_is_parsed_and_unknown_plan_keys_are_rejected() {
    let cfg: Config = toml::from_str(
        r#"[plan]
auto_accept = true
[[providers]]
id = "x"
adapter = "fake"
"#,
    )
    .unwrap();
    assert!(cfg.plan.auto_accept);
    assert!(cfg.dispatch_config().plan_auto_accept);
    assert!(toml::from_str::<Config>("[plan]\nbogus = 1\n").is_err());
}

/// ADR-0056 D1（Phase 78）: `[[mcp.listeners]]` は `Config::validate` が検査する（`auth = "none"`
/// は loopback だけ）。
#[test]
fn mcp_listeners_parse_and_validate_are_wired_into_config() {
    let cfg: Config = toml::from_str(
        r#"[[providers]]
id = "x"
adapter = "fake"
[[mcp.listeners]]
listen = "127.0.0.1:18200"
auth = "token"
[[mcp.listeners]]
listen = "127.0.0.1:18201"
auth = "none"
client = "chatgpt"
"#,
    )
    .unwrap();
    cfg.validate().unwrap();
    assert!(cfg.mcp.effective_enabled());
    assert_eq!(cfg.mcp.resolve_listeners().unwrap().len(), 2);

    let bad: Config = toml::from_str(
        r#"[[providers]]
id = "x"
adapter = "fake"
[[mcp.listeners]]
listen = "0.0.0.0:18201"
auth = "none"
client = "chatgpt"
"#,
    )
    .unwrap();
    assert!(matches!(bad.validate(), Err(ConfigError::Invalid(_))));
}

#[test]
fn rejects_unknown_adapter_and_missing_providers() {
    let cfg: Config = toml::from_str("").unwrap();
    assert!(matches!(cfg.validate(), Err(ConfigError::Invalid(_))));
    let cfg: Config =
        toml::from_str("[[providers]]\nid = \"x\"\nadapter = \"bogus-adapter\"\n").unwrap();
    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("bogus-adapter"));
    assert!(toml::from_str::<Config>("bogus = 1\n").is_err());
}

/// ADR-0051 Phase 106追記: `[selfdeploy] push` / `push_remote` の既定と検査。
#[test]
fn selfdeploy_push_defaults_to_true_and_origin_and_rejects_blank_remote() {
    let cfg: Config = toml::from_str("[[providers]]\nid = \"x\"\nadapter = \"fake\"\n").unwrap();
    assert!(cfg.selfdeploy.push);
    assert_eq!(cfg.selfdeploy.push_remote, "origin");
    cfg.validate().unwrap();

    let cfg: Config = toml::from_str(
            "[[providers]]\nid = \"x\"\nadapter = \"fake\"\n[selfdeploy]\npush = false\npush_remote = \"\"\n",
        )
        .unwrap();
    assert!(!cfg.selfdeploy.push);
    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("push_remote"));
}

/// ADR-0019: `sync = "worktree"` が読めて、worktree の設定が `ClusterSpec` と `ViewContext` に写ること。
/// 例の設定ファイル（config/celeris.clusters.example.toml）もここで一度読んで、書き間違いを拾う。
#[test]
fn parses_worktree_sync_and_maps_it_to_the_worker_settings() {
    let path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config/celeris.clusters.example.toml"
    ));
    let cfg = Config::load(path).unwrap();
    cfg.validate().unwrap();
    let specs = cfg.cluster_specs();
    assert_eq!(specs["pegasus"].sync, task_worker::SyncMode::Worktree);
    // ADR-0032 D1: pegasus/sirius は 2 要素認証（totp）、fern03 は鍵だけで入れる（publickey）の例。
    assert_eq!(specs["pegasus"].auth, "totp");
    assert_eq!(specs["sirius"].auth, "totp");
    assert_eq!(specs["fern03"].auth, "publickey");

    let cfg: Config = toml::from_str(
        r#"[[providers]]
id = "x"
adapter = "fake"
[[clusters]]
id = "pegasus"
host = "pegasus"
sync = "worktree"
worktree_root = "/work/NBB/rmaeda/.celeris-worktrees"
worktree_base = "origin/main"
worktree_paths = ["src", "Cargo.toml"]
"#,
    )
    .unwrap();
    cfg.validate().unwrap();
    let spec = &cfg.cluster_specs()["pegasus"];
    assert_eq!(spec.sync, task_worker::SyncMode::Worktree);
    assert_eq!(
        spec.worktree.root.as_deref(),
        Some(Path::new("/work/NBB/rmaeda/.celeris-worktrees"))
    );
    assert_eq!(spec.worktree.base, "origin/main");
    assert_eq!(
        spec.worktree.paths,
        vec!["src".to_string(), "Cargo.toml".to_string()]
    );
    assert_eq!(spec.worktree.branch_prefix, "celeris/");
    let view = &cfg.cluster_view_infos()["pegasus"];
    assert_eq!(view.sync, "worktree");
    assert_eq!(
        view.worktree_root.as_deref(),
        Some(Path::new("/work/NBB/rmaeda/.celeris-worktrees"))
    );
}

/// 既定は `sync = "rsync"` のまま（ADR-0018 からの互換）。知らない sync と自動削除は設定エラー。
#[test]
fn rejects_unknown_sync_modes_and_worktree_auto_removal() {
    let base = |extra: &str| {
        format!(
            r#"[[providers]]
id = "x"
adapter = "fake"
[[clusters]]
id = "c"
host = "h"
{extra}
"#
        )
    };
    let cfg: Config = toml::from_str(&base("")).unwrap();
    assert_eq!(cfg.clusters[0].sync, "rsync");
    assert_eq!(cfg.cluster_specs()["c"].sync, task_worker::SyncMode::Rsync);

    let cfg: Config = toml::from_str(&base(r#"sync = "worktre""#)).unwrap();
    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("sync must be"), "{err}");

    let cfg: Config = toml::from_str(&base(r#"remove_worktree_when = "done""#)).unwrap();
    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("remove_worktree_when"), "{err}");
}

/// ADR-0032 D1: `auth` の既定は `"manual"`（省略した既存設定の挙動は変わらない）。3 値だけ許し、
/// `ClusterSpec` と `ClusterViewInfo` の両方に写る。それ以外は設定エラー。
#[test]
fn cluster_auth_defaults_to_manual_and_only_three_values_are_accepted() {
    let base = |extra: &str| {
        format!(
            r#"[[providers]]
id = "x"
adapter = "fake"
[[clusters]]
id = "c"
host = "h"
{extra}
"#
        )
    };
    // 既定: auth を書かなければ "manual"。既存設定の挙動が変わらない。
    let cfg: Config = toml::from_str(&base("")).unwrap();
    cfg.validate().unwrap();
    assert_eq!(cfg.clusters[0].auth, "manual");
    assert_eq!(cfg.cluster_specs()["c"].auth, "manual");
    assert_eq!(cfg.cluster_view_infos()["c"].auth, "manual");

    for auth in ["manual", "publickey", "totp"] {
        let cfg: Config = toml::from_str(&base(&format!(r#"auth = "{auth}""#))).unwrap();
        cfg.validate().unwrap();
        assert_eq!(cfg.clusters[0].auth, auth);
        assert_eq!(cfg.cluster_specs()["c"].auth, auth);
        assert_eq!(cfg.cluster_view_infos()["c"].auth, auth);
    }

    let cfg: Config = toml::from_str(&base(r#"auth = "password""#)).unwrap();
    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("auth must be"), "{err}");
    assert!(err.contains("password"), "{err}");
}

/// ADR-0078 D1: `control_persist` の既定は "yes"。"yes" か正の秒数だけを受ける。
#[test]
fn cluster_control_persist_defaults_to_yes_and_accepts_only_yes_or_positive_seconds() {
    let base = |extra: &str| {
        format!(
            r#"[[providers]]
id = "x"
adapter = "fake"
[[clusters]]
id = "c"
host = "h"
{extra}
"#
        )
    };
    let cfg: Config = toml::from_str(&base("")).unwrap();
    cfg.validate().unwrap();
    assert_eq!(cfg.clusters[0].control_persist, "yes");

    for ok in ["yes", "28800", "1"] {
        let cfg: Config = toml::from_str(&base(&format!(r#"control_persist = "{ok}""#))).unwrap();
        cfg.validate().unwrap();
        assert_eq!(cfg.clusters[0].control_persist, ok);
    }
    for bad in [
        "no",
        "0",
        "",
        "10m",
        "-5",
        "+5",
        "1.5",
        "99999999999999999999999",
    ] {
        let cfg: Config = toml::from_str(&base(&format!(r#"control_persist = "{bad}""#))).unwrap();
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("control_persist must be"), "{bad}: {err}");
    }
}

/// ADR-0090 D7: `job_wait` の既定は `{poll_secs = 300, max_wait_secs = 86400}`。`poll_secs >= 30`、
/// `poll_secs <= max_wait_secs <= 14 日` を検証し、`ClusterSpec.job_wait` に写す。未知の欄は拒む。
#[test]
fn cluster_job_wait_defaults_and_validation() {
    let base = |extra: &str| {
        format!(
            r#"[[providers]]
id = "x"
adapter = "fake"
[[clusters]]
id = "sirius"
host = "sirius"
{extra}
"#
        )
    };
    let cfg: Config = toml::from_str(&base("")).unwrap();
    cfg.validate().unwrap();
    assert_eq!(cfg.clusters[0].job_wait.poll_secs, 300);
    assert_eq!(cfg.clusters[0].job_wait.max_wait_secs, 86_400);
    let cfg: Config = toml::from_str(&base(
        "job_wait = { poll_secs = 600, max_wait_secs = 43200 }",
    ))
    .unwrap();
    cfg.validate().unwrap();
    let spec = cfg.cluster_specs();
    assert_eq!(
        spec["sirius"].job_wait,
        task_core::cluster_job::ClusterJobWaitLimits {
            poll_secs: 600,
            max_wait_secs: 43_200
        }
    );
    for (bad, needle) in [
        ("job_wait = { poll_secs = 5 }", "poll_secs must be >= 30"),
        (
            "job_wait = { poll_secs = 600, max_wait_secs = 60 }",
            "must be >= job_wait.poll_secs",
        ),
        (
            "job_wait = { max_wait_secs = 99999999 }",
            "max_wait_secs must be <=",
        ),
    ] {
        let cfg: Config = toml::from_str(&base(bad)).unwrap();
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains(needle), "{bad}: {err}");
        assert!(err.contains("[[clusters]] sirius"), "{err}");
    }
    assert!(toml::from_str::<Config>(&base("job_wait = { every = 5 }")).is_err());
}

#[test]
fn loads_claude_code_dogfood_example_config() {
    let path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config/celeris.claude-code.example.toml"
    ));
    let cfg = Config::load(path).unwrap();
    assert!(cfg.validate().is_ok());
    assert_eq!(cfg.providers[0].adapter, "claude-code");
    assert_eq!(cfg.adapters.claude_code.command, "claude");
}

#[test]
fn accepts_claude_code_adapter_with_default_config() {
    let cfg: Config =
        toml::from_str("[[providers]]\nid = \"x\"\nadapter = \"claude-code\"\n").unwrap();
    assert!(cfg.validate().is_ok());
    assert_eq!(cfg.adapters.claude_code.command, "claude");
    assert_eq!(
        cfg.adapters.claude_code.permission_mode,
        "bypassPermissions"
    );
}

#[test]
fn rejects_unknown_fields_in_claude_code_adapter_config() {
    let text = "[[providers]]\nid = \"x\"\nadapter = \"claude-code\"\n\n[adapters.claude_code]\nbogus = 1\n";
    assert!(toml::from_str::<Config>(text).is_err());
}

#[test]
fn loads_codex_dogfood_example_config() {
    let path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config/celeris.codex.example.toml"
    ));
    let cfg = Config::load(path).unwrap();
    assert!(cfg.validate().is_ok());
    assert_eq!(cfg.providers[0].adapter, "codex");
    assert_eq!(cfg.adapters.codex.command, "codex");
}

#[test]
fn accepts_codex_adapter_with_default_config() {
    let cfg: Config = toml::from_str("[[providers]]\nid = \"x\"\nadapter = \"codex\"\n").unwrap();
    assert!(cfg.validate().is_ok());
    assert_eq!(cfg.adapters.codex.command, "codex");
    assert!(cfg.adapters.codex.model.is_none());
}

/// ADR-0026 D2: `[adapters.acp]` の既定値（opencode を素の状態で使う）。
#[test]
fn accepts_acp_adapter_with_default_config() {
    let cfg: Config = toml::from_str("[[providers]]\nid = \"x\"\nadapter = \"acp\"\n").unwrap();
    assert!(cfg.validate().is_ok());
    assert_eq!(cfg.adapters.acp.command, "opencode");
    assert_eq!(cfg.adapters.acp.args, vec!["acp".to_string()]);
    assert_eq!(
        cfg.adapters.acp.permission,
        task_worker::AcpPermission::Allow
    );
    assert_eq!(cfg.adapters.acp.model_option_id, "model");
    assert_eq!(cfg.adapters.acp.startup_timeout_secs, 300);
    assert!(cfg.adapters.acp.env.is_empty());
    assert!(cfg.providers[0].command.is_none());
    assert!(cfg.providers[0].args.is_none());
}

#[test]
fn rejects_unknown_fields_in_acp_adapter_config() {
    let text = "[[providers]]\nid = \"x\"\nadapter = \"acp\"\n\n[adapters.acp]\nbogus = 1\n";
    assert!(toml::from_str::<Config>(text).is_err());
}

/// ADR-0026 D2: `permission` は `AcpPermission` の `allow`/`deny` 以外は設定エラー（deny_unknown ではなく
/// serde の enum 検証で拒否される）。
#[test]
fn rejects_unknown_acp_permission_value() {
    let text =
        "[[providers]]\nid = \"x\"\nadapter = \"acp\"\n\n[adapters.acp]\npermission = \"maybe\"\n";
    assert!(toml::from_str::<Config>(text).is_err());
}

/// ADR-0026 D2: `command`/`args` は `adapter = "acp"` の行だけで意味を持つ。行ごとに上書きできる。
#[test]
fn command_and_args_are_only_allowed_on_acp_providers_and_override_per_row() {
    let cfg: Config =
        toml::from_str("[[providers]]\nid = \"x\"\nadapter = \"fake\"\ncommand = \"whatever\"\n")
            .unwrap();
    let err = cfg.validate().unwrap_err().to_string();
    assert!(
        err.contains("command/args are only allowed when adapter"),
        "{err}"
    );

    let cfg: Config =
        toml::from_str("[[providers]]\nid = \"x\"\nadapter = \"codex\"\nargs = [\"x\"]\n").unwrap();
    assert!(cfg.validate().is_err());

    let cfg: Config = toml::from_str(
        "[[providers]]\nid = \"x\"\nadapter = \"acp\"\ncommand = \"goose\"\nargs = [\"acp\"]\n",
    )
    .unwrap();
    assert!(cfg.validate().is_ok());
    assert_eq!(cfg.providers[0].command.as_deref(), Some("goose"));
    assert_eq!(
        cfg.providers[0].args.as_deref(),
        Some(&["acp".to_string()][..])
    );
}

/// ADR-0026 D6: 冷スタート用の例の設定ファイルが読め、Phase 15 の設定検証を通る。
#[test]
fn loads_acp_opencode_example_config() {
    let path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config/celeris.acp-opencode.example.toml"
    ));
    let cfg = Config::load(path).unwrap();
    assert!(cfg.validate().is_ok());
    assert_eq!(cfg.providers[0].adapter, "acp");
    assert_eq!(cfg.adapters.acp.command, "opencode");
    assert_eq!(
        cfg.adapters.acp.permission,
        task_worker::AcpPermission::Allow
    );
    assert_eq!(
        cfg.providers[0]
            .env
            .get("OPENCODE_DISABLE_PROJECT_CONFIG")
            .map(String::as_str),
        Some("1")
    );
}

/// ADR-0061: `aider` 用の例の設定ファイルが読め、設定検証を通る。
#[test]
fn loads_aider_example_config() {
    let path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config/celeris.aider.example.toml"
    ));
    let cfg = Config::load(path).unwrap();
    assert!(cfg.validate().is_ok());
    assert_eq!(cfg.providers[0].adapter, "aider");
    assert_eq!(cfg.providers[0].model, "anthropic/claude-sonnet-5");
    assert_eq!(
        cfg.providers[0]
            .env
            .get("ANTHROPIC_API_KEY")
            .map(String::as_str),
        Some("sk-ant-...")
    );
}

/// ADR-0063 Phase 109d C2: `[adapters.paperqa]` の既定値（python インタプリタを素の状態で使う。
/// `pqa` CLI ではない）。
#[test]
fn accepts_paperqa_adapter_with_default_config() {
    let cfg: Config = toml::from_str("[[providers]]\nid = \"x\"\nadapter = \"paperqa\"\n").unwrap();
    assert!(cfg.validate().is_ok());
    assert_eq!(cfg.adapters.paperqa.command, "python");
    assert_eq!(cfg.adapters.paperqa.max_asks, 10);
    assert!(cfg.adapters.paperqa.settings.is_none());
    assert!(cfg.adapters.paperqa.paper_directory.is_none());
    assert!(cfg.adapters.paperqa.index_directory.is_none());
    assert!(cfg.adapters.paperqa.index_name.is_none());
    assert!(cfg.adapters.paperqa.extra_args.is_empty());
    assert!(cfg.adapters.paperqa.env.is_empty());
    assert!(cfg.providers[0].settings.is_none());
    // ADR-0035 D1 / D3: 取得と証拠ゲートの既定値。
    assert_eq!(
        cfg.adapters.paperqa.acquire,
        task_worker::AcquireConfig::default()
    );
    assert!(cfg.adapters.paperqa.acquire.command.is_none());
    assert_eq!(cfg.adapters.paperqa.acquire.max_candidates, 30);
    assert_eq!(cfg.adapters.paperqa.acquire.max_pdfs, 12);
    assert_eq!(cfg.adapters.paperqa.acquire.per_query, 20);
    assert_eq!(cfg.adapters.paperqa.acquire.timeout_secs, 30);
    assert!(cfg.adapters.paperqa.acquire.mailto.is_none());
    assert_eq!(
        cfg.adapters.paperqa.evidence,
        task_worker::PaperQaEvidence {
            min_candidates: 5,
            min_pdfs: 3,
            min_cited: 2,
            insufficient_is_error: false,
        }
    );
}

/// ADR-0035 D1 / D3: `[adapters.paperqa.acquire]` と `[adapters.paperqa.evidence]` を読む
/// （`0` を書けばその項目を見ない・取得の段を行わない）。
#[test]
fn reads_paperqa_acquire_and_evidence_tables() {
    let text = "[[providers]]\nid = \"x\"\nadapter = \"paperqa\"\n\n\
             [adapters.paperqa.acquire]\ncommand = \"/opt/pq/.venv/bin/python3\"\nmax_candidates = 40\n\
             max_pdfs = 4\nper_query = 10\ntimeout_secs = 60\nmailto = \"who@example.org\"\n\n\
             [adapters.paperqa.evidence]\nmin_candidates = 0\nmin_pdfs = 1\nmin_cited = 0\n";
    let cfg: Config = toml::from_str(text).unwrap();
    assert!(cfg.validate().is_ok());
    let acquire = &cfg.adapters.paperqa.acquire;
    assert_eq!(
        acquire.command.as_deref(),
        Some("/opt/pq/.venv/bin/python3")
    );
    assert_eq!(acquire.max_candidates, 40);
    assert_eq!(acquire.max_pdfs, 4);
    assert_eq!(acquire.per_query, 10);
    assert_eq!(acquire.timeout_secs, 60);
    assert_eq!(acquire.mailto.as_deref(), Some("who@example.org"));
    assert_eq!(
        cfg.adapters.paperqa.evidence,
        task_worker::PaperQaEvidence {
            min_candidates: 0,
            min_pdfs: 1,
            min_cited: 0,
            insufficient_is_error: false,
        }
    );
    // 部分指定でも残りは既定値。
    let partial: Config = toml::from_str(
            "[[providers]]\nid = \"x\"\nadapter = \"paperqa\"\n\n[adapters.paperqa.acquire]\nmax_pdfs = 2\n",
        )
        .unwrap();
    assert_eq!(partial.adapters.paperqa.acquire.max_pdfs, 2);
    assert_eq!(partial.adapters.paperqa.acquire.max_candidates, 30);
    // 綴り間違いは設定エラー（deny_unknown_fields）。
    assert!(
            toml::from_str::<Config>(
                "[[providers]]\nid = \"x\"\nadapter = \"paperqa\"\n\n[adapters.paperqa.acquire]\nmax_pdf = 2\n"
            )
            .is_err()
        );
    assert!(
            toml::from_str::<Config>(
                "[[providers]]\nid = \"x\"\nadapter = \"paperqa\"\n\n[adapters.paperqa.evidence]\nmin_pdf = 2\n"
            )
            .is_err()
        );
}

#[test]
fn rejects_unknown_fields_in_paperqa_adapter_config() {
    let text =
        "[[providers]]\nid = \"x\"\nadapter = \"paperqa\"\n\n[adapters.paperqa]\nbogus = 1\n";
    assert!(toml::from_str::<Config>(text).is_err());
}

/// ADR-0029 D1: `[adapters.local_deep_research]` の既定値（`python3` を素の状態で使う。mode 既定 quick）。
#[test]
fn accepts_local_deep_research_adapter_with_default_config() {
    let cfg: Config =
        toml::from_str("[[providers]]\nid = \"x\"\nadapter = \"local-deep-research\"\n").unwrap();
    assert!(cfg.validate().is_ok());
    assert_eq!(cfg.adapters.local_deep_research.command, "python3");
    assert_eq!(
        cfg.adapters.local_deep_research.mode,
        task_worker::LdrMode::Quick
    );
    assert!(cfg.adapters.local_deep_research.iterations.is_none());
    assert!(
        cfg.adapters
            .local_deep_research
            .questions_per_iteration
            .is_none()
    );
    assert!(cfg.adapters.local_deep_research.settings.is_empty());
    assert!(cfg.adapters.local_deep_research.env.is_empty());
    // ADR-0031 D2: 既定の閾値。
    assert_eq!(
        cfg.adapters.local_deep_research.evidence.min_search_results,
        5
    );
    assert_eq!(cfg.adapters.local_deep_research.evidence.min_sources, 3);
    assert_eq!(cfg.adapters.local_deep_research.evidence.min_cited, 2);
    assert_eq!(cfg.adapters.local_deep_research.evidence.min_domains, 2);
    // ADR-0063 D2（Phase 109）: 再挑戦の既定値。
    assert_eq!(
        cfg.adapters.local_deep_research.retry_mode,
        task_worker::LdrMode::Detailed
    );
    assert_eq!(cfg.adapters.local_deep_research.retry_iterations, Some(5));
}

#[test]
fn rejects_unknown_fields_in_local_deep_research_adapter_config() {
    let text = "[[providers]]\nid = \"x\"\nadapter = \"local-deep-research\"\n\n[adapters.local_deep_research]\nbogus = 1\n";
    assert!(toml::from_str::<Config>(text).is_err());
}

/// ADR-0031 D2: `[adapters.local_deep_research.evidence]` を読める。`0` を書けばその項目は無効になる
/// （下の値のとおり読めることだけをここでは確認する。ゲートの判定自体は `task_worker::local_deep_research`
/// 側のテスト）。未知のキーは拒否する。
#[test]
fn reads_local_deep_research_evidence_thresholds() {
    let text = "[[providers]]\nid = \"x\"\nadapter = \"local-deep-research\"\n\n\
             [adapters.local_deep_research.evidence]\n\
             min_search_results = 10\n\
             min_sources = 4\n\
             min_cited = 1\n\
             min_domains = 0\n";
    let cfg: Config = toml::from_str(text).unwrap();
    assert!(cfg.validate().is_ok());
    let ev = cfg.adapters.local_deep_research.evidence;
    assert_eq!(ev.min_search_results, 10);
    assert_eq!(ev.min_sources, 4);
    assert_eq!(ev.min_cited, 1);
    assert_eq!(ev.min_domains, 0);
}

#[test]
fn rejects_unknown_fields_in_local_deep_research_evidence_table() {
    let text = "[[providers]]\nid = \"x\"\nadapter = \"local-deep-research\"\n\n\
             [adapters.local_deep_research.evidence]\nbogus = 1\n";
    assert!(toml::from_str::<Config>(text).is_err());
}

/// ADR-0029 D1: `mode`/`iterations`/`questions_per_iteration`/`settings`/`env` を読める。
#[test]
fn reads_local_deep_research_adapter_settings() {
    let text = "[adapters.local_deep_research]\n\
             command = \"/home/u/celeris/ldr/.venv/bin/python\"\n\
             mode = \"detailed\"\n\
             iterations = 2\n\
             questions_per_iteration = 2\n\
             env = { OPENAI_API_KEY = \"unused\" }\n\
             \n\
             [adapters.local_deep_research.settings]\n\
             \"llm.provider\" = \"openai_endpoint\"\n\
             \"search.engine.web.searxng.default_params.engines\" = \"[\\\"bing\\\"]\"\n\
             \n\
             [[providers]]\n\
             id = \"ldr\"\n\
             adapter = \"local-deep-research\"\n";
    let cfg: Config = toml::from_str(text).unwrap();
    assert!(cfg.validate().is_ok());
    assert_eq!(
        cfg.adapters.local_deep_research.command,
        "/home/u/celeris/ldr/.venv/bin/python"
    );
    assert_eq!(
        cfg.adapters.local_deep_research.mode,
        task_worker::LdrMode::Detailed
    );
    assert_eq!(cfg.adapters.local_deep_research.iterations, Some(2));
    assert_eq!(
        cfg.adapters.local_deep_research.questions_per_iteration,
        Some(2)
    );
    assert_eq!(
        cfg.adapters
            .local_deep_research
            .settings
            .get("llm.provider")
            .map(String::as_str),
        Some("openai_endpoint")
    );
    assert_eq!(
        cfg.adapters
            .local_deep_research
            .settings
            .get("search.engine.web.searxng.default_params.engines")
            .map(String::as_str),
        Some("[\"bing\"]")
    );
    assert_eq!(
        cfg.adapters
            .local_deep_research
            .env
            .get("OPENAI_API_KEY")
            .map(String::as_str),
        Some("unused")
    );
}

/// ADR-0029 D1: `local-deep-research` の行には `paperqa` 専用の `settings`（`ProviderConfig.settings`）を
/// 書けない（`paperqa` の行だけで意味を持つフィールドのまま。LDR の設定は `[adapters.local_deep_research]`
/// の table 側だけで持つ、という celeris 側の実装判断）。
#[test]
fn rejects_row_level_settings_field_for_local_deep_research_provider() {
    let cfg: Config = toml::from_str(
        "[[providers]]\nid = \"x\"\nadapter = \"local-deep-research\"\nsettings = \"whatever\"\n",
    )
    .unwrap();
    let err = cfg.validate().unwrap_err().to_string();
    assert!(
        err.contains("settings is only allowed when adapter"),
        "{err}"
    );
}

/// ADR-0027 D3: `settings` は `adapter = "paperqa"` の行だけで意味を持つ。行ごとに上書きできる
/// （`acp` の `command`/`args` と同じ作り）。
#[test]
fn settings_is_only_allowed_on_paperqa_providers_and_overrides_per_row() {
    let cfg: Config =
        toml::from_str("[[providers]]\nid = \"x\"\nadapter = \"fake\"\nsettings = \"whatever\"\n")
            .unwrap();
    let err = cfg.validate().unwrap_err().to_string();
    assert!(
        err.contains("settings is only allowed when adapter"),
        "{err}"
    );

    let cfg: Config = toml::from_str(
        "[[providers]]\nid = \"x\"\nadapter = \"paperqa\"\nsettings = \"/settings/other\"\n",
    )
    .unwrap();
    assert!(cfg.validate().is_ok());
    assert_eq!(
        cfg.providers[0].settings.as_deref(),
        Some("/settings/other")
    );
}

/// ADR-0027 D3: `[adapters.paperqa]` の `paper_directory`/`index_directory`/`settings`（共通・行の上書き
/// どちらも）は他のパス設定と同じく設定ファイルのディレクトリ基準で絶対化する。
#[test]
fn paperqa_paths_are_resolved_relative_to_the_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[adapters.paperqa]\n\
             paper_directory = \"papers\"\n\
             index_directory = \"index\"\n\
             settings = \"settings/qwen-local\"\n\
             \n\
             [[providers]]\n\
             id = \"pqa\"\n\
             adapter = \"paperqa\"\n\
             settings = \"settings/other\"\n",
    )
    .unwrap();
    let cfg = Config::load(&path).unwrap();
    let base = dir.path().canonicalize().unwrap();
    assert_eq!(
        cfg.adapters.paperqa.paper_directory,
        Some(base.join("papers"))
    );
    assert_eq!(
        cfg.adapters.paperqa.index_directory,
        Some(base.join("index"))
    );
    assert_eq!(
        cfg.adapters.paperqa.settings.as_deref(),
        Some(
            base.join("settings/qwen-local")
                .to_string_lossy()
                .into_owned()
                .as_str()
        )
    );
    assert_eq!(
        cfg.providers[0].settings.as_deref(),
        Some(
            base.join("settings/other")
                .to_string_lossy()
                .into_owned()
                .as_str()
        )
    );
}

/// ADR-0027 D3: 分野・調査ハーネスを両方載せた例の設定ファイルが読め、検証を通る。
#[test]
fn loads_research_example_config() {
    let path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config/celeris.research.example.toml"
    ));
    let cfg = Config::load(path).unwrap();
    assert!(cfg.validate().is_ok());
    // ADR-0063 Phase 109d C2: `pqa` CLI ではなく venv の python（`paperqa_ask.py` を起動する）。
    assert_eq!(
        cfg.adapters.paperqa.command,
        "/home/u/celeris/paperqa/.venv/bin/python"
    );
    // `.json` を付けずに渡す（実機の仕様）。
    assert_eq!(
        cfg.adapters.paperqa.settings.as_deref(),
        Some("/home/u/celeris/paperqa/settings/proxy")
    );
    assert_eq!(
        cfg.adapters.paperqa.paper_directory.as_deref(),
        Some(Path::new("/home/u/celeris/paperqa/papers"))
    );
    assert_eq!(
        cfg.adapters
            .paperqa
            .env
            .get("OPENAI_BASE_URL")
            .map(String::as_str),
        Some("http://127.0.0.1:18100/v1")
    );
    let paperqa_provider = cfg
        .providers
        .iter()
        .find(|p| p.adapter == "paperqa")
        .expect("paperqa provider");
    assert_eq!(paperqa_provider.model, "openai/celeris/standard");
    let genre_ids: Vec<&str> = cfg.genres.iter().map(|g| g.id.as_str()).collect();
    assert_eq!(genre_ids, vec!["coding", "literature"]);
    let literature = cfg
        .genres
        .iter()
        .find(|g| g.id == "literature")
        .expect("literature genre");
    assert_eq!(
        literature.default_role.as_deref(),
        Some("literature-reader")
    );
    assert_eq!(
        literature.roles,
        vec![
            "literature-scout".to_string(),
            "literature-reader".to_string(),
            "novelty-skeptic".to_string()
        ]
    );
    // ADR-0028 D1: 能力・入出力の目安も読める（ADR-0035 で取得の段が入ったので中身が変わった）。
    assert_eq!(
        literature.capabilities,
        vec![
            "学術文献の検索と取得（arXiv / OpenAlex）".to_string(),
            "PDF 全文からの根拠抽出".to_string(),
            "引用付きの要約".to_string()
        ]
    );
    assert_eq!(
        literature.input_artifacts,
        vec![
            "question".to_string(),
            "pdf".to_string(),
            "bibliography".to_string()
        ]
    );
    // Phase 38（ADR-0028 追記）: `名前: 説明` の形で書ける（設定は文字列のまま読み、名前は `:` の前）。
    // ADR-0063 Phase 109b A3: `report.md` が標準（answer.md と同じ内容）。
    assert_eq!(
        literature.output_artifacts,
        vec![
            "report.md: 引用付きの答え（これが答え。answer.md と同じ内容。ADR-0063 Phase 109b A3）"
                .to_string(),
            "answer.md: report.md と同じ内容（PaperQA 固有の名前）".to_string(),
            "papers.json: 検索した論文の一覧（コーパス。答えではない）".to_string(),
            "sources.json: 出典と引用の有無".to_string(),
            "queries.json: 使った検索語".to_string()
        ]
    );
    assert_eq!(
        cfg.genre_specs()
            .iter()
            .find(|g| g.id == "literature")
            .map(|g| g.output_artifact_names()),
        Some(vec![
            "report.md",
            "answer.md",
            "papers.json",
            "sources.json",
            "queries.json"
        ])
    );
    // ADR-0035 D1 / D3: 取得と証拠ゲートの例の値。
    assert_eq!(cfg.adapters.paperqa.acquire.max_candidates, 30);
    assert_eq!(cfg.adapters.paperqa.acquire.max_pdfs, 12);
    assert_eq!(cfg.adapters.paperqa.acquire.per_query, 20);
    assert!(
        cfg.adapters.paperqa.acquire.command.is_none(),
        "既定は pqa の隣の python3"
    );
    assert_eq!(
        cfg.adapters.paperqa.evidence,
        task_worker::PaperQaEvidence {
            min_candidates: 5,
            min_pdfs: 3,
            min_cited: 2,
            insufficient_is_error: false,
        }
    );
    // ADR-0063 D1（Phase 109）: 既定でアブストの妥協を許す。
    assert!(cfg.adapters.paperqa.acquire.abstract_fallback);
    assert_eq!(
        cfg.adapters
            .paperqa
            .env
            .get("RES_OPTIONS")
            .map(String::as_str),
        Some("single-request")
    );
    let coding = cfg
        .genres
        .iter()
        .find(|g| g.id == "coding")
        .expect("coding genre");
    assert!(!coding.capabilities.is_empty());
    assert!(!coding.input_artifacts.is_empty());
    assert!(!coding.output_artifacts.is_empty());
}

/// ADR-0029 D1/D2: Web 調査（Local Deep Research）の例の設定ファイルが読め、検証を通る。
/// `web-research` 分野の manifest は ADR-0029 D2 のとおり。
#[test]
fn loads_web_research_example_config() {
    let path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config/celeris.web-research.example.toml"
    ));
    let cfg = Config::load(path).unwrap();
    assert!(cfg.validate().is_ok());
    assert_eq!(
        cfg.adapters.local_deep_research.command,
        "/home/u/celeris/ldr/.venv/bin/python"
    );
    assert_eq!(
        cfg.adapters.local_deep_research.mode,
        task_worker::LdrMode::Quick
    );
    // ADR-0031 D4: 既定は Tavily（鍵は `env_from_secrets` で渡す）。
    assert_eq!(
        cfg.adapters
            .local_deep_research
            .settings
            .get("search.tool")
            .map(String::as_str),
        Some("tavily")
    );
    // 実機の罠（PROGRESS の Phase 21「真因: DNS」）: これが無いと、このホストの DNS では
    // LDR の DNS ピン留めが 5 秒で fail-closed し、どのエンジンでも「0 件」になる。
    assert_eq!(
        cfg.adapters
            .local_deep_research
            .env
            .get("RES_OPTIONS")
            .map(String::as_str),
        Some("single-request")
    );
    let ldr_provider = cfg
        .providers
        .iter()
        .find(|p| p.adapter == "local-deep-research")
        .expect("ldr provider");
    assert_eq!(ldr_provider.model, "celeris/cheap");
    // ADR-0031 D2: 既定の証拠ゲート閾値を明示している。
    assert_eq!(
        cfg.adapters.local_deep_research.evidence.min_search_results,
        5
    );
    assert_eq!(cfg.adapters.local_deep_research.evidence.min_sources, 3);
    assert_eq!(cfg.adapters.local_deep_research.evidence.min_cited, 2);
    assert_eq!(cfg.adapters.local_deep_research.evidence.min_domains, 2);
    // ADR-0063 D2（Phase 109）: 例の設定でも既定値を明示している。
    assert_eq!(
        cfg.adapters.local_deep_research.retry_mode,
        task_worker::LdrMode::Detailed
    );
    assert_eq!(cfg.adapters.local_deep_research.retry_iterations, Some(5));
    let genre = cfg
        .genres
        .iter()
        .find(|g| g.id == "web-research")
        .expect("web-research genre");
    assert_eq!(genre.default_role.as_deref(), Some("web-scout"));
    assert_eq!(genre.roles, vec!["web-scout".to_string()]);
    // Phase 38（ADR-0028 追記）: `名前: 説明` で書ける（名前は `:` の前）。
    assert_eq!(
        genre.output_artifacts,
        vec![
            "report.md: 出典付きの調査報告（これが答え）".to_string(),
            "sources.json: 出典と引用の有無".to_string(),
            "research.json: 検索の記録（クエリと件数）".to_string()
        ]
    );
    assert_eq!(
        cfg.genre_specs()
            .iter()
            .find(|g| g.id == "web-research")
            .map(|g| g.output_artifact_names()),
        Some(vec!["report.md", "sources.json", "research.json"])
    );
    let role = cfg
        .roles
        .iter()
        .find(|r| r.id == "web-scout")
        .expect("web-scout role");
    assert_eq!(role.adapter.as_deref(), Some("local-deep-research"));
    // ADR-0030 D1: `[secrets] dir` が読め、設定ファイル基準で絶対化される。鍵の値そのものはファイルに無い。
    let secrets = cfg.secrets.as_ref().expect("[secrets]");
    assert!(secrets.dir.is_absolute());
    assert_eq!(
        secrets.dir.file_name().and_then(|n| n.to_str()),
        Some("secrets")
    );
    assert!(
        !std::fs::read_to_string(path).unwrap().contains("tvly-"),
        "example config must not contain a real key"
    );
}

/// ADR-0010 D6/D9: バックオフと `[reviewer]` の既定値・指定値が DispatchConfig に写る。
#[test]
fn backoff_and_reviewer_settings_map_to_dispatch_config() {
    let cfg: Config = toml::from_str("[[providers]]\nid = \"x\"\nadapter = \"fake\"\n").unwrap();
    assert!(cfg.validate().is_ok());
    let d = cfg.dispatch_config();
    assert_eq!(d.retry_backoff_base, Duration::from_secs(10));
    assert_eq!(d.retry_backoff_max, Duration::from_secs(300));
    assert_eq!(d.max_requeues, 5);
    assert_eq!(d.min_free_disk_mb, 5120);
    assert_eq!(
        d.reviewer_hint,
        WorkerHint {
            tier: Tier::Standard,
            adapter: None
        }
    );

    let text = r#"retry_backoff_base_secs = 0
retry_backoff_max_secs = 0
max_requeues = 0
[reviewer]
adapter = "claude-code"
tier = "cheap"
[[providers]]
id = "f"
adapter = "fake"
[[providers]]
id = "c"
adapter = "claude-code"
tiers = ["cheap"]
"#;
    let cfg: Config = toml::from_str(text).unwrap();
    assert!(cfg.validate().is_ok());
    let d = cfg.dispatch_config();
    assert_eq!(d.retry_backoff_base, Duration::ZERO);
    assert_eq!(d.max_requeues, 0);
    assert_eq!(
        d.reviewer_hint,
        WorkerHint {
            tier: Tier::Cheap,
            adapter: Some("claude-code".into())
        }
    );
}

/// `[scratch.cargo]` は従来どおり有効。廃止された cache 設定は worker に渡さない。
#[test]
fn scratch_cargo_defaults_disable_incremental() {
    let cfg: Config = toml::from_str("[[providers]]\nid = \"x\"\nadapter = \"fake\"\n").unwrap();
    let s = cfg.scratch_settings_unchecked();
    assert_eq!(
        s.cargo,
        task_worker::scratch::CargoTuning {
            incremental: false,
            dev_debug: Some("line-tables-only".to_string()),
        }
    );
    let cfg: Config = toml::from_str(
        "[[providers]]\nid = \"x\"\nadapter = \"fake\"\n[scratch.cargo]\nincremental = true\ndev_debug = \"\"\n",
    )
    .unwrap();
    assert_eq!(
        cfg.scratch_settings_unchecked().cargo,
        task_worker::scratch::CargoTuning {
            incremental: true,
            dev_debug: None,
        }
    );
    assert!(toml::from_str::<Config>("[scratch.cargo]\nbogus = 1\n").is_err());
}

/// 旧節は未知の項目を含んでも読めるが、検出して警告でき、値は実行設定へ届かない。
#[test]
fn legacy_scratch_cache_sections_are_ignored_and_reported() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[[providers]]\nid = \"x\"\nadapter = \"fake\"\n[scratch.sccache]\nenabled = true\nport = 4300\nbogus = 1\n[scratch.cache_server]\nenabled = true\nport = 4299\n[scratch.l2]\nenabled = true\ndir = \"/nfs/l2\"\n",
    )
    .unwrap();
    let cfg = Config::load(&path).unwrap();
    assert_eq!(
        cfg.scratch.deprecated_sections(),
        ["scratch.sccache", "scratch.l2", "scratch.cache_server"]
    );
    let settings = cfg.scratch_settings_unchecked();
    let env = task_worker::scratch::cargo_env(&settings, &task_worker::scratch::Owner::task("01T"));
    assert!(
        env.iter()
            .all(|(k, _)| !k.starts_with("SCCACHE_") && k != "RUSTC_WRAPPER"),
        "{env:?}"
    );
    let clean: Config = toml::from_str("").unwrap();
    assert!(clean.scratch.deprecated_sections().is_empty());
}

/// ADR-0075 D7: `[scratch] dir` の既定は `build_cache_dir` の親の `scratch/`。
#[test]
fn scratch_defaults_follow_the_build_cache_parent() {
    let cfg: Config = toml::from_str(
            "[[providers]]\nid = \"x\"\nadapter = \"fake\"\n[workspace]\nbuild_cache_dir = \"/var/lib/celeris/build-cache\"\n",
        )
        .unwrap();
    let s = cfg.scratch_settings_unchecked();
    assert!(s.enabled);
    assert_eq!(s.dir, PathBuf::from("/var/lib/celeris/scratch"));
    assert_eq!(s.targets_max_bytes, 100 * task_worker::scratch::GIB);
    assert_eq!(s.total_max_bytes, 150 * task_worker::scratch::GIB);
    assert_eq!((s.high_watermark, s.low_watermark), (0.90, 0.70));
    assert_eq!(s.external_lease_ttl_secs, 21_600);
    assert_eq!(s.gc_max_per_tick, 8);
    assert!(s.adopt);
    // 明示すればそれを使う。未知のキーは拒否。
    let cfg: Config = toml::from_str(
            "[[providers]]\nid = \"x\"\nadapter = \"fake\"\n[scratch]\ndir = \"/srv/scratch\"\ntargets_max_gb = 10\nhigh_watermark = 0.8\n",
        )
        .unwrap();
    let s = cfg.scratch_settings_unchecked();
    assert_eq!(s.dir, PathBuf::from("/srv/scratch"));
    assert_eq!(s.targets_max_bytes, 10 * task_worker::scratch::GIB);
    assert_eq!(s.high_watermark, 0.8);
    assert!(
        toml::from_str::<Config>(
            "[[providers]]\nid = \"x\"\nadapter = \"fake\"\n[scratch]\nbogus = 1\n"
        )
        .is_err()
    );
    // 起動時の検査は一時ディレクトリ（ローカル）では有効のまま。
    let tmp = tempfile::tempdir().unwrap();
    let cfg: Config = toml::from_str(&format!(
        "[[providers]]\nid = \"x\"\nadapter = \"fake\"\n[scratch]\ndir = \"{}\"\n",
        tmp.path().join("scratch").display()
    ))
    .unwrap();
    assert!(cfg.dispatch_config().scratch.enabled);
}

/// ADR-0129 (3)(4): `[scratch] mount` が mount されていなければ `dir` を既定の場所へ戻す。`seed_reflink` の既定は false。
#[test]
fn scratch_mount_falls_back_to_default_dir_and_seed_reflink_defaults_off() {
    let tmp = tempfile::tempdir().unwrap();
    let not_mounted = tmp.path().join("local");
    std::fs::create_dir_all(&not_mounted).unwrap();
    let cfg: Config = toml::from_str(&format!(
        "[[providers]]\nid = \"x\"\nadapter = \"fake\"\n[scratch]\nmount = \"{m}\"\ndir = \"{m}/celeris/scratch\"\n",
        m = not_mounted.display()
    ))
    .unwrap();
    let unchecked = cfg.scratch_settings_unchecked();
    assert!(!unchecked.seed_reflink);
    assert_eq!(unchecked.mount.as_deref(), Some(not_mounted.as_path()));
    let s = cfg.scratch_settings();
    assert_eq!(s.dir, cfg.default_scratch_dir());
    assert!(s.dir_fallback_reason.unwrap().contains("not mounted"));
    let cfg: Config = toml::from_str(
        "[[providers]]\nid = \"x\"\nadapter = \"fake\"\n[scratch]\nseed_reflink = true\n",
    )
    .unwrap();
    assert!(cfg.scratch_settings_unchecked().seed_reflink);
}

/// ADR-0075 D7: `[scratch] enabled = false` で F5-fix の挙動に戻す（dispatcher は build_cache_dir を使う）。
#[test]
fn scratch_can_be_disabled() {
    let cfg: Config = toml::from_str(
        "[[providers]]\nid = \"x\"\nadapter = \"fake\"\n[scratch]\nenabled = false\n",
    )
    .unwrap();
    let d = cfg.dispatch_config();
    assert!(!d.scratch.enabled);
    assert_eq!(d.scratch.disabled_reason, None);
    assert!(d.shared_build_cache);
}

#[test]
fn dispatch_min_free_disk_mb_can_be_configured() {
    let cfg: Config = toml::from_str(
        "[[providers]]\nid = \"x\"\nadapter = \"fake\"\n[dispatch]\nmin_free_disk_mb = 2048\n",
    )
    .unwrap();
    assert_eq!(cfg.dispatch_config().min_free_disk_mb, 2048);
}

/// ADR-0010 D9: Reviewer run を満たせるプロバイダが無い設定はエラー。未知キーも拒否。
#[test]
fn rejects_reviewer_without_matching_provider_and_unknown_reviewer_keys() {
    let cfg: Config = toml::from_str(
        "[reviewer]\nadapter = \"codex\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
    )
    .unwrap();
    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("[reviewer]") && err.contains("codex"), "{err}");
    // ADR-0069 Phase 118 D4: `[reviewer] tier` が未設定なら lane は動的（worker lane に一致・
    // 天井で丸め）なので、どれか 1 tier を提供していれば足りる（従来は既定の Standard 固定で
    // 検査していたため、frontier だけのプロバイダはこの検査に落ちていた）。
    let cfg: Config =
        toml::from_str("[[providers]]\nid = \"x\"\nadapter = \"fake\"\ntiers = [\"frontier\"]\n")
            .unwrap();
    assert!(cfg.validate().is_ok());
    // `[reviewer] tier` を明示すれば、その 1 tier を提供するプロバイダが無ければ従来どおりエラー。
    let cfg: Config = toml::from_str(
            "[reviewer]\ntier = \"cheap\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\ntiers = [\"frontier\"]\n",
        )
        .unwrap();
    assert!(cfg.validate().unwrap_err().to_string().contains("Cheap"));
    assert!(toml::from_str::<Config>("[reviewer]\nbogus = 1\n").is_err());
    let cfg: Config = toml::from_str("retry_backoff_base_secs = 20\nretry_backoff_max_secs = 10\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n").unwrap();
    assert!(cfg.validate().is_err());
}

/// Phase 7 監査: cooldown 0（requeue のホットループ）と、リース延長の前提を破る猶予の組み合わせを拒否する。
#[test]
fn rejects_zero_cooldown_and_unsafe_lease_grace() {
    let providers = "[[providers]]\nid = \"x\"\nadapter = \"fake\"\n";
    let cfg: Config = toml::from_str(&format!("error_cooldown_secs = 0\n{providers}")).unwrap();
    assert!(
        cfg.validate()
            .unwrap_err()
            .to_string()
            .contains("error_cooldown_secs")
    );
    let cfg: Config = toml::from_str(&format!(
        "lease_grace_secs = 10\nkill_grace_secs = 10\n{providers}"
    ))
    .unwrap();
    assert!(
        cfg.validate()
            .unwrap_err()
            .to_string()
            .contains("lease_grace_secs")
    );
    let cfg: Config = toml::from_str(&format!(
        "lease_grace_secs = 60\nkill_grace_secs = 1\ntick_ms = 50\n{providers}"
    ))
    .unwrap();
    assert!(cfg.validate().is_ok());
}

/// ADR-0012 D1: 同じアダプタ種別のプロバイダを複数並べ、それぞれに env を持たせられる。ID の重複は拒否。
#[test]
fn multi_account_providers_parse_and_duplicate_ids_are_rejected() {
    let path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config/celeris.multi-account.example.toml"
    ));
    let cfg = Config::load(path).unwrap();
    let claude: Vec<&ProviderConfig> = cfg
        .providers
        .iter()
        .filter(|p| p.adapter == "claude-code")
        .collect();
    assert!(claude.len() >= 2);
    assert_ne!(
        claude[0].env.get("CLAUDE_CONFIG_DIR"),
        claude[1].env.get("CLAUDE_CONFIG_DIR")
    );

    let dup = "[[providers]]\nid = \"a\"\nadapter = \"fake\"\n[[providers]]\nid = \"a\"\nadapter = \"fake\"\n";
    let cfg: Config = toml::from_str(dup).unwrap();
    assert!(
        cfg.validate()
            .unwrap_err()
            .to_string()
            .contains("duplicate provider id")
    );
}

/// ADR-0013 D3 / D11: `[api]` は既定で無効。loopback 以外はトークンファイル必須。相対パスは設定ファイル基準。
#[test]
fn api_section_defaults_to_disabled_and_requires_token_off_loopback() {
    let providers = "[[providers]]\nid = \"x\"\nadapter = \"fake\"\n";
    let cfg: Config = toml::from_str(providers).unwrap();
    assert!(cfg.validate().is_ok());
    assert!(cfg.api.listen.is_none());

    let cfg: Config =
        toml::from_str(&format!("[api]\nlisten = \"127.0.0.1:7700\"\n{providers}")).unwrap();
    assert!(cfg.validate().is_ok());
    let cfg: Config =
        toml::from_str(&format!("[api]\nlisten = \"[::1]:7700\"\n{providers}")).unwrap();
    assert!(cfg.validate().is_ok());

    let cfg: Config =
        toml::from_str(&format!("[api]\nlisten = \"0.0.0.0:7700\"\n{providers}")).unwrap();
    assert!(
        cfg.validate()
            .unwrap_err()
            .to_string()
            .contains("token_file is required")
    );
    let cfg: Config = toml::from_str(&format!(
        "[api]\nlisten = \"0.0.0.0:7700\"\ntoken_file = \"api.token\"\n{providers}"
    ))
    .unwrap();
    assert!(cfg.validate().is_ok());
    assert!(toml::from_str::<Config>("[api]\nbogus = 1\n").is_err());

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        format!(
            "[api]\nlisten = \"127.0.0.1:7700\"\ntoken_file = \"secrets/api.token\"\n{providers}"
        ),
    )
    .unwrap();
    // token_file が無い・空なら起動時の設定エラー（値は出さない）。
    let err = Config::load(&path).unwrap_err().to_string();
    assert!(
        err.contains("token_file") && err.contains("cannot be read"),
        "{err}"
    );
    std::fs::create_dir_all(dir.path().join("secrets")).unwrap();
    std::fs::write(dir.path().join("secrets/api.token"), " \n").unwrap();
    assert!(
        Config::load(&path)
            .unwrap_err()
            .to_string()
            .contains("is empty")
    );
    std::fs::write(dir.path().join("secrets/api.token"), "  tok-123\n").unwrap();
    let cfg = Config::load(&path).unwrap();
    assert_eq!(cfg.api.read_token().unwrap().as_deref(), Some("tok-123"));
    assert_eq!(
        cfg.api.token_file.unwrap(),
        dir.path().canonicalize().unwrap().join("secrets/api.token")
    );
}

/// ADR-0016 D1 / D2: `[[roles]]` と `[delegation]` を読み、task-core の型と DispatchConfig に写す。
#[test]
fn roles_and_delegation_are_parsed_and_mapped() {
    let providers = "[[providers]]\nid = \"x\"\nadapter = \"fake\"\n";
    // 既定（節を書かなければ空の役割表と DelegationLimits の既定）。
    let cfg: Config = toml::from_str(providers).unwrap();
    assert!(cfg.validate().is_ok());
    assert!(cfg.roles.is_empty());
    assert_eq!(
        cfg.delegation_limits(),
        task_core::DelegationLimits::default()
    );
    assert_eq!(
        cfg.delegation_limits(),
        DelegationLimits {
            max_delegate_per_run: 8,
            max_tree_depth: 5,
            max_tree_runs: 100,
            on_child_failure: task_core::OnChildFailure::RetryThenAsk,
        }
    );

    let text = format!(
        r#"[[roles]]
id = "lead"
tier = "frontier"
max_turns = 40
max_wall_secs = 1800
instructions = "You lead the work. Delegate implementation."

[[roles]]
id = "implementer"
adapter = "fake"

[delegation]
max_delegate_per_run = 3
max_tree_depth = 2

{providers}"#
    );
    let cfg: Config = toml::from_str(&text).unwrap();
    assert!(cfg.validate().is_ok());
    let specs = cfg.role_specs();
    assert_eq!(specs.len(), 2);
    assert_eq!(specs[0].id, "lead");
    assert_eq!(specs[0].tier, Some(Tier::Frontier));
    assert_eq!(
        (specs[0].max_turns, specs[0].max_wall_secs),
        (Some(40), Some(1800))
    );
    assert!(
        specs[0]
            .instructions
            .as_deref()
            .unwrap()
            .starts_with("You lead")
    );
    assert_eq!(specs[0].adapter, None);
    assert_eq!(specs[1].adapter.as_deref(), Some("fake"));
    assert_eq!(specs[1].tier, None);
    // 書いていない値は既定のまま。
    let limits = cfg.delegation_limits();
    assert_eq!(
        limits,
        DelegationLimits {
            max_delegate_per_run: 3,
            max_tree_depth: 2,
            max_tree_runs: 100,
            on_child_failure: task_core::OnChildFailure::RetryThenAsk,
        }
    );
    let d = cfg.dispatch_config();
    assert_eq!(d.roles, specs);
    assert_eq!(d.delegation, limits);

    assert!(toml::from_str::<Config>("[[roles]]\nid = \"a\"\nbogus = 1\n").is_err());
    assert!(toml::from_str::<Config>("[delegation]\nbogus = 1\n").is_err());
}

/// ADR-0021 D4: `on_child_failure` は `retry_then_ask`（既定）と `ignore` だけ。知らない値は設定エラー。
#[test]
fn delegation_on_child_failure_is_parsed_and_validated() {
    let with = |v: &str| {
        format!(
            "[delegation]\non_child_failure = \"{v}\"\n[[providers]]\nid = \"p\"\nadapter = \"fake\"\n"
        )
    };
    let cfg: Config = toml::from_str(&with("ignore")).unwrap();
    cfg.validate().unwrap();
    assert_eq!(
        cfg.delegation_limits().on_child_failure,
        task_core::OnChildFailure::Ignore
    );

    let cfg: Config = toml::from_str(&with("retry_then_ask")).unwrap();
    cfg.validate().unwrap();
    assert_eq!(
        cfg.delegation_limits().on_child_failure,
        task_core::OnChildFailure::RetryThenAsk
    );

    let cfg: Config = toml::from_str(&with("fail_parent")).unwrap();
    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("on_child_failure"), "{err}");
}

/// ADR-0016: 役割 id の重複、未知の adapter、0 の上限は設定エラー。
#[test]
fn rejects_duplicate_roles_unknown_role_adapter_and_zero_limits() {
    let providers = "[[providers]]\nid = \"x\"\nadapter = \"fake\"\n";
    let dup = format!("[[roles]]\nid = \"lead\"\n[[roles]]\nid = \"lead\"\n{providers}");
    let cfg: Config = toml::from_str(&dup).unwrap();
    assert_eq!(
        cfg.validate().unwrap_err().to_string(),
        "invalid config: duplicate role id: lead"
    );

    let bogus = format!("[[roles]]\nid = \"lead\"\nadapter = \"bogus\"\n{providers}");
    let cfg: Config = toml::from_str(&bogus).unwrap();
    assert_eq!(
        cfg.validate().unwrap_err().to_string(),
        "invalid config: [[roles]] lead: adapter \"bogus\" is not available in this build (fake, claude-code, codex, acp, browser-specialist, paperqa, local-deep-research, langmem only)"
    );

    let empty = format!("[[roles]]\nid = \"  \"\n{providers}");
    let cfg: Config = toml::from_str(&empty).unwrap();
    assert!(
        cfg.validate()
            .unwrap_err()
            .to_string()
            .contains("id must not be empty")
    );

    let zero_turns = format!("[[roles]]\nid = \"lead\"\nmax_turns = 0\n{providers}");
    let cfg: Config = toml::from_str(&zero_turns).unwrap();
    assert!(
        cfg.validate()
            .unwrap_err()
            .to_string()
            .contains("max_turns must be >= 1")
    );
    let zero_wall = format!("[[roles]]\nid = \"lead\"\nmax_wall_secs = 0\n{providers}");
    let cfg: Config = toml::from_str(&zero_wall).unwrap();
    assert!(
        cfg.validate()
            .unwrap_err()
            .to_string()
            .contains("max_wall_secs must be >= 1")
    );

    for key in ["max_delegate_per_run", "max_tree_depth", "max_tree_runs"] {
        let text = format!("[delegation]\n{key} = 0\n{providers}");
        let cfg: Config = toml::from_str(&text).unwrap();
        let err = cfg.validate().unwrap_err().to_string();
        assert_eq!(
            err,
            format!("invalid config: [delegation] {key} must be >= 1")
        );
    }
}

/// ADR-0027 D1: `[[genres]]` を読み、task-core の `GenreSpec` に写す。`[[genres]]` を書かない設定は
/// 今までどおり動く（分野は任意）。
#[test]
fn genres_are_parsed_and_mapped() {
    let providers = "[[providers]]\nid = \"x\"\nadapter = \"fake\"\n";
    let cfg: Config = toml::from_str(providers).unwrap();
    assert!(cfg.validate().is_ok());
    assert!(cfg.genres.is_empty());
    assert!(cfg.genre_specs().is_empty());

    let text = format!(
        r#"[[roles]]
id = "lead"

[[roles]]
id = "implementer"

[[genres]]
id = "coding"
description = "write and fix code"
default_role = "implementer"
roles = ["lead", "implementer"]

[[genres]]
id = "related-research"
description = "先行研究の確認・新規性の検討"
capabilities = ["学術文献の検索", "引用グラフの探索", "PDF 全文からの根拠抽出"]
input_artifacts = ["question", "pdf", "bibliography"]
output_artifacts = ["answer.md", "citations.json"]
default_role = "lead"
roles = ["lead"]

{providers}"#
    );
    let cfg: Config = toml::from_str(&text).unwrap();
    assert!(cfg.validate().is_ok());
    let specs = cfg.genre_specs();
    assert_eq!(specs.len(), 2);
    assert_eq!(specs[0].id, "coding");
    assert_eq!(specs[0].description, "write and fix code");
    assert_eq!(specs[0].default_role.as_deref(), Some("implementer"));
    assert_eq!(
        specs[0].roles,
        vec!["lead".to_string(), "implementer".to_string()]
    );
    // ADR-0028 D1: 3 フィールドを書かなければ空（既存設定との互換）。
    assert!(specs[0].capabilities.is_empty());
    assert!(specs[0].input_artifacts.is_empty());
    assert!(specs[0].output_artifacts.is_empty());
    // ADR-0028 D1: 書けば `GenreSpec` に写る。
    assert_eq!(
        specs[1].capabilities,
        vec![
            "学術文献の検索".to_string(),
            "引用グラフの探索".to_string(),
            "PDF 全文からの根拠抽出".to_string()
        ]
    );
    assert_eq!(
        specs[1].input_artifacts,
        vec![
            "question".to_string(),
            "pdf".to_string(),
            "bibliography".to_string()
        ]
    );
    assert_eq!(
        specs[1].output_artifacts,
        vec!["answer.md".to_string(), "citations.json".to_string()]
    );
    let d = cfg.dispatch_config();
    assert_eq!(d.genres, specs);

    assert!(
        toml::from_str::<Config>("[[genres]]\nid = \"a\"\ndescription = \"d\"\nbogus = 1\n")
            .is_err()
    );
}

/// ADR-0027 D1: 分野 id の重複、知らない役割を指す `roles`/`default_role`、`roles` に無い
/// `default_role` は設定エラー。
#[test]
fn rejects_duplicate_genre_ids_and_genres_referencing_unknown_or_mismatched_roles() {
    let providers = "[[providers]]\nid = \"x\"\nadapter = \"fake\"\n";
    let roles = "[[roles]]\nid = \"lead\"\n\n[[roles]]\nid = \"implementer\"\n";

    let dup = format!(
        "{roles}[[genres]]\nid = \"coding\"\ndescription = \"d\"\n[[genres]]\nid = \"coding\"\ndescription = \"d\"\n{providers}"
    );
    let cfg: Config = toml::from_str(&dup).unwrap();
    assert_eq!(
        cfg.validate().unwrap_err().to_string(),
        "invalid config: duplicate genre id: coding"
    );

    let empty_id = format!("[[genres]]\nid = \"  \"\ndescription = \"d\"\n{providers}");
    let cfg: Config = toml::from_str(&empty_id).unwrap();
    assert!(
        cfg.validate()
            .unwrap_err()
            .to_string()
            .contains("id must not be empty")
    );

    let unknown_role_in_roles = format!(
        "{roles}[[genres]]\nid = \"coding\"\ndescription = \"d\"\nroles = [\"lead\", \"nobody\"]\n{providers}"
    );
    let cfg: Config = toml::from_str(&unknown_role_in_roles).unwrap();
    assert_eq!(
        cfg.validate().unwrap_err().to_string(),
        "invalid config: [[genres]] coding: role \"nobody\" in roles is not defined in [[roles]]"
    );

    let unknown_default_role = format!(
        "{roles}[[genres]]\nid = \"coding\"\ndescription = \"d\"\nroles = [\"lead\"]\ndefault_role = \"nobody\"\n{providers}"
    );
    let cfg: Config = toml::from_str(&unknown_default_role).unwrap();
    assert_eq!(
        cfg.validate().unwrap_err().to_string(),
        "invalid config: [[genres]] coding: default_role \"nobody\" is not defined in [[roles]]"
    );

    let default_role_not_in_roles = format!(
        "{roles}[[genres]]\nid = \"coding\"\ndescription = \"d\"\nroles = [\"lead\"]\ndefault_role = \"implementer\"\n{providers}"
    );
    let cfg: Config = toml::from_str(&default_role_not_in_roles).unwrap();
    assert_eq!(
        cfg.validate().unwrap_err().to_string(),
        "invalid config: [[genres]] coding: default_role \"implementer\" must be included in roles"
    );
}

/// Phase 30（ADR-0033 D4 追記）: `[conversation] genre` の既定は `task_core::CONVERSATION_GENRE`
/// （`"secretary"`）で、`[[genres]]` を書かない最小構成は今までどおり動く。明示したのに
/// `[[genres]]` に無ければ「対話用の分野が無い」設定エラー。明示して存在すれば通る。
#[test]
fn conversation_genre_defaults_to_secretary_and_an_unknown_genre_is_a_config_error() {
    let providers = "[[providers]]\nid = \"x\"\nadapter = \"fake\"\n";

    // `[conversation]` を書かない: 既定は `secretary`。`[[genres]]` の中身は検証しない
    // （最小構成 = genres 無しでも壊れない）。
    let cfg: Config = toml::from_str(providers).unwrap();
    assert!(cfg.validate().is_ok());
    assert_eq!(cfg.conversation_genre_id(), task_core::CONVERSATION_GENRE);
    assert_eq!(cfg.conversation_genre_id(), "secretary");

    // `[conversation]` を書いて `genre` を省略: それでも既定は `secretary`。
    let text = format!("[conversation]\n{providers}");
    let cfg: Config = toml::from_str(&text).unwrap();
    assert_eq!(cfg.conversation_genre_id(), "secretary");
    // `secretary` が `[[genres]]` に無いので設定エラー（明示した以上は検証する）。
    assert_eq!(
        cfg.validate().unwrap_err().to_string(),
        "invalid config: [conversation]: genre \"secretary\" is not defined in [[genres]] (対話用の分野が無い)"
    );

    // 存在しない分野を明示して指す: 設定エラー。
    let text = format!("[conversation]\ngenre = \"nope\"\n{providers}");
    let cfg: Config = toml::from_str(&text).unwrap();
    assert_eq!(
        cfg.validate().unwrap_err().to_string(),
        "invalid config: [conversation]: genre \"nope\" is not defined in [[genres]] (対話用の分野が無い)"
    );

    // 存在する分野を明示して指す: 通る。
    let text = format!(
        "[conversation]\ngenre = \"secretary\"\n[[genres]]\nid = \"secretary\"\ndescription = \"d\"\n{providers}"
    );
    let cfg: Config = toml::from_str(&text).unwrap();
    assert!(cfg.validate().is_ok());
    assert_eq!(cfg.conversation_genre_id(), "secretary");

    // 未知のキーは設定エラー。
    assert!(toml::from_str::<Config>("[conversation]\nbogus = 1\n").is_err());
}

#[test]
fn rejects_unknown_fields_in_codex_adapter_config() {
    let text = "[[providers]]\nid = \"x\"\nadapter = \"codex\"\n\n[adapters.codex]\nbogus = 1\n";
    assert!(toml::from_str::<Config>(text).is_err());
}

/// ADR-0017 M1: `providers_include` が `providers.d/*.toml` をファイル名昇順で読み、
/// `[[providers]]` と合わせて重複 id を検出する。
#[test]
fn providers_include_merges_files_in_filename_order_and_still_rejects_duplicate_ids() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
            &path,
            "providers_include = \"providers.d/*.toml\"\n[[providers]]\nid = \"inline\"\nadapter = \"fake\"\n",
        )
        .unwrap();
    std::fs::create_dir_all(dir.path().join("providers.d")).unwrap();
    std::fs::write(
            dir.path().join("providers.d/b-acct.toml"),
            "id = \"b-acct\"\nadapter = \"claude-code\"\nconcurrency = 2\n[env]\nCLAUDE_CONFIG_DIR = \"/x/b\"\n",
        )
        .unwrap();
    std::fs::write(
        dir.path().join("providers.d/a-acct.toml"),
        "id = \"a-acct\"\nadapter = \"fake\"\n",
    )
    .unwrap();

    let cfg = Config::load(&path).unwrap();
    let ids: Vec<&str> = cfg.providers.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["inline", "a-acct", "b-acct"],
        "providers.d files load in filename order after inline ones"
    );
    let b = cfg.providers.iter().find(|p| p.id == "b-acct").unwrap();
    assert_eq!(b.concurrency, 2);
    assert_eq!(
        b.env.get("CLAUDE_CONFIG_DIR").map(String::as_str),
        Some("/x/b")
    );
    assert_eq!(
        cfg.providers_dir.as_deref(),
        Some(
            dir.path()
                .join("providers.d")
                .canonicalize()
                .unwrap()
                .as_path()
        )
    );

    // 重複 id（inline と providers.d の両方に "inline"）は既存の検証がそのまま拒否する。
    std::fs::write(
        dir.path().join("providers.d/dup.toml"),
        "id = \"inline\"\nadapter = \"fake\"\n",
    )
    .unwrap();
    let err = Config::load(&path).unwrap_err().to_string();
    assert!(err.contains("duplicate provider id"), "{err}");
}

/// `providers_include` は末尾が `*.toml` である glob だけを受け付ける。
#[test]
fn providers_include_rejects_patterns_not_ending_in_glob_toml() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
            &path,
            "providers_include = \"providers.d/*.yaml\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
        )
        .unwrap();
    let err = Config::load(&path).unwrap_err().to_string();
    assert!(err.contains("must end with"), "{err}");
}

/// `providers.d/` がまだ無い（1 つもアカウントを追加していない）ときは空のまま、inline だけで起動できる。
#[test]
fn providers_include_with_missing_directory_is_empty_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
            &path,
            "providers_include = \"providers.d/*.toml\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
        )
        .unwrap();
    let cfg = Config::load(&path).unwrap();
    assert_eq!(cfg.providers.len(), 1);
}

// ---- ADR-0024/0025: [accounts] / account_pool ----

/// `account_pool = true` は `adapter = "claude-code"` かつ `[accounts] claude_dir` を要求する（ADR-0024 D2）。
#[test]
fn account_pool_requires_claude_code_adapter_and_accounts_section() {
    // account_pool のプロバイダはあるが [accounts] が無い。
    let cfg: Config = toml::from_str(
        "[[providers]]\nid = \"pool\"\nadapter = \"claude-code\"\naccount_pool = true\n",
    )
    .unwrap();
    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("[accounts]"), "{err}");

    // [accounts] はあるが adapter が claude-code/codex でない。
    let cfg: Config = toml::from_str(
            "[accounts]\nclaude_dir = \"acct\"\n[[providers]]\nid = \"pool\"\nadapter = \"fake\"\naccount_pool = true\n",
        )
        .unwrap();
    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("claude-code"), "{err}");

    // 両方あれば通る。
    let cfg: Config = toml::from_str(
            "[accounts]\nclaude_dir = \"acct\"\n[[providers]]\nid = \"pool\"\nadapter = \"claude-code\"\naccount_pool = true\n",
        )
        .unwrap();
    assert!(cfg.validate().is_ok());
    assert_eq!(cfg.account_pool_providers(), ["pool".to_string()].into());
}

/// ADR-0025 D1: `account_pool = true` の codex プロバイダは `[accounts] codex_dir` を要求する
/// （`claude_dir` だけでは足りない）。
#[test]
fn account_pool_for_codex_requires_codex_dir_specifically() {
    let cfg: Config = toml::from_str(
            "[accounts]\nclaude_dir = \"acct\"\n[[providers]]\nid = \"pool\"\nadapter = \"codex\"\naccount_pool = true\n",
        )
        .unwrap();
    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("codex_dir"), "{err}");

    let cfg: Config = toml::from_str(
            "[accounts]\ncodex_dir = \"acct\"\n[[providers]]\nid = \"pool\"\nadapter = \"codex\"\naccount_pool = true\n",
        )
        .unwrap();
    assert!(cfg.validate().is_ok());
    assert_eq!(cfg.account_pool_providers(), ["pool".to_string()].into());
}

/// ADR 2026-10-06 D2 / D3: `adapter = "acp"` + `llm_source = "opencode_go"` + `account_pool = "opencode-go"` の行は
/// `[accounts] opencode_dir` があれば通り、無ければ設定エラー。
#[test]
fn opencode_go_pool_row_requires_opencode_dir() {
    let row = "[[providers]]\nid = \"go\"\nadapter = \"acp\"\nllm_source = \"opencode_go\"\nmodel = \"opencode-go/kimi-k3\"\naccount_pool = \"opencode-go\"\ntiers = [\"standard\"]\n";
    let cfg: Config =
        toml::from_str(&format!("[accounts]\nopencode_dir = \"acct\"\n{row}")).unwrap();
    cfg.validate().expect("opencode_dir set: valid");
    assert_eq!(cfg.account_pool_providers(), ["go".to_string()].into());
    assert_eq!(
        cfg.account_pool_adapters().get("go"),
        Some(&AccountAdapter::OpencodeGo)
    );
    assert_eq!(
        cfg.accounts.as_ref().unwrap().opencode_go_usage_url,
        "https://opencode.ai/zen/go/v1/usage"
    );
    assert_eq!(
        cfg.provider_llm_source("go").unwrap().source,
        task_core::LlmSourceRef::OpencodeGo
    );

    // opencode_dir が無い（claude_dir だけ）と拒否。
    let cfg: Config = toml::from_str(&format!("[accounts]\nclaude_dir = \"acct\"\n{row}")).unwrap();
    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("opencode_dir"), "{err}");

    // opencode_go は acp 以外の adapter では使えない。
    let cfg: Config = toml::from_str(
        "[accounts]\nopencode_dir = \"acct\"\n[[providers]]\nid = \"x\"\nadapter = \"codex\"\nllm_source = \"opencode_go\"\n",
    )
    .unwrap();
    assert!(cfg.validate().is_err());
}

/// ADR 2026-10-06 D3: 例の設定 `config/celeris.opencode-go.example.toml` が読めて、routing の source 名が
/// `opencode-go`（subscription）になる。
#[test]
fn opencode_go_example_config_loads_and_routes_as_opencode_go_subscription() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/celeris.opencode-go.example.toml");
    let cfg = Config::load(&path).expect("example loads");
    let catalog = cfg.routing_catalog().expect("catalog");
    let dep = catalog
        .deployments
        .iter()
        .find(|d| d.id == "provider:opencode-go")
        .expect("deployment");
    assert_eq!(dep.source_ref, "opencode-go");
    assert_eq!(
        dep.billing,
        task_core::model_router::profiles::Billing::Subscription
    );
}

/// `[accounts]` の既定値と、相対 `claude_dir`/`codex_dir` の解決（設定ファイル基準）。
#[test]
fn accounts_section_defaults_and_relative_dirs_are_resolved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
            &path,
            "[accounts]\nclaude_dir = \"claude-accounts\"\ncodex_dir = \"codex-accounts\"\n[[providers]]\nid = \"pool\"\nadapter = \"claude-code\"\naccount_pool = true\n",
        )
        .unwrap();
    let cfg = Config::load(&path).unwrap();
    let accounts = cfg.accounts.as_ref().unwrap();
    let claude_dir = accounts.claude_dir.clone().expect("claude_dir");
    let codex_dir = accounts.codex_dir.clone().expect("codex_dir");
    assert!(claude_dir.is_absolute());
    assert_eq!(
        claude_dir,
        dir.path().canonicalize().unwrap().join("claude-accounts")
    );
    assert!(codex_dir.is_absolute());
    assert_eq!(
        codex_dir,
        dir.path().canonicalize().unwrap().join("codex-accounts")
    );
    assert_eq!(accounts.max_runs_per_account, 2);
    assert_eq!(accounts.check_model, "haiku");

    let d = cfg.dispatch_config();
    let runtime = d.accounts.expect("dispatch_config carries [accounts]");
    assert_eq!(
        runtime.root_for(AccountAdapter::ClaudeCode),
        Some(&claude_dir)
    );
    assert_eq!(runtime.root_for(AccountAdapter::Codex), Some(&codex_dir));
    assert_eq!(runtime.max_runs_per_account, 2);
    assert_eq!(runtime.check_model, "haiku");
    assert_eq!(runtime.fallback_cooldown_secs, cfg.error_cooldown_secs);
}

/// `max_runs_per_account = 0` は設定エラー。未知キーも拒否。どちらの根ディレクトリも無ければ設定エラー。
#[test]
fn accounts_section_rejects_zero_max_runs_and_unknown_keys() {
    let cfg: Config = toml::from_str(
            "[accounts]\nclaude_dir = \"acct\"\nmax_runs_per_account = 0\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
        )
        .unwrap();
    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("max_runs_per_account"), "{err}");

    assert!(toml::from_str::<Config>("[accounts]\nbogus = 1\n").is_err());

    // Neither claude_dir nor codex_dir: deserializes fine (both optional) but validate() rejects it.
    let cfg: Config =
        toml::from_str("[accounts]\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n").unwrap();
    let err = cfg.validate().unwrap_err().to_string();
    assert!(
        err.contains("claude_dir") && err.contains("codex_dir"),
        "{err}"
    );
}

/// `ensure_accounts_dir` は設定された根ディレクトリ（claude_dir・codex_dir それぞれ）を 0700 で作る
/// （無ければ）。`[accounts]` が無ければ何もしない。
#[test]
fn ensure_accounts_dir_creates_the_directories_with_0700() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
            &path,
            "[accounts]\nclaude_dir = \"claude-accounts\"\ncodex_dir = \"codex-accounts\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
        )
        .unwrap();
    let cfg = Config::load(&path).unwrap();
    let claude_dir = cfg.accounts.as_ref().unwrap().claude_dir.clone().unwrap();
    let codex_dir = cfg.accounts.as_ref().unwrap().codex_dir.clone().unwrap();
    assert!(!claude_dir.exists());
    assert!(!codex_dir.exists());
    cfg.ensure_accounts_dir().unwrap();
    assert!(claude_dir.is_dir());
    assert!(codex_dir.is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for d in [&claude_dir, &codex_dir] {
            let mode = std::fs::metadata(d).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }
    }
    // 既にあれば触らない（既存の中身・権限を壊さない）。
    cfg.ensure_accounts_dir().unwrap();

    // [accounts] 無しは no-op。
    let no_accounts: Config =
        toml::from_str("[[providers]]\nid = \"x\"\nadapter = \"fake\"\n").unwrap();
    assert!(no_accounts.ensure_accounts_dir().is_ok());
}

// ---- ADR-0037: [notify] ----

/// `[notify]` は書かなくても既定値が入り、D6 の送り出し間隔を設定できる。
#[test]
fn notify_defaults_are_used_when_the_section_is_absent() {
    let cfg: Config = toml::from_str("[[providers]]\nid = \"x\"\nadapter = \"fake\"\n").unwrap();
    assert_eq!(cfg.notify, crate::notify::NotifyConfig::default());
    assert_eq!(cfg.notify.discord_webhook_secret, "discord-webhook");
    assert_eq!(cfg.notify.interval_secs, 30);
    assert_eq!(cfg.notify.base_url(), None);

    let cfg: Config = toml::from_str(
        "[notify]\ndiscord_webhook_secret = \"hook\"\ninterval_secs = 60\n\
             gui_base_url = \"http://192.168.1.103:7700/\"\n",
    )
    .unwrap();
    assert_eq!(cfg.notify.discord_webhook_secret, "hook");
    assert_eq!(cfg.notify.interval_secs, 60);
    assert_eq!(cfg.notify.base_url(), Some("http://192.168.1.103:7700"));

    let cfg: Config = toml::from_str(
        "[notify]\ninbox_batch_secs = 15\ninbox_reminder_secs = 7200\n\
         digest_interval_secs = 1800\ndigest_max_lines = 5\n",
    )
    .unwrap();
    assert_eq!(cfg.notify.inbox_batch_secs, 15);
    assert_eq!(cfg.notify.inbox_reminder_secs, 7200);
    assert_eq!(cfg.notify.digest_interval_secs, 1800);
    assert_eq!(cfg.notify.digest_max_lines, 5);

    // 未知キーは拒否。
    assert!(toml::from_str::<Config>("[notify]\nbogus = 1\n").is_err());
}

// ---- ADR-0030: [secrets] / env_from_secrets ----

/// `[secrets] dir` を読み、相対パスを設定ファイル基準で絶対化する。
#[test]
fn secrets_dir_is_parsed_and_resolved_relative_to_the_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[secrets]\ndir = \"secrets\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
    )
    .unwrap();
    let cfg = Config::load(&path).unwrap();
    let secrets = cfg.secrets.as_ref().unwrap();
    assert!(secrets.dir.is_absolute());
    assert_eq!(
        secrets.dir,
        dir.path().canonicalize().unwrap().join("secrets")
    );

    // 節を書かなければ `None`。
    let no_secrets: Config =
        toml::from_str("[[providers]]\nid = \"x\"\nadapter = \"fake\"\n").unwrap();
    assert!(no_secrets.secrets.is_none());
    // 未知キーは拒否。
    assert!(toml::from_str::<Config>("[secrets]\nbogus = 1\n").is_err());
}

/// ADR-0047 D1 / D2（Phase 61）: `[knowledge]` は既定でも値を持ち（`~/.local/share/celeris/knowledge`）、
/// 相対パスは設定ファイル基準で絶対化され、`default_mounts` の綴り間違いは `validate()` が弾く。
/// **ディレクトリは作らない**（用意するのは `celerisctl knowledge init` だけ）。
#[test]
fn the_knowledge_section_resolves_its_root_and_checks_the_default_mounts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
            &path,
            "db = \"t.sqlite3\"\n[knowledge]\nroot = \"kb\"\ndefault_mounts = [\"kb:user\", \"memory\"]\n\
             [[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
        )
        .unwrap();
    let cfg = Config::load(&path).unwrap();
    assert!(cfg.knowledge.root.is_absolute());
    assert_eq!(
        cfg.knowledge.root,
        dir.path().canonicalize().unwrap().join("kb")
    );
    // 読んだだけでは作らない。
    assert!(!cfg.knowledge.root.exists());
    assert_eq!(
        cfg.knowledge.mounts().expect("mounts"),
        vec![
            task_core::KnowledgeMount::kb("user"),
            task_core::KnowledgeMount::memory(None)
        ]
    );
    assert_eq!(cfg.dispatch_config().knowledge.root, cfg.knowledge.root);
    assert_eq!(cfg.dispatch_config().knowledge.default_mounts.len(), 2);

    // 節を書かなければ既定（`~/.local/share/celeris/knowledge` と `kb:user` / `kb:environment`）。
    let default: Config =
        toml::from_str("[[providers]]\nid = \"x\"\nadapter = \"fake\"\n").unwrap();
    assert_eq!(default.knowledge.root, task_core::knowledge::default_root());
    assert_eq!(
        default.knowledge.default_mounts,
        vec!["kb:user", "kb:environment"]
    );
    // 未知キーは拒否。
    assert!(toml::from_str::<Config>("[knowledge]\nbogus = 1\n").is_err());
    // 綴り間違いは `validate()` で落ちる（黙って無視しない）。
    let bad: Config =
            toml::from_str("[knowledge]\ndefault_mounts = [\"nope:x\"]\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n")
                .unwrap();
    let why = bad.validate().expect_err("bad mount").to_string();
    assert!(why.contains("[knowledge] default_mounts"), "{why}");
    // KB の外を指す scope も落ちる。
    let escape: Config =
            toml::from_str("[knowledge]\ndefault_mounts = [\"kb:../etc\"]\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n")
                .unwrap();
    assert!(escape.validate().is_err());
}

/// Phase 65b: `dispatch_config().knowledge.langmem_api_key` が `[knowledge.langmem].api_key_secret`
/// を `[secrets] dir` から解決した平文の値になること（`build_adapters` の langmem アダプタと同じ
/// 解決）。到達性 probe（`task_dispatch::Dispatcher::knowledge_reachability`）がこれを
/// `Authorization: Bearer` に使う（`llm-proxy` のような認証必須の上流のため）。
#[test]
fn dispatch_config_resolves_the_langmem_api_key_from_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let secrets_dir = dir.path().join("secrets");
    std::fs::create_dir_all(&secrets_dir).unwrap();
    std::fs::write(secrets_dir.join("langmem-key"), "sk-test-value\n").unwrap();
    let text = format!(
        r#"
[secrets]
dir = "{secrets}"

[knowledge.langmem]
enabled = true
provider = "openai-compatible"
base_url = "http://127.0.0.1:18100/v1"
model = "celeris/cheap"
api_key_secret = "langmem-key"

[[providers]]
id = "x"
adapter = "fake"
"#,
        secrets = secrets_dir.display()
    );
    let cfg: Config = toml::from_str(&text).unwrap();
    assert_eq!(
        cfg.dispatch_config().knowledge.langmem_api_key.as_deref(),
        Some("sk-test-value")
    );

    // `api_key_secret` が無ければ `None`（従来どおり、probe はトークン無しで検査する）。
    let without: Config = toml::from_str(
            "[knowledge.langmem]\nenabled = true\nbase_url = \"http://127.0.0.1:18100/v1\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
        )
        .unwrap();
    assert!(
        without
            .dispatch_config()
            .knowledge
            .langmem_api_key
            .is_none()
    );
}

/// ADR-0033 D6（Phase 24）: `[memory] dir` は設定ファイル基準で絶対化され、0700 で作られ、
/// `dispatch_config()` に渡る。`[memory]` が無ければ記憶は無効（`memory_dir = None`）。
#[test]
fn memory_dir_is_resolved_created_with_0700_and_passed_to_the_dispatcher() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
            &path,
            "db = \"t.sqlite3\"\n[memory]\ndir = \"memory\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
        )
        .unwrap();
    let cfg = Config::load(&path).unwrap();
    let memory = cfg.memory.as_ref().expect("[memory]");
    assert!(memory.dir.is_absolute());
    assert_eq!(memory.dir, dir.path().join("memory"));
    assert_eq!(cfg.dispatch_config().memory_dir.as_ref(), Some(&memory.dir));

    cfg.ensure_memory_dir().unwrap();
    assert!(memory.dir.is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&memory.dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
    // 2 回目は何もしない（既にある）。
    cfg.ensure_memory_dir().unwrap();

    let without: Config =
        toml::from_str("[[providers]]\nid = \"x\"\nadapter = \"fake\"\n").unwrap();
    assert!(without.memory.is_none());
    assert!(without.dispatch_config().memory_dir.is_none());
    assert!(without.ensure_memory_dir().is_ok());
    // 知らないキーは拒否する（他の節と同じ）。
    assert!(toml::from_str::<Config>("[memory]\ndir = \"m\"\nbogus = 1\n").is_err());
}

/// ADR-0040 D6（Phase 48）/ ADR-0045 D2: `[selfdeploy] releases_dir` の既定は
/// `~/.local/celeris/releases`。明示した相対パスは従来どおり設定ファイルのディレクトリ基準。
#[test]
fn selfdeploy_releases_dir_defaults_to_the_state_dir_and_resolves_relative_paths() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    // 節を書かない構成では新しい既定が効く。
    std::fs::write(
        &path,
        "db = \"t.sqlite3\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
    )
    .unwrap();
    let cfg = Config::load(&path).unwrap();
    assert!(cfg.selfdeploy.releases_dir.is_absolute());
    assert!(
        cfg.selfdeploy
            .releases_dir
            .ends_with(".local/celeris/releases"),
        "{:?}",
        cfg.selfdeploy.releases_dir
    );

    // 明示した相対パスも設定ファイル基準。
    std::fs::write(
            &path,
            "db = \"t.sqlite3\"\n[selfdeploy]\nreleases_dir = \"rel\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
        )
        .unwrap();
    let cfg = Config::load(&path).unwrap();
    assert_eq!(cfg.selfdeploy.releases_dir, dir.path().join("rel"));

    // 絶対パスはそのまま。
    std::fs::write(
            &path,
            "db = \"t.sqlite3\"\n[selfdeploy]\nreleases_dir = \"/srv/releases\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
        )
        .unwrap();
    let cfg = Config::load(&path).unwrap();
    assert_eq!(cfg.selfdeploy.releases_dir, PathBuf::from("/srv/releases"));

    // 知らないキーは拒否する（他の節と同じ）。
    assert!(toml::from_str::<Config>("[selfdeploy]\nbogus = 1\n").is_err());

    // ADR-0041 D3: `repo` は既定 `~/workspace/agent-platform` で、`~` は celeris の $HOME で展開する。
    std::fs::write(
        &path,
        "db = \"t.sqlite3\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
    )
    .unwrap();
    let cfg = Config::load(&path).unwrap();
    match task_core::home_dir() {
        Some(home) => assert_eq!(cfg.selfdeploy.repo, home.join("workspace/agent-platform")),
        // $HOME が無い環境では展開できないので、設定ファイル基準の相対として残る。
        None => assert_eq!(
            cfg.selfdeploy.repo,
            dir.path().join("~/workspace/agent-platform")
        ),
    }
    // 明示した絶対パスはそのまま（存在しなくてよい。`on_main` が `null` になるだけ）。
    std::fs::write(
            &path,
            "db = \"t.sqlite3\"\n[selfdeploy]\nrepo = \"/srv/agent-platform\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
        )
        .unwrap();
    let cfg = Config::load(&path).unwrap();
    assert_eq!(cfg.selfdeploy.repo, PathBuf::from("/srv/agent-platform"));
}

/// ADR-0043 D5（Phase 54）: `[github]` は書かなくてよく（既定は `gh` / `merge`）、
/// 知らない `merge_method` と空の `gh` は設定エラー。
#[test]
fn github_defaults_to_gh_and_merge_and_rejects_other_merge_methods() {
    let base = "[[providers]]\nid = \"x\"\nadapter = \"fake\"\n".to_string();
    let cfg: Config = toml::from_str(&base).expect("defaults");
    assert_eq!(cfg.github.gh, "gh");
    assert_eq!(cfg.github.merge_method, "merge");
    assert!(cfg.validate().is_ok());

    let cfg: Config = toml::from_str(&format!(
        "{base}\n[github]\ngh = \"/opt/gh\"\nmerge_method = \"squash\"\n"
    ))
    .expect("explicit");
    assert_eq!(cfg.github.gh, "/opt/gh");
    assert_eq!(cfg.github.merge_method, "squash");
    assert!(cfg.validate().is_ok());

    let bad: Config = toml::from_str(&format!(
        "{base}\n[github]\nmerge_method = \"rebase-merge\"\n"
    ))
    .expect("parse");
    assert!(matches!(bad.validate(), Err(ConfigError::Invalid(m)) if m.contains("merge_method")));
    let blank: Config = toml::from_str(&format!("{base}\n[github]\ngh = \"  \"\n")).expect("parse");
    assert!(matches!(blank.validate(), Err(ConfigError::Invalid(m)) if m.contains("gh")));
    // 未知のキーは弾く（他の節と同じ流儀）。
    assert!(toml::from_str::<Config>(&format!("{base}\n[github]\nbogus = 1\n")).is_err());
}

/// ADR-0043 D3（Phase 56）: `[containers]` の既定（`auto` / `celeris-worker:latest` /
/// `~/.local/celeris/containers` / 1800 秒）と `DispatchConfig` への写り、綴り間違いの拒否。
#[test]
fn containers_defaults_reach_the_dispatcher_and_bad_values_are_rejected() {
    let base = "[[providers]]\nid = \"x\"\nadapter = \"fake\"\n".to_string();
    let cfg: Config = toml::from_str(&base).expect("defaults");
    assert_eq!(cfg.containers.runtime, "auto");
    assert_eq!(cfg.containers.image_default, "celeris-worker:latest");
    assert_eq!(cfg.containers.build_timeout_secs, 1800);
    assert!(cfg.validate().is_ok());
    let dispatch = cfg.dispatch_config();
    assert_eq!(
        dispatch.containers.preference,
        task_worker::RuntimePreference::Auto
    );
    assert_eq!(dispatch.containers.image_default, "celeris-worker:latest");
    assert_eq!(dispatch.containers.build_timeout, Duration::from_secs(1800));

    let cfg: Config = toml::from_str(&format!(
            "{base}\n[containers]\nruntime = \"podman\"\nimage_default = \"x:1\"\nbuild_timeout_secs = 60\n"
        ))
        .expect("explicit");
    assert!(cfg.validate().is_ok());
    assert_eq!(
        cfg.dispatch_config().containers.preference,
        task_worker::RuntimePreference::Podman
    );
    assert_eq!(
        cfg.dispatch_config().containers.build_timeout,
        Duration::from_secs(60)
    );

    // 知らない runtime・空のイメージ・0 秒は設定エラー（黙ってホスト実行に倒れない）。
    let bad: Config =
        toml::from_str(&format!("{base}\n[containers]\nruntime = \"lxc\"\n")).expect("parse");
    assert!(matches!(bad.validate(), Err(ConfigError::Invalid(m)) if m.contains("runtime")));
    let bad: Config =
        toml::from_str(&format!("{base}\n[containers]\nimage_default = \"  \"\n")).expect("parse");
    assert!(matches!(bad.validate(), Err(ConfigError::Invalid(m)) if m.contains("image_default")));
    let bad: Config =
        toml::from_str(&format!("{base}\n[containers]\nbuild_timeout_secs = 0\n")).expect("parse");
    assert!(
        matches!(bad.validate(), Err(ConfigError::Invalid(m)) if m.contains("build_timeout_secs"))
    );
    // 未知のキーは弾く。
    assert!(toml::from_str::<Config>(&format!("{base}\n[containers]\nbogus = 1\n")).is_err());
}

/// ADR-0045 D2: 省略したときの既定の置き場（`db` / `workspace_root` / `[selfdeploy] releases_dir` /
/// `[memory] dir` / `[secrets] dir` / `[accounts]` の 2 つ / `[containers] build_dir`）が
/// `~/.local/celeris` と `~/.config/celeris` の下になる。**書いてあれば従来どおり**
/// （相対は設定ファイルのディレクトリ基準）。
#[test]
fn omitted_paths_default_to_the_celeris_xdg_layout() {
    // 生の（`Config::load` を通す前の）既定値。`$HOME` に依らない。
    let raw: Config = toml::from_str(
        "[[providers]]\nid = \"x\"\nadapter = \"fake\"\n[memory]\n[secrets]\n[accounts]\n",
    )
    .unwrap();
    assert_eq!(
        raw.db.path,
        PathBuf::from("~/.local/celeris/celeris.sqlite3")
    );
    assert_eq!(
        raw.workspace_root,
        PathBuf::from("~/.local/celeris/workspaces")
    );
    assert_eq!(
        raw.selfdeploy.releases_dir,
        PathBuf::from("~/.local/celeris/releases")
    );
    assert_eq!(
        raw.containers.build_dir,
        PathBuf::from("~/.local/celeris/containers")
    );
    assert_eq!(
        raw.memory.as_ref().expect("[memory]").dir,
        PathBuf::from("~/.local/celeris/memory")
    );
    assert_eq!(
        raw.secrets.as_ref().expect("[secrets]").dir,
        PathBuf::from("~/.config/celeris/secrets")
    );
    // `[accounts]` の 2 つは Option のまま（`None` = 設定していない。ADR-0024 D2 / ADR-0025 D1）。
    let accounts = raw.accounts.as_ref().expect("[accounts]");
    assert!(accounts.claude_dir.is_none() && accounts.codex_dir.is_none());

    // `Config::load` は `~` を `$HOME` で展開し、絶対パスにする。
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[[providers]]\nid = \"x\"\nadapter = \"fake\"\n[memory]\n[secrets]\n",
    )
    .unwrap();
    let cfg = Config::load(&path).unwrap();
    for p in [
        &cfg.db.path,
        &cfg.workspace_root,
        &cfg.selfdeploy.releases_dir,
        &cfg.containers.build_dir,
        &cfg.memory.as_ref().expect("[memory]").dir,
        &cfg.secrets.as_ref().expect("[secrets]").dir,
    ] {
        assert!(p.is_absolute(), "{p:?}");
    }
    assert!(
        cfg.db.path.ends_with(".local/celeris/celeris.sqlite3"),
        "{:?}",
        cfg.db.path
    );
    assert!(
        cfg.selfdeploy
            .releases_dir
            .ends_with(".local/celeris/releases"),
        "{:?}",
        cfg.selfdeploy.releases_dir
    );
    assert!(
        cfg.memory
            .as_ref()
            .expect("[memory]")
            .dir
            .ends_with(".local/celeris/memory")
    );
    assert!(
        cfg.secrets
            .as_ref()
            .expect("[secrets]")
            .dir
            .ends_with(".config/celeris/secrets")
    );

    // 書いてあれば従来どおり（相対は設定ファイルのディレクトリ基準）。
    std::fs::write(
            &path,
            "db = \"d.sqlite3\"\nworkspace_root = \"ws\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n\
             [memory]\ndir = \"mem\"\n[secrets]\ndir = \"sec\"\n[accounts]\nclaude_dir = \"ca\"\ncodex_dir = \"co\"\n\
             [selfdeploy]\nreleases_dir = \"rel\"\n",
        )
        .unwrap();
    let cfg = Config::load(&path).unwrap();
    let base = dir.path().canonicalize().unwrap();
    assert_eq!(cfg.db.path, base.join("d.sqlite3"));
    assert_eq!(cfg.workspace_root, base.join("ws"));
    assert_eq!(cfg.selfdeploy.releases_dir, base.join("rel"));
    assert_eq!(cfg.memory.as_ref().expect("[memory]").dir, base.join("mem"));
    assert_eq!(
        cfg.secrets.as_ref().expect("[secrets]").dir,
        base.join("sec")
    );
    let accounts = cfg.accounts.as_ref().expect("[accounts]");
    assert_eq!(
        accounts.claude_dir.as_deref(),
        Some(base.join("ca").as_path())
    );
    assert_eq!(
        accounts.codex_dir.as_deref(),
        Some(base.join("co").as_path())
    );
}

/// ADR-0043 D3 / ADR-0042 D3: `[containers] build_dir` の既定は `~/.local/celeris/containers`
/// （`~` を展開し、相対なら設定ファイル基準）。
#[test]
fn containers_build_dir_expands_home_and_resolves_relative_paths() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "db = \"t.sqlite3\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
    )
    .unwrap();
    let cfg = Config::load(&path).unwrap();
    assert!(
        cfg.containers.build_dir.is_absolute(),
        "{:?}",
        cfg.containers.build_dir
    );
    assert!(
        cfg.containers
            .build_dir
            .ends_with(".local/celeris/containers"),
        "{:?}",
        cfg.containers.build_dir
    );

    std::fs::write(
            &path,
            "db = \"t.sqlite3\"\n[containers]\nbuild_dir = \"images\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
        )
        .unwrap();
    let cfg = Config::load(&path).unwrap();
    assert_eq!(
        cfg.containers.build_dir,
        dir.path().canonicalize().unwrap().join("images")
    );
}

/// ADR-0041 D1（Phase 49）/ ADR-0042 D3（Phase 52）: `[workspace] worktree_branch_prefix` は
/// 既定 **`celeris/`** で、`DispatchConfig` に写る。空文字列は設定エラー。
#[test]
fn workspace_worktree_branch_prefix_defaults_to_celeris_slash_and_reaches_the_dispatcher() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    // 節を書かなくても既定が効く。
    std::fs::write(
        &path,
        "db = \"t.sqlite3\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
    )
    .unwrap();
    let cfg = Config::load(&path).unwrap();
    assert_eq!(cfg.workspace.worktree_branch_prefix, "celeris/");
    let dispatch = cfg.dispatch_config();
    assert_eq!(dispatch.worktree_branch_prefix, "celeris/");
    // ADR-0045 D2: `releases_dir` の既定は `~/.local/celeris/releases`。
    let releases = dispatch.releases_dir.as_deref().expect("releases_dir");
    assert!(
        releases.ends_with(".local/celeris/releases"),
        "{releases:?}"
    );

    std::fs::write(
            &path,
            "db = \"t.sqlite3\"\n[workspace]\nworktree_branch_prefix = \"bot/\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
        )
        .unwrap();
    assert_eq!(
        Config::load(&path)
            .unwrap()
            .workspace
            .worktree_branch_prefix,
        "bot/"
    );

    // 空は拒否する（ブランチ名がタスク id そのものになってしまう）。
    std::fs::write(
            &path,
            "db = \"t.sqlite3\"\n[workspace]\nworktree_branch_prefix = \"\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
        )
        .unwrap();
    assert!(Config::load(&path).is_err());
    // 知らないキーは拒否する（他の節と同じ）。
    assert!(toml::from_str::<Config>("[workspace]\nbogus = 1\n").is_err());
}

/// `ensure_secrets_dir` は `[secrets] dir` を 0700 で作る（無ければ）。`[secrets]` が無ければ何もしない。
#[test]
fn ensure_secrets_dir_creates_the_directory_with_0700() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[secrets]\ndir = \"secrets\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
    )
    .unwrap();
    let cfg = Config::load(&path).unwrap();
    let secrets_dir = cfg.secrets.as_ref().unwrap().dir.clone();
    assert!(!secrets_dir.exists());
    cfg.ensure_secrets_dir().unwrap();
    assert!(secrets_dir.is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&secrets_dir)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
    }
    // 既にあれば触らない。
    cfg.ensure_secrets_dir().unwrap();

    // [secrets] 無しは no-op。
    let no_secrets: Config =
        toml::from_str("[[providers]]\nid = \"x\"\nadapter = \"fake\"\n").unwrap();
    assert!(no_secrets.ensure_secrets_dir().is_ok());
}

/// `env_from_secrets` は `[adapters.*]` と行の両方で読める（未知キーは拒否）。
#[test]
fn env_from_secrets_is_parsed_on_adapters_and_providers() {
    let text = r#"[adapters.local_deep_research]
env_from_secrets = { LDR_SEARCH_ENGINE_WEB_TAVILY_API_KEY = "tavily", LDR_SEARCH_ENGINE_WEB_EXA_API_KEY = "exa" }

[[providers]]
id = "ldr"
adapter = "local-deep-research"
env_from_secrets = { LDR_SEARCH_ENGINE_WEB_TAVILY_API_KEY = "tavily-row" }
"#;
    let cfg: Config = toml::from_str(text).unwrap();
    assert!(cfg.validate().is_ok());
    assert_eq!(
        cfg.adapters
            .local_deep_research
            .env_from_secrets
            .get("LDR_SEARCH_ENGINE_WEB_TAVILY_API_KEY")
            .map(String::as_str),
        Some("tavily")
    );
    assert_eq!(
        cfg.providers[0]
            .env_from_secrets
            .get("LDR_SEARCH_ENGINE_WEB_TAVILY_API_KEY")
            .map(String::as_str),
        Some("tavily-row")
    );

    for (section, extra) in [
        ("claude_code", ""),
        ("codex", ""),
        ("fake", ""),
        ("acp", ""),
        ("paperqa", ""),
    ] {
        let _ = extra;
        let text = format!(
            "[adapters.{section}]\nenv_from_secrets = {{ FOO = \"bar\" }}\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n"
        );
        let cfg: Config = toml::from_str(&text).unwrap_or_else(|e| panic!("{section}: {e}"));
        let _ = cfg;
    }
}

/// ADR-0089（Phase R6-5）: `[execution] max_cos_runs` の既定（2）・写し・範囲（0..=8）。
#[test]
fn execution_max_cos_runs_default_and_validation() {
    let base = "[[providers]]\nid = \"x\"\nadapter = \"fake\"\n";
    let cfg: Config = toml::from_str(base).unwrap();
    cfg.validate().unwrap();
    assert_eq!(cfg.execution.max_cos_runs, 2);
    assert_eq!(cfg.dispatch_config().execution.max_cos_runs, 2);
    for ok in [0, 1, 8] {
        let cfg: Config =
            toml::from_str(&format!("{base}[execution]\nmax_cos_runs = {ok}\n")).unwrap();
        cfg.validate().unwrap();
        assert_eq!(cfg.dispatch_config().execution.max_cos_runs, ok);
    }
    let cfg: Config = toml::from_str(&format!("{base}[execution]\nmax_cos_runs = 9\n")).unwrap();
    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("max_cos_runs"), "{err}");
}

/// ADR-0079 D3（Phase R1a）: `[execution.tree]` の既定（`enabled = false`・`max_depth = 3` 層・D3 / U-R4 の
/// 上限）と範囲の検査（`max_depth` は task の層数で 1..=3）。値は `ExecutionLimits.tree` に写る。
#[test]
fn execution_tree_defaults_and_validation() {
    let base = "[[providers]]\nid = \"x\"\nadapter = \"fake\"\n";
    let cfg: Config = toml::from_str(base).unwrap();
    cfg.validate().unwrap();
    assert!(!cfg.execution.tree.enabled);
    let limits = cfg.dispatch_config().execution.limits;
    assert_eq!(limits.tree, task_core::TreeLimits::default());
    assert!(!limits.tree.enabled);
    assert_eq!(limits.tree.max_depth, 3);
    assert!(limits.tree.auto_leaf);
    assert_eq!(limits.tree.auto_leaf_max_compactions, 2);
    assert_eq!(limits.tree.auto_leaf_max_continuations, 2);
    let custom: Config = toml::from_str(&format!("{base}[execution.tree]\nauto_leaf = false\nauto_leaf_max_compactions = 0\nauto_leaf_max_continuations = 4\n")).unwrap();
    custom.validate().unwrap();
    let custom = custom.dispatch_config().execution.limits.tree;
    assert!(!custom.auto_leaf);
    assert_eq!(custom.auto_leaf_max_compactions, 0);
    assert_eq!(custom.auto_leaf_max_continuations, 4);

    // /1・/2 の上限は変わらない。
    assert_eq!(limits, task_core::ExecutionLimits::default());

    let cfg: Config = toml::from_str(&format!(
        "{base}[execution.tree]\nenabled = true\nmax_depth = 2\nmax_units_per_stage = 4\n\
             max_tree_tokens = 5000000\napproval_near_limit_ratio = 0.75\n"
    ))
    .unwrap();
    cfg.validate().unwrap();
    let limits = cfg.dispatch_config().execution.limits;
    let tree = limits.tree;
    assert!(tree.enabled);
    assert_eq!(tree.max_depth, 2);
    assert_eq!(tree.max_units_per_stage, 4);
    assert_eq!(tree.max_tree_tokens, Some(5_000_000));
    assert_eq!(tree.approval_near_limit_permille, 750);
    assert_eq!(
        (
            tree.max_tree_leaves,
            tree.max_tree_runs,
            tree.max_tree_replans
        ),
        (120, 400, 30)
    );
    assert_eq!(cfg.execution.max_replans, 5);
    assert_eq!(tree.gate_depth_step, 2);
    // Phase R2a: 深さの閾値の刻みと木の run・replan・leaf の上限も設定から写る。
    let cfg: Config = toml::from_str(&format!(
        "{base}[execution.tree]\nenabled = true\ngate_depth_step = 3\nmax_tree_runs = 7\n\
             max_tree_replans = 1\nmax_tree_leaves = 9\nmax_parallel_child_tasks = 1\n"
    ))
    .unwrap();
    cfg.validate().unwrap();
    let t = cfg.dispatch_config().execution.limits.tree;
    assert_eq!(
        (t.gate_depth_step, t.max_tree_runs, t.max_tree_replans),
        (3, 7, 1)
    );
    assert_eq!((t.max_tree_leaves, t.max_parallel_child_tasks), (9, 1));
    assert_eq!(task_core::tree::gate_threshold(2, t.gate_depth_step), 8);
    assert_eq!(
        task_core::ExecutionLimits {
            tree: task_core::TreeLimits::default(),
            ..limits
        },
        task_core::ExecutionLimits::default(),
        "[execution.tree] must not change the /1・/2 limits"
    );

    for bad in [
        "max_depth = 0",
        "max_depth = 4",
        "max_units_per_stage = 0",
        "max_tree_runs = 0",
        "max_tree_tokens = 0",
        "gate_depth_step = 11",
        "max_tree_leaves = 0",
        "max_tree_replans = 0",
        "max_child_tasks_per_plan = 0",
        "max_parallel_child_tasks = 0",
        "max_stages = 0",
        "approval_near_limit_ratio = 0.0",
        "approval_near_limit_ratio = 1.5",
        "max_open_decisions = 4\nmax_open_decisions_per_plan = 8",
    ] {
        let cfg: Config = toml::from_str(&format!("{base}[execution.tree]\n{bad}\n")).unwrap();
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("[execution.tree]"), "{bad}: {err}");
    }
    // 綴り間違いは設定エラー（`deny_unknown_fields`）。
    assert!(toml::from_str::<Config>(&format!("{base}[execution.tree]\nmax_detph = 3\n")).is_err());
}

/// 例の設定ファイルにコメントアウトされた `[accounts]` / `account_pool` の節も構文として妥当なことを確認する
/// （読み込み自体は動かないが `toml` として壊れていないことは grep で確認できる）。
#[test]
fn multi_account_example_mentions_account_pool_commented_out() {
    let path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config/celeris.multi-account.example.toml"
    ));
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.contains("# [accounts]"));
    assert!(text.contains("# account_pool = true"));
    // 既存の受け入れ条件（Config::load が通る）はコメントアウトされているので変わらない。
    assert!(Config::load(path).is_ok());
}

#[test]
fn delivery_auto_resolve_defaults_follow_the_parallel_integration_adr() {
    let cfg: Config = toml::from_str("").unwrap();
    let auto = &cfg.selfdeploy.delivery.auto_resolve;
    assert!(auto.enabled);
    assert_eq!(auto.max_attempts, 3);
    assert_eq!(
        auto.generated.globs,
        vec![
            "docs/protocol/*.schema.json".to_string(),
            "docs/api/v1/*.schema.json".to_string()
        ]
    );
    let cmd = auto.generated.command().unwrap();
    assert_eq!(&cmd[..4], ["env", "UPDATE_SCHEMA=1", "cargo", "test"]);
    for krate in ["task-core", "task-worker", "task-api"] {
        assert!(cmd.iter().any(|a| a == krate), "{krate}");
    }
    cfg.selfdeploy.delivery.validate().unwrap();
}

#[test]
fn delivery_auto_resolve_overrides_and_validation() {
    let cfg = Config::parse_with_delivery(
        "[delivery.auto_resolve]\nenabled = false\nmax_attempts = 5\n[delivery.auto_resolve.generated]\ncmd = []\n",
    )
    .unwrap();
    let auto = &cfg.selfdeploy.delivery.auto_resolve;
    assert!(!auto.enabled);
    assert_eq!(auto.max_attempts, 5);
    assert_eq!(auto.generated.command(), None);
    cfg.selfdeploy.delivery.validate().unwrap();

    let cfg = Config::parse_with_delivery(
        "[delivery.auto_resolve.generated]\ncmd = [\"sh\", \"regen.sh\"]\nglobs = [\"docs/api/v1/*.schema.json\", \"docs/protocol/*.schema.json\"]\n",
    )
    .unwrap();
    assert_eq!(
        cfg.selfdeploy.delivery.auto_resolve.generated.command(),
        Some(vec!["sh".to_string(), "regen.sh".to_string()])
    );
    cfg.selfdeploy.delivery.validate().unwrap();

    for bad in [
        "[delivery.auto_resolve]\nmax_attempts = 0\n",
        "[delivery.auto_resolve.generated]\nglobs = [\"docs/**/*.json\"]\n",
    ] {
        let cfg = Config::parse_with_delivery(bad).unwrap();
        assert!(cfg.selfdeploy.delivery.validate().is_err(), "{bad}");
    }
    assert!(Config::parse_with_delivery("[delivery.auto_resolve]\nunknown = 1\n").is_err());
}

/// ADR-0136: hot な path は既存の key で `/local` 側に変えられ、書かなければ従来の home の既定のまま。
/// `[storage]` を書かなければ mount 検査はしない。
#[test]
fn hot_paths_follow_the_adr_0136_keys_and_default_to_the_previous_locations() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[[providers]]\nid = \"x\"\nadapter = \"fake\"\n").unwrap();
    let cfg = Config::load(&path).unwrap();
    let home = task_core::home_dir().expect("HOME in tests");
    let state = home.join(".local/celeris");
    let defaults: Vec<(&str, PathBuf)> = vec![
        ("[db].path", state.clone()),
        ("workspace_root", state.join("workspaces")),
        ("[workspace].build_cache_dir", state.join("build-cache")),
        ("[scratch].dir", state.join("scratch")),
        ("[containers].build_dir", state.join("containers")),
        ("[selfdeploy].releases_dir", state.join("releases")),
    ];
    assert_eq!(cfg.hot_paths(), defaults);
    assert_eq!(cfg.db.path, state.join("celeris.sqlite3"));
    assert_eq!(cfg.storage, StorageConfig::default());
    assert!(cfg.check_hot_mount(None).is_ok(), "no [storage]: no check");

    std::fs::write(
        &path,
        r#"
workspace_root = "/local/celeris/data/workspaces"
[db]
path = "/local/celeris/data/db/celeris.sqlite3"
backup_dir = "/local/celeris/state/backups"
[workspace]
build_cache_dir = "/local/celeris/data/build-cache"
[scratch]
dir = "/local/celeris/data/scratch"
[containers]
build_dir = "/local/celeris/data/containers"
[memory]
dir = "/local/celeris/data/memory"
[selfdeploy]
releases_dir = "/local/celeris/state/releases"
[storage]
hot_mount = "/local"
[[providers]]
id = "x"
adapter = "fake"
"#,
    )
    .unwrap();
    let cfg = Config::load(&path).unwrap();
    let local = PathBuf::from("/local/celeris");
    assert_eq!(
        cfg.hot_paths(),
        vec![
            ("[db].path", local.join("data/db")),
            ("workspace_root", local.join("data/workspaces")),
            (
                "[workspace].build_cache_dir",
                local.join("data/build-cache")
            ),
            ("[scratch].dir", local.join("data/scratch")),
            ("[containers].build_dir", local.join("data/containers")),
            ("[selfdeploy].releases_dir", local.join("state/releases")),
            ("[memory].dir", local.join("data/memory")),
            ("[db].backup_dir", local.join("state/backups")),
        ]
    );
    assert_eq!(cfg.storage.hot_mount.as_deref(), Some(Path::new("/local")));
    assert_eq!(
        cfg.dispatch_config().memory_dir.as_deref(),
        Some(local.join("data/memory").as_path())
    );

    // 相対 path・`/` は拒否、知らない key も拒否。
    assert!(
        toml::from_str::<Config>("[storage]\nhot_mount = \"local\"\n")
            .unwrap()
            .validate()
            .is_err()
    );
    assert!(
        toml::from_str::<Config>("[storage]\nhot_mount = \"/\"\n")
            .unwrap()
            .validate()
            .is_err()
    );
    assert!(toml::from_str::<Config>("[storage]\nbogus = 1\n").is_err());
}

/// ADR-0136: `[storage] hot_mount` が mount point そのものでなければ（rootfs 上の同名 dir・mountinfo が
/// 読めない）理由を出して止め、dir は作らない。注入した mountinfo と tempdir で決定的に確かめる。
#[test]
fn hot_mount_check_refuses_a_plain_directory_on_the_root_filesystem() {
    let tmp = tempfile::tempdir().unwrap();
    let mount = tmp.path().join("local");
    let workspaces = mount.join("celeris/data/workspaces");
    let mut cfg: Config = toml::from_str("").unwrap();
    cfg.storage.hot_mount = Some(mount.clone());
    cfg.workspace_root = workspaces.clone();
    cfg.memory = Some(MemoryConfig {
        dir: mount.join("celeris/data/memory"),
    });
    cfg.storage.validate().unwrap();

    let root_only = "25 1 0:24 / / rw,relatime shared:1 - ext4 /dev/mapper/root rw\n";
    let err = cfg
        .check_hot_mount(Some(root_only))
        .unwrap_err()
        .to_string();
    assert!(err.contains("not a mount point"), "{err}");
    assert!(
        err.contains(&format!("workspace_root = {}", workspaces.display())),
        "{err}"
    );
    assert!(err.contains("[memory].dir"), "{err}");
    // 検査は何も作らない（rootfs に同名 dir を作らない）。
    assert!(!mount.exists());

    // 子の mount（`<mount>/sub`）や前方一致する別名（`<mount>2`）では足りない。
    let near = format!(
        "{root_only}30 25 0:60 / {m}/sub rw - btrfs /dev/sdb rw\n31 25 0:61 / {m}2 rw - btrfs /dev/sdc rw\n",
        m = mount.display()
    );
    assert!(cfg.check_hot_mount(Some(&near)).is_err());

    let err = cfg.check_hot_mount(None).unwrap_err().to_string();
    assert!(err.contains("cannot read /proc/self/mountinfo"), "{err}");

    let mounted = format!(
        "{root_only}40 25 0:70 / {} rw,relatime shared:9 - btrfs /dev/mapper/pve-local rw,compress=zstd:1\n",
        mount.display()
    );
    assert!(cfg.check_hot_mount(Some(&mounted)).is_ok());
    assert!(!mount.exists());

    // mountinfo の 8 進 escape（空白 = `\040`）を戻して比べる。
    assert!(is_mount_point(
        "50 25 0:80 / /mnt/hot\\040data rw - btrfs /dev/sdd rw\n",
        Path::new("/mnt/hot data")
    ));
    assert!(!is_mount_point("garbage\n", Path::new("/local")));
}

/// ADR-0139 の試験用: proxy を有効にし、`[api]` のトークンと `[secrets]` の鍵を置いた設定。
fn langmem_proxy_config(dir: &Path, base_url: &str, secret: Option<&str>) -> Config {
    let secrets_dir = dir.join("secrets");
    std::fs::create_dir_all(&secrets_dir).unwrap();
    let token_file = dir.join("api.token");
    std::fs::write(&token_file, "proxy-token\n").unwrap();
    let secret_line = match secret {
        Some(value) => {
            std::fs::write(secrets_dir.join("celeris-api-token"), format!("{value}\n")).unwrap();
            "api_key_secret = \"celeris-api-token\"".to_string()
        }
        None => String::new(),
    };
    let text = format!(
        r#"
[api]
token_file = "{token}"

[secrets]
dir = "{secrets}"

[llm_proxy]
enabled = true
listen = "127.0.0.1:18100"

[knowledge.langmem]
enabled = true
provider = "openai-compatible"
base_url = "{base_url}"
model = "celeris/cheap"
{secret_line}

[[providers]]
id = "langmem-main"
adapter = "langmem"
tiers = ["cheap", "standard"]
"#,
        token = token_file.display(),
        secrets = secrets_dir.display(),
    );
    toml::from_str(&text).unwrap()
}

/// ADR-0139 D2: `base_url` が同じ celeris の llm-proxy を指すなら、langmem（アダプタと probe）の鍵は
/// proxy が照合する `[api]` のトークン。`api_key_secret` が無い・違う値でも bearer は必ず渡る。
#[test]
fn langmem_api_key_config_uses_the_api_token_when_targeting_llm_proxy() {
    for (secret, warns) in [
        (Some("proxy-token"), false),
        (Some("other-value"), true),
        (None, false),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let cfg = langmem_proxy_config(dir.path(), "http://127.0.0.1:18100/v1", secret);
        assert!(cfg.langmem_targets_llm_proxy());
        assert_eq!(
            cfg.langmem_api_key().as_deref(),
            Some("proxy-token"),
            "{secret:?}"
        );
        assert_eq!(
            cfg.dispatch_config().knowledge.langmem_api_key.as_deref(),
            Some("proxy-token")
        );
        let warnings = cfg.langmem_auth_warnings();
        assert_eq!(!warnings.is_empty(), warns, "{secret:?}: {warnings:?}");
        // 警告に鍵の値を載せない。
        assert!(
            warnings
                .iter()
                .all(|w| !w.contains("proxy-token") && !w.contains("other-value"))
        );
    }
    // localhost 表記も同じ proxy。
    let dir = tempfile::tempdir().unwrap();
    let cfg = langmem_proxy_config(dir.path(), "http://localhost:18100/v1", None);
    assert_eq!(cfg.langmem_api_key().as_deref(), Some("proxy-token"));
}

/// ADR-0139 D2/D3: proxy 以外を指すときは `api_key_secret` の値。解決できなければ起動時に警告する。
#[test]
fn langmem_api_key_config_uses_the_secret_for_other_endpoints_and_warns_if_missing() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = langmem_proxy_config(dir.path(), "http://127.0.0.1:18000/v1", Some("sk-direct"));
    assert!(!cfg.langmem_targets_llm_proxy());
    assert_eq!(cfg.langmem_api_key().as_deref(), Some("sk-direct"));
    assert!(cfg.langmem_auth_warnings().is_empty());

    std::fs::remove_file(dir.path().join("secrets/celeris-api-token")).unwrap();
    assert!(cfg.langmem_api_key().is_none());
    let warnings = cfg.langmem_auth_warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("cannot be resolved"), "{warnings:?}");
}

/// ADR-0139 D3: proxy を指しているのに `[api]` のトークンも `api_key_secret` も読めなければ、
/// run を起こす前（設定の読み込み時）に警告する。無効な `[knowledge.langmem]` は何も言わない。
#[test]
fn langmem_auth_warnings_config_reports_a_missing_proxy_bearer() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = langmem_proxy_config(dir.path(), "http://127.0.0.1:18100/v1", None);
    std::fs::remove_file(dir.path().join("api.token")).unwrap();
    assert!(cfg.langmem_api_key().is_none());
    let warnings = cfg.langmem_auth_warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("401"), "{warnings:?}");

    cfg.knowledge.langmem.enabled = false;
    assert!(cfg.langmem_auth_warnings().is_empty());
}

/// ADR-0139 D1: verify（本番の config を読む staging）は `[llm_proxy] listen` に bind しない。
#[test]
fn verify_mode_config_does_not_serve_the_llm_proxy() {
    use task_core::DaemonMode;
    assert!(crate::daemon::run::serves_llm_proxy(DaemonMode::Normal));
    assert!(!crate::daemon::run::serves_llm_proxy(DaemonMode::Verify));
}

/// ADR-0132 付記 L1/L4: 本番の形（2026-10-02）では `opencode-qwen`（`openai_compatible:qwen`）だけが
/// ローカルの行で、probe 先はその source の `base_url`。プールの行と専用契約のアダプタは入らない。
#[test]
fn cheap_local_first_legacy_production_yields_opencode_qwen() {
    let fixture = include_str!("fixtures/provider_kind_legacy_production.toml");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, fixture).unwrap();
    let cfg = Config::load(&path).unwrap();
    assert!(cfg.execution.cheap_local_first);
    assert_eq!(
        cfg.local_cheap_providers(),
        vec![task_dispatch::LocalProviderSpec {
            provider: "opencode-qwen".into(),
            health: vec![task_dispatch::LocalHealthTarget {
                base_url: "http://127.0.0.1:9/v1".into(),
                bearer_token: None,
            }],
        }]
    );
}

/// ADR-0132 付記 L1: `celeris`（proxy）の cheap 行は、有効な `openai_compatible` source と
/// `[llm_proxy.models.qwen].cheap` があるときだけローカル。probe 先は有効な source 全部。
#[test]
fn cheap_local_first_celeris_row_needs_a_local_source_and_qwen_cheap() {
    let row = "[[providers]]\nid = \"proxy-acp\"\nadapter = \"acp\"\nmodel = \"celeris/cheap\"\ntiers = [\"cheap\"]\n\
               [[providers]]\nid = \"pool\"\nadapter = \"claude-code\"\naccount_pool = true\ntiers = [\"cheap\"]\n";
    let sources = "[[llm_proxy.sources.openai_compatible]]\nid = \"qwen\"\nbase_url = \"http://127.0.0.1:9/v1\"\napi_key = \"k\"\n\
                   [[llm_proxy.sources.openai_compatible]]\nid = \"off\"\nbase_url = \"http://127.0.0.1:10/v1\"\nenabled = false\n\
                   [[llm_proxy.sources.openai_compatible]]\nid = \"qwen2\"\nbase_url = \"http://127.0.0.1:11/v1\"\n";
    let cfg: Config = toml::from_str(&format!("{sources}{row}")).unwrap();
    assert_eq!(
        cfg.local_cheap_providers(),
        vec![task_dispatch::LocalProviderSpec {
            provider: "proxy-acp".into(),
            health: vec![
                task_dispatch::LocalHealthTarget {
                    base_url: "http://127.0.0.1:9/v1".into(),
                    bearer_token: Some("k".into()),
                },
                task_dispatch::LocalHealthTarget {
                    base_url: "http://127.0.0.1:11/v1".into(),
                    bearer_token: None,
                },
            ],
        }]
    );
    // ローカルの source が無い proxy の行はローカルではない。
    let cfg: Config = toml::from_str(row).unwrap();
    assert!(cfg.local_cheap_providers().is_empty());
    let cfg: Config = toml::from_str(&format!(
        "[[llm_proxy.sources.openai_compatible]]\nid = \"off\"\nbase_url = \"http://127.0.0.1:10/v1\"\nenabled = false\n{row}"
    ))
    .unwrap();
    assert!(cfg.local_cheap_providers().is_empty());
    // proxy の cheap が Qwen に向かわない（`models.qwen.cheap` が無い）ならローカルではない。
    let cfg: Config = toml::from_str(&format!("{sources}[llm_proxy.models.qwen]\n{row}")).unwrap();
    assert!(cfg.llm_proxy.models.qwen.is_empty());
    assert!(cfg.local_cheap_providers().is_empty());
}

/// ADR-0132 付記 L7: `[execution] cheap_local_first = false` でローカルの行を作らない。
#[test]
fn cheap_local_first_can_be_disabled() {
    let body = "[[llm_proxy.sources.openai_compatible]]\nid = \"qwen\"\nbase_url = \"http://127.0.0.1:9/v1\"\n\
                [[providers]]\nid = \"direct\"\nadapter = \"acp\"\ntiers = [\"cheap\"]\nllm_source = \"openai_compatible:qwen\"\n";
    let cfg: Config = toml::from_str(body).unwrap();
    assert_eq!(cfg.local_cheap_providers().len(), 1);
    let cfg: Config =
        toml::from_str(&format!("[execution]\ncheap_local_first = false\n{body}")).unwrap();
    assert!(!cfg.execution.cheap_local_first);
    assert!(cfg.local_cheap_providers().is_empty());
    // cheap を提供しない行はローカルではない。
    let cfg: Config = toml::from_str(&body.replace("[\"cheap\"]", "[\"standard\"]")).unwrap();
    assert!(cfg.local_cheap_providers().is_empty());
}

/// ADR-0132 付記 L6: ローカルの行の同時実行数は既存の `concurrency`（既定 1）。
#[test]
fn cheap_local_first_concurrency_comes_from_the_provider_row() {
    let head = "[[llm_proxy.sources.openai_compatible]]\nid = \"qwen\"\nbase_url = \"http://127.0.0.1:9/v1\"\n";
    let row = "[[providers]]\nid = \"direct\"\nadapter = \"acp\"\ntiers = [\"cheap\"]\nllm_source = \"openai_compatible:qwen\"\n";
    for (extra, expected) in [("", 1), ("concurrency = 3\n", 3)] {
        let cfg: Config = toml::from_str(&format!("{head}{row}{extra}")).unwrap();
        assert_eq!(cfg.local_cheap_providers()[0].provider, "direct");
        let spec = cfg
            .provider_specs()
            .into_iter()
            .find(|p| p.id == "direct")
            .unwrap();
        assert_eq!(spec.concurrency, expected);
    }
}

#[test]
fn routing_legacy_config_normalizes_with_warnings() {
    use task_core::Tier;
    let fixture = include_str!("fixtures/provider_kind_legacy_production.toml");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, fixture).unwrap();
    let cfg = Config::load(&path).unwrap();
    let catalog = cfg.routing_catalog_snapshot.as_ref().unwrap();
    assert!(
        catalog
            .deployments
            .iter()
            .any(|d| d.source_ref == "openai_compatible:qwen"
                && d.allowed_lanes == vec![Tier::Cheap])
    );
    assert!(
        catalog
            .deployments
            .iter()
            .filter(|d| d.model_profile_id.to_ascii_lowercase().contains("qwen"))
            .all(|d| d.allowed_lanes == vec![Tier::Cheap])
    );
    assert!(
        catalog
            .warnings
            .iter()
            .any(|w| w.contains("llm_proxy.models.qwen"))
    );
    assert!(
        catalog
            .warnings
            .iter()
            .all(|w| !w.contains("fixture-secret-sentinel"))
    );
    let mut dedup = catalog.warnings.clone();
    dedup.sort();
    dedup.dedup();
    assert_eq!(catalog.warnings, dedup);

    let new = format!(
        "{fixture}\n[[model_routing.models]]\nid = \"legacy:qwen:qwen3.8-27b\"\nrevision = \"v1\"\n"
    );
    std::fs::write(&path, new).unwrap();
    let cfg = Config::load(&path).unwrap();
    let catalog = cfg.routing_catalog_snapshot.as_ref().unwrap();
    assert_eq!(
        catalog
            .models
            .iter()
            .find(|m| m.id == "legacy:qwen:qwen3.8-27b")
            .unwrap()
            .revision,
        "v1"
    );
    assert!(catalog.warnings.iter().any(|w| w.contains("overrides")));

    std::fs::write(&path, format!("{fixture}\n[model_routing]\nmode = \"shadow\"\n[model_routing.policies.cheap]\nmin_quality = 0.55\n")).unwrap();
    let cfg = Config::load(&path).unwrap();
    let catalog = cfg.routing_catalog_snapshot.as_ref().unwrap();
    assert_eq!(
        catalog.mode,
        task_core::model_router::policy::RoutingMode::Shadow
    );
    assert_eq!(
        catalog
            .policies
            .iter()
            .find(|p| p.lane == Tier::Cheap)
            .unwrap()
            .min_quality,
        0.55
    );
    std::fs::write(
        &path,
        format!("{fixture}\n[model_routing.policies.cheap]\nmin_quality = 1.5\n"),
    )
    .unwrap();
    assert!(
        Config::load(&path)
            .unwrap_err()
            .to_string()
            .contains("min_quality")
    );

    std::fs::write(&path, format!("{fixture}\n[[model_routing.deployments]]\nid = \"legacy:openai-compatible:qwen:Cheap\"\nsource_ref = \"openai-compatible:qwen\"\nmodel_profile_id = \"legacy:qwen:qwen3.8-27b\"\nupstream_model = \"qwen3.8-27b\"\nhost = \"local-gpu\"\n")).unwrap();
    let cfg = Config::load(&path).unwrap();
    let dep = cfg
        .routing_catalog_snapshot
        .as_ref()
        .unwrap()
        .deployments
        .iter()
        .find(|d| d.id == "legacy:openai-compatible:qwen:Cheap")
        .unwrap();
    assert_eq!(dep.allowed_lanes, vec![Tier::Cheap]);
    assert_eq!(
        dep.billing,
        task_core::model_router::profiles::Billing::SelfHosted
    );
    assert_eq!(dep.host.as_deref(), Some("local-gpu"));

    std::fs::write(&path, format!("{fixture}\n[[model_routing.deployments]]\nid = \"bad\"\nsource_ref = \"openai_compatible:missing\"\nmodel_profile_id = \"legacy:qwen:qwen3.8-27b\"\nupstream_model = \"qwen3.8-27b\"\nallowed_lanes = [\"cheap\"]\n")).unwrap();
    assert!(
        Config::load(&path)
            .unwrap_err()
            .to_string()
            .contains("unknown source_ref")
    );
    std::fs::write(&path, format!("{fixture}\n[[model_routing.deployments]]\nid = \"legacy:openai-compatible:qwen:Cheap\"\nsource_ref = \"codex-oauth\"\nmodel_profile_id = \"legacy:qwen:qwen3.8-27b\"\nupstream_model = \"qwen3.8-27b\"\nallowed_lanes = [\"cheap\"]\n")).unwrap();
    assert!(
        Config::load(&path)
            .unwrap_err()
            .to_string()
            .contains("identity conflicts")
    );
    std::fs::write(
        &path,
        format!("{fixture}\n[model_routing]\nmode = \"enforce\"\n"),
    )
    .unwrap();
    assert!(
        Config::load(&path)
            .unwrap_err()
            .to_string()
            .contains("Phase 2")
    );
}

#[test]
fn routing_legacy_equivalence_qwen_is_cheap_only() {
    use task_core::Tier;
    let fixture = include_str!("fixtures/provider_kind_legacy_production.toml");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, fixture).unwrap();
    let cfg = Config::load(&path).unwrap();
    let catalog = cfg.routing_catalog_snapshot.as_ref().unwrap();
    let legacy = cfg
        .provider_specs()
        .into_iter()
        .find(|p| p.id == "opencode-qwen")
        .unwrap();
    let normalized = catalog
        .deployments
        .iter()
        .find(|d| d.id == "provider:opencode-qwen")
        .unwrap();
    assert_eq!(legacy.tiers, vec![Tier::Cheap]);
    assert_eq!(normalized.allowed_lanes, legacy.tiers);
    assert_eq!(normalized.upstream_model, "qwen-local/qwen3.8-27b");
    assert_eq!(
        cfg.provider_llm_source(&legacy.id).unwrap().source,
        task_core::LlmSourceRef::OpenaiCompatible("qwen".into())
    );
    assert_eq!(normalized.source_ref, "openai_compatible:qwen");
}

fn routing_phase2_load(extra: &str) -> Result<Config, String> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let db = dir.path().join("db.sqlite3");
    let ws = dir.path().join("ws");
    std::fs::write(
        &path,
        format!(
            "db = {db:?}\nworkspace_root = {ws:?}\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n{extra}"
        ),
    )
    .unwrap();
    Config::load(&path).map_err(|e| e.to_string())
}

#[test]
fn routing_shadow_config_defaults_off_and_requires_caps() {
    let legacy = routing_phase2_load("").unwrap();
    let policy = &legacy.model_routing.runtime.as_ref().unwrap().shadow;
    assert!(!policy.execute);
    assert_eq!(policy.sample_rate, 0.0);
    assert!(policy.daily_caps().is_none());

    let decision = routing_phase2_load("[model_routing]\nmode = \"shadow\"\n").unwrap();
    assert!(
        !decision
            .model_routing
            .runtime
            .as_ref()
            .unwrap()
            .shadow
            .execute
    );

    let base = "[model_routing.shadow]\nexecute = true\nsample_rate = 1.0\n";
    let err = routing_phase2_load(base).unwrap_err();
    assert!(err.contains("daily_max_requests"), "{err}");

    let capped = format!(
        "{base}candidate_policy = \"candidate-v1\"\ndaily_max_requests = 10\ndaily_max_tokens = 1000\n\
         daily_max_effective_usd = 1.5\nmax_concurrency = 2\nmax_queue_depth = 4\n\
         timeout_ms = 3000\n"
    );
    let err = routing_phase2_load(&capped).unwrap_err();
    assert!(err.contains("allowlist"), "{err}");
    let partial = format!("{capped}[model_routing.shadow.allowlist]\ntask_kinds = [\"*\"]\n");
    let err = routing_phase2_load(&partial).unwrap_err();
    assert!(err.contains("every allowlist dimension"), "{err}");
    let valid = format!(
        "{capped}[model_routing.shadow.allowlist]\n\
         task_kinds = [\"*\"]\nroles = [\"*\"]\nlanes = [\"cheap\"]\nsources = [\"qwen\"]\n"
    );
    let config = routing_phase2_load(&valid).unwrap();
    let runtime = config.model_routing.runtime.as_ref().unwrap();
    assert!(runtime.shadow.execute);
    assert_eq!(runtime.shadow.daily_caps().unwrap().max_requests, 10);
    assert_eq!(runtime.shadow.allowlist.sources, ["qwen"]);
    assert_eq!(
        runtime.shadow_candidate_policy.as_deref(),
        Some("candidate-v1")
    );

    for (suffix, reason) in [
        ("sample_rate = 1.1\n", "sample_rate"),
        ("sample_rate = -0.1\n", "sample_rate"),
        ("daily_max_tokens = 0\n", "daily_max_tokens"),
    ] {
        let err = routing_phase2_load(&format!("[model_routing.shadow]\n{suffix}")).unwrap_err();
        assert!(err.contains(reason), "{err}");
    }
}

#[test]
fn routing_sidecar_config_defaults_off_and_rejects_primary() {
    let config = routing_phase2_load("").unwrap();
    let sidecar = &config.model_routing.estimator.sidecar;
    assert!(!sidecar.enabled);
    assert!(sidecar.shadow_only);
    assert!(!sidecar.send_prompt);
    assert_eq!(sidecar.protocol_version, 1);

    for (snippet, expected) in [
        ("shadow_only = false\n", "shadow_only"),
        ("enabled = true\n", "requires endpoint"),
        (
            "enabled = true\nendpoint = \"http://127.0.0.1:8080\"\n\
             estimator_id = \"test\"\nestimator_version = \"v1\"\n",
            "daily_max_requests",
        ),
        ("endpoint = \"http://example.com:8080\"\n", "endpoint"),
    ] {
        let err = routing_phase2_load(&format!("[model_routing.estimator.sidecar]\n{snippet}"))
            .unwrap_err();
        assert!(err.contains(expected), "{err}");
    }

    let valid = "[model_routing.estimator.sidecar]\n\
        enabled = true\nendpoint = \"http://127.0.0.1:8080\"\n\
        estimator_id = \"test\"\nestimator_version = \"v1\"\n\
        daily_max_requests = 10\n\
        [model_routing.estimator.sidecar.allowlist]\n\
        task_kinds = [\"*\"]\nroles = [\"*\"]\nlanes = [\"cheap\"]\nsources = [\"qwen\"]\n";
    let config = routing_phase2_load(valid).unwrap();
    assert!(config.model_routing.estimator.sidecar.enabled);
    assert!(config.model_routing.estimator.sidecar.shadow_only);
}

#[test]
fn routing_sidecar_privacy_and_dependencies_gate_prompt() {
    use task_core::model_router::shadow::ShadowTarget;

    let base = "[model_routing.estimator.sidecar]\n\
        endpoint = \"https://router.example/estimate\"\n\
        network_allowlist = [\"router.example\"]\n";
    let config = routing_phase2_load(base).unwrap();
    let sidecar = &config.model_routing.estimator.sidecar;
    let target = ShadowTarget {
        task_kind: "coding".into(),
        role: "software-engineering".into(),
        lane: "cheap".into(),
        source: "qwen".into(),
    };
    assert!(!sidecar.allows_prompt(&target));
    assert!(sidecar.allows_dependencies(&["router.example".into()]));
    assert!(!sidecar.allows_dependencies(&["embeddings.example".into()]));

    let err = routing_phase2_load(&format!("{base}send_prompt = true\n")).unwrap_err();
    assert!(err.contains("prompt_allowlist"), "{err}");
    let config = routing_phase2_load(&format!(
        "{base}send_prompt = true\n\
         [model_routing.estimator.sidecar.prompt_allowlist]\n\
         task_kinds = [\"coding\"]\nroles = [\"software-engineering\"]\n\
         lanes = [\"cheap\"]\nsources = [\"qwen\"]\n"
    ))
    .unwrap();
    assert!(
        config
            .model_routing
            .estimator
            .sidecar
            .allows_prompt(&target)
    );
    assert!(
        !config
            .model_routing
            .estimator
            .sidecar
            .allows_dependencies(&["embeddings.example".into()])
    );
}

#[test]
fn routing_enforce_opt_in_validates_heuristic_only() {
    use task_core::model_router::policy::RoutingMode;
    // heuristic の明示 opt-in だけが通る。
    let cfg = routing_phase2_load(
        "[model_routing]\nmode = \"enforce\"\n[model_routing.estimator]\nkind = \"heuristic\"\n",
    )
    .unwrap();
    let runtime = cfg.model_routing.runtime.as_ref().unwrap();
    assert_eq!(runtime.mode, RoutingMode::Enforce);
    assert_eq!(
        cfg.routing_catalog_snapshot.as_ref().unwrap().mode,
        RoutingMode::Enforce
    );
    assert_eq!(runtime.enforce_routes, vec!["standalone", "server"]);
    assert_eq!(runtime.dispatch_settings().mode, RoutingMode::Enforce);
    let cfg = routing_phase2_load(
        "[model_routing]\nmode = \"enforce\"\nenforce_routes = [\"standalone\"]\n[model_routing.estimator]\nkind = \"heuristic\"\n",
    )
    .unwrap();
    assert_eq!(
        cfg.model_routing.runtime.as_ref().unwrap().enforce_routes,
        vec!["standalone"]
    );
    // opt-in の無い enforce は拒否（Phase 1 の文言を保つ）。
    let err = routing_phase2_load("[model_routing]\nmode = \"enforce\"\n").unwrap_err();
    assert!(
        err.contains("Phase 2") && err.contains("heuristic"),
        "{err}"
    );
    // heuristic 以外の optimizer は enforce でも shadow でも拒否。
    for mode in ["enforce", "shadow", "legacy"] {
        let err = routing_phase2_load(&format!(
            "[model_routing]\nmode = \"{mode}\"\n[model_routing.estimator]\nkind = \"routellm\"\n"
        ))
        .unwrap_err();
        assert!(err.contains("only \"heuristic\""), "{mode}: {err}");
    }
    // standalone でもサーバ全体の制約でもない経路は拒否。
    for route in ["task", "org", ""] {
        let err = routing_phase2_load(&format!(
            "[model_routing]\nmode = \"enforce\"\nenforce_routes = [\"{route}\"]\n[model_routing.estimator]\nkind = \"heuristic\"\n"
        ))
        .unwrap_err();
        assert!(err.contains("standalone or server-wide"), "{route}: {err}");
    }
    let err = routing_phase2_load(
        "[model_routing]\nmode = \"enforce\"\nenforce_routes = []\n[model_routing.estimator]\nkind = \"heuristic\"\n",
    )
    .unwrap_err();
    assert!(err.contains("enforce_routes is empty"), "{err}");
    // shadow + heuristic の明示は従来どおり通る（mode=shadow の挙動は変えない）。
    let cfg = routing_phase2_load(
        "[model_routing]\nmode = \"shadow\"\n[model_routing.estimator]\nkind = \"heuristic\"\n",
    )
    .unwrap();
    assert_eq!(
        cfg.model_routing.runtime.as_ref().unwrap().mode,
        RoutingMode::Shadow
    );
    // 不正な数値も拒否する。
    for (extra, needle) in [
        (
            "[model_routing]\nobservation_ttl_seconds = 0\n",
            "observation_ttl_seconds",
        ),
        (
            "[model_routing.subscription_windows.five_hour]\nreserve_value_usd = -1\n",
            "reserve_value_usd",
        ),
        (
            "[[model_routing.resource_groups]]\nid = \"gpu0\"\nusd_per_gpu_second = nan\n",
            "usd_per_gpu_second",
        ),
        (
            "[[model_routing.resource_groups]]\nid = \"gpu0\"\nconcurrency_limit = 0\n",
            "concurrency_limit",
        ),
        (
            "[[model_routing.resource_groups]]\nid = \"g\"\n[[model_routing.resource_groups]]\nid = \"g\"\n",
            "duplicate id",
        ),
        ("[model_routing.retry]\nclient = 1\n", "retry.client"),
        (
            "[model_routing.retry]\ntotal_attempts = 0\n",
            "total_attempts",
        ),
    ] {
        let err = routing_phase2_load(extra).unwrap_err();
        assert!(err.contains(needle), "{extra}: {err}");
    }
}

#[test]
fn routing_config_defaults_do_not_seed_unknown_as_zero() {
    use task_core::model_router::policy::{FreshnessPolicy, RoutingMode};
    let cfg = routing_phase2_load("").unwrap();
    let runtime = cfg.model_routing.runtime.as_ref().unwrap();
    assert_eq!(runtime.mode, RoutingMode::Legacy);
    assert_eq!(runtime.freshness, FreshnessPolicy::default());
    assert!(runtime.window_reserves.is_empty());
    assert!(runtime.resource_groups.is_empty());
    let rates = runtime.self_host_rates(Some("gpu0"));
    assert_eq!(rates.usd_per_gpu_second, None);
    assert_eq!(rates.usd_per_wait_second, None);
    assert!(runtime.capacity_limits(None).resource_groups.is_empty());
    assert_eq!(
        runtime.fallback,
        llm_proxy::fallback::FallbackSettings::default()
    );
    let settings = runtime.dispatch_settings();
    assert_eq!(settings, task_dispatch::DispatchRoutingSettings::default());
    // 一部だけ書いた欄も、書いていない成分は unknown のまま（0 にしない）。
    let cfg = routing_phase2_load(
        "[model_routing]\nobservation_ttl_seconds = 120\n\
         [model_routing.subscription_windows.five_hour]\nreserve_value_usd = 4.0\n\
         [model_routing.subscription_windows.seven_day]\n\
         [[model_routing.resource_groups]]\nid = \"gpu0\"\nconcurrency_limit = 2\n\
         [[model_routing.resource_groups]]\nid = \"gpu1\"\nusd_per_gpu_second = 0.001\n\
         [model_routing.retry]\nserver = 1\n",
    )
    .unwrap();
    let runtime = cfg.model_routing.runtime.as_ref().unwrap();
    assert_eq!(runtime.freshness.observation_ttl_seconds, 120.0);
    assert_eq!(runtime.window_reserves.get("five_hour"), Some(&4.0));
    assert_eq!(runtime.window_reserves.get("seven_day"), None);
    assert_eq!(
        runtime.self_host_rates(Some("gpu0")).usd_per_gpu_second,
        None
    );
    assert_eq!(
        runtime.self_host_rates(Some("gpu1")).usd_per_gpu_second,
        Some(0.001)
    );
    assert_eq!(
        runtime.self_host_rates(Some("gpu1")).usd_per_wait_second,
        None
    );
    let caps = runtime.capacity_limits(Some(3));
    assert_eq!(caps.per_account, Some(3));
    assert_eq!(caps.resource_groups.get("gpu0"), Some(&2));
    assert_eq!(caps.resource_groups.get("gpu1"), None);
    assert_eq!(runtime.fallback.limits.server, 1);
    assert_eq!(
        runtime.fallback.limits.rate_limited,
        llm_proxy::fallback::RetryLimits::default().rate_limited
    );
    assert_eq!(runtime.dispatch_settings().window_reserves.len(), 1);
}

#[test]
fn routing_phase3_escalation_defaults_and_validation() {
    let cfg = routing_phase2_load("").unwrap();
    let runtime = cfg.model_routing.runtime.as_ref().unwrap();
    assert_eq!(
        runtime.escalation,
        task_core::EscalationThresholds::default()
    );
    assert_eq!(runtime.context_safety_margin, None);
    let cfg = routing_phase2_load("[model_routing]\ncontext_safety_margin = 512\n[model_routing.escalation]\nquality_failures_per_lane = 3\nmax_total_attempts = 5\n").unwrap();
    let runtime = cfg.model_routing.runtime.as_ref().unwrap();
    assert_eq!(runtime.context_safety_margin, Some(512));
    assert_eq!(runtime.escalation.quality_failures_per_lane, 3);
    assert_eq!(runtime.escalation.max_total_attempts, 5);
    for key in ["quality_failures_per_lane", "max_total_attempts"] {
        let err =
            routing_phase2_load(&format!("[model_routing.escalation]\n{key} = 0\n")).unwrap_err();
        assert!(err.contains(key), "{err}");
    }
    let cfg = routing_phase2_load("[model_routing]\nfuture_context_field = 1\n[model_routing.escalation]\nfuture_threshold = 7\n").unwrap();
    assert_eq!(
        cfg.model_routing.runtime.as_ref().unwrap().escalation,
        task_core::EscalationThresholds::default()
    );
}

/// ADR 2026-10-07-worker-no-subagents-no-llm-cli D7: `[adapters.<id>] subagents` は `deny`（既定・省略・未知の値）/
/// `allow_cos` / `allow` に決定的に解決される。3 つの adapter で同じ。
#[test]
fn adapter_subagents_setting_resolves_deterministically_and_defaults_to_deny() {
    use task_worker::tool_policy::SubagentPolicy;
    let cfg: Config =
        toml::from_str("[[providers]]\nid = \"x\"\nadapter = \"claude-code\"\n").unwrap();
    assert_eq!(cfg.adapters.claude_code.subagents, "");
    assert_eq!(
        cfg.adapters.claude_code.resolved_subagents(),
        SubagentPolicy::Deny
    );
    assert_eq!(
        cfg.adapters.codex.resolved_subagents(),
        SubagentPolicy::Deny
    );
    assert_eq!(cfg.adapters.acp.resolved_subagents(), SubagentPolicy::Deny);

    let cfg: Config = toml::from_str(
        "[[providers]]\nid = \"x\"\nadapter = \"claude-code\"\n\n[adapters.claude_code]\nsubagents = \"allow\"\n\n[adapters.codex]\nsubagents = \"allow_cos\"\n\n[adapters.acp]\nsubagents = \"whatever\"\n",
    )
    .unwrap();
    assert!(cfg.validate().is_ok());
    assert_eq!(
        cfg.adapters.claude_code.resolved_subagents(),
        SubagentPolicy::Allow
    );
    assert_eq!(
        cfg.adapters.codex.resolved_subagents(),
        SubagentPolicy::AllowCos
    );
    assert_eq!(cfg.adapters.acp.resolved_subagents(), SubagentPolicy::Deny);
}
