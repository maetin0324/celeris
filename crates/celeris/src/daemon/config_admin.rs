//! ADR 2026-10-22-dynamic-config: `GET/PATCH /config/fields` の daemon 側。欄の一覧（分類・稼働中の値・再起動待ち）、
//! 変更の検証 → config.toml への書き戻し → 稼働中の daemon への反映 → `config_events` への追記。

use serde_json::Value;
use task_api::{
    ConfigAdminError, ConfigChangeView, ConfigFieldView, ConfigFieldsView, ConfigPatchChange,
    ConfigPatchResult, ConfigRuleView,
};
use task_core::NewConfigEvent;
use task_dispatch::Dispatcher;

use super::admin::apply_config;
use crate::Config;
use crate::config::edit;
use crate::config::fields::{self, FieldClass, display_value, is_secret_key, rule_for};

fn source_path(config: &Config) -> Result<std::path::PathBuf, ConfigAdminError> {
    config.source_path.clone().ok_or_else(|| {
        ConfigAdminError::Internal(
            "config was not loaded from a file; the settings screen cannot edit it".to_string(),
        )
    })
}

fn read_file(path: &std::path::Path) -> Result<String, ConfigAdminError> {
    std::fs::read_to_string(path)
        .map_err(|e| ConfigAdminError::Internal(format!("cannot read {}: {e}", path.display())))
}

/// `text` の `key` の値（table の途中でも引ける）。無ければ `None`。
fn value_at(text: &str, key: &str) -> Option<Value> {
    let table: toml::Table = toml::from_str(text).ok()?;
    let mut current = serde_json::to_value(&table).ok()?;
    for segment in key.split('.') {
        current = current.get(segment)?.clone();
    }
    Some(current)
}

/// `GET /config/fields` の本体。
pub(crate) fn config_fields(config: &Config) -> Result<ConfigFieldsView, ConfigAdminError> {
    let path = source_path(config)?;
    let file_text = read_file(&path)?;
    let now = fields::flatten(&file_text).map_err(ConfigAdminError::Internal)?;
    let started = match config.loaded_text.as_deref() {
        Some(text) => fields::flatten(text).map_err(ConfigAdminError::Internal)?,
        None => now.clone(),
    };
    let mut keys: std::collections::BTreeSet<&String> = now.keys().collect();
    keys.extend(started.keys());
    let fields = keys
        .into_iter()
        .map(|key| {
            let rule = rule_for(key);
            let value = now.get(key);
            let running = match rule.class {
                FieldClass::Dynamic => value,
                _ => started.get(key),
            };
            let pending_restart = rule.class != FieldClass::Dynamic && value != started.get(key);
            ConfigFieldView {
                key: key.clone(),
                class: rule.class.as_str().to_string(),
                apply_unit: rule.apply_unit.to_string(),
                reason: rule.reason.to_string(),
                secret: is_secret_key(key),
                pending_restart,
                value: value.map(|v| display_value(key, v)),
                running: running.map(|v| display_value(key, v)),
            }
        })
        .collect();
    let rules = fields::RULES
        .iter()
        .map(|r| ConfigRuleView {
            key: r.key.to_string(),
            class: r.class.as_str().to_string(),
            apply_unit: r.apply_unit.to_string(),
            reason: r.reason.to_string(),
        })
        .collect();
    Ok(ConfigFieldsView {
        config_path: Some(path.display().to_string()),
        fields,
        rules,
    })
}

/// `PATCH /config/fields` の本体。どこかで失敗したら config.toml も稼働中の状態も元のまま。
pub(crate) fn config_update(
    dispatcher: &mut Dispatcher,
    config: &mut Config,
    changes: Vec<ConfigPatchChange>,
    actor: &str,
) -> Result<ConfigPatchResult, ConfigAdminError> {
    // 1. 欄ごとの分類。起動時のみ・種・secret の欄が 1 つでもあれば全体を断る。
    for change in &changes {
        let rule = rule_for(&change.key);
        match rule.class {
            FieldClass::Dynamic => {}
            FieldClass::Startup => {
                return Err(ConfigAdminError::RestartRequired(format!(
                    "{} は起動時にしか決められない欄です（{}）。config.toml を書き換えて celeris を再起動すると効きます",
                    change.key, rule.reason
                )));
            }
            FieldClass::Seed => {
                return Err(ConfigAdminError::RestartRequired(format!(
                    "{} は DB が正の欄です（{}）。この画面では変えません",
                    change.key, rule.reason
                )));
            }
        }
        if is_secret_key(&change.key) {
            return Err(ConfigAdminError::Invalid(format!(
                "{} は secret の欄です。値は設定画面では扱いません（secret は既存の登録経路で入れます）",
                change.key
            )));
        }
    }
    // 2. 本文を編集し、同じ検証（Config::load_text）と反映の組み立てに通す。
    let path = source_path(config)?;
    let old_text = read_file(&path)?;
    let mut new_text = old_text.clone();
    for change in &changes {
        new_text = edit::set_field(&new_text, &change.key, &change.value)
            .map_err(ConfigAdminError::Invalid)?;
    }
    let new_config = Config::load_text(&path, &new_text)
        .map_err(|e| ConfigAdminError::Invalid(format!("invalid config: {e}")))?;
    // 3. 書き戻し → 反映。反映に落ちたら本文を戻す（何も変わらない）。
    write_atomically(&path, &new_text)?;
    if let Err(message) = apply_config(dispatcher, config, new_config) {
        if let Err(error) = write_atomically(&path, &old_text) {
            tracing::error!(
                ?error,
                "could not restore config.toml after a rejected change"
            );
        }
        return Err(ConfigAdminError::Invalid(format!(
            "invalid config: {message}"
        )));
    }
    // 4. 履歴。値は secret を伏せたもの（secret の欄はここまでに断っているので実質出ない）。
    let mut views = Vec::new();
    let mut events = Vec::new();
    for change in &changes {
        let rule = rule_for(&change.key);
        let before = value_at(&old_text, &change.key);
        let after = value_at(&new_text, &change.key);
        views.push(ConfigChangeView {
            key: change.key.clone(),
            class: rule.class.as_str().to_string(),
            apply_unit: rule.apply_unit.to_string(),
            before: before.clone(),
            after: after.clone(),
        });
        if before != after {
            events.push(NewConfigEvent {
                actor: actor.to_string(),
                key: change.key.clone(),
                class: rule.class.as_str().to_string(),
                apply_unit: rule.apply_unit.to_string(),
                before: before.unwrap_or(Value::Null),
                after: after.unwrap_or(Value::Null),
            });
        }
    }
    if let Err(error) = dispatcher
        .store()
        .config_events_append(&events, time::OffsetDateTime::now_utc())
    {
        tracing::error!(%error, "config change applied but its history could not be recorded");
        return Err(ConfigAdminError::Internal(format!(
            "the change was applied but its history could not be recorded: {error}"
        )));
    }
    tracing::info!(
        who = actor,
        op = "config_update",
        count = events.len(),
        "admin: config updated"
    );
    Ok(ConfigPatchResult { changes: views })
}

/// 同じ directory の一時 file に書いて rename する（権限は元の file のものを引き継ぐ）。
fn write_atomically(path: &std::path::Path, text: &str) -> Result<(), ConfigAdminError> {
    let tmp = path.with_extension("toml.tmp-config-edit");
    let result = (|| -> std::io::Result<()> {
        std::fs::write(&tmp, text)?;
        if let Ok(meta) = std::fs::metadata(path) {
            std::fs::set_permissions(&tmp, meta.permissions())?;
        }
        std::fs::rename(&tmp, path)
    })();
    result.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        ConfigAdminError::Internal(format!("cannot write {}: {e}", path.display()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use task_core::route_select::RoutingMode;

    const BASE: &str = r#"# 手書きのコメント
db = "celeris.sqlite3"
workspace_root = "ws"
tick_ms = 200
max_concurrency = 2

[api]
listen = "127.0.0.1:0"

[[api.tokens]]
principal = "operator:test"
token_file = "api.token"

[selfdeploy]
releases_dir = "releases"
repo = "repo"
detach = "inline"

[llm_sources.local]
kind = "fake"
accounts = ["a1"]
models = ["fake"]

[harnesses.fake]
kind = "fake"
api_formats = ["fake"]
source_kinds = ["fake"]

[route_defaults.fake]
candidates = [{ source = "local", model = "fake" }]

[harness_selection]
execution_class_defaults = {}
rules = [{ harness = "fake", required_capabilities = [] }]

[execution]
# replan の上限
max_replans = 5
"#;

    struct Rig {
        _dir: tempfile::TempDir,
        path: std::path::PathBuf,
        config: Config,
        dispatcher: Dispatcher,
    }

    fn rig() -> Rig {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("ws")).unwrap();
        std::fs::write(dir.path().join("api.token"), "tok\n").unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, BASE).unwrap();
        let config = Config::load(&path).unwrap();
        let dispatcher = crate::build_dispatcher(&config, Default::default()).unwrap();
        Rig {
            _dir: dir,
            path,
            config,
            dispatcher,
        }
    }

    fn patch(
        rig: &mut Rig,
        key: &str,
        value: Value,
    ) -> Result<ConfigPatchResult, ConfigAdminError> {
        config_update(
            &mut rig.dispatcher,
            &mut rig.config,
            vec![ConfigPatchChange {
                key: key.to_string(),
                value,
            }],
            "admin",
        )
    }

    #[test]
    fn max_replans_changes_without_restart_and_is_written_back_with_comments() {
        let mut rig = rig();
        assert_eq!(rig.dispatcher.config().execution.max_replans, 5);
        let result = patch(&mut rig, "execution.max_replans", json!(9)).unwrap();
        assert_eq!(result.changes[0].before, Some(json!(5)));
        assert_eq!(result.changes[0].after, Some(json!(9)));
        assert_eq!(result.changes[0].apply_unit, "next_dispatch");
        assert_eq!(rig.dispatcher.config().execution.max_replans, 9);
        assert_eq!(rig.config.execution.max_replans, 9);
        let text = std::fs::read_to_string(&rig.path).unwrap();
        assert!(text.contains("# 手書きのコメント") && text.contains("# replan の上限"));
        assert!(text.contains("max_replans = 9"));
        // 再起動後も残る。
        assert_eq!(Config::load(&rig.path).unwrap().execution.max_replans, 9);
        // 履歴（誰が・どの欄を・前後の値）。
        let events = rig.dispatcher.store().config_events_recent(10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].actor, "admin");
        assert_eq!(events[0].key, "execution.max_replans");
        assert_eq!(events[0].before, json!(5));
        assert_eq!(events[0].after, json!(9));
    }

    #[test]
    fn execution_class_defaults_routing_and_jev_reach_the_running_daemon() {
        let mut rig = rig();
        assert!(
            rig.dispatcher
                .llm_route_registry()
                .harness_selection
                .execution_class_defaults
                .is_empty()
        );
        patch(
            &mut rig,
            "harness_selection.execution_class_defaults.planner",
            json!(["fake"]),
        )
        .unwrap();
        assert_eq!(
            rig.dispatcher
                .llm_route_registry()
                .harness_selection
                .execution_class_defaults
                .len(),
            1
        );
        assert_eq!(rig.dispatcher.routing_mode(), RoutingMode::Fixed);
        patch(&mut rig, "routing.mode", json!("decision_assisted")).unwrap();
        assert_eq!(rig.dispatcher.routing_mode(), RoutingMode::DecisionAssisted);
        assert!(!rig.config.jev.enabled);
        patch(&mut rig, "jev.enabled", json!(true)).unwrap();
        assert!(rig.config.jev.enabled);
        patch(&mut rig, "max_concurrency", json!(4)).unwrap();
        assert_eq!(rig.dispatcher.max_concurrency(), 4);
        // 欄を消すと既定値に戻る。
        patch(&mut rig, "execution.max_replans", Value::Null).unwrap();
        assert_eq!(rig.dispatcher.config().execution.max_replans, 5);
        assert!(
            !std::fs::read_to_string(&rig.path)
                .unwrap()
                .contains("max_replans")
        );
    }

    #[test]
    fn account_max_runs_changes_without_restart_and_survives_a_config_change() {
        let mut rig = rig();
        crate::accounts_admin::set_account_limit(
            &rig.config,
            &mut rig.dispatcher,
            task_dispatch::capacity::IMPLICIT_ACCOUNT_ADAPTER,
            "local",
            Some(2),
        )
        .unwrap();
        let limit = |rig: &Rig| rig.dispatcher.implicit_account_limit("local").unwrap().0;
        assert_eq!(limit(&rig), 2);
        patch(&mut rig, "execution.max_replans", json!(6)).unwrap();
        assert_eq!(
            limit(&rig),
            2,
            "a config change keeps the stored account limit"
        );
        crate::accounts_admin::set_account_limit(
            &rig.config,
            &mut rig.dispatcher,
            task_dispatch::capacity::IMPLICIT_ACCOUNT_ADAPTER,
            "local",
            Some(3),
        )
        .unwrap();
        assert_eq!(limit(&rig), 3);
    }

    #[test]
    fn invalid_or_fixed_changes_are_rejected_and_change_nothing() {
        let mut rig = rig();
        let before = std::fs::read_to_string(&rig.path).unwrap();
        for (key, value) in [
            ("execution.max_replans", json!("many")),
            ("execution.no_such_field", json!(1)),
            ("routing.mode", json!("nope")),
            ("max_concurrency", json!(0)),
            ("a..b", json!(1)),
        ] {
            let error = patch(&mut rig, key, value).unwrap_err();
            assert!(
                matches!(
                    error,
                    ConfigAdminError::Invalid(_) | ConfigAdminError::RestartRequired(_)
                ),
                "{key}: {error:?}"
            );
        }
        // 起動時のみの欄は再起動が要る旨で断る。
        let error = patch(&mut rig, "api.listen", json!("0.0.0.0:1")).unwrap_err();
        assert!(matches!(&error, ConfigAdminError::RestartRequired(m) if m.contains("再起動")));
        let error = patch(&mut rig, "db.path", json!("/x")).unwrap_err();
        assert!(matches!(error, ConfigAdminError::RestartRequired(_)));
        // secret の欄は値を扱わない。
        let error = patch(&mut rig, "notify.discord_webhook_secret", json!("x")).unwrap_err();
        assert!(matches!(error, ConfigAdminError::Invalid(_)));
        assert_eq!(std::fs::read_to_string(&rig.path).unwrap(), before);
        assert_eq!(rig.dispatcher.config().execution.max_replans, 5);
        assert!(
            rig.dispatcher
                .store()
                .config_events_recent(10)
                .unwrap()
                .is_empty()
        );
        // 2 件のうち 1 件が不正なら、正しい方も入らない。
        let error = config_update(
            &mut rig.dispatcher,
            &mut rig.config,
            vec![
                ConfigPatchChange {
                    key: "execution.max_replans".into(),
                    value: json!(7),
                },
                ConfigPatchChange {
                    key: "execution.max_repairs".into(),
                    value: json!("x"),
                },
            ],
            "admin",
        )
        .unwrap_err();
        assert!(matches!(error, ConfigAdminError::Invalid(_)));
        assert_eq!(rig.dispatcher.config().execution.max_replans, 5);
        assert_eq!(std::fs::read_to_string(&rig.path).unwrap(), before);
    }

    #[test]
    fn fields_view_classifies_redacts_and_flags_pending_restart() {
        let mut rig = rig();
        let view = config_fields(&rig.config).unwrap();
        let field = |key: &str| view.fields.iter().find(|f| f.key == key).unwrap().clone();
        assert_eq!(field("execution.max_replans").class, "dynamic");
        assert_eq!(field("execution.max_replans").value, Some(json!(5)));
        assert_eq!(field("api.listen").class, "startup");
        assert_eq!(field("api.listen").apply_unit, "restart");
        assert!(field("api.tokens").secret);
        assert_eq!(field("api.tokens").value, Some(json!("<redacted>")));
        assert!(!view.fields.iter().any(|f| f.pending_restart));
        assert!(!view.rules.is_empty());
        // ファイルだけ書き換えた起動時のみの欄は「再起動待ち」。running は起動時の値のまま。
        let text = std::fs::read_to_string(&rig.path)
            .unwrap()
            .replace("tick_ms = 200", "tick_ms = 300");
        std::fs::write(&rig.path, text).unwrap();
        let view = config_fields(&rig.config).unwrap();
        let tick = view.fields.iter().find(|f| f.key == "tick_ms").unwrap();
        assert!(tick.pending_restart);
        assert_eq!(tick.value, Some(json!(300)));
        assert_eq!(tick.running, Some(json!(200)));
        // 動的な欄の変更後は running も新しい値。
        patch(&mut rig, "execution.max_replans", json!(8)).unwrap();
        let view = config_fields(&rig.config).unwrap();
        let f = view
            .fields
            .iter()
            .find(|f| f.key == "execution.max_replans")
            .unwrap();
        assert_eq!(
            (f.value.clone(), f.running.clone()),
            (Some(json!(8)), Some(json!(8)))
        );
    }
}
