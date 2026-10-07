use super::*;

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

#[test]
fn finds_markdown_outside_artifacts_dir_and_skips_declared_and_excluded() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    write(ws, "docs/paper/phase1/framing-candidates.md", "# framing\n");
    write(ws, "artifacts/result.json", "{}");
    write(ws, "artifacts/notes.md", "already inside artifacts/, skip");
    write(ws, "node_modules/pkg/readme.md", "skip: excluded dir");
    write(ws, "README.md", "declared already, skip");
    write(ws, "empty.md", "");

    let mut existing = HashSet::new();
    existing.insert((
        "README.md".to_string(),
        task_worker::artifact::sha256_file(&ws.join("README.md")).unwrap(),
    ));

    let found = scan_undeclared_markdown_artifacts(ws, &ws.join("artifacts"), &existing);
    let paths: Vec<&str> = found.iter().map(|a| a.path.as_str()).collect();
    assert_eq!(paths, vec!["docs/paper/phase1/framing-candidates.md"]);
    assert!(!found[0].declared);
    assert_eq!(found[0].name, "framing-candidates.md");
    assert_eq!(found[0].kind, "md");
}

#[test]
fn caps_at_max_files() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    for i in 0..(MAX_FILES + 5) {
        write(ws, &format!("doc-{i:02}.md"), "x");
    }
    let found = scan_undeclared_markdown_artifacts(ws, &ws.join("artifacts"), &HashSet::new());
    assert_eq!(found.len(), MAX_FILES);
}

#[test]
fn skips_files_over_the_byte_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    write(ws, "big.md", &"x".repeat((MAX_BYTES + 1) as usize));
    write(ws, "small.md", "ok");
    let found = scan_undeclared_markdown_artifacts(ws, &ws.join("artifacts"), &HashSet::new());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, "small.md");
}

// ---- ADR-0074 D6.3（Phase F1 (j)）: git worktree の Task の走査 ----

/// git worktree の Task で `artifacts/report.md` が未申告成果物として拾われる。リポジトリ全体
/// （worktree のコード）は走査しない（成果物ディレクトリの外は見ない）。
#[test]
fn git_worktree_task_registers_report_md_as_an_artifact() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    write(ws, "artifacts/report.md", "# report\n");
    write(ws, "artifacts/result.json", "{}"); // 機械的なファイル、拾わない
    write(ws, "artifacts/checkpoint.json", "{}");
    write(ws, "src/lib.rs", "fn main() {}"); // worktree のコード、拾わない（成果物置き場の外）
    write(ws, "README.md", "repo readme, not an artifact"); // 同上

    let found = scan_undeclared_artifacts_in_dir(ws, &ws.join("artifacts"), &HashSet::new());
    let paths: Vec<&str> = found.iter().map(|a| a.path.as_str()).collect();
    assert_eq!(paths, vec!["artifacts/report.md"], "{found:?}");
    assert!(!found[0].declared);
    assert_eq!(found[0].kind, "md");
}

/// md 以外の人が読む拡張子（html/pdf/csv/png）も拾う。
#[test]
fn git_worktree_task_registers_other_human_readable_extensions() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    write(ws, "artifacts/dashboard.html", "<html></html>");
    write(ws, "artifacts/data.csv", "a,b\n1,2\n");
    write(ws, "artifacts/notes.txt", "not a tracked extension");

    let found = scan_undeclared_artifacts_in_dir(ws, &ws.join("artifacts"), &HashSet::new());
    let paths: Vec<&str> = found.iter().map(|a| a.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["artifacts/dashboard.html", "artifacts/data.csv"],
        "{found:?}"
    );
}

/// 既に `Event::ArtifactProduced` で登録済みのパスは拾わない。
#[test]
fn git_worktree_task_skips_already_declared_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    write(ws, "artifacts/report.md", "# report\n");
    let mut existing = HashSet::new();
    existing.insert((
        "artifacts/report.md".to_string(),
        task_worker::artifact::sha256_file(&ws.join("artifacts/report.md")).unwrap(),
    ));
    let found = scan_undeclared_artifacts_in_dir(ws, &ws.join("artifacts"), &existing);
    assert!(found.is_empty(), "{found:?}");
}

// ---- ADR-0067 付記 2026-10-07 ----

/// D3-b: 同じ path でも中身が変わっていれば新しい版として拾う（`(path, sha256)` の重複規則）。
/// 全体 `*.md` 走査も同じ規則。
#[test]
fn a_changed_file_at_a_registered_path_is_a_new_version() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    write(ws, "artifacts/report.md", "# report v2\n");
    write(ws, "notes.md", "# notes v2\n");
    let mut existing = HashSet::new();
    existing.insert(("artifacts/report.md".to_string(), "0".repeat(64)));
    existing.insert(("notes.md".to_string(), "0".repeat(64)));

    let found = scan_undeclared_artifacts_in_dir(ws, &ws.join("artifacts"), &existing);
    let paths: Vec<&str> = found.iter().map(|a| a.path.as_str()).collect();
    assert_eq!(paths, vec!["artifacts/report.md"], "{found:?}");

    let found = scan_undeclared_markdown_artifacts(ws, &ws.join("artifacts"), &existing);
    let paths: Vec<&str> = found.iter().map(|a| a.path.as_str()).collect();
    assert_eq!(paths, vec!["notes.md"], "{found:?}");
}

/// D3-b: `registered_keys` は申告済み・未申告どちらの `ArtifactProduced` も鍵にし、sha256 は小文字で比べる。
#[test]
fn registered_keys_cover_declared_and_undeclared_artifacts() {
    let mk = |path: &str, sha: &str, declared: bool| Event::ArtifactProduced {
        run_id: "r".into(),
        artifact: ArtifactRef {
            name: "x".into(),
            path: path.into(),
            sha256: sha.into(),
            kind: "md".into(),
            declared,
        },
    };
    let events = vec![
        mk("artifacts/a.md", "ABCD", true),
        mk("artifacts/b.md", "ef01", false),
        Event::WorkspacePruned { removed: vec![] },
    ];
    let keys = registered_keys(events.iter());
    assert!(keys.contains(&("artifacts/a.md".to_string(), "abcd".to_string())));
    assert!(keys.contains(&("artifacts/b.md".to_string(), "ef01".to_string())));
    assert_eq!(keys.len(), 2);
    assert!(is_registered(&keys, "artifacts/a.md", "abcd"));
    assert!(is_registered(&keys, "artifacts/a.md", "ABCD"));
    assert!(!is_registered(&keys, "artifacts/a.md", "ffff"));
}

/// D3-a: `Done` / `Question` / `Waiting` は走る。`Yielded` / `BudgetExhausted` / `Error` は走らない。
#[test]
fn terminal_wants_scan_for_done_question_and_waiting_only() {
    assert!(terminal_wants_scan(&Terminal::Done {
        summary: "ok".into(),
        evidence: vec![],
        usage: None,
    }));
    assert!(terminal_wants_scan(&Terminal::Question {
        text: "q".into()
    }));
    assert!(terminal_wants_scan(&Terminal::Waiting {
        request: task_core::cluster_job::ClusterJobWaitRequest {
            cluster: Some("sirius".into()),
            scheduler: task_core::cluster_job::ClusterScheduler::Pbs,
            jobs: vec!["1".into()],
            poll_secs: None,
            timeout_secs: None,
            summary: "submitted".into(),
        },
        checkpoint: None,
        usage: None,
    }));
    assert!(!terminal_wants_scan(&Terminal::Yielded {
        checkpoint: serde_json::Value::Null,
        usage: None,
    }));
    assert!(!terminal_wants_scan(&Terminal::BudgetExhausted {
        kind: task_core::BudgetKind::Turns,
        message: "m".into(),
        usage: None,
    }));
    assert!(!terminal_wants_scan(&Terminal::Error {
        message: "e".into(),
        retryable: false,
    }));
}

/// D3-c: svg と json も拾い、celeris 自身が書く json（delegate / followups / plan / review / knowledge-candidates）は
/// 拾わない。順序は拡張子のクラス（md → html → pdf → csv → png → svg → json）→ 深さ → path。
#[test]
fn in_dir_scan_orders_by_class_then_depth_then_path_and_skips_machine_files() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    write(ws, "artifacts/final/F1.png", "png");
    write(ws, "artifacts/final/F1.svg", "svg");
    write(ws, "artifacts/final/manifest.json", "{}");
    write(ws, "artifacts/final/report.md", "# r");
    write(ws, "artifacts/zz.md", "# zz");
    write(ws, "artifacts/aa.csv", "a,b");
    write(ws, "artifacts/delegate.json", "{}");
    write(ws, "artifacts/followups.json", "{}");
    write(ws, "artifacts/plan.json", "{}");
    write(ws, "artifacts/review.json", "{}");
    write(ws, "artifacts/knowledge-candidates.json", "{}");
    write(ws, "artifacts/execution-plan-v3.json", "{}");
    write(ws, "artifacts/project-plan.json", "{}");
    write(ws, "artifacts/result.json", "{}");
    write(ws, "artifacts/notes.txt", "txt");

    let found = scan_undeclared_artifacts_in_dir(ws, &ws.join("artifacts"), &HashSet::new());
    let paths: Vec<&str> = found.iter().map(|a| a.path.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            "artifacts/zz.md",
            "artifacts/final/report.md",
            "artifacts/aa.csv",
            "artifacts/final/F1.png",
            "artifacts/final/F1.svg",
            "artifacts/final/manifest.json",
        ],
        "{found:?}"
    );
    let kinds: Vec<&str> = found.iter().map(|a| a.kind.as_str()).collect();
    assert_eq!(kinds, vec!["md", "md", "csv", "png", "svg", "json"]);
}

/// D3-c: 深さ `MAX_DEPTH_IN_DIR` を超える file（job の raw）は見ない。上限は `MAX_FILES_IN_DIR` 件で、
/// 登録済みは数えない（次の走査で続きから登録される）。
#[test]
fn in_dir_scan_respects_depth_and_count_limits() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    write(ws, "artifacts/a/b/c/deep-ok.md", "depth 4");
    write(ws, "artifacts/a/b/c/d/too-deep.md", "depth 5");
    write(
        ws,
        "artifacts/cmp4/run2/runs/43451/cells/benchfs/rep1/w1a/cell.json",
        "{}",
    );
    for i in 0..(MAX_FILES_IN_DIR + 3) {
        write(ws, &format!("artifacts/doc-{i:03}.md"), "x");
    }

    let found = scan_undeclared_artifacts_in_dir(ws, &ws.join("artifacts"), &HashSet::new());
    assert_eq!(found.len(), MAX_FILES_IN_DIR, "{found:?}");
    assert!(found.iter().all(|a| a.path.starts_with("artifacts/doc-")));
    assert!(!found.iter().any(|a| a.path.contains("too-deep")));
    assert!(!found.iter().any(|a| a.path.contains("cell.json")));

    // 1 回目の分を登録済みにすると、2 回目は残り（深さ 4 の md を含む）が出る。
    let existing: HashSet<ArtifactKey> = found
        .iter()
        .map(|a| (a.path.clone(), a.sha256.clone()))
        .collect();
    let rest = scan_undeclared_artifacts_in_dir(ws, &ws.join("artifacts"), &existing);
    let paths: Vec<&str> = rest.iter().map(|a| a.path.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            "artifacts/doc-064.md",
            "artifacts/doc-065.md",
            "artifacts/doc-066.md",
            "artifacts/a/b/c/deep-ok.md",
        ],
        "{rest:?}"
    );
}

// ---- ADR-0067 付記 2026-10-07 D3-d: 既存 task の補完 ----

mod backfill_tests {
    use super::*;
    use crate::undeclared_artifacts::backfill::{backfill_task, default_targets, latest_run_id};
    use task_core::{
        Budget, Check, Criterion, SqliteStore, Task, TaskId, TaskKind, TaskStore, WorkerHint,
        WorkspaceSpec,
    };
    use time::OffsetDateTime;

    fn task(workspace: WorkspaceSpec) -> Task {
        let now = OffsetDateTime::now_utc();
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
            status: task_core::Status::Blocked,
            priority: 0,
            worker_hint: WorkerHint {
                tier: task_core::Tier::Standard,
                adapter: None,
            },
            workspace,
            budget: Budget {
                max_turns: 1,
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
            labels: Vec::new(),
            category: Default::default(),
        }
    }

    fn remote() -> WorkspaceSpec {
        WorkspaceSpec::Remote {
            cluster: "sirius".into(),
            path: PathBuf::from("/work/NBB/x"),
            mode: None,
        }
    }

    fn run_row(task_id: TaskId, run_id: &str, started_at: &str) -> task_core::RunRow {
        task_core::RunRow {
            run_id: run_id.to_string(),
            task_id: task_id.to_string(),
            work_unit_id: None,
            role: task_core::RunIndexRole::Worker,
            seq: 1,
            status: task_core::RunIndexStatus::Question,
            adapter: Some("claude-code".into()),
            model: None,
            account: None,
            session_id: None,
            checkpoint: None,
            usage: None,
            metrics: None,
            started_at: started_at.to_string(),
            finished_at: None,
        }
    }

    fn produced(store: &dyn TaskStore, id: TaskId) -> Vec<(String, ArtifactRef)> {
        store
            .events_for(id)
            .unwrap()
            .into_iter()
            .filter_map(|(_, ev)| match ev {
                Event::ArtifactProduced { run_id, artifact } => Some((run_id, artifact)),
                _ => None,
            })
            .collect()
    }

    /// remote の task の写し（`<root>/<task_id>/artifacts/`）から未登録の成果物を拾い、最後の run の id で登録する。
    /// `--dry-run` は登録しない。2 回目は増えない。中身が変われば新しい版。
    #[test]
    fn backfill_registers_mirror_artifacts_once_with_the_latest_run_id() {
        let store = SqliteStore::open_in_memory().unwrap();
        let root = tempfile::tempdir().unwrap();
        let t = task(remote());
        store.insert(&t).unwrap();
        store
            .run_index_start(run_row(t.id, "run-old", "2026-10-06T00:00:00Z"))
            .unwrap();
        store
            .run_index_start(run_row(t.id, "run-new", "2026-10-07T00:00:00Z"))
            .unwrap();
        let mirror = root.path().join(t.id.to_string());
        write(&mirror, "artifacts/cmp4/final/report.md", "# report\n");
        write(&mirror, "artifacts/cmp4/final/F1-bandwidth.png", "png");
        write(&mirror, "artifacts/cmp4/final/tables.md", "| a |\n");
        write(&mirror, "artifacts/checkpoint.json", "{}");
        write(
            &mirror,
            "artifacts/cmp4/run2/runs/43451/cells/benchfs/rep1/w1a/cell.json",
            "{}",
        );

        let dry = backfill_task(&store, &t, root.path(), true).unwrap();
        assert_eq!(dry.run_id, "run-new");
        let paths: Vec<&str> = dry.artifacts.iter().map(|a| a.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "artifacts/cmp4/final/report.md",
                "artifacts/cmp4/final/tables.md",
                "artifacts/cmp4/final/F1-bandwidth.png",
            ],
            "{dry:?}"
        );
        assert!(
            produced(&store, t.id).is_empty(),
            "dry-run must not register"
        );

        let first = backfill_task(&store, &t, root.path(), false).unwrap();
        assert_eq!(first.artifacts.len(), 3);
        let got = produced(&store, t.id);
        assert_eq!(got.len(), 3, "{got:?}");
        assert!(
            got.iter()
                .all(|(run_id, a)| run_id == "run-new" && !a.declared)
        );

        let again = backfill_task(&store, &t, root.path(), false).unwrap();
        assert!(again.artifacts.is_empty(), "{again:?}");
        assert_eq!(produced(&store, t.id).len(), 3);

        write(&mirror, "artifacts/cmp4/final/report.md", "# report v2\n");
        let changed = backfill_task(&store, &t, root.path(), false).unwrap();
        let paths: Vec<&str> = changed.artifacts.iter().map(|a| a.path.as_str()).collect();
        assert_eq!(paths, vec!["artifacts/cmp4/final/report.md"]);
        assert_eq!(produced(&store, t.id).len(), 4);
    }

    /// 写しが無い task は空（エラーにしない）。`runs` 索引が無ければ `WorkerStarted` の最後の run id、それも無ければ空文字。
    #[test]
    fn backfill_without_a_mirror_is_empty_and_run_id_falls_back() {
        let store = SqliteStore::open_in_memory().unwrap();
        let root = tempfile::tempdir().unwrap();
        let t = task(remote());
        store.insert(&t).unwrap();
        let report = backfill_task(&store, &t, root.path(), false).unwrap();
        assert!(report.artifacts.is_empty());
        assert_eq!(report.run_id, "");

        store
            .append_event(
                t.id,
                &Event::WorkerStarted {
                    run_id: "run-from-event".into(),
                    adapter: "claude-code".into(),
                    model: "m".into(),
                    provider: None,
                    account: None,
                    role: None,
                    task_role: None,
                },
            )
            .unwrap();
        let events: Vec<Event> = store
            .events_for(t.id)
            .unwrap()
            .into_iter()
            .map(|(_, e)| e)
            .collect();
        assert_eq!(latest_run_id(&store, t.id, &events), "run-from-event");
    }

    /// `--task` 無しの対象は remote（cluster）の task 全部（id 順）。local は含まない。
    #[test]
    fn default_targets_are_the_remote_tasks() {
        let store = SqliteStore::open_in_memory().unwrap();
        let local = task(WorkspaceSpec::Local {
            path: PathBuf::from("x"),
            mode: None,
        });
        let r1 = task(remote());
        let r2 = task(remote());
        for t in [&local, &r1, &r2] {
            store.insert(t).unwrap();
        }
        let targets = default_targets(&store).unwrap();
        let mut ids: Vec<TaskId> = targets.iter().map(|t| t.id).collect();
        let mut want = vec![r1.id, r2.id];
        want.sort_by_key(|id| id.to_string());
        assert_eq!(ids.len(), 2);
        ids.sort_by_key(|id| id.to_string());
        assert_eq!(ids, want);
    }
}
