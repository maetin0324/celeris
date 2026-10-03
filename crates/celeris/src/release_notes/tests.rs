//! ADR 2026-10-04-release-notes: 説明の生成と昇格の要約の試験。git は tempdir の中の使い捨てリポジトリだけ。

use super::*;

const T1: &str = "01JAAAAAAAAAAAAAAAAAAAAAA1";
const T2: &str = "01JAAAAAAAAAAAAAAAAAAAAAA2";

struct Repo {
    dir: tempfile::TempDir,
}

impl Repo {
    fn new() -> Self {
        let r = Self {
            dir: tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}")),
        };
        r.git(&["init", "-q", "-b", "main"]);
        r.git(&["config", "user.name", "t"]);
        r.git(&["config", "user.email", "t@example.invalid"]);
        r.git(&["config", "commit.gpgsign", "false"]);
        r
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn git(&self, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(self.path())
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn write(&self, rel: &str, body: &str) {
        let p = self.path().join(rel);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).unwrap_or_else(|e| panic!("mkdir: {e}"));
        }
        std::fs::write(p, body).unwrap_or_else(|e| panic!("write: {e}"));
    }

    /// ファイルを書いて commit し、その sha を返す。
    fn commit(&self, rel: &str, body: &str, msg: &str) -> String {
        self.write(rel, body);
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", msg]);
        self.head()
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }
}

fn input<'a>(
    repo: &'a Path,
    base: &'a str,
    sha: &'a str,
    deliveries: Option<&'a [DeliveryHead]>,
) -> NotesInput<'a> {
    NotesInput {
        repo,
        base: Some(base),
        sha,
        schema_from: Some(10),
        schema_to: Some(11),
        gate: None,
        deliveries,
        generated_at: "2026-10-04T00:00:00Z".into(),
    }
}

fn delivery(task: &str, head: &str, base: Option<&str>) -> DeliveryHead {
    DeliveryHead {
        task_id: task.into(),
        repo: "r".into(),
        branch: format!("celeris/{task}"),
        base: base.map(str::to_string),
        head: head.into(),
        reviewed_sha: None,
        merge_candidate_sha: None,
        state: "merged".into(),
        release: None,
    }
}

#[test]
fn notes_group_a_merged_task_branch_and_list_the_rest() {
    let r = Repo::new();
    r.commit("README.md", "x\n", "init");
    r.commit(
        "config/celeris.example.toml",
        "[old]\nk = 1\n",
        "add config",
    );
    let base = r.head();
    r.git(&["checkout", "-q", "-b", &format!("celeris/{T1}")]);
    r.commit("a.txt", "a\n", "task work 1");
    r.commit("b.txt", "b\n", "task work 2");
    r.git(&["checkout", "-q", "main"]);
    r.git(&[
        "merge",
        "--no-ff",
        "-q",
        "-m",
        &format!("Merge branch 'celeris/{T1}'"),
        &format!("celeris/{T1}"),
    ]);
    r.commit("direct.txt", "d\n", "direct fix");
    r.commit(
        "crates/task-core/migrations/0099_x.sql",
        "select 1;\n",
        "add migration",
    );
    r.commit(
        // 文書リンク検査に実在の ADR と見なされないよう分けて書く（一時 repo の中のファイル）。
        concat!("agent-docs/", "adr/2026-10-04-x.md"),
        "# X の決定\n\nbody\n",
        "add adr",
    );
    let sha = r.commit(
        "config/celeris.example.toml",
        "[old]\nk = 1\n\n[new]\nkey = 2\n",
        "config",
    );
    let gate = serde_json::json!({"steps":[
        {"step":"fmt","skipped":false},
        {"step":"e2e","skipped":true,"reason":"no display"}
    ]});
    let mut inp = input(r.path(), &base, &sha, Some(&[]));
    inp.gate = Some(&gate);
    let n = generate(&inp, &NoLookup).unwrap_or_else(|e| panic!("generate: {e}"));

    assert_eq!(n.tasks.len(), 1, "{:?}", n.tasks);
    assert_eq!(n.tasks[0].task_id, T1);
    assert_eq!(n.tasks[0].source, "branch");
    assert!(n.tasks[0].commits[0].subject.contains(T1));
    assert!(n.deliveries_known);
    let direct: Vec<&str> = n
        .direct_commits
        .iter()
        .map(|c| c.subject.as_str())
        .collect();
    assert_eq!(direct, ["config", "add adr", "add migration", "direct fix"]);
    assert_eq!(n.migrations.len(), 1);
    assert_eq!(n.migrations[0].status, "added");
    assert!(n.migrations[0].path.ends_with("0099_x.sql"));
    assert_eq!(n.schema.changed, Some(true));
    assert_eq!(n.adrs.len(), 1);
    assert_eq!(n.adrs[0].title.as_deref(), Some("X の決定"));
    let cfg = n
        .config_example
        .as_ref()
        .unwrap_or_else(|| panic!("config"));
    assert!(cfg.needs_review);
    assert!(cfg.added_lines.contains(&"key = 2".to_string()));
    assert_eq!(cfg.added_sections, ["[new]"]);
    assert_eq!(n.gate_skips.len(), 1);
    assert_eq!(n.gate_skips[0].step, "e2e");
    assert_eq!(n.gate_skips[0].reason, "no display");
    // first-parent: merge + 4 direct。
    assert_eq!(n.first_parent.len(), 5);
    assert_eq!(n.first_parent[0], sha);
}

#[test]
fn notes_without_a_known_base_are_empty_but_valid() {
    let r = Repo::new();
    let sha = r.commit("a", "a", "init");
    let mut inp = input(r.path(), "deadbeef", &sha, None);
    inp.base = Some("deadbeef");
    let n = generate(&inp, &NoLookup).unwrap_or_else(|e| panic!("generate: {e}"));
    assert!(n.base.is_none());
    assert!(n.tasks.is_empty() && n.direct_commits.is_empty());
    assert!(!n.deliveries_known);
}

#[test]
fn fast_forward_delivery_attributes_only_down_to_delivery_base() {
    let r = Repo::new();
    r.commit("a", "a", "init");
    let old = r.commit("old", "o", "older direct commit");
    // T1 の区間（base = old）。
    r.git(&["checkout", "-q", "-b", "work1"]);
    r.commit("t1a", "1", "t1 first");
    r.commit("t1b", "1", "t1 second");
    r.git(&["checkout", "-q", "main"]);
    r.git(&["merge", "--ff-only", "-q", "work1"]);
    let t1_head = r.head();
    // T2（子 task）の区間（base = t1_head）。
    r.git(&["checkout", "-q", "-b", "work2"]);
    let t2_head = r.commit("t2a", "2", "t2 only");
    r.git(&["checkout", "-q", "main"]);
    r.git(&["merge", "--ff-only", "-q", "work2"]);
    let sha = r.head();

    // old より古い範囲を起点にして「old は帰属しない」を見る。
    let base = r.git(&["rev-parse", "HEAD~4"]);
    let ds = vec![
        delivery(T1, &t1_head, Some(&old)),
        delivery(T2, &t2_head, Some(&t1_head)),
    ];
    let mut lookup: HashMap<String, TaskInfo> = HashMap::new();
    lookup.insert(
        T1.into(),
        TaskInfo {
            title: Some("親の題".into()),
            summary: Some("親の要約".into()),
            status: Some("done".into()),
            parent_id: None,
        },
    );
    lookup.insert(
        T2.into(),
        TaskInfo {
            title: Some("子の題".into()),
            parent_id: Some(T1.into()),
            ..TaskInfo::default()
        },
    );
    let n = generate(&input(r.path(), &base, &sha, Some(&ds)), &lookup)
        .unwrap_or_else(|e| panic!("generate: {e}"));

    assert_eq!(n.tasks.len(), 1, "child folded under parent: {:?}", n.tasks);
    let t = &n.tasks[0];
    assert_eq!(t.task_id, T1);
    assert_eq!(t.source, "delivery");
    assert_eq!(t.title.as_deref(), Some("親の題"));
    assert_eq!(t.summary.as_deref(), Some("親の要約"));
    assert_eq!(t.children.len(), 1);
    assert_eq!(t.children[0].task_id, T2);
    assert_eq!(t.children[0].title.as_deref(), Some("子の題"));
    let subjects: Vec<&str> = t.commits.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(subjects, ["t1 second", "t1 first"]);
    // old と init の側は帰属しない（直接の commit）。
    let direct: Vec<&str> = n
        .direct_commits
        .iter()
        .map(|c| c.subject.as_str())
        .collect();
    assert_eq!(direct, ["older direct commit"]);
    assert_eq!(n.first_parent.len(), 4);
}

#[test]
fn task_info_picks_the_last_done_worker_outcome() {
    let v = serde_json::json!({
        "task": {"title": "T", "status": "done", "parent_id": "P"},
        "runs": [
            {"role": "worker", "outcome": "done", "outcome_text": "first"},
            {"role": "reviewer", "outcome": "done", "outcome_text": "review"},
            {"role": "worker", "outcome": "done", "outcome_text": "  last done  "},
            {"role": "worker", "outcome": "failed", "outcome_text": "bad"},
        ]
    });
    let i = task_info_from_detail(&v);
    assert_eq!(i.title.as_deref(), Some("T"));
    assert_eq!(i.summary.as_deref(), Some("last done"));
    assert_eq!(i.status.as_deref(), Some("done"));
    assert_eq!(i.parent_id.as_deref(), Some("P"));
    let none = task_info_from_detail(&serde_json::json!({"task": {"title": "x"}}));
    assert_eq!(none.summary, None);
}

#[test]
fn task_id_in_parses_branch_forms_and_rejects_non_ulids() {
    assert_eq!(
        task_id_in(&format!("Merge branch 'celeris/{T1}'")),
        Some(T1.into())
    );
    assert_eq!(
        task_id_in(&format!("integrate celeris-wu/{T2}/drop-symlink")),
        Some(T2.into())
    );
    assert_eq!(task_id_in("celeris/not-a-ulid-at-all"), None);
    assert_eq!(
        task_id_in("celeris/01JAAAAAAAAAAAAAAAAAAAAAAI"),
        None,
        "I は ULID に無い"
    );
    assert_eq!(
        task_id_in(&format!("celeris/{T1}X")),
        None,
        "26 文字で終わらない"
    );
    assert_eq!(task_id_in("integrate wu/foo"), None);
}

// ---- 集約 ----

fn task(id: &str, title: &str, commits: &[&str]) -> ReleaseNoteTask {
    ReleaseNoteTask {
        task_id: id.into(),
        title: Some(title.into()),
        summary: None,
        status: None,
        source: "delivery".into(),
        commits: commits
            .iter()
            .map(|c| ReleaseNoteCommit {
                sha: (*c).into(),
                subject: format!("s {c}"),
            })
            .collect(),
        children: Vec::new(),
    }
}

fn notes(sha: &str, base: Option<&str>, fp: &[&str], tasks: Vec<ReleaseNoteTask>) -> ReleaseNotes {
    ReleaseNotes {
        version: 1,
        sha: sha.into(),
        sha12: sha.chars().take(12).collect(),
        base: base.map(str::to_string),
        generated_at: "2026-10-04T00:00:00Z".into(),
        first_parent: fp.iter().map(|s| (*s).into()).collect(),
        truncated: false,
        deliveries_known: true,
        tasks,
        direct_commits: Vec::new(),
        migrations: Vec::new(),
        schema: ReleaseNoteSchema {
            from: None,
            to: None,
            changed: None,
        },
        adrs: Vec::new(),
        config_example: None,
        gate_skips: Vec::new(),
    }
}

fn meta(
    sha: &str,
    built: &str,
    schema: u32,
    live_ok: Option<bool>,
    n: Option<ReleaseNotes>,
) -> ReleaseMeta {
    ReleaseMeta {
        sha12: sha.chars().take(12).collect(),
        sha: sha.into(),
        built_at: Some(built.into()),
        schema_version: Some(schema),
        live_ok,
        notes: n,
    }
}

const C: &str = "cccccccccccc";
const A: &str = "aaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbb";

#[test]
fn aggregate_dedupes_tasks_across_releases_with_the_same_base() {
    let cur = meta(C, "2026-10-01T00:00:00Z", 10, Some(true), None);
    let a = meta(
        A,
        "2026-10-02T00:00:00Z",
        10,
        Some(true),
        Some(notes(A, Some(C), &[A], vec![task(T1, "one", &[A])])),
    );
    let b = meta(
        B,
        "2026-10-03T00:00:00Z",
        11,
        Some(false),
        Some(notes(
            B,
            Some(C),
            &["b2", A],
            vec![task(T2, "two", &["b2"]), task(T1, "one", &[A])],
        )),
    );
    let all = vec![cur.clone(), a, b.clone()];
    let p = aggregate(Some(&cur), &b, &all);
    assert!(p.complete, "{:?}", p.problem);
    let ids: Vec<&str> = p.tasks.iter().map(|t| t.task_id.as_str()).collect();
    assert_eq!(ids, [T2, T1]);
    let rel: Vec<&str> = p.releases.iter().map(|r| r.sha12.as_str()).collect();
    assert_eq!(rel, [B, A]);
    assert_eq!(p.schema.from, Some(10));
    assert_eq!(p.schema.to, Some(11));
    assert_eq!(p.schema.changed, Some(true));
    assert_eq!(p.mode.as_deref(), Some("stop-start"));
    assert_eq!(p.from.as_deref(), Some(C));
    assert_eq!(p.to, B);
}

#[test]
fn aggregate_follows_the_base_chain() {
    let cur = meta(C, "2026-10-01T00:00:00Z", 10, None, None);
    let a = meta(
        A,
        "2026-10-02T00:00:00Z",
        10,
        Some(true),
        Some(notes(A, Some(C), &["a1"], vec![task(T1, "one", &["a1"])])),
    );
    let b = meta(
        B,
        "2026-10-03T00:00:00Z",
        12,
        Some(true),
        Some(notes(B, Some(A), &["b2"], vec![task(T2, "two", &["b2"])])),
    );
    let all = vec![cur.clone(), a, b.clone()];
    let p = aggregate(Some(&cur), &b, &all);
    assert!(p.complete, "{:?}", p.problem);
    let ids: Vec<&str> = p.tasks.iter().map(|t| t.task_id.as_str()).collect();
    assert_eq!(ids, [T2, T1]);
    assert_eq!(p.releases.len(), 2);
    assert_eq!(p.schema.from, Some(10));
    assert_eq!(p.schema.to, Some(12));
    assert_eq!(p.mode.as_deref(), Some("live"));
}

#[test]
fn aggregate_cuts_at_current_when_it_is_inside_the_target_first_parent() {
    let cur = meta(C, "2026-10-01T00:00:00Z", 10, None, None);
    // base は current より古い。first_parent の中に current（cccccccccccc）がある。
    let b = meta(
        B,
        "2026-10-03T00:00:00Z",
        10,
        Some(true),
        Some(notes(
            B,
            Some("0ld0ld0ld0ld"),
            &["b3", "b2", C, "old1"],
            vec![
                task(T2, "newer", &["b2"]),
                task(T1, "already in current", &["old1"]),
            ],
        )),
    );
    let all = vec![cur.clone(), b.clone()];
    let p = aggregate(Some(&cur), &b, &all);
    assert!(p.complete, "{:?}", p.problem);
    let ids: Vec<&str> = p.tasks.iter().map(|t| t.task_id.as_str()).collect();
    assert_eq!(ids, [T2]);
}

#[test]
fn aggregate_reports_incomplete_when_the_chain_cannot_be_followed() {
    let cur = meta(C, "2026-10-01T00:00:00Z", 10, None, None);
    let b = meta(
        B,
        "2026-10-03T00:00:00Z",
        10,
        None,
        Some(notes(
            B,
            Some("eeeeeeeeeeee"),
            &["b2"],
            vec![task(T2, "two", &["b2"])],
        )),
    );
    let all = vec![cur.clone(), b.clone()];
    let p = aggregate(Some(&cur), &b, &all);
    assert!(!p.complete);
    assert!(p.problem.is_some());
    assert_eq!(p.tasks.len(), 1, "対象側の分は出す");
    assert_eq!(p.mode, None);
    // notes.json 自体が無い場合。
    let nb = meta(B, "2026-10-03T00:00:00Z", 10, None, None);
    let p = aggregate(Some(&cur), &nb, &[cur.clone(), nb.clone()]);
    assert!(!p.complete);
    assert!(p.problem.unwrap_or_default().contains("notes.json"));
}

#[test]
fn aggregate_merges_child_tasks_once() {
    let cur = meta(C, "2026-10-01T00:00:00Z", 10, None, None);
    let mut parent = task(T1, "parent", &["a1"]);
    parent.children.push(ReleaseNoteChild {
        task_id: T2.into(),
        title: Some("child".into()),
    });
    let a = meta(
        A,
        "2026-10-02T00:00:00Z",
        10,
        None,
        Some(notes(A, Some(C), &["a1"], vec![parent])),
    );
    // 次のリリースでは子が単独で出ている。
    let b = meta(
        B,
        "2026-10-03T00:00:00Z",
        10,
        None,
        Some(notes(B, Some(A), &["b1"], vec![task(T2, "child", &["b1"])])),
    );
    let p = aggregate(Some(&cur), &b, &[cur.clone(), a, b.clone()]);
    assert_eq!(p.tasks.len(), 1);
    assert_eq!(p.tasks[0].task_id, T1);
    assert_eq!(p.tasks[0].children.len(), 1);
}

fn write_release(root: &Path, m: &ReleaseMeta, verify_live: Option<bool>) {
    let dir = root.join(&m.sha12);
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("mkdir: {e}"));
    let manifest = serde_json::json!({
        "sha": m.sha, "built_at": m.built_at, "schema_version": m.schema_version
    });
    std::fs::write(dir.join("manifest.json"), manifest.to_string())
        .unwrap_or_else(|e| panic!("write: {e}"));
    if let Some(l) = verify_live {
        std::fs::write(
            dir.join("verify.json"),
            format!(r#"{{"ok":true,"live_ok":{l}}}"#),
        )
        .unwrap_or_else(|e| panic!("write: {e}"));
    }
    if let Some(n) = &m.notes {
        std::fs::write(
            dir.join("notes.json"),
            serde_json::to_string(n).unwrap_or_default(),
        )
        .unwrap_or_else(|e| panic!("write: {e}"));
    }
}

#[test]
fn preview_from_dir_reads_release_dirs_and_the_current_symlink() {
    let home = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let root = home.path().join("releases");
    let cur = meta(C, "2026-10-01T00:00:00Z", 10, None, None);
    let a = meta(
        A,
        "2026-10-02T00:00:00Z",
        10,
        Some(true),
        Some(notes(A, Some(C), &["a1"], vec![task(T1, "one", &["a1"])])),
    );
    let b = meta(
        B,
        "2026-10-03T00:00:00Z",
        11,
        Some(true),
        Some(notes(B, Some(A), &["b2"], vec![task(T2, "two", &["b2"])])),
    );
    write_release(&root, &cur, None);
    write_release(&root, &a, Some(true));
    write_release(&root, &b, Some(true));
    // 壊れた notes.json は notes なしとして扱われ、落ちない。
    std::fs::create_dir_all(root.join("dddddddddddd")).unwrap_or_else(|e| panic!("mkdir: {e}"));
    std::fs::write(root.join("dddddddddddd/notes.json"), "{ broken")
        .unwrap_or_else(|e| panic!("w: {e}"));
    std::fs::create_dir_all(root.join(".build")).unwrap_or_else(|e| panic!("mkdir: {e}"));
    std::os::unix::fs::symlink(format!("releases/{C}"), home.path().join("current"))
        .unwrap_or_else(|e| panic!("symlink: {e}"));

    let p = preview_from_dir(&root, B).unwrap_or_else(|| panic!("preview"));
    assert!(p.complete, "{:?}", p.problem);
    assert_eq!(p.from.as_deref(), Some(C));
    let ids: Vec<&str> = p.tasks.iter().map(|t| t.task_id.as_str()).collect();
    assert_eq!(ids, [T2, T1]);
    assert_eq!(p.mode.as_deref(), Some("live"));
    assert_eq!(p.schema.changed, Some(true));
    assert!(preview_from_dir(&root, "ffffffffffff").is_none());
    let broken = preview_from_dir(&root, "dddddddddddd").unwrap_or_else(|| panic!("broken"));
    assert!(!broken.complete);
}

#[test]
fn markdown_renderers_contain_titles_and_schema() {
    let mut n = notes(A, Some(C), &["a1"], vec![task(T1, "題の文字列", &["a1"])]);
    n.schema = ReleaseNoteSchema {
        from: Some(10),
        to: Some(11),
        changed: Some(true),
    };
    let md = render_markdown(&n);
    assert!(md.contains("題の文字列"), "{md}");
    assert!(md.contains("schema_version 10 → 11"), "{md}");

    let cur = meta(C, "2026-10-01T00:00:00Z", 10, None, None);
    let a = meta(A, "2026-10-02T00:00:00Z", 11, Some(true), Some(n));
    let p = aggregate(Some(&cur), &a, &[cur.clone(), a.clone()]);
    let md = render_preview_markdown(&p);
    assert!(md.contains("題の文字列"), "{md}");
    assert!(md.contains("schema_version 10 → 11"), "{md}");
    assert!(md.contains("live"), "{md}");
}
