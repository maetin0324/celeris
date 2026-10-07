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

// ---- ADR-0067 付記 2026-10-07: remote（cluster）の task の成果物は手元の写し（`<root>/<task_id>/`）で解決する ----

fn remote_task() -> Task {
    use task_core::{
        Budget, Check, Criterion, Status, TaskId, TaskKind, Tier, WorkerHint, WorkspaceSpec,
    };
    let now = time::OffsetDateTime::now_utc();
    Task {
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "cmp4".into(),
        objective: "compare".into(),
        acceptance: vec![Criterion {
            text: "c".into(),
            check: Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
        }],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Blocked,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::Remote {
            cluster: "sirius".into(),
            path: PathBuf::from("/work/NBB/cmp4"),
            mode: None,
        },
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
        conversation: None,
    }
}

/// dispatcher / backfill が remote の写し `artifacts/...` の相対 path で登録した `ArtifactProduced` は、
/// `GET /tasks/{id}/artifacts` で `exists: true`・`sha256_matches: Some(true)` として出る。
#[test]
fn remote_task_artifacts_resolve_under_the_local_mirror() {
    let root = tempfile::tempdir().unwrap();
    let task = remote_task();
    let mirror = root.path().join(task.id.to_string());
    let final_dir = mirror.join("artifacts").join("cmp4").join("final");
    std::fs::create_dir_all(&final_dir).unwrap();
    std::fs::write(final_dir.join("report.md"), "# report\n").unwrap();
    let sha256 = task_worker::artifact::sha256_file(&final_dir.join("report.md")).unwrap();
    let rows = vec![task_core::EventRow {
        id: 1,
        task_id: task.id,
        seq: 0,
        ts: "2026-10-07T00:00:00Z".into(),
        event: task_core::Event::ArtifactProduced {
            run_id: "run-q".into(),
            artifact: task_core::ArtifactRef {
                name: "report.md".into(),
                path: "artifacts/cmp4/final/report.md".into(),
                sha256: sha256.clone(),
                kind: "md".into(),
                declared: false,
            },
        },
    }];
    let views = artifact_views(&task, root.path(), &rows);
    assert_eq!(views.len(), 1);
    assert!(views[0].exists, "{:?}", views[0]);
    assert_eq!(views[0].sha256_matches, Some(true));
    assert_eq!(views[0].run_id, "run-q");
    assert!(!views[0].artifact.declared);
}
