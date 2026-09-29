use super::*;

fn req(range: Option<&str>, offset: Option<u64>, length: Option<u64>) -> FileRequest {
    FileRequest {
        range: range.map(str::to_string),
        offset,
        length,
        download: false,
    }
}

#[test]
fn ulid_text_accepts_only_crockford_uppercase_26_chars() {
    assert!(is_ulid_text("01J9ZX5T3K8Q7W6V5R4P3N2M1H"));
    assert!(!is_ulid_text("01j9zx5t3k8q7w6v5r4p3n2m1h"));
    assert!(!is_ulid_text("01J9ZX5T3K8Q7W6V5R4P3N2M1I"));
    assert!(!is_ulid_text(".."));
    assert!(!is_ulid_text("01J9ZX5T3K8Q7W6V5R4P3N2M1"));
}

#[test]
fn content_type_table_is_closed() {
    let ct = |name: &str| content_type_for(Path::new(name));
    assert_eq!(ct("stdout.jsonl"), "text/plain; charset=utf-8");
    assert_eq!(ct("main.RS"), "text/plain; charset=utf-8");
    assert_eq!(ct("result.json"), "application/json");
    assert_eq!(ct("report.md"), "text/markdown; charset=utf-8");
    assert_eq!(ct("a.png"), "image/png");
    assert_eq!(ct("a.jpeg"), "image/jpeg");
    assert_eq!(ct("a.webp"), "image/webp");
    assert_eq!(ct("index.html"), "application/octet-stream");
    assert_eq!(ct("icon.svg"), "application/octet-stream");
    assert_eq!(ct("noext"), "application/octet-stream");
}

#[test]
fn content_disposition_escapes_filenames() {
    assert_eq!(
        content_disposition(false, "bench.json"),
        "inline; filename=\"bench.json\"; filename*=UTF-8''bench.json"
    );
    assert_eq!(
        content_disposition(true, "レポート \"x\".md"),
        "attachment; filename=\"____ _x_.md\"; filename*=UTF-8''%E3%83%AC%E3%83%9D%E3%83%BC%E3%83%88%20%22x%22.md"
    );
}

#[test]
fn range_and_offset_planning_follows_the_spec() {
    let ok = |r: &FileRequest, size| plan_slice(r, size).ok();
    let code = |r: &FileRequest, size| plan_slice(r, size).err().map(|p| p.code());
    assert_eq!(
        ok(&req(None, None, None), 10),
        Some(Slice {
            start: 0,
            len: 10,
            partial: false
        })
    );
    assert_eq!(
        ok(&req(Some("bytes=2-5"), None, None), 10),
        Some(Slice {
            start: 2,
            len: 4,
            partial: true
        })
    );
    assert_eq!(
        ok(&req(Some("bytes=8-"), None, None), 10),
        Some(Slice {
            start: 8,
            len: 2,
            partial: true
        })
    );
    assert_eq!(
        ok(&req(Some("bytes=5-100"), None, None), 10),
        Some(Slice {
            start: 5,
            len: 5,
            partial: true
        })
    );
    assert_eq!(
        ok(&req(Some("bytes=-3"), None, None), 10),
        Some(Slice {
            start: 7,
            len: 3,
            partial: true
        })
    );
    assert_eq!(
        code(&req(Some("bytes=10-"), None, None), 10),
        Some("range_not_satisfiable")
    );
    assert_eq!(
        code(&req(Some("bytes=0-1,4-5"), None, None), 10),
        Some("range_not_satisfiable")
    );
    assert_eq!(
        code(&req(Some("bytes=5-2"), None, None), 10),
        Some("range_not_satisfiable")
    );
    assert_eq!(
        code(&req(Some("bytes=abc"), None, None), 10),
        Some("range_not_satisfiable")
    );
    assert_eq!(
        ok(&req(Some("items=0-1"), None, None), 10),
        Some(Slice {
            start: 0,
            len: 10,
            partial: false
        })
    );
    assert_eq!(
        ok(&req(None, Some(4), None), 10),
        Some(Slice {
            start: 4,
            len: 6,
            partial: false
        })
    );
    assert_eq!(
        ok(&req(None, Some(4), Some(3)), 10),
        Some(Slice {
            start: 4,
            len: 3,
            partial: false
        })
    );
    assert_eq!(
        ok(&req(None, Some(10), None), 10),
        Some(Slice {
            start: 10,
            len: 0,
            partial: false
        })
    );
    assert_eq!(
        code(&req(None, Some(11), None), 10),
        Some("range_not_satisfiable")
    );
}

#[test]
fn range_and_offset_together_are_a_bad_request() {
    let mut headers = HeaderMap::new();
    headers.insert(header::RANGE, HeaderValue::from_static("bytes=0-1"));
    let err = FileRequest::parse(Some("offset=1"), &headers)
        .err()
        .map(|p| p.code());
    assert_eq!(err, Some("bad_request"));
}

#[test]
fn artifact_paths_that_are_not_workspace_relative_are_forbidden() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let ws = dir.path().canonicalize().unwrap_or_else(|e| panic!("{e}"));
    for bad in ["", "/etc/passwd", "../x", "artifacts/../../x"] {
        assert_eq!(
            resolve_artifact(&ws, bad).err().map(|p| p.code()),
            Some("path_forbidden"),
            "{bad}"
        );
    }
    assert_eq!(
        resolve_artifact(&ws, "artifacts/missing.txt")
            .err()
            .map(|p| p.code()),
        Some("file_not_found")
    );
}
