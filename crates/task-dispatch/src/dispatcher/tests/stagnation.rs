//! ADR 2026-10-27-project-stagnation-inbox D3・D4: dispatcher が `scan_interval_secs` ごとに停滞を判定して snapshot の
//! `stagnation` を作り直す。偽の時計と一時ディレクトリの releases_dir だけで、外部ネットワークに出ない。

use super::*;

fn write(dir: &std::path::Path, name: &str, body: serde_json::Value) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(name), body.to_string()).unwrap();
}

/// verify 済みで昇格されていない release は閾値（偽の時計）で 1 件だけ出て、tick を重ねても増えず（同じ key・since）、
/// 昇格（promoted.json）で次の判定から消える。判定は間隔ごとで、`set_stagnation` の閾値が効く。
#[tokio::test]
async fn release_unpromoted_appears_at_threshold_once_and_closes_on_promotion() {
    let dir = tempfile::tempdir().unwrap();
    let releases = dir.path().join("releases");
    let verified = OffsetDateTime::now_utc() + time::Duration::minutes(5);
    let at = verified
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    write(
        &releases.join("ca6f52cb0000"),
        "manifest.json",
        serde_json::json!({"built_at": at}),
    );
    write(
        &releases.join("ca6f52cb0000"),
        "verify.json",
        serde_json::json!({"ok": true, "at": at}),
    );

    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::from_millis(0),
    });
    let mut d = dispatcher(store, adapter, 1);
    d.config.releases_dir = Some(releases.clone());
    d.set_stagnation(
        task_ops::stagnation::StagnationPolicy {
            release_unpromoted_secs: 600,
            ..Default::default()
        },
        60,
    );
    let clock = Arc::new(StdMutex::new(verified));
    d.test_now = Some(clock.clone());
    let release_items = |d: &Dispatcher| {
        d.stagnation
            .iter()
            .filter(|f| f.kind == task_ops::stagnation::StagnationKind::ReleaseUnpromoted)
            .map(|f| (f.key.clone(), f.since.clone()))
            .collect::<Vec<_>>()
    };

    d.tick().unwrap();
    *clock.lock().unwrap() = verified + time::Duration::seconds(599);
    d.tick().unwrap();
    assert!(release_items(&d).is_empty(), "not before 600 s");
    *clock.lock().unwrap() = verified + time::Duration::seconds(660);
    d.tick().unwrap();
    let first = release_items(&d);
    assert_eq!(
        first,
        vec![("release_unpromoted-selfdeploy".to_string(), at.clone())]
    );
    for s in [720, 3600, 7200] {
        *clock.lock().unwrap() = verified + time::Duration::seconds(s);
        d.tick().unwrap();
        assert_eq!(
            release_items(&d),
            first,
            "re-detection at +{s}s adds nothing"
        );
    }

    // 昇格された。間隔（60 秒）の内は前の判定のまま、過ぎれば消える。
    write(
        &releases.join("ca6f52cb0000"),
        "promoted.json",
        serde_json::json!({"promoted_at": at}),
    );
    *clock.lock().unwrap() = verified + time::Duration::seconds(7230);
    d.tick().unwrap();
    assert_eq!(release_items(&d), first);
    *clock.lock().unwrap() = verified + time::Duration::seconds(7300);
    d.tick().unwrap();
    assert!(release_items(&d).is_empty());
}
