use super::*;

#[test]
fn front_matter_round_trips() {
    let front = FrontMatter {
        title: Some("pegasus の使い方".into()),
        tags: vec!["hpc".into(), "pegasus".into()],
        scope: Some("environment".into()),
        sources: vec!["task:01J1".into(), "human".into()],
        created: Some("2026-09-20".into()),
        updated: Some("2026-09-20".into()),
        confidence: Some(Confidence::High),
        path: None,
        op: None,
    };
    let page = render_page(&front, "# pegasus\n\npjsub で投げる。\n");
    let (again, body) = front_matter(&page);
    assert_eq!(again, front, "{page}");
    assert_eq!(body, "\n# pegasus\n\npjsub で投げる。\n");
    // 2 回目も同じ（冪等）。
    assert_eq!(render_page(&again, body), page);
    // 題名は front matter が勝つ。
    assert_eq!(
        title_of(&page, "environment/clusters/pegasus.md"),
        "pegasus の使い方"
    );
}

/// ADR-0047 D4（Phase 62）: `_inbox` 専用の `op` も往復する。
#[test]
fn front_matter_round_trips_the_op_key() {
    let front = FrontMatter {
        title: Some("退役候補".into()),
        scope: Some("environment".into()),
        sources: vec!["task:01J2".into()],
        path: Some("environment/tools/old.md".into()),
        op: Some("retire".into()),
        ..FrontMatter::default()
    };
    let page = render_page(&front, "古くなった。\n");
    assert!(page.contains("op: retire"), "{page}");
    let (again, _) = front_matter(&page);
    assert_eq!(again, front);
}

#[test]
fn front_matter_reads_block_lists_and_ignores_unclosed_blocks() {
    let raw = "---\ntitle: x\ntags:\n  - a\n  - \"b\"\nsources:\n  - url:https://e.com\nconfidence: medium\n---\n本文\n";
    let (front, body) = front_matter(raw);
    assert_eq!(front.tags, vec!["a", "b"]);
    assert_eq!(front.sources, vec!["url:https://e.com"]);
    assert_eq!(front.confidence, Some(Confidence::Medium));
    assert_eq!(body, "本文\n");
    // 閉じていない `---` は front matter ではない。
    let (front, body) = front_matter("---\ntitle: x\n# 本文\n");
    assert_eq!(front, FrontMatter::default());
    assert!(body.starts_with("---"));
    // front matter が無いページは題名を本文から取る。
    assert_eq!(title_of("# 題名\n", "user/profile.md"), "題名");
    assert_eq!(title_of("本文\n", "user/profile.md"), "profile.md");
}

#[test]
fn paths_are_confined_to_the_knowledge_root() {
    assert_eq!(page_path("user/profile.md").expect("ok"), "user/profile.md");
    assert_eq!(page_path("./user/a.md").expect("ok"), "user/a.md");
    for bad in ["../x.md", "/etc/x.md", "user/../../x.md"] {
        assert_eq!(page_path(bad), Err(PathError::Forbidden), "{bad}");
    }
    assert_eq!(page_path("a.txt"), Err(PathError::NotMarkdown));
    assert_eq!(page_path("  "), Err(PathError::Empty));
    assert!(is_inbox("_inbox/2026-x.md"));
    assert!(!is_inbox("_inboxed/x.md"));
    assert_eq!(
        scope_dir("project:pluvio").as_deref(),
        Some("projects/pluvio")
    );
    assert_eq!(scope_dir("user").as_deref(), Some("user"));
    assert_eq!(scope_dir("  "), None);
}

fn sample_index() -> Index {
    Index {
        generated_at: "2026-09-20T00:00:00Z".into(),
        items: vec![
            IndexItem {
                path: "environment/clusters/pegasus.md".into(),
                title: "pegasus".into(),
                tags: vec!["hpc".into(), "cluster".into()],
                scope: Some("environment".into()),
                updated: Some("2026-09-01T00:00:00Z".into()),
                ..IndexItem::default()
            },
            IndexItem {
                path: "user/profile.md".into(),
                title: "cluster の好み".into(),
                tags: vec![],
                scope: Some("user".into()),
                updated: Some("2026-09-10T00:00:00Z".into()),
                ..IndexItem::default()
            },
            IndexItem {
                path: "experience/2026/09/hpc-run.md".into(),
                title: "計測".into(),
                tags: vec![],
                scope: Some("experience".into()),
                updated: Some("2026-09-19T00:00:00Z".into()),
                ..IndexItem::default()
            },
            IndexItem {
                path: "_inbox/2026-09-20-x.md".into(),
                title: "cluster の候補".into(),
                tags: vec!["cluster".into()],
                ..IndexItem::default()
            },
        ],
    }
}

/// ADR-0047 D3: tag 一致 → title 一致 → 本文一致 → `updated` の新しさ。`_inbox` は出ない。
#[test]
fn search_ranks_tags_above_titles_above_bodies() {
    let index = sample_index();
    let hits = search(
        &index,
        "cluster",
        None,
        10,
        &["experience/2026/09/hpc-run.md".to_string()],
    );
    let paths: Vec<&str> = hits.iter().map(|h| h.item.path.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            "environment/clusters/pegasus.md", // tag 一致
            "user/profile.md",                 // title 一致
            "experience/2026/09/hpc-run.md",   // 本文一致だけ
        ],
        "{hits:#?}"
    );
    assert_eq!(hits[0].tag_matches, 1);
    assert!(hits[2].body_match && hits[2].tag_matches == 0);
    // `_inbox` は検索に出ない（候補は正本ではない）。
    assert!(!paths.contains(&"_inbox/2026-09-20-x.md"));

    // scope で絞る（KB の相対パスの接頭辞でも front matter の `scope` でも当たる）。
    let only_env = search(&index, "cluster", Some("environment"), 10, &[]);
    assert_eq!(only_env.len(), 1);
    assert_eq!(only_env[0].item.path, "environment/clusters/pegasus.md");
    assert_eq!(
        search(&index, "cluster", Some("environment/clusters"), 10, &[]).len(),
        1
    );
    assert!(search(&index, "cluster", Some("projects/pluvio"), 10, &[]).is_empty());

    // limit で切る。
    assert_eq!(search(&index, "cluster", None, 1, &[]).len(), 1);
    // 当たらない語は 0 件。
    assert!(search(&index, "みつからない", None, 10, &[]).is_empty());
    // 空の query は scope の全件を `updated` の新しい順で返す。
    let all = search(&index, "  ", None, 10, &[]);
    assert_eq!(
        all.iter().map(|h| h.item.path.as_str()).collect::<Vec<_>>(),
        vec![
            "experience/2026/09/hpc-run.md",
            "user/profile.md",
            "environment/clusters/pegasus.md"
        ]
    );
}

#[test]
fn mounts_parse_from_the_short_form_and_merge_without_duplicates() {
    assert_eq!(
        "kb:user".parse::<KnowledgeMount>().expect("kb"),
        KnowledgeMount::kb("user")
    );
    assert_eq!(
        "kb:environment/clusters"
            .parse::<KnowledgeMount>()
            .expect("kb"),
        KnowledgeMount::kb("environment/clusters")
    );
    assert_eq!(
        "repo:pluvio:doc".parse::<KnowledgeMount>().expect("repo"),
        KnowledgeMount::repo("pluvio", Some("doc".into()))
    );
    assert_eq!(
        "dir:/opt/notes".parse::<KnowledgeMount>().expect("dir"),
        KnowledgeMount::dir("/opt/notes")
    );
    assert_eq!(
        "memory".parse::<KnowledgeMount>().expect("memory"),
        KnowledgeMount::memory(None)
    );
    assert_eq!(
        "memory:cos".parse::<KnowledgeMount>().expect("memory"),
        KnowledgeMount::memory(Some("cos".into()))
    );
    assert!("kb:../etc".parse::<KnowledgeMount>().is_err());
    assert!("nope:x".parse::<KnowledgeMount>().is_err());
    assert_eq!(KnowledgeMount::kb("user").label(), "kb:user");

    let org = vec![KnowledgeMount::kb("user"), KnowledgeMount::memory(None)];
    let project = vec![
        KnowledgeMount::kb("projects/pluvio"),
        KnowledgeMount::kb("user"),
    ];
    let merged = merge_mounts(&[&org, &project]);
    assert_eq!(
        merged,
        vec![
            KnowledgeMount::kb("user"),
            KnowledgeMount::memory(None),
            KnowledgeMount::kb("projects/pluvio"),
        ]
    );
}

/// ADR-0047 D4: 秘密は決定的なパターンで弾く。
#[test]
fn secrets_are_refused_by_pattern() {
    assert!(secret_finding("api key: sk-abcdef123").is_some());
    assert!(secret_finding("token ghp_0123456789").is_some());
    assert!(secret_finding("-----BEGIN OPENSSH PRIVATE KEY-----").is_some());
    assert!(secret_finding("AKIAIOSFODNN7EXAMPLE").is_some());
    assert!(secret_finding("AIzaSyA-0123").is_some());
    // 大文字の AKIA は小文字の `akia` とは違う（誤検知を避ける）。
    assert!(secret_finding("akiaiosfodnn7example").is_none());
    assert!(secret_finding("pegasus は pjsub で投げる").is_none());
}

fn ok_candidate(op: CandidateOp) -> Candidate {
    Candidate {
        op,
        path: "environment/clusters/pegasus.md".into(),
        title: "pegasus の使い方".into(),
        tags: vec!["hpc".into()],
        scope: "environment".into(),
        body: "pjsub -L node=1 で投げる。".into(),
        sources: vec!["task:01J1".into()],
        confidence: Confidence::High,
    }
}

/// ADR-0047 D4: 候補の決定的な検査。`retire` だけ本文が空でもよい。
#[test]
fn validate_candidate_checks_path_title_sources_body_size_and_secrets() {
    assert_eq!(
        validate_candidate(&ok_candidate(CandidateOp::Create)).expect("ok"),
        "environment/clusters/pegasus.md"
    );

    let mut retiring = ok_candidate(CandidateOp::Retire);
    retiring.body = String::new();
    assert_eq!(
        validate_candidate(&retiring).expect("retire may have an empty body"),
        "environment/clusters/pegasus.md"
    );

    let mut escapes = ok_candidate(CandidateOp::Create);
    escapes.path = "../../etc/passwd".into();
    assert!(matches!(
        validate_candidate(&escapes),
        Err(CandidateProblem::Path(PathError::Forbidden))
    ));

    let mut no_title = ok_candidate(CandidateOp::Create);
    no_title.title = "  ".into();
    assert_eq!(
        validate_candidate(&no_title),
        Err(CandidateProblem::NoTitle)
    );

    let mut no_sources = ok_candidate(CandidateOp::Create);
    no_sources.sources = vec![];
    assert_eq!(
        validate_candidate(&no_sources),
        Err(CandidateProblem::NoSources)
    );

    let mut no_body = ok_candidate(CandidateOp::Update);
    no_body.body = "   ".into();
    assert_eq!(validate_candidate(&no_body), Err(CandidateProblem::NoBody));

    let mut too_large = ok_candidate(CandidateOp::Create);
    too_large.body = "x".repeat(MAX_PAGE_BYTES + 1);
    assert_eq!(
        validate_candidate(&too_large),
        Err(CandidateProblem::TooLarge)
    );

    let mut secret = ok_candidate(CandidateOp::Create);
    secret.body = "API キーは sk-abc123def456 です".into();
    assert!(matches!(
        validate_candidate(&secret),
        Err(CandidateProblem::Secret(_))
    ));

    assert!(CandidateOp::Create.direct_commit_eligible());
    assert!(CandidateOp::Update.direct_commit_eligible());
    assert!(!CandidateOp::Merge.direct_commit_eligible());
    assert!(!CandidateOp::Retire.direct_commit_eligible());
    assert_eq!("update".parse::<CandidateOp>(), Ok(CandidateOp::Update));
    assert!("bogus".parse::<CandidateOp>().is_err());
}

/// ADR-0047 D4: 依頼文は入力の各節を含み、出典に `task:<id>` を促す（決定的。LLM は呼ばない）。
#[test]
fn maintenance_objective_includes_every_section() {
    let input = MaintenanceInput {
        task_id: "01J1".into(),
        task_title: "pegasus の初期セットアップ".into(),
        task_objective: "pegasus に pjsub の使い方を確認する".into(),
        report_headline: "pjsub の投げ方を確認した".into(),
        report_body: "pjsub -L node=1 で投げられる。".into(),
        result_summary: "pjsub -L node=1 -L elapse=01:00 で動作確認済み".into(),
        comments: vec!["human: 良さそう".into()],
        related_pages: vec![RelatedPage {
            path: "environment/clusters/pegasus.md".into(),
            title: "pegasus の使い方".into(),
            excerpt: "既存の使い方メモ".into(),
        }],
        notes_excerpt: "- 2026-09-10: pegasus は pjsub".into(),
        existing_titles: vec!["environment/clusters/pegasus.md — pegasus の使い方".into()],
    };
    let text = maintenance_objective(&input);
    assert!(text.contains("01J1"), "{text}");
    assert!(text.contains("pjsub の投げ方を確認した"), "{text}");
    assert!(text.contains("pjsub -L node=1 -L elapse=01:00"), "{text}");
    assert!(text.contains("human: 良さそう"), "{text}");
    assert!(text.contains("既存の使い方メモ"), "{text}");
    assert!(text.contains("2026-09-10: pegasus は pjsub"), "{text}");
    assert!(text.contains("pegasus の使い方"), "{text}");
    assert!(text.contains("sources"), "{text}");

    let minimal = maintenance_objective(&MaintenanceInput {
        task_id: "01J2".into(),
        task_title: "x".into(),
        task_objective: "y".into(),
        ..MaintenanceInput::default()
    });
    assert!(!minimal.contains("---- 報告"), "{minimal}");
    assert!(!minimal.contains("---- コメント"), "{minimal}");
}

/// ADR-0047 付記（2026-10-04）H1〜H3: 人の印の判定・run の出典の正し方・`sources` 行だけの書き換え。
#[test]
fn human_marks_protect_only_authored_and_legacy_and_user_pages() {
    let authored = "---\ntitle: a\nsources: [\"human:authored\", \"url:x\"]\n---\n\nbody\n";
    let author_key = "---\ntitle: a\nauthor: human\n---\n\nbody\n";
    let legacy = "---\ntitle: a\nsources:\n  - human\n---\n\nbody\n";
    let singular = "---\ntitle: a\nsource: human\n---\n\nbody\n";
    let instruction =
        "---\ntitle: a\nsources: [\"human:instruction\", \"task:01J1\"]\n---\n\nbody\n";
    assert!(protected_page("projects/x/a.md", authored));
    assert!(protected_page("projects/x/a.md", author_key));
    assert!(protected_page("projects/x/a.md", legacy));
    assert!(protected_page("projects/x/a.md", singular));
    assert!(!protected_page("projects/x/a.md", instruction));
    assert!(protected_page("user/a.md", instruction));
    // 本文に書かれた `author: human` は印ではない。
    assert!(!protected_page(
        "projects/x/a.md",
        "---\ntitle: a\n---\n\nauthor: human\n"
    ));
}

#[test]
fn normalize_agent_sources_turns_human_marks_into_instruction() {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert_eq!(
        normalize_agent_sources(
            &s(&["human", "human:authored", " url:x "]),
            &[],
            Some("01J1")
        ),
        s(&["human:instruction", "url:x", "task:01J1"])
    );
    // 既存ページが同じ印を持つなら保つ（出典を保つ update で保護を落とさない）。
    assert_eq!(
        normalize_agent_sources(
            &s(&["human:authored", "task:01J1"]),
            &s(&["human:authored"]),
            Some("01J2")
        ),
        s(&["human:authored", "task:01J1"])
    );
    // 人の印が無ければ task を足さない。
    assert_eq!(
        normalize_agent_sources(&s(&["url:x"]), &[], Some("01J1")),
        s(&["url:x"])
    );
}

#[test]
fn replace_sources_rewrites_only_the_sources_lines() {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let block =
        "---\ntitle: a\nsources:\n  - human\n  - \"url:x\"\ntags: [t]\nextra: keep\n---\n\nbody\n";
    let out = replace_sources(block, &s(&["human:instruction", "url:x"])).expect("rewrite");
    assert_eq!(
        out,
        "---\ntitle: a\nsources: [\"human:instruction\", \"url:x\"]\ntags: [t]\nextra: keep\n---\n\nbody\n"
    );
    let none = "---\ntitle: a\n---\n\nbody\n";
    assert_eq!(
        replace_sources(none, &s(&["human:authored"])).expect("add"),
        "---\ntitle: a\nsources: [\"human:authored\"]\n---\n\nbody\n"
    );
    assert_eq!(replace_sources("body only\n", &s(&["x"])), None);
}

/// ADR-0047 付記 H1: 人の GUI 編集は『人が書いた』印を付ける（旧形 `human` は置き換え、`user/` と既存の印は触らない）。
#[test]
fn mark_human_authored_adds_the_authored_mark_for_gui_edits() {
    let instruction = "---\ntitle: t\nsources: [task:01X, human:instruction]\n---\n\n本文\n";
    let marked = mark_human_authored("projects/a.md", instruction);
    assert_eq!(
        marked,
        "---\ntitle: t\nsources: [\"task:01X\", \"human:instruction\", \"human:authored\"]\n---\n\n本文\n"
    );
    assert!(protected_page("projects/a.md", &marked));
    // 同じ入力には同じ出力（再保存は unchanged のまま）、二度付けしない。
    assert_eq!(mark_human_authored("projects/a.md", &marked), marked);

    let legacy = "---\ntitle: t\nsources: [human]\n---\n\n本文\n";
    assert_eq!(
        mark_human_authored("projects/a.md", legacy),
        "---\ntitle: t\nsources: [\"human:authored\"]\n---\n\n本文\n"
    );

    let bare = "# メモ\n";
    let marked = mark_human_authored("projects/b.md", bare);
    assert_eq!(
        marked,
        "---\nsources: [\"human:authored\"]\n---\n\n# メモ\n"
    );
    assert!(human_authored(&marked));

    let author = "---\ntitle: t\nauthor: human\n---\n\n本文\n";
    assert_eq!(mark_human_authored("projects/c.md", author), author);
    assert_eq!(mark_human_authored("user/notes.md", bare), bare);
}
