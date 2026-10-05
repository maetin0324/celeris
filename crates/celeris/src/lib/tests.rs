use super::*;
use crate::daemon::{admin::*, bootstrap::*, secrets::*};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use task_api::types::GenreConfigView;
use task_core::{DaemonMode, InstanceRole, SharedRole, SqliteStore, TaskStore};
use time::OffsetDateTime;

#[test]
fn browser_specialist_provider_uses_configured_acp_harness() {
    let cfg: Config = toml::from_str(
        r#"
[[providers]]
id = "browser-specialist-test"
adapter = "browser-specialist"
tiers = ["standard"]
command = "scripted-acp"
args = ["acp"]
"#,
    )
    .unwrap();
    cfg.validate().unwrap();
    let adapters = build_adapters(&cfg);
    assert_eq!(
        adapters["browser-specialist-test"].id(),
        "browser-specialist"
    );
}

// ---- ADR-0033 D1（Phase 23）: 組織図の種蒔き ----

/// 空の DB には例の組織図（11 ノード）が入り、2 回目は何もしない（以後は DB が正）。
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
"#
        ),
    )
    .unwrap();
    let config = Config::load(&path).unwrap();
    let store = SqliteStore::open(&config.db.path).unwrap();

    assert_eq!(seed_org_if_empty(&store, &config).unwrap(), 14);
    let nodes = store.org_list().unwrap();
    assert_eq!(nodes.len(), 14);
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
    assert_eq!(store.org_list().unwrap().len(), 14);
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
            "db = \"celeris.sqlite3\"\nworkspace_root = \"ws\"\norg_include = \"org.toml\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
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
    let config: Config = toml::from_str(&format!(
        "db = \"{}\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
        db.display()
    ))
    .unwrap();
    let store = SqliteStore::open(&config.db.path).unwrap();
    assert_eq!(seed_org_if_empty(&store, &config).unwrap(), 0);
    assert!(store.org_list().unwrap().is_empty());
}

/// ADR-0012 D1: 同じ claude-code を使う 2 アカウントが、それぞれの env と model を持つアダプタになる。
#[test]
fn build_adapters_creates_one_adapter_per_provider_with_merged_env_and_model() {
    let text = r#"
[adapters.claude_code]
model = "adapter-default-model"
env = { SHARED = "base", CLAUDE_CONFIG_DIR = "/base" }

[[providers]]
id = "acct-a"
adapter = "claude-code"
model = "model-a"
env = { CLAUDE_CONFIG_DIR = "/accounts/a" }

[[providers]]
id = "acct-b"
adapter = "claude-code"
env = { CLAUDE_CONFIG_DIR = "/accounts/b" }

[[providers]]
id = "local-fake"
adapter = "fake"
model = "fake"
"#;
    let cfg: Config = toml::from_str(text).unwrap();
    cfg.validate().unwrap();
    let adapters = build_adapters(&cfg);
    assert_eq!(adapters.len(), 3);
    assert_eq!(adapters["acct-a"].id(), "claude-code");
    assert_eq!(adapters["local-fake"].id(), "fake");

    let a = merged_env(&cfg.adapters.claude_code.env, &cfg.providers[0].env);
    assert_eq!(
        a,
        vec![
            ("CLAUDE_CONFIG_DIR".into(), "/accounts/a".into()),
            ("SHARED".into(), "base".into())
        ]
    );
    let b = merged_env(&cfg.adapters.claude_code.env, &cfg.providers[1].env);
    assert!(b.contains(&("CLAUDE_CONFIG_DIR".into(), "/accounts/b".into())));

    let models = effective_models(&cfg);
    assert_eq!(models["acct-a"], "model-a");
    assert_eq!(models["acct-b"], "adapter-default-model");
    assert_eq!(models["local-fake"], "fake");

    // ADR-0013 D4: スナップショットの定義部分は設定の順・実効モデル（env は載せない）。
    let lives = provider_lives(&cfg);
    let ids: Vec<&str> = lives.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["acct-a", "acct-b", "local-fake"]);
    assert_eq!(lives[1].model.as_deref(), Some("adapter-default-model"));
    assert_eq!(
        (
            lives[0].adapter.as_str(),
            lives[0].concurrency,
            lives[0].in_use
        ),
        ("claude-code", 1, 0)
    );
    assert!(!hostname().is_empty());

    // ADR-0015 D3: マウント点の最長一致でファイルシステム種別を引く。
    let mountinfo = "\
25 30 0:24 / / rw,relatime shared:1 - ext4 /dev/mapper/root rw
26 25 0:52 / /home rw,relatime shared:2 - nfs4 server:/home rw,vers=4.2
27 26 0:53 / /home/u/local rw,relatime shared:3 - ext4 /dev/sdb1 rw";
    assert_eq!(
        filesystem_type_in(mountinfo, Path::new("/var/lib/celeris")).as_deref(),
        Some("ext4")
    );
    assert_eq!(
        filesystem_type_in(mountinfo, Path::new("/home/u/workspace")).as_deref(),
        Some("nfs4")
    );
    // 同じマウント点に autofs と実体が並ぶ場合は後の行（実体）を採る。
    let autofs_first = "\
25 30 0:24 / / rw,relatime shared:1 - ext4 /dev/mapper/root rw
26 25 0:51 / /home rw,relatime shared:2 - autofs systemd-1 rw
27 25 0:52 / /home rw,relatime shared:3 - nfs4 server:/home rw,vers=4.2";
    assert_eq!(
        filesystem_type_in(autofs_first, Path::new("/home/u/x")).as_deref(),
        Some("nfs4")
    );
    assert_eq!(
        filesystem_type_in(mountinfo, Path::new("/home/u/local/db")).as_deref(),
        Some("ext4")
    );
    assert_eq!(filesystem_type_in("garbage", Path::new("/home")), None);

    // ADR-0013 D11: /config の要約には env のキー名だけが載り、値は載らない。
    let view = config_view(&cfg, "127.0.0.1:7710".parse().unwrap());
    assert_eq!(view.providers[0].env_keys, ["CLAUDE_CONFIG_DIR"]);
    assert_eq!(
        view.providers[1].model.as_deref(),
        Some("adapter-default-model")
    );
    assert_eq!(
        (view.api.bind.as_str(), view.api.auth_required),
        ("127.0.0.1:7710", false)
    );
    let json = serde_json::to_string(&view).unwrap();
    for secret in ["/accounts/a", "/accounts/b", "/base", "base\""] {
        assert!(!json.contains(secret), "{secret} leaked: {json}");
    }
}

/// ADR-0027 D1 / ADR-0028 D1: `[[genres]]` は `config_view` の `genres[]` に設定順のまま写る。
/// `capabilities` / `input_artifacts` / `output_artifacts` を書かない分野は空のまま（既存設定との互換）。
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

[[providers]]
id = "local-fake"
adapter = "fake"
model = "fake"
"#;
    let cfg: Config = toml::from_str(text).unwrap();
    cfg.validate().unwrap();
    let view = config_view(&cfg, "127.0.0.1:7710".parse().unwrap());
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

[[providers]]
id = "local-fake"
adapter = "fake"
model = "fake"
"#;
    let cfg: Config = toml::from_str(text).unwrap();
    cfg.validate().unwrap();
    let view = config_view(&cfg, "127.0.0.1:7710".parse().unwrap());
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
    // 説明が付くだけ（GUI は `:` の前を名前として扱う。`docs/api/v1/gui-api.md`）。
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
fn build_adapters_wires_an_acp_provider_with_merged_env_and_row_model() {
    let text = r#"
[adapters.acp]
env = { SHARED = "base", OPENCODE_DISABLE_PROJECT_CONFIG = "1" }
permission = "deny"
model_option_id = "model"
startup_timeout_secs = 120

[[providers]]
id = "opencode-qwen"
adapter = "acp"
tiers = ["standard"]
model = "qwen-local/qwen3.8-27b"
env = { OPENCODE_CONFIG = "/x/qwen.json" }

[[providers]]
id = "opencode-default"
adapter = "acp"
tiers = ["standard"]
command = "goose"
args = ["acp"]
"#;
    let cfg: Config = toml::from_str(text).unwrap();
    cfg.validate().unwrap();
    let adapters = build_adapters(&cfg);
    assert_eq!(adapters.len(), 2);
    assert_eq!(adapters["opencode-qwen"].id(), "acp");
    assert_eq!(adapters["opencode-default"].id(), "acp");

    let models = effective_models(&cfg);
    assert_eq!(models["opencode-qwen"], "qwen-local/qwen3.8-27b");
    // 行に model が無ければ空文字（`[adapters.acp]` にモデルの既定値が無いため、他のアダプタのような
    // フォールバックは起きない）。
    assert_eq!(models["opencode-default"], "");

    let merged = merged_env(&cfg.adapters.acp.env, &cfg.providers[0].env);
    assert_eq!(
        merged,
        vec![
            ("OPENCODE_CONFIG".to_string(), "/x/qwen.json".to_string()),
            (
                "OPENCODE_DISABLE_PROJECT_CONFIG".to_string(),
                "1".to_string()
            ),
            ("SHARED".to_string(), "base".to_string()),
        ]
    );

    // 行の command/args が [adapters.acp] の既定（opencode/["acp"]）を上書きする。
    assert_eq!(cfg.providers[1].command.as_deref(), Some("goose"));
    assert_eq!(
        cfg.providers[1].args.as_deref(),
        Some(&["acp".to_string()][..])
    );
}

/// ADR-0061: `aider` プロバイダの行は `[adapters.aider]` の env に重ね、`model` は行の値が
/// `[adapters.aider]` の既定を上書きする（`codex` と同じ規則）。
#[test]
fn build_adapters_wires_an_aider_provider_with_merged_env_and_row_model() {
    let text = r#"
[adapters.aider]
env = { SHARED = "base" }
model = "anthropic/claude-haiku-4"

[[providers]]
id = "aider-1"
adapter = "aider"
tiers = ["standard"]
model = "anthropic/claude-sonnet-5"
env = { ANTHROPIC_API_KEY = "sk-x" }
"#;
    let cfg: Config = toml::from_str(text).unwrap();
    cfg.validate().unwrap();
    let adapters = build_adapters(&cfg);
    assert_eq!(adapters.len(), 1);
    assert_eq!(adapters["aider-1"].id(), "aider");

    let models = effective_models(&cfg);
    assert_eq!(models["aider-1"], "anthropic/claude-sonnet-5");

    let merged = merged_env(&cfg.adapters.aider.env, &cfg.providers[0].env);
    assert_eq!(
        merged,
        vec![
            ("ANTHROPIC_API_KEY".to_string(), "sk-x".to_string()),
            ("SHARED".to_string(), "base".to_string()),
        ]
    );
}

/// ADR-0027 D3: `paperqa` プロバイダの行は `[adapters.paperqa]` の env に重ね、`settings` は行の値が
/// あればそちらを使い、`model` は行の値がそのまま（`[adapters.paperqa]` にモデルの既定値は無い）。
#[test]
fn build_adapters_wires_a_paperqa_provider_with_merged_env_and_row_overrides() {
    let text = r#"
[adapters.paperqa]
command = "/opt/paperqa/.venv/bin/pqa"
settings = "/opt/paperqa/settings/base"
paper_directory = "/opt/paperqa/papers"
index_directory = "/opt/paperqa/index"
env = { SHARED = "base", OPENAI_BASE_URL = "http://old:1/v1" }

[[providers]]
id = "paperqa-qwen"
adapter = "paperqa"
tiers = ["standard"]
model = "openai/qwen3.8-27b"
settings = "/opt/paperqa/settings/qwen-local"
env = { OPENAI_BASE_URL = "http://127.0.0.1:18000/v1" }

[[providers]]
id = "paperqa-default"
adapter = "paperqa"
tiers = ["standard"]
"#;
    let cfg: Config = toml::from_str(text).unwrap();
    cfg.validate().unwrap();
    let adapters = build_adapters(&cfg);
    assert_eq!(adapters.len(), 2);
    assert_eq!(adapters["paperqa-qwen"].id(), "paperqa");
    assert_eq!(adapters["paperqa-default"].id(), "paperqa");

    let models = effective_models(&cfg);
    assert_eq!(models["paperqa-qwen"], "openai/qwen3.8-27b");
    // 行に model が無ければ空文字（`[adapters.paperqa]` にモデルの既定値が無いため、他のアダプタのような
    // フォールバックは起きない。ADR-0026 D3 と同じ理由）。
    assert_eq!(models["paperqa-default"], "");

    let merged = merged_env(&cfg.adapters.paperqa.env, &cfg.providers[0].env);
    assert_eq!(
        merged,
        vec![
            (
                "OPENAI_BASE_URL".to_string(),
                "http://127.0.0.1:18000/v1".to_string()
            ),
            ("SHARED".to_string(), "base".to_string()),
        ]
    );

    // 行の settings が [adapters.paperqa] の既定を上書きする。上書きしない行は共通設定のまま。
    assert_eq!(
        cfg.providers[0].settings.as_deref(),
        Some("/opt/paperqa/settings/qwen-local")
    );
    assert!(cfg.providers[1].settings.is_none());
    assert_eq!(
        cfg.adapters.paperqa.settings.as_deref(),
        Some("/opt/paperqa/settings/base")
    );
}

/// ADR-0029 D1: `local-deep-research` プロバイダの行は `[adapters.local_deep_research]` の env に重ね、
/// `model` は行の値がそのまま（`[adapters.local_deep_research]` にモデルの既定値は無い。`acp`/`paperqa`
/// と同じ理由）。行ごとの `settings` の上書きは無い（celeris 側の実装判断。`ProviderConfig.settings` は
/// `paperqa` 専用のまま）。
#[test]
fn build_adapters_wires_a_local_deep_research_provider_with_merged_env() {
    let text = r#"
[adapters.local_deep_research]
command = "/opt/ldr/.venv/bin/python"
mode = "detailed"
iterations = 3
env = { SHARED = "base", OPENAI_BASE_URL = "http://old:1/v1" }

[adapters.local_deep_research.settings]
"llm.provider" = "openai_endpoint"
"search.tool" = "wikipedia"

[[providers]]
id = "ldr-qwen"
adapter = "local-deep-research"
tiers = ["standard"]
model = "qwen3.8-27b"
env = { OPENAI_BASE_URL = "http://127.0.0.1:18000/v1" }

[[providers]]
id = "ldr-default"
adapter = "local-deep-research"
tiers = ["standard"]
"#;
    let cfg: Config = toml::from_str(text).unwrap();
    cfg.validate().unwrap();
    let adapters = build_adapters(&cfg);
    assert_eq!(adapters.len(), 2);
    assert_eq!(adapters["ldr-qwen"].id(), "local-deep-research");
    assert_eq!(adapters["ldr-default"].id(), "local-deep-research");

    let models = effective_models(&cfg);
    assert_eq!(models["ldr-qwen"], "qwen3.8-27b");
    // 行に model が無ければ空文字（`[adapters.local_deep_research]` にモデルの既定値が無い）。
    assert_eq!(models["ldr-default"], "");

    let merged = merged_env(&cfg.adapters.local_deep_research.env, &cfg.providers[0].env);
    assert_eq!(
        merged,
        vec![
            (
                "OPENAI_BASE_URL".to_string(),
                "http://127.0.0.1:18000/v1".to_string()
            ),
            ("SHARED".to_string(), "base".to_string()),
        ]
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
        cfg.adapters.local_deep_research.mode,
        task_worker::LdrMode::Detailed
    );
    assert_eq!(cfg.adapters.local_deep_research.iterations, Some(3));
}

/// ADR-0047 D4: `[adapters.langmem]`（起動コマンド）と `[knowledge.langmem]`（LLM の接続先）を
/// 合わせて 1 つの `LangMemConfig` にする。`api_key_secret` は `[secrets] dir` から解決される。
#[test]
fn build_adapters_wires_a_langmem_provider_from_both_config_sections() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let secrets_dir = dir.path().join("secrets");
    std::fs::create_dir_all(&secrets_dir).unwrap_or_else(|e| panic!("mkdir: {e}"));
    std::fs::write(secrets_dir.join("langmem-key"), "sk-test-value\n")
        .unwrap_or_else(|e| panic!("write: {e}"));
    let text = format!(
        r#"
[secrets]
dir = "{secrets}"

[adapters.langmem]
command = "/opt/langmem/.venv/bin/python"
idle_timeout_secs = 120
env = {{ SHARED = "base" }}

[knowledge.langmem]
enabled = true
provider = "openai-compatible"
base_url = "http://bnode150:18000/v1"
model = "qwen3.8-27b"
api_key_secret = "langmem-key"
max_related_pages = 5

[[providers]]
id = "langmem-main"
adapter = "langmem"
tiers = ["cheap", "standard"]
"#,
        secrets = secrets_dir.display()
    );
    let cfg: Config = toml::from_str(&text).unwrap_or_else(|e| panic!("parse: {e}"));
    cfg.validate().unwrap_or_else(|e| panic!("validate: {e}"));
    assert!(cfg.knowledge.langmem.enabled);
    assert_eq!(cfg.knowledge.langmem.max_related_pages, 5);
    let adapters = build_adapters(&cfg);
    assert_eq!(adapters.len(), 1);
    assert_eq!(adapters["langmem-main"].id(), "langmem");

    let usage = secret_usage(&cfg);
    assert!(
        usage["langmem-key"]
            .iter()
            .any(|u| u.scope == "adapter" && u.name == "langmem" && u.env == "api_key"),
        "{usage:?}"
    );
}

/// `[knowledge.langmem]` の既定は無効（`enabled = false`）で、`base_url`/`model`/`api_key_secret` は
/// 無い（ADR-0047 D4 §6: 明示的に有効化するまで知識整理 run は起きない）。
#[test]
fn langmem_knowledge_config_defaults_to_disabled() {
    let cfg: Config = toml::from_str("[[providers]]\nid = \"x\"\nadapter = \"fake\"\n").unwrap();
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
    let cfg: Config = toml::from_str("[[providers]]\nid = \"x\"\nadapter = \"bogus\"\n").unwrap();
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

/// `build_adapters` は秘密が無くても設定エラーにせず、そのプロバイダのアダプタを組み立てる（run 自体は
/// ワーカーの認証エラーで失敗する。ADR-0030 D2）。
#[test]
fn build_adapters_does_not_fail_when_a_referenced_secret_is_missing() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let secrets_dir = dir.path().join("secrets");
    std::fs::create_dir_all(&secrets_dir).unwrap_or_else(|e| panic!("mkdir: {e}"));
    // `tavily` の秘密ファイルは書かない。

    let text = format!(
        r#"[secrets]
dir = {secrets_dir:?}

[adapters.local_deep_research]
env_from_secrets = {{ LDR_SEARCH_ENGINE_WEB_TAVILY_API_KEY = "tavily" }}

[[providers]]
id = "ldr"
adapter = "local-deep-research"
"#
    );
    let cfg: Config = toml::from_str(&text).unwrap_or_else(|e| panic!("{e}"));
    cfg.validate().unwrap_or_else(|e| panic!("{e}"));
    let adapters = build_adapters(&cfg);
    assert_eq!(adapters.len(), 1);
    assert_eq!(adapters["ldr"].id(), "local-deep-research");
}

/// `Config::load` は `[secrets] dir` を相対パスのまま toml から読むので、絶対化した設定を経由するには
/// `Config::load` を使う（`toml::from_str` だけのテストでは相対のまま）。
#[test]
fn secret_usage_maps_adapter_and_provider_env_from_secrets_to_secret_ids() {
    let text = r#"[secrets]
dir = "secrets"

[adapters.local_deep_research]
env_from_secrets = { LDR_SEARCH_ENGINE_WEB_TAVILY_API_KEY = "tavily", LDR_SEARCH_ENGINE_WEB_EXA_API_KEY = "exa" }

[[providers]]
id = "ldr-tavily"
adapter = "local-deep-research"
env_from_secrets = { LDR_SEARCH_ENGINE_WEB_TAVILY_API_KEY = "tavily" }

[[providers]]
id = "ldr-exa"
adapter = "local-deep-research"
env_from_secrets = { LDR_SEARCH_ENGINE_WEB_EXA_API_KEY = "exa" }
"#;
    let cfg: Config = toml::from_str(text).unwrap_or_else(|e| panic!("{e}"));
    cfg.validate().unwrap_or_else(|e| panic!("{e}"));
    let usage = secret_usage(&cfg);
    assert_eq!(usage.len(), 2);
    let tavily = usage.get("tavily").expect("tavily uses");
    assert_eq!(tavily.len(), 2);
    assert!(tavily.iter().any(|u| u.scope == "adapter"
        && u.name == "local-deep-research"
        && u.env == "LDR_SEARCH_ENGINE_WEB_TAVILY_API_KEY"));
    assert!(tavily.iter().any(|u| u.scope == "provider"
        && u.name == "ldr-tavily"
        && u.env == "LDR_SEARCH_ENGINE_WEB_TAVILY_API_KEY"));
    let exa = usage.get("exa").expect("exa uses");
    assert_eq!(exa.len(), 2);
    assert!(
        exa.iter()
            .any(|u| u.scope == "adapter" && u.name == "local-deep-research")
    );
    assert!(
        exa.iter()
            .any(|u| u.scope == "provider" && u.name == "ldr-exa")
    );

    // 未参照の id は現れない。
    assert!(!usage.contains_key("unused"));

    // `env_from_secrets` を書かない設定は空のまま。
    let plain: Config = toml::from_str("[[providers]]\nid = \"x\"\nadapter = \"fake\"\n")
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(secret_usage(&plain).is_empty());
}

/// `api_settings` は `[secrets] dir` と `secret_usage` を `ApiSettings` に写す。
#[test]
fn api_settings_carries_secrets_dir_and_usage() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let path = dir.path().join("config.toml");
    std::fs::write(
            &path,
            "[secrets]\ndir = \"secrets\"\n\n[adapters.local_deep_research]\nenv_from_secrets = { TAVILY = \"tavily\" }\n\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
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
                    "db = {db:?}\nworkspace_root = {ws:?}\n[accounts]\nclaude_dir = {accounts_dir:?}\nmax_runs_per_account = {max_runs}\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n"
                ),
            )
            .unwrap_or_else(|e| panic!("{e}"));
    };
    write_config(2);
    let mut config = Config::load(&config_path).unwrap_or_else(|e| panic!("{e}"));
    let mut dispatcher =
        build_dispatcher(&config, Default::default()).unwrap_or_else(|e| panic!("{e}"));

    // [accounts] が変わっていなければ通る。
    assert!(reload_providers(&mut dispatcher, &mut config).is_ok());

    // max_runs_per_account を変えると、次の reload はエラーになる。
    write_config(3);
    let err = reload_providers(&mut dispatcher, &mut config).unwrap_err();
    assert!(err.contains("[accounts]"), "{err}");
    assert!(err.contains("restart"), "{err}");
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
                     [[providers]]\nid = \"x\"\nadapter = \"fake\"\n\
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

[[providers]]
id = "x"
adapter = "fake"

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

[[providers]]
id = "x"
adapter = "fake"

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
            adapter: None,
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

[[providers]]
id = "x"
adapter = "fake"

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

#[test]
fn explicit_credential_reference_missing_blocks_instead_of_using_inherited_auth() {
    let cfg: Config = toml::from_str(
        r#"[[providers]]
id = "gpt"
adapter = "codex"
[providers.env_from_secrets]
OPENAI_API_KEY = "missing-key"
[providers.tier_models.standard]
name = "sol"
model_id = "explicit-id"
"#,
    )
    .unwrap();
    let adapters = build_adapters(&cfg);
    assert!(
        adapters["gpt"]
            .model_for_tier(task_core::Tier::Standard)
            .unwrap_err()
            .contains("missing-key")
    );
}

#[test]
fn routing_config_reload_is_atomic() {
    use std::sync::Arc;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let db = dir.path().join("db.sqlite3");
    let ws = dir.path().join("ws");
    let base = format!(
        "db = {db:?}\nworkspace_root = {ws:?}\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n"
    );
    std::fs::write(&path, &base).unwrap();
    let mut config = Config::load(&path).unwrap();
    let old = Arc::clone(config.routing_catalog_snapshot.as_ref().unwrap());
    let shared = Arc::clone(config.routing_catalog_state.as_ref().unwrap());
    let mut dispatcher = build_dispatcher(&config, Default::default()).unwrap();
    std::fs::write(
        &path,
        format!("{base}\n[model_routing]\nmode = \"enforce\"\n"),
    )
    .unwrap();
    assert!(
        reload_providers(&mut dispatcher, &mut config)
            .unwrap_err()
            .contains("Phase 2")
    );
    assert!(Arc::ptr_eq(
        config.routing_catalog_snapshot.as_ref().unwrap(),
        &old
    ));
    assert!(Arc::ptr_eq(&shared.read().unwrap(), &old));
    std::fs::write(
        &path,
        format!("{base}\n[model_routing]\nmode = \"shadow\"\n"),
    )
    .unwrap();
    reload_providers(&mut dispatcher, &mut config).unwrap();
    assert!(!Arc::ptr_eq(
        config.routing_catalog_snapshot.as_ref().unwrap(),
        &old
    ));
    assert!(Arc::ptr_eq(
        &shared.read().unwrap(),
        config.routing_catalog_snapshot.as_ref().unwrap()
    ));
    assert_eq!(
        config.routing_catalog_snapshot.as_ref().unwrap().mode,
        task_core::model_router::policy::RoutingMode::Shadow
    );
}

#[test]
fn routing_runtime_is_wired_to_the_dispatcher_and_reload_is_atomic() {
    use task_core::model_router::policy::RoutingMode;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let db = dir.path().join("db.sqlite3");
    let ws = dir.path().join("ws");
    let base = format!(
        "db = {db:?}\nworkspace_root = {ws:?}\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n"
    );
    std::fs::write(
        &path,
        format!(
            "{base}\n[model_routing.subscription_windows.five_hour]\nreserve_value_usd = 2.0\n"
        ),
    )
    .unwrap();
    let mut config = Config::load(&path).unwrap();
    let mut dispatcher = build_dispatcher(&config, Default::default()).unwrap();
    assert_eq!(dispatcher.dispatch_routing().mode, RoutingMode::Legacy);
    assert_eq!(
        dispatcher
            .dispatch_routing()
            .window_reserves
            .get("five_hour"),
        Some(&2.0)
    );
    let enforce = format!(
        "{base}\n[model_routing]\nmode = \"enforce\"\n[model_routing.estimator]\nkind = \"heuristic\"\n"
    );
    std::fs::write(&path, &enforce).unwrap();
    reload_providers(&mut dispatcher, &mut config).unwrap();
    assert_eq!(dispatcher.dispatch_routing().mode, RoutingMode::Enforce);
    assert!(dispatcher.dispatch_routing().window_reserves.is_empty());
    let old = std::sync::Arc::clone(config.routing_runtime.as_ref().unwrap());
    // 不正な設定は dispatcher・runtime・catalog のどれも変えない。
    std::fs::write(
        &path,
        format!("{base}\n[model_routing]\nmode = \"enforce\"\nenforce_routes = [\"task\"]\n[model_routing.estimator]\nkind = \"heuristic\"\n"),
    )
    .unwrap();
    assert!(
        reload_providers(&mut dispatcher, &mut config)
            .unwrap_err()
            .contains("standalone or server-wide")
    );
    std::fs::write(
        &path,
        format!("{base}\n[model_routing]\nmode = \"enforce\"\n[model_routing.estimator]\nkind = \"routellm\"\n"),
    )
    .unwrap();
    assert!(
        reload_providers(&mut dispatcher, &mut config)
            .unwrap_err()
            .contains("only \"heuristic\"")
    );
    assert_eq!(dispatcher.dispatch_routing().mode, RoutingMode::Enforce);
    assert!(std::sync::Arc::ptr_eq(
        config.routing_runtime.as_ref().unwrap(),
        &old
    ));
    std::fs::write(&path, &base).unwrap();
    reload_providers(&mut dispatcher, &mut config).unwrap();
    assert_eq!(dispatcher.dispatch_routing().mode, RoutingMode::Legacy);
}
