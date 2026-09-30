use super::*;

#[test]
fn page_paths_are_confined_to_the_docs_root() {
    assert_eq!(page_path("docs", "docs/a/b.md").expect("ok"), "docs/a/b.md");
    // 根を書かなければ根の下だと解釈する。
    assert_eq!(page_path("docs", "a/b.md").expect("ok"), "docs/a/b.md");
    assert_eq!(page_path("docs", "./a.md").expect("ok"), "docs/a.md");
    // 根が違えばその根に入る。
    assert_eq!(page_path("doc", "docs/a.md").expect("ok"), "doc/docs/a.md");
    // 根がリポジトリのルートなら何でもよい。
    assert_eq!(page_path(".", "a.md").expect("ok"), "a.md");
    for bad in ["../x.md", "/etc/x.md", "docs/../../x.md"] {
        assert_eq!(page_path("docs", bad), Err(PathError::Forbidden), "{bad}");
    }
    assert_eq!(page_path("docs", "a.txt"), Err(PathError::NotMarkdown));
    assert_eq!(page_path("docs", "  "), Err(PathError::Empty));
    // 大文字の拡張子も Markdown。
    assert_eq!(page_path("docs", "A.MD").expect("ok"), "docs/A.MD");
}

#[test]
fn relative_links_stay_inside_the_docs_root() {
    assert_eq!(
        resolve_relative("docs", "docs/a/b.md", "../c.md").as_deref(),
        Some("docs/c.md")
    );
    assert_eq!(
        resolve_relative("docs", "docs/a.md", "b.md").as_deref(),
        Some("docs/b.md")
    );
    assert_eq!(resolve_relative("docs", "docs/a.md", "../../etc.md"), None);
    assert_eq!(resolve_relative("docs", "docs/a.md", "../out.md"), None);
}

#[test]
fn slugs_fall_back_to_the_project_id() {
    assert_eq!(slugify("Pluvio PoC").as_deref(), Some("pluvio-poc"));
    assert_eq!(slugify("調査").as_deref(), None);
    assert_eq!(project_slug("調査", "01JABCDEF"), "01jabcdef");
    assert_eq!(project_slug("BenchFS Paper", "01J"), "benchfs-paper");
}

#[test]
fn front_matter_reads_title_tags_and_tasks() {
    let raw = "---\ntitle: 調べたこと\ntags: [research, fs]\ntasks:\n  - 01J1\n  - \"01J2\"\n---\n\n# 別の題名\n本文\n";
    let (front, body) = front_matter(raw);
    assert_eq!(front.title.as_deref(), Some("調べたこと"));
    assert_eq!(front.tags, vec!["research", "fs"]);
    assert_eq!(front.tasks, vec!["01J1", "01J2"]);
    assert!(body.starts_with("\n# 別の題名"), "{body:?}");
    // front matter の `title` が勝つ。
    assert_eq!(title_of(raw, "docs/x.md"), "調べたこと");
    // 無ければ 1 行目の `# `。
    assert_eq!(title_of("# 題名\n\n本文\n", "docs/x.md"), "題名");
    // それも無ければファイル名。
    assert_eq!(title_of("本文だけ\n", "docs/a/x.md"), "x.md");
    // 閉じていない `---` は front matter ではない。
    let (front, body) = front_matter("---\ntitle: x\n# 本文\n");
    assert_eq!(front, FrontMatter::default());
    assert!(body.starts_with("---"));
}

#[test]
fn merging_front_matter_keeps_what_is_there_and_adds_the_task() {
    let merged = merge_front_matter("# 答え\n\n本文\n", Some("答え"), "01J1");
    assert!(
        merged.starts_with("---\ntitle: 答え\ntasks: [01J1]\n---\n"),
        "{merged}"
    );
    assert!(merged.ends_with("# 答え\n\n本文\n"), "{merged}");
    // 既に front matter があれば混ぜる（同じタスクは 2 回足さない）。
    let again = merge_front_matter(&merged, Some("別"), "01J1");
    assert_eq!(again.matches("01J1").count(), 1, "{again}");
    assert!(again.contains("title: 答え"), "{again}");
    let two = merge_front_matter(&merged, None, "01J2");
    assert!(two.contains("tasks: [01J1, 01J2]"), "{two}");
    // `tags` は残る。
    let tagged = merge_front_matter("---\ntags: [a]\n---\n本文\n", Some("題"), "01J3");
    assert!(
        tagged.contains("tags: [a]") && tagged.contains("tasks: [01J3]"),
        "{tagged}"
    );
}

#[test]
fn rendering_drops_raw_html_and_links_tasks_and_pages() {
    let html = render(
        "# 題\n\n<script>alert(1)</script>\n\n[t](celeris:task/01J1) と [[sub/other.md]] と [[../out.md]]\n\n\
         | a | b |\n| --- | --- |\n| 1 | 2 |\n",
        "docs",
        "docs/index.md",
        "/projects/P1/docs",
    );
    assert!(!html.contains("<script"), "{html}");
    assert!(
        html.contains("alert(1)") || !html.contains("script"),
        "{html}"
    );
    assert!(html.contains("href=\"/tasks/01J1\""), "{html}");
    assert!(
        html.contains("/projects/P1/docs?path=docs/sub/other.md"),
        "{html}"
    );
    // 根の外に出る `[[…]]` はリンクにしない。
    assert!(html.contains("[[../out.md]]"), "{html}");
    assert!(html.contains("<table>"), "{html}");
}

/// ここから下は git を起こす（tempdir のリポジトリだけ。ネットワークには出ない）。
fn repo_with_docs() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo = dir.path().join("repo");
    init_docs_repo(&repo, "Pluvio PoC").expect("init");
    dir
}

#[test]
fn a_new_docs_repository_has_one_page_on_main() {
    let dir = repo_with_docs();
    let repo = dir.path().join("repo");
    assert!(branch_exists(&repo, "main"));
    assert_eq!(list(&repo, "main", "docs"), vec![INITIAL_PAGE.to_string()]);
    let raw = read_page(&repo, "main", INITIAL_PAGE).expect("page");
    assert!(raw.starts_with("# Pluvio PoC"), "{raw}");
    assert!(blob_sha(&repo, "main", INITIAL_PAGE).is_some());
    let log = history(&repo, "main", INITIAL_PAGE);
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].author, DOCS_AUTHOR_NAME);
    // 2 回目は何もしない（既にある git リポジトリ）。
    init_docs_repo(&repo, "Pluvio PoC").expect("again");
    assert_eq!(history(&repo, "main", INITIAL_PAGE).len(), 1);
}

#[test]
fn committing_a_page_checks_the_etag_and_moves_the_default_branch() {
    let dir = repo_with_docs();
    let repo = dir.path().join("repo");
    let temp = dir.path().join("tmp/w1");

    // 新規作成（etag なし）。
    let created = commit_page(
        &repo,
        "main",
        &temp,
        &PageEdit {
            path: "docs/a.md".into(),
            body: Some("# A\n".into()),
            etag: None,
            message: "docs: docs/a.md".into(),
            overwrite: false,
        },
    );
    let etag = match created {
        WriteOutcome::Written {
            etag, unchanged, ..
        } => {
            assert!(!unchanged);
            etag.expect("etag")
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(
        read_page(&repo, "main", "docs/a.md").as_deref(),
        Some("# A\n")
    );

    // 既にあるのに etag が無ければ 409。
    assert!(matches!(
        commit_page(
            &repo,
            "main",
            &dir.path().join("tmp/w2"),
            &PageEdit {
                path: "docs/a.md".into(),
                body: Some("# B\n".into()),
                etag: None,
                message: "docs: docs/a.md".into(),
                overwrite: false,
            }
        ),
        WriteOutcome::EtagMismatch { .. }
    ));
    // 違う etag も 409。
    assert!(matches!(
        commit_page(
            &repo,
            "main",
            &dir.path().join("tmp/w3"),
            &PageEdit {
                path: "docs/a.md".into(),
                body: Some("# B\n".into()),
                etag: Some("0".repeat(40)),
                message: "docs: docs/a.md".into(),
                overwrite: false,
            }
        ),
        WriteOutcome::EtagMismatch { .. }
    ));
    // 正しい etag なら通り、etag は変わる。
    let updated = commit_page(
        &repo,
        "main",
        &dir.path().join("tmp/w4"),
        &PageEdit {
            path: "docs/a.md".into(),
            body: Some("# B\n".into()),
            etag: Some(etag.clone()),
            message: "docs: docs/a.md".into(),
            overwrite: false,
        },
    );
    let next = match updated {
        WriteOutcome::Written { etag, .. } => etag.expect("etag"),
        other => panic!("{other:?}"),
    };
    assert_ne!(next, etag);
    assert_eq!(history(&repo, "main", "docs/a.md").len(), 2);

    // 削除。
    let deleted = commit_page(
        &repo,
        "main",
        &dir.path().join("tmp/w5"),
        &PageEdit {
            path: "docs/a.md".into(),
            body: None,
            etag: Some(next),
            message: "docs: remove docs/a.md".into(),
            overwrite: false,
        },
    );
    assert!(
        matches!(deleted, WriteOutcome::Written { etag: None, .. }),
        "{deleted:?}"
    );
    assert!(read_page(&repo, "main", "docs/a.md").is_none());
    // 無いものは消せない。
    assert_eq!(
        commit_page(
            &repo,
            "main",
            &dir.path().join("tmp/w6"),
            &PageEdit {
                path: "docs/a.md".into(),
                body: None,
                etag: None,
                message: "docs: remove".into(),
                overwrite: false,
            }
        ),
        WriteOutcome::Missing
    );
}

/// ADR-0043 D5 と同じ規則: 人が default_branch を編集中なら 409（何も触らない）。
#[test]
fn writing_while_the_default_branch_is_dirty_is_busy() {
    let dir = repo_with_docs();
    let repo = dir.path().join("repo");
    std::fs::write(repo.join("docs/README.md"), b"human is editing\n").expect("write");
    let outcome = commit_page(
        &repo,
        "main",
        &dir.path().join("tmp/w"),
        &PageEdit {
            path: "docs/a.md".into(),
            body: Some("# A\n".into()),
            etag: None,
            message: "docs: docs/a.md".into(),
            overwrite: false,
        },
    );
    assert!(matches!(outcome, WriteOutcome::Busy { .. }), "{outcome:?}");
    assert!(read_page(&repo, "main", "docs/a.md").is_none());
}

#[test]
fn the_tree_lists_markdown_and_grep_finds_pages() {
    let dir = repo_with_docs();
    let repo = dir.path().join("repo");
    for (path, body) in [
        ("docs/one.md", "# ひとつ\n調査の結果\n"),
        ("docs/sub/two.md", "# ふたつ\nnothing here\n"),
        ("docs/not-a-page.txt", "調査\n"),
    ] {
        let outcome = commit_page(
            &repo,
            "main",
            &dir.path().join(format!("tmp/{}", path.replace('/', "-"))),
            &PageEdit {
                path: path.into(),
                body: Some(body.into()),
                etag: None,
                message: format!("docs: {path}"),
                overwrite: false,
            },
        );
        assert!(
            matches!(outcome, WriteOutcome::Written { .. }),
            "{path}: {outcome:?}"
        );
    }
    let mut paths = list(&repo, "main", "docs");
    paths.sort();
    assert_eq!(
        paths,
        vec!["docs/README.md", "docs/one.md", "docs/sub/two.md"]
    );
    assert_eq!(grep(&repo, "main", "docs", "調査"), vec!["docs/one.md"]);
    assert!(grep(&repo, "main", "docs", "見つからない").is_empty());
    let last = last_commits(&repo, "main", "docs");
    assert_eq!(
        last.get("docs/one.md").map(|c| c.subject.as_str()),
        Some("docs: docs/one.md")
    );
    assert!(last.get("docs/one.md").is_some_and(|c| c.at.contains('T')));
}
