use super::*;

#[test]
fn browser_ledger_release_uses_release_dir_of_current_exe() {
    let exe = Path::new("/home/u/.local/celeris/releases/0123456789ab/bin/celeris");
    assert_eq!(
        conformance_path_for(None, Some(exe), Some("0123456789ab")),
        Some(PathBuf::from(
            "/home/u/.local/celeris/releases/0123456789ab/browser/conformance.json"
        ))
    );
}

#[test]
fn browser_ledger_release_env_override_wins() {
    let exe = Path::new("/r/0123456789ab/bin/celeris");
    assert_eq!(
        conformance_path_for(
            Some(OsString::from("/tmp/x/ledger.json")),
            Some(exe),
            Some("0123456789ab")
        ),
        Some(PathBuf::from("/tmp/x/ledger.json"))
    );
    // env だけでも（--release 無しの開発 daemon）使う
    assert_eq!(
        conformance_path_for(Some(OsString::from("/tmp/x/ledger.json")), None, None),
        Some(PathBuf::from("/tmp/x/ledger.json"))
    );
}

#[test]
fn browser_ledger_release_none_without_env_or_release() {
    let exe = Path::new("/r/0123456789ab/bin/celeris");
    assert_eq!(conformance_path_for(None, Some(exe), None), None);
    // 空の env・空の release は無い扱い
    assert_eq!(
        conformance_path_for(Some(OsString::new()), Some(exe), Some("  ")),
        None
    );
    // exe が分からなければ未配置
    assert_eq!(conformance_path_for(None, None, Some("0123456789ab")), None);
}
