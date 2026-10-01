use super::*;

#[test]
fn secret_ids_reject_path_traversal_and_empty() {
    assert!(valid_secret_id("tavily"));
    assert!(valid_secret_id("acct-2_ok"));
    assert!(!valid_secret_id(""));
    assert!(!valid_secret_id(".hidden"));
    assert!(!valid_secret_id("../escape"));
    assert!(!valid_secret_id("a/b"));
    assert!(!valid_secret_id(&"x".repeat(65)));
}

#[test]
fn trim_secret_value_drops_trailing_newlines_only() {
    assert_eq!(trim_secret_value("abc\n"), "abc");
    assert_eq!(trim_secret_value("abc\r\n"), "abc");
    assert_eq!(trim_secret_value("abc"), "abc");
    assert_eq!(trim_secret_value(" abc \n"), " abc ");
}

#[test]
fn fingerprint_is_first_8_hex_chars_of_sha256_and_does_not_reveal_the_value() {
    let fp = fingerprint("tvly-abc123");
    assert_eq!(fp.len(), 8);
    assert!(fp.chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(fp, "tvly-abc123");
    // 同じ値は同じ指紋、違う値は違う指紋。
    assert_eq!(fingerprint("same"), fingerprint("same"));
    assert_ne!(fingerprint("a"), fingerprint("b"));
}

#[test]
fn write_then_list_round_trips_with_0600_and_trims_trailing_newline() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    write_secret_file(dir.path(), "tavily", "tvly-secret\n")
        .unwrap_or_else(|e| panic!("write: {e}"));

    let path = secret_file_path(dir.path(), "tavily");
    assert!(path.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path)
            .unwrap_or_else(|e| panic!("metadata: {e}"))
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    let items = list_secret_files(dir.path()).unwrap_or_else(|e| panic!("list: {e}"));
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "tavily");
    assert_eq!(items[0].fingerprint, fingerprint("tvly-secret"));
    assert!(!items[0].updated_at.is_empty());

    // 書き直すと（同じ id）値が置き換わる。
    write_secret_file(dir.path(), "tavily", "tvly-new").unwrap_or_else(|e| panic!("rewrite: {e}"));
    let items = list_secret_files(dir.path()).unwrap_or_else(|e| panic!("list: {e}"));
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].fingerprint, fingerprint("tvly-new"));

    // 一時ファイルは残らない。
    let entries: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap_or_else(|e| panic!("read_dir: {e}"))
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(entries, vec!["tavily".to_string()]);
}

#[test]
fn list_secret_files_skips_hidden_and_invalid_names() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    std::fs::write(dir.path().join(".hidden-tmp"), "x").unwrap_or_else(|e| panic!("write: {e}"));
    std::fs::write(dir.path().join("bad name!"), "x").unwrap_or_else(|e| panic!("write: {e}"));
    write_secret_file(dir.path(), "ok-id", "v").unwrap_or_else(|e| panic!("write: {e}"));

    let items = list_secret_files(dir.path()).unwrap_or_else(|e| panic!("list: {e}"));
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "ok-id");
}

#[test]
fn list_secret_files_missing_dir_is_empty_not_an_error() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let items = list_secret_files(&dir.path().join("nope")).unwrap_or_else(|e| panic!("list: {e}"));
    assert!(items.is_empty());
}

#[test]
fn delete_secret_file_removes_the_file() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    write_secret_file(dir.path(), "tavily", "v").unwrap_or_else(|e| panic!("write: {e}"));
    assert!(secret_file_path(dir.path(), "tavily").exists());
    delete_secret_file(dir.path(), "tavily").unwrap_or_else(|e| panic!("delete: {e}"));
    assert!(!secret_file_path(dir.path(), "tavily").exists());
    assert!(delete_secret_file(dir.path(), "tavily").is_err());
}
