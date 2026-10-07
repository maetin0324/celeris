use super::*;
use crate::Tier;
use crate::model::Event;
use crate::model_catalog::assignments::{AssignmentState, RoleAssignmentReader};
use crate::{ModelCatalogStore, SqliteStore, TaskStore};

fn models(ids: &[&str]) -> Vec<DiscoveredModel> {
    ids.iter().map(|id| DiscoveredModel::new(*id)).collect()
}

#[test]
fn openai_list_parses_ids_and_display_names() {
    let body = r#"{"object":"list","data":[
        {"id":"glm-5","object":"model"},
        {"id":"claude-x","display_name":"Claude X"},
        {"object":"model"},
        {"id":"glm-5"},
        {"id":""}]}"#;
    let got = parse_openai_models_list(body).unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].model_id, "glm-5");
    assert_eq!(got[0].display_name, None);
    assert_eq!(got[1].display_name.as_deref(), Some("Claude X"));
    assert!(parse_openai_models_list("not json").is_err());
    assert!(parse_openai_models_list(r#"{"models":[]}"#).is_err());
}

#[test]
fn opencode_stdout_strips_prefix_and_ignores_other_lines() {
    let out = "INFO  starting\nopencode-go/glm-5\nopencode-go/kimi-k2.5\nopenai/gpt-5\n\n  opencode-go/minimax-m2.5  \nopencode-go/glm-5\nopencode-go/ has space\n";
    let got = parse_opencode_models_stdout(out, "opencode-go");
    let ids: Vec<&str> = got.iter().map(|m| m.model_id.as_str()).collect();
    assert_eq!(ids, ["glm-5", "kimi-k2.5", "minimax-m2.5"]);
}

#[test]
fn codex_page_parses_with_cursor_and_skips_hidden() {
    let page = serde_json::json!({
        "data": [
            {"id": "gpt-5.1-codex", "model": "gpt-5.1-codex", "displayName": "GPT-5.1 Codex", "hidden": false},
            {"model": "gpt-5-mini", "displayName": "Mini"},
            {"id": "internal", "hidden": true}
        ],
        "nextCursor": "page2"
    });
    let (got, next) = parse_codex_model_list(&page).unwrap();
    assert_eq!(next.as_deref(), Some("page2"));
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].model_id, "gpt-5.1-codex");
    assert_eq!(got[0].display_name.as_deref(), Some("GPT-5.1 Codex"));
    assert_eq!(got[1].model_id, "gpt-5-mini");
    let (_, last) =
        parse_codex_model_list(&serde_json::json!({"data": [], "nextCursor": null})).unwrap();
    assert_eq!(last, None);
    assert!(parse_codex_model_list(&serde_json::json!({})).is_err());
}

#[test]
fn source_validity() {
    assert!(CatalogSource::new("claude-oauth").is_valid());
    assert!(CatalogSource::openai_compatible("qwen").is_valid());
    assert!(!CatalogSource::new("openai-compatible:").is_valid());
    assert!(!CatalogSource::new("bogus").is_valid());
}

#[test]
fn apply_marks_removed_then_restored_and_keeps_override() {
    let store = SqliteStore::open_in_memory().unwrap();
    let src = CatalogSource::new("opencode-go");

    let d1 = store
        .model_catalog_apply(&src, &models(&["a", "b"]), 100)
        .unwrap();
    assert_eq!(d1.added, ["a", "b"]);
    assert!(d1.removed.is_empty() && d1.restored.is_empty());

    let ov = CatalogOverride {
        disabled: true,
        tier: Some(crate::Tier::Cheap),
        alias: Some("fast".into()),
        note: Some("n".into()),
    };
    store
        .model_catalog_set_override(&src, "b", &ov, 150)
        .unwrap();

    // b が消える。
    let d2 = store
        .model_catalog_apply(&src, &models(&["a"]), 200)
        .unwrap();
    assert_eq!(d2.removed, ["b"]);
    assert!(d2.added.is_empty());
    let list = store.model_catalog_list().unwrap();
    let b = list.iter().find(|e| e.model_id == "b").unwrap();
    assert!(!b.available);
    assert_eq!(b.first_seen, 100);
    assert_eq!(b.last_seen, 100);
    let a = list.iter().find(|e| e.model_id == "a").unwrap();
    assert!(a.available);
    assert_eq!(a.last_seen, 200);

    // 変化なしの反映は event を書かない。
    let events_before = store.events_since(0, 100).unwrap().len();
    let d_same = store
        .model_catalog_apply(&src, &models(&["a"]), 250)
        .unwrap();
    assert!(d_same.is_empty());
    assert_eq!(store.events_since(0, 100).unwrap().len(), events_before);

    // b が戻る。
    let d3 = store
        .model_catalog_apply(&src, &models(&["a", "b"]), 300)
        .unwrap();
    assert_eq!(d3.restored, ["b"]);
    let b = store
        .model_catalog_list()
        .unwrap()
        .into_iter()
        .find(|e| e.model_id == "b")
        .unwrap();
    assert!(b.available);

    // 上書きは反映をまたいで残る。
    let ovs = store.model_catalog_overrides().unwrap();
    assert_eq!(ovs.len(), 1);
    assert_eq!(ovs[0].value, ov);
    assert_eq!(ovs[0].updated_at, 150);

    // event は added / removed / restored の 3 回だけ、疑似 task の列に残る。
    let changed: Vec<_> = store
        .events_since(0, 100)
        .unwrap()
        .into_iter()
        .filter(|r| matches!(r.event, Event::ModelCatalogChanged { .. }))
        .collect();
    assert_eq!(changed.len(), 3);
    assert!(changed.iter().all(|r| r.task_id == catalog_event_task_id()));

    let rec = store.model_catalog_discovery_records().unwrap();
    assert_eq!(rec.len(), 1);
    assert!(rec[0].ok);
    assert_eq!(rec[0].count, 2);

    assert!(store.model_catalog_delete_override(&src, "b").unwrap());
    assert!(!store.model_catalog_delete_override(&src, "b").unwrap());
}

#[test]
fn failure_is_recorded_without_touching_catalog() {
    let store = SqliteStore::open_in_memory().unwrap();
    let src = CatalogSource::new("claude-oauth");
    store
        .model_catalog_apply(&src, &models(&["m"]), 10)
        .unwrap();
    store
        .model_catalog_record_failure(&src, "HTTP 500", 20)
        .unwrap();
    let list = store.model_catalog_list().unwrap();
    assert_eq!(list.len(), 1);
    assert!(list[0].available);
    let rec = store.model_catalog_discovery_records().unwrap();
    assert!(!rec[0].ok);
    assert_eq!(rec[0].error.as_deref(), Some("HTTP 500"));
    assert_eq!(rec[0].at, 20);
}

fn assignment_events(store: &SqliteStore) -> Vec<Event> {
    store
        .events_since(0, 100)
        .unwrap()
        .into_iter()
        .filter(|r| matches!(r.event, Event::ModelRoleAssignmentChanged { .. }))
        .map(|r| r.event)
        .collect()
}

/// ADR 2026-10-06 model-role-assignments D1/D6: set は upsert で event を残し、delete は無ければ false。
#[test]
fn role_assignment_set_and_delete_append_events() {
    let store = SqliteStore::open_in_memory().unwrap();
    let src = CatalogSource::new("opencode-go");
    let a = store
        .model_role_assignment_set(&src, Tier::Cheap, "glm-5", Some("n"), "admin", 10)
        .unwrap();
    assert_eq!(a.model_id, "glm-5");
    store
        .model_role_assignment_set(&src, Tier::Cheap, "kimi", None, "admin", 20)
        .unwrap();
    let rows = store.model_role_assignments().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].model_id, "kimi");
    assert_eq!(rows[0].note, None);
    assert_eq!(rows[0].updated_at, 20);

    assert!(
        store
            .model_role_assignment_delete(&src, Tier::Cheap, "admin", 30)
            .unwrap()
    );
    assert!(
        !store
            .model_role_assignment_delete(&src, Tier::Cheap, "admin", 31)
            .unwrap()
    );
    assert!(store.model_role_assignments().unwrap().is_empty());

    let events = assignment_events(&store);
    assert_eq!(events.len(), 3);
    let tuple = |e: &Event| match e {
        Event::ModelRoleAssignmentChanged {
            model_id, previous, ..
        } => (model_id.clone(), previous.clone()),
        _ => (None, None),
    };
    assert_eq!(tuple(&events[0]), (Some("glm-5".into()), None));
    assert_eq!(
        tuple(&events[1]),
        (Some("kimi".into()), Some("glm-5".into()))
    );
    assert_eq!(tuple(&events[2]), (None, Some("kimi".into())));
}

/// catalog の反映・上書きの変更は割り当て表を触らない。view は実効の状態を返す。
#[test]
fn role_assignments_survive_catalog_changes_and_view_reflects_them() {
    let store = SqliteStore::open_in_memory().unwrap();
    let src = CatalogSource::new("opencode-go");
    store
        .model_catalog_apply(&src, &models(&["a", "b"]), 10)
        .unwrap();
    store
        .model_role_assignment_set(&src, Tier::Frontier, "a", None, "admin", 11)
        .unwrap();
    store
        .model_role_assignment_set(&src, Tier::Cheap, "b", None, "admin", 11)
        .unwrap();
    // a が消え、b は disabled になる。
    store
        .model_catalog_apply(&src, &models(&["b"]), 20)
        .unwrap();
    let ov = CatalogOverride {
        disabled: true,
        ..CatalogOverride::default()
    };
    store
        .model_catalog_set_override(&src, "b", &ov, 21)
        .unwrap();
    assert_eq!(store.model_role_assignments().unwrap().len(), 2);
    let view = store.model_role_assignment_view().unwrap();
    assert_eq!(
        view.get("opencode-go", Tier::Frontier).map(|a| a.state),
        Some(AssignmentState::Excluded {
            reason: "catalog:unavailable"
        })
    );
    assert_eq!(
        view.get("opencode-go", Tier::Cheap).map(|a| a.state),
        Some(AssignmentState::Excluded {
            reason: "override:disabled"
        })
    );
    // 上書きを消しても割り当ては残り、b は Assigned に戻る。
    store.model_catalog_delete_override(&src, "b").unwrap();
    assert_eq!(store.model_role_assignments().unwrap().len(), 2);
    let reader: &dyn RoleAssignmentReader = &store;
    let view = reader.assignment_view().unwrap();
    assert_eq!(
        view.get("opencode-go", Tier::Cheap).map(|a| a.state),
        Some(AssignmentState::Assigned)
    );
}

#[test]
fn role_members_multiple_roles_priority_and_explicit_empty_survive_restart() {
    use crate::model_catalog::assignments::{RoleMember, apply_to_bindings};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("roles.db");
    let src = CatalogSource::new("opencode-go");
    let member = |id: &str, priority| RoleMember {
        source: src.clone(),
        model_id: id.into(),
        priority,
    };
    {
        let store = SqliteStore::open(&path).unwrap();
        store
            .model_catalog_apply(&src, &models(&["a", "b", "c"]), 1)
            .unwrap();
        store
            .model_role_members_replace(
                Tier::Standard,
                &[member("a", 9), member("b", 1), member("c", 2)],
                &[src.clone()],
                "admin",
                2,
            )
            .unwrap();
        store
            .model_role_members_replace(
                Tier::Frontier,
                &[member("a", 0)],
                &[src.clone()],
                "admin",
                3,
            )
            .unwrap();
        let view = store.model_role_assignment_view().unwrap();
        assert_eq!(
            view.get(src.as_str(), Tier::Standard).unwrap().model_id,
            "b"
        );
        assert_eq!(
            view.get(src.as_str(), Tier::Frontier).unwrap().model_id,
            "a"
        );
        // Missing and disabled models remain members but cannot be chosen.
        store
            .model_catalog_apply(&src, &models(&["a", "c"]), 4)
            .unwrap();
        store
            .model_catalog_set_override(
                &src,
                "c",
                &CatalogOverride {
                    disabled: true,
                    ..Default::default()
                },
                5,
            )
            .unwrap();
        let view = store.model_role_assignment_view().unwrap();
        assert_eq!(view.members(src.as_str(), Tier::Standard).len(), 3);
        assert_eq!(
            view.get(src.as_str(), Tier::Standard).unwrap().model_id,
            "a"
        );
        // Invalid batch rolls back without changing any membership.
        assert!(
            store
                .model_role_members_replace(
                    Tier::Standard,
                    &[member("a", 1), member("a", 2)],
                    &[src.clone()],
                    "admin",
                    6
                )
                .is_err()
        );
        assert_eq!(store.model_role_assignment_view().unwrap(), view);
        store
            .model_role_members_replace(Tier::Standard, &[], &[src.clone()], "admin", 7)
            .unwrap();
    }
    let store = SqliteStore::open(&path).unwrap();
    let view = store.model_role_assignment_view().unwrap();
    assert!(view.manages(src.as_str(), Tier::Standard));
    assert!(view.members(src.as_str(), Tier::Standard).is_empty());
    let configured = [(
        Tier::Standard,
        crate::model_routing::ModelBinding {
            name: "legacy".into(),
            model_id: Some("legacy".into()),
            unavailable_reason: None,
            reasoning_effort: None,
        },
    )]
    .into_iter()
    .collect();
    let effective = apply_to_bindings(
        &configured,
        src.as_str(),
        &[Tier::Standard],
        crate::model_catalog::assignments::WireRule::proxy(src.as_str()),
        &view,
    );
    assert!(crate::model_routing::resolve(&effective, Tier::Standard).is_err());
    assert_eq!(
        view.get(src.as_str(), Tier::Frontier).unwrap().model_id,
        "a"
    );
}

#[test]
fn role_members_migration_preserves_v1_assignment_and_metadata() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(include_str!(
        "../../migrations/0053_model_role_assignments.sql"
    ))
    .unwrap();
    conn.execute("INSERT INTO model_role_assignments VALUES ('opencode-go', 'standard', 'old', 'keep', 12, 'admin')", []).unwrap();
    conn.execute_batch(include_str!(
        "../../migrations/0054_model_role_memberships.sql"
    ))
    .unwrap();
    let row: (String, u32, String, i64, String) = conn
        .query_row(
            "SELECT model_id, priority, note, updated_at, updated_by FROM model_role_assignments",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(row, ("old".into(), 0, "keep".into(), 12, "admin".into()));
    conn.execute("INSERT INTO model_role_assignments VALUES ('opencode-go', 'standard', 'second', 1, NULL, 13, 'admin')", []).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM model_role_scopes", [], |r| r
            .get::<_, u32>(0))
            .unwrap(),
        1
    );
}
