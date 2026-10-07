//! ADR 2026-10-06 model-role-assignments D2/D6: 同じ store の割り当てを dispatcher・llm-proxy・routing catalog の
//! 3 か所が読み、同じ `(source, tier) → model` を返し、`Excluded` の lane をすべてが外すことを固定する。
//! `build_dispatcher`（daemon と同じ配線: store を reader として渡す）と、別の接続で書いた割り当てを使う。
//! 外部ネットワークには出ない。

use std::sync::Arc;

use celeris::Config;
use celeris::config::{apply_model_catalog, apply_role_assignments};
use llm_proxy::legacy_catalog::{
    LegacyCatalog, normalize_legacy_config, normalize_legacy_config_with,
};
use task_core::model_catalog::assignments::AssignmentState;
use task_core::model_catalog::{CatalogSource, DiscoveredModel};
use task_core::{ModelCatalogStore, SqliteStore, Tier};
use task_dispatch::SnapshotPublisher;

fn config_text(dir: &std::path::Path) -> String {
    format!(
        r#"db = {db:?}
workspace_root = {ws:?}

[[providers]]
id = "claude"
adapter = "claude-code"
tiers = ["frontier", "standard", "cheap"]
llm_source = "claude_oauth"
[providers.tier_models.frontier]
name = "cfg-frontier"
model_id = "cfg-frontier"
[providers.tier_models.standard]
name = "cfg-standard"
model_id = "cfg-standard"
[providers.tier_models.cheap]
name = "cfg-cheap"
model_id = "cfg-cheap"

[[providers]]
id = "codex"
adapter = "codex"
tiers = ["frontier", "standard", "cheap"]
llm_source = "codex_oauth"
[providers.tier_models.frontier]
name = "gpt-cfg-frontier"
model_id = "gpt-cfg-frontier"
[providers.tier_models.standard]
name = "gpt-cfg-standard"
model_id = "gpt-cfg-standard"
[providers.tier_models.cheap]
name = "gpt-cfg-cheap"
model_id = "gpt-cfg-cheap"

[[providers]]
id = "opencode-go"
adapter = "acp"
tiers = ["frontier", "standard", "cheap"]
llm_source = "opencode_go"

[[providers]]
id = "go-pi"
adapter = "pi"
tiers = ["frontier", "standard", "cheap"]
llm_source = "opencode_go"
model = "opencode-go/grok-4.6"
extensions = ["hashline.ts"]
tools = ["hashline_read", "hashline_edit", "bash"]

[[providers]]
id = "qwen-acp"
adapter = "acp"
tiers = ["cheap"]
llm_source = "openai_compatible:qwen"
model = "qwen-local/qwen3.8-27b"

[[providers]]
id = "qwen-pi"
adapter = "pi"
tiers = ["cheap"]
llm_source = "openai_compatible:qwen"
model = "qwen-local/qwen3.8-27b"
extensions = ["hashline.ts"]
tools = ["hashline_read", "hashline_edit", "bash"]

[llm_proxy]
enabled = true
listen = "127.0.0.1:0"
[llm_proxy.sources.claude_oauth]
accounts_dir = {claude_dir:?}
[llm_proxy.sources.codex_oauth]
accounts_dir = {codex_dir:?}
[llm_proxy.models.claude]
frontier = "proxy-claude-f"
standard = "proxy-claude-s"
cheap = "proxy-claude-c"
[llm_proxy.models.gpt]
frontier = "proxy-gpt-f"
standard = "proxy-gpt-s"
cheap = "proxy-gpt-c"
[[llm_proxy.sources.openai_compatible]]
id = "qwen"
base_url = "http://127.0.0.1:9/v1"
[llm_proxy.models.qwen]
cheap = "qwen3.8-27b"
"#,
        db = dir.join("db.sqlite3"),
        ws = dir.join("ws"),
        claude_dir = dir.join("claude-accounts"),
        codex_dir = dir.join("codex-accounts"),
    )
}

fn models(ids: &[&str]) -> Vec<DiscoveredModel> {
    ids.iter()
        .map(|id| DiscoveredModel {
            model_id: (*id).into(),
            display_name: None,
            capabilities: serde_json::json!({}),
        })
        .collect()
}

fn routing_dep<'a>(
    catalog: &'a celeris::config::RoutingCatalog,
    id: &str,
) -> Option<&'a task_core::model_router::profiles::DeploymentProfile> {
    catalog.deployments.iter().find(|d| d.id == id)
}

fn proxy_wire(catalog: &LegacyCatalog, source: &str, tier: Tier) -> Option<String> {
    catalog
        .deployment(source, tier)
        .map(|d| d.upstream_model.clone())
}

#[test]
fn dispatcher_llm_proxy_and_routing_catalog_agree_on_assignments() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("claude-accounts")).unwrap();
    std::fs::create_dir_all(dir.path().join("codex-accounts")).unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, config_text(dir.path())).unwrap();
    let config = Config::load(&path).unwrap();

    // daemon と同じ配線の dispatcher（store が reader）。割り当ては別の接続で書く。
    let mut dispatcher = celeris::build_dispatcher(&config, Default::default()).unwrap();
    let (tx, _) = tokio::sync::watch::channel(None);
    dispatcher.set_snapshot_publisher(SnapshotPublisher {
        tx,
        instance_id: "test".into(),
        hostname: "test".into(),
        started_at: String::new(),
        tick_ms: 1000,
        providers: celeris::provider_lives(&config),
        provider_checks: Default::default(),
    });
    let store = Arc::new(SqliteStore::open(&config.db.path).unwrap());

    // 割り当て前は 3 か所とも config のまま。
    assert_eq!(
        dispatcher.effective_lane_model("claude", Tier::Standard),
        Ok(Some("cfg-standard".into()))
    );
    let plain_proxy = normalize_legacy_config(&config.llm_proxy);
    assert_eq!(
        proxy_wire(&plain_proxy, "claude-oauth", Tier::Standard).as_deref(),
        Some("proxy-claude-s")
    );

    // claude-oauth/standard → 利用可能なモデル、codex-oauth/cheap → 発見で消えたモデル（Excluded）。
    let claude = CatalogSource::new("claude-oauth");
    let codex = CatalogSource::new("codex-oauth");
    // claude の cheap は config の model（provider / proxy とも）が発見で消え、利用可能な別の model を割り当てる。
    store
        .model_catalog_apply(
            &claude,
            &models(&[
                "claude-assigned",
                "claude-ok",
                "cfg-cheap",
                "proxy-claude-c",
            ]),
            10,
        )
        .unwrap();
    store
        .model_catalog_apply(&claude, &models(&["claude-assigned", "claude-ok"]), 20)
        .unwrap();
    let og = CatalogSource::new("opencode-go");
    store
        .model_catalog_apply(&og, &models(&["glm-5"]), 10)
        .unwrap();
    store
        .model_role_assignment_set(&claude, Tier::Cheap, "claude-ok", None, "test", 30)
        .unwrap();
    store
        .model_role_assignment_set(&og, Tier::Standard, "glm-5", None, "test", 30)
        .unwrap();
    store
        .model_catalog_apply(&codex, &models(&["gpt-gone"]), 10)
        .unwrap();
    store.model_catalog_apply(&codex, &models(&[]), 20).unwrap();
    store
        .model_role_assignment_set(&claude, Tier::Standard, "claude-assigned", None, "test", 30)
        .unwrap();
    store
        .model_role_assignment_set(&codex, Tier::Cheap, "gpt-gone", None, "test", 30)
        .unwrap();
    let view = store.model_role_assignment_view().unwrap();
    assert_eq!(
        view.get("codex-oauth", Tier::Cheap).map(|a| a.state),
        Some(AssignmentState::Excluded {
            reason: "catalog:unavailable"
        })
    );

    // 1. dispatcher（run が実際に使う実効 bindings）。
    assert_eq!(
        dispatcher.effective_lane_model("claude", Tier::Standard),
        Ok(Some("claude-assigned".into()))
    );
    assert!(
        dispatcher
            .effective_lane_model("codex", Tier::Cheap)
            .is_err()
    );
    // config の model が消えた lane も、割り当て先が利用可能なら使える。
    assert_eq!(
        dispatcher.effective_lane_model("claude", Tier::Cheap),
        Ok(Some("claude-ok".into()))
    );
    // model も tier_models も無い opencode go の acp 行は、割り当てた lane だけ。
    assert_eq!(
        dispatcher.effective_lane_model("opencode-go", Tier::Standard),
        Ok(Some("opencode-go/glm-5".into()))
    );
    assert!(
        dispatcher
            .effective_lane_model("opencode-go", Tier::Cheap)
            .is_err()
    );
    // 割り当ての無い lane は config のまま。
    assert_eq!(
        dispatcher.effective_lane_model("claude", Tier::Frontier),
        Ok(Some("cfg-frontier".into()))
    );
    assert_eq!(
        dispatcher.effective_lane_model("codex", Tier::Standard),
        Ok(Some("gpt-cfg-standard".into()))
    );

    // 2. llm-proxy の legacy catalog。
    let proxy = normalize_legacy_config_with(&config.llm_proxy, &view);
    assert_eq!(
        proxy_wire(&proxy, "claude-oauth", Tier::Standard).as_deref(),
        Some("claude-assigned")
    );
    assert_eq!(proxy_wire(&proxy, "codex-oauth", Tier::Cheap), None);
    assert_eq!(
        proxy_wire(&proxy, "claude-oauth", Tier::Frontier).as_deref(),
        Some("proxy-claude-f")
    );
    assert_eq!(
        proxy_wire(&proxy, "codex-oauth", Tier::Standard).as_deref(),
        Some("proxy-gpt-s")
    );
    assert_eq!(
        proxy_wire(&proxy, "claude-oauth", Tier::Cheap).as_deref(),
        Some("claude-ok")
    );
    assert!(proxy.warnings.contains(
        &"model_role_assignments: deployment legacy:codex-oauth:Cheap excluded (catalog:unavailable)"
            .to_string()
    ));

    // 3. routing catalog（`refresh_routing_catalog` と同じ順: config → 割り当て → catalog）。
    let mut routing = config.routing_catalog().unwrap();
    let entries = store.model_catalog_list().unwrap();
    let overrides = store.model_catalog_overrides().unwrap();
    // 割り当てが先（`refresh_routing_catalog` と同じ順）。後段の catalog は何も外さず、警告も重ならない。
    let applied = apply_role_assignments(&mut routing, &view, &config.provider_lane_seeds());
    assert!(!applied.is_empty());
    let dropped = apply_model_catalog(&mut routing, &entries, &overrides);
    assert!(dropped.is_empty(), "{dropped:?}");
    assert_eq!(
        routing
            .warnings
            .iter()
            .filter(|w| w.contains("legacy:codex-oauth:Cheap"))
            .count(),
        1
    );
    for id in ["legacy:claude-oauth:Standard", "provider:claude/standard"] {
        let dep = routing_dep(&routing, id).unwrap_or_else(|| panic!("{id} present"));
        assert_eq!(dep.upstream_model, "claude-assigned", "{id}");
        assert!(
            routing.models.iter().any(|m| m.id == dep.model_profile_id),
            "{id}: model profile exists"
        );
    }
    for id in ["legacy:codex-oauth:Cheap", "provider:codex/cheap"] {
        assert!(routing_dep(&routing, id).is_none(), "{id} excluded");
        assert!(
            routing.warnings.contains(&format!(
                "model_role_assignments: deployment {id} excluded (catalog:unavailable)"
            )),
            "{id} warned: {:?}",
            routing.warnings
        );
    }
    // config の model が消えた lane は割り当て先で復活する。
    for id in ["legacy:claude-oauth:Cheap", "provider:claude/cheap"] {
        assert_eq!(
            routing_dep(&routing, id).map(|d| d.upstream_model.as_str()),
            Some("claude-ok"),
            "{id}"
        );
    }
    // tier_models も model も無い acp 行は seed から deployment が足される（割り当てた lane だけ）。
    let og_dep = routing_dep(&routing, "provider:opencode-go/standard").expect("seeded");
    assert_eq!(og_dep.upstream_model, "opencode-go/glm-5");
    assert_eq!(og_dep.allowed_lanes, vec![Tier::Standard]);
    assert!(routing_dep(&routing, "provider:opencode-go/cheap").is_none());
    // 割り当ての無い lane の deployment は config のまま。
    assert_eq!(
        routing_dep(&routing, "legacy:claude-oauth:Frontier").map(|d| d.upstream_model.as_str()),
        Some("proxy-claude-f")
    );
    assert_eq!(
        routing_dep(&routing, "provider:claude/frontier").map(|d| d.upstream_model.as_str()),
        Some("cfg-frontier")
    );
    assert_eq!(
        routing_dep(&routing, "provider:codex/standard").map(|d| d.upstream_model.as_str()),
        Some("gpt-cfg-standard")
    );

    // 割り当てを外すと、次の解決から 3 か所とも config に戻る（DB を書けば効く）。
    assert!(
        store
            .model_role_assignment_delete(&codex, Tier::Cheap, "test", 40)
            .unwrap()
    );
    assert_eq!(
        dispatcher.effective_lane_model("codex", Tier::Cheap),
        Ok(Some("gpt-cfg-cheap".into()))
    );
    let view = store.model_role_assignment_view().unwrap();
    assert_eq!(
        proxy_wire(
            &normalize_legacy_config_with(&config.llm_proxy, &view),
            "codex-oauth",
            Tier::Cheap
        )
        .as_deref(),
        Some("proxy-gpt-c")
    );
}

#[test]
fn role_members_catalog_proxy_and_dispatch_share_priority_and_empty_scope() {
    use task_core::model_catalog::assignments::RoleMember;
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("claude-accounts")).unwrap();
    std::fs::create_dir_all(dir.path().join("codex-accounts")).unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, config_text(dir.path())).unwrap();
    let config = Config::load(&path).unwrap();
    let store = SqliteStore::open(&config.db.path).unwrap();
    let source = CatalogSource::new("claude-oauth");
    store
        .model_catalog_apply(&source, &models(&["a", "b"]), 1)
        .unwrap();
    store
        .model_role_members_replace(
            Tier::Standard,
            &[
                RoleMember {
                    source: source.clone(),
                    model_id: "a".into(),
                    priority: 8,
                },
                RoleMember {
                    source: source.clone(),
                    model_id: "b".into(),
                    priority: 1,
                },
            ],
            std::slice::from_ref(&source),
            "admin",
            2,
        )
        .unwrap();
    let mut dispatcher = celeris::build_dispatcher(&config, Default::default()).unwrap();
    let (tx, _) = tokio::sync::watch::channel(None);
    dispatcher.set_snapshot_publisher(SnapshotPublisher {
        tx,
        instance_id: "test".into(),
        hostname: "test".into(),
        started_at: String::new(),
        tick_ms: 1000,
        providers: celeris::provider_lives(&config),
        provider_checks: Default::default(),
    });
    let view = store.model_role_assignment_view().unwrap();
    let proxy = normalize_legacy_config_with(&config.llm_proxy, &view);
    let mut routing = config.routing_catalog().unwrap();
    apply_role_assignments(&mut routing, &view, &config.provider_lane_seeds());
    let proxy_models: Vec<_> = proxy
        .deployments
        .iter()
        .filter(|d| d.source_ref == "claude-oauth" && d.allowed_lanes == [Tier::Standard])
        .map(|d| d.upstream_model.as_str())
        .collect();
    assert_eq!(proxy_models, ["b", "a"]);
    let routing_models: Vec<_> = routing
        .deployments
        .iter()
        .filter(|d| d.id.starts_with("provider:claude/") && d.allowed_lanes == [Tier::Standard])
        .map(|d| d.upstream_model.as_str())
        .collect();
    assert_eq!(routing_models, ["b", "a"]);
    assert_eq!(
        dispatcher
            .effective_lane_model("claude", Tier::Standard)
            .unwrap()
            .as_deref(),
        Some("b")
    );
    store
        .model_role_members_replace(Tier::Standard, &[], &[source], "admin", 3)
        .unwrap();
    let view = store.model_role_assignment_view().unwrap();
    let proxy = normalize_legacy_config_with(&config.llm_proxy, &view);
    assert!(proxy.deployment("claude-oauth", Tier::Standard).is_none());
    assert!(
        dispatcher
            .effective_lane_model("claude", Tier::Standard)
            .is_err()
    );
    let mut routing = config.routing_catalog().unwrap();
    apply_role_assignments(&mut routing, &view, &config.provider_lane_seeds());
    assert!(
        !routing
            .deployments
            .iter()
            .any(|d| d.id.starts_with("provider:claude/")
                && d.allowed_lanes.contains(&Tier::Standard))
    );
}

/// ADR 2026-10-06 model-role-assignments 付記（2026-10-07 wire-prefix）: self-host（`openai-compatible:qwen`）の
/// ACP 行・Pi 行、その proxy の cheap lane、opencode go の Pi 行について、同じ割り当てから dispatcher・llm-proxy・
/// routing catalog が同じ実行用の名前を出す。本番で見つかった形（cheap に `qwen3.8-27b`）は provider 行で
/// `qwen-local/qwen3.8-27b` のまま、proxy では `qwen3.8-27b`。別の model でも接頭辞は行のものを引き継ぐ。
#[test]
fn self_host_acp_pi_and_proxy_agree_on_the_wire_prefix() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("claude-accounts")).unwrap();
    std::fs::create_dir_all(dir.path().join("codex-accounts")).unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, config_text(dir.path())).unwrap();
    let config = Config::load(&path).unwrap();
    let mut dispatcher = celeris::build_dispatcher(&config, Default::default()).unwrap();
    let (tx, _) = tokio::sync::watch::channel(None);
    dispatcher.set_snapshot_publisher(SnapshotPublisher {
        tx,
        instance_id: "test".into(),
        hostname: "test".into(),
        started_at: String::new(),
        tick_ms: 1000,
        providers: celeris::provider_lives(&config),
        provider_checks: Default::default(),
    });
    let store = Arc::new(SqliteStore::open(&config.db.path).unwrap());
    let qwen = CatalogSource::new("openai-compatible:qwen");
    let og = CatalogSource::new("opencode-go");
    store
        .model_catalog_apply(&qwen, &models(&["qwen3.8-27b", "qwen3.9-32b"]), 10)
        .unwrap();
    store
        .model_catalog_apply(&og, &models(&["glm-5", "grok-4.6"]), 10)
        .unwrap();

    // 割り当て前: 行の config の model で走る（binding 無し）。proxy・routing catalog は config の値。
    assert_eq!(
        dispatcher.effective_lane_model("qwen-acp", Tier::Cheap),
        Ok(None)
    );
    assert_eq!(
        dispatcher.effective_lane_model("qwen-pi", Tier::Cheap),
        Ok(None)
    );

    let check = |assigned: &str, wire: &str| {
        let view = store.model_role_assignment_view().unwrap();
        // 1. dispatcher（run が受け取る model）。ACP 行も Pi 行も `qwen-local/<id>`。
        for provider in ["qwen-acp", "qwen-pi"] {
            assert_eq!(
                dispatcher.effective_lane_model(provider, Tier::Cheap),
                Ok(Some(wire.to_string())),
                "{provider} {assigned}"
            );
        }
        // 2. llm-proxy: 上流へは素の model_id。
        let proxy = normalize_legacy_config_with(&config.llm_proxy, &view);
        assert_eq!(
            proxy_wire(&proxy, "openai-compatible:qwen", Tier::Cheap).as_deref(),
            Some(assigned)
        );
        // 3. routing catalog: provider の deployment は接頭辞付き、proxy の deployment は素の model_id。
        let mut routing = config.routing_catalog().unwrap();
        apply_role_assignments(&mut routing, &view, &config.provider_lane_seeds());
        let dropped = apply_model_catalog(
            &mut routing,
            &store.model_catalog_list().unwrap(),
            &store.model_catalog_overrides().unwrap(),
        );
        assert!(dropped.is_empty(), "{dropped:?}");
        for id in ["provider:qwen-acp/cheap", "provider:qwen-pi/cheap"] {
            assert_eq!(
                routing_dep(&routing, id).map(|d| d.upstream_model.as_str()),
                Some(wire),
                "{id} {assigned}"
            );
        }
        assert_eq!(
            routing_dep(&routing, "legacy:openai-compatible:qwen:Cheap")
                .map(|d| d.upstream_model.as_str()),
            Some(assigned)
        );
        routing
    };

    // 本番の形: config と同じ `qwen3.8-27b` を割り当てる → 接頭辞は落ちない。
    store
        .model_role_assignment_set(&qwen, Tier::Cheap, "qwen3.8-27b", None, "test", 30)
        .unwrap();
    check("qwen3.8-27b", "qwen-local/qwen3.8-27b");
    // 別の model でも行の接頭辞を引き継ぐ。
    store
        .model_role_assignment_set(&qwen, Tier::Cheap, "qwen3.9-32b", None, "test", 40)
        .unwrap();
    let routing = check("qwen3.9-32b", "qwen-local/qwen3.9-32b");
    assert!(
        routing
            .models
            .iter()
            .any(|m| m.id == "qwen-local/qwen3.9-32b"),
        "provider の model profile は wire の id"
    );

    // opencode go の Pi 行（`model = opencode-go/grok-4.6`、tier_models 無し）: 割り当ての `glm-5` は
    // `opencode-go/glm-5`。model も tier_models も無い acp 行（seed）も同じ名前。
    store
        .model_role_assignment_set(&og, Tier::Standard, "glm-5", None, "test", 50)
        .unwrap();
    let view = store.model_role_assignment_view().unwrap();
    assert_eq!(
        dispatcher.effective_lane_model("go-pi", Tier::Standard),
        Ok(Some("opencode-go/glm-5".into()))
    );
    assert_eq!(
        dispatcher.effective_lane_model("opencode-go", Tier::Standard),
        Ok(Some("opencode-go/glm-5".into()))
    );
    // source に割り当てがあると、binding も割り当ても無い lane は `assignment:none`（ADR 付記の既存規則）。
    assert_eq!(
        dispatcher.effective_lane_model("go-pi", Tier::Cheap),
        Err("cheap: assignment:none".into())
    );
    let mut routing = config.routing_catalog().unwrap();
    apply_role_assignments(&mut routing, &view, &config.provider_lane_seeds());
    for id in ["provider:go-pi/standard", "provider:opencode-go/standard"] {
        assert_eq!(
            routing_dep(&routing, id).map(|d| d.upstream_model.as_str()),
            Some("opencode-go/glm-5"),
            "{id}"
        );
    }
    assert_eq!(
        routing_dep(&routing, "provider:go-pi").map(|d| d.upstream_model.as_str()),
        Some("opencode-go/grok-4.6")
    );
}
