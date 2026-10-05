//! ADR-0047 付記（2026-10-04）H2・H4 の試験（一時 KB。決定的）。

use super::*;

fn kb_dir() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("knowledge");
    init(&root).expect("init");
    (dir, root)
}

fn agent_candidate(path: &str, sources: &[&str], confidence: Confidence) -> kb::Candidate {
    kb::Candidate {
        op: kb::CandidateOp::Create,
        path: path.to_string(),
        title: format!("{path} の題"),
        tags: vec!["hpc".into()],
        scope: "environment".into(),
        body: "人の指示: ジョブは /work に置く。".into(),
        sources: sources.iter().map(|s| s.to_string()).collect(),
        confidence,
    }
}

/// H2: knowledge run の候補に付いた `human` / `human:authored` は、直接コミットでも `_inbox/` 行きでも
/// `human:instruction` に直り、`task:<id>` が添えられる。run は保護されるページを作れない。
#[test]
fn knowledge_run_marks_human_instruction_on_both_write_paths() {
    let (_dir, root) = kb_dir();
    let out = apply_candidates(
        &root,
        "01M3RM9HP2P6MABRNSB1N1CEHJ",
        &[
            agent_candidate("environment/tools/direct.md", &["human"], Confidence::High),
            agent_candidate(
                "environment/tools/inboxed.md",
                &["human:authored", "url:https://example.org"],
                Confidence::Medium,
            ),
        ],
    );
    assert_eq!(out.committed, vec!["environment/tools/direct.md"]);
    assert_eq!(out.inboxed.len(), 1, "{out:?}");
    for path in [
        "environment/tools/direct.md".to_string(),
        out.inboxed[0].clone(),
    ] {
        let raw = std::fs::read_to_string(root.join(&path)).expect("read");
        let sources = kb::front_matter(&raw).0.sources;
        assert!(
            sources.contains(&kb::SOURCE_HUMAN_INSTRUCTION.to_string()),
            "{path}: {sources:?}"
        );
        assert!(
            sources.contains(&"task:01M3RM9HP2P6MABRNSB1N1CEHJ".to_string()),
            "{path}: {sources:?}"
        );
        assert!(
            !sources
                .iter()
                .any(|s| s == "human" || s == "human:authored")
        );
        assert!(!kb::protected_page(&path, &raw), "{path}: {raw}");
    }
}

/// H2: 出典を保つ `update`（GC・整理）は、対象ページが既に持つ『人が書いた』印を落とさない。
#[test]
fn knowledge_run_keeps_an_existing_authored_mark_on_update() {
    let (_dir, root) = kb_dir();
    std::fs::create_dir_all(root.join("environment/tools")).expect("dir");
    std::fs::write(
        root.join("environment/tools/mine.md"),
        "---\ntitle: mine\nscope: environment\nsources: [\"human:authored\"]\n---\n\n# mine\n",
    )
    .expect("write");
    commit_paths(
        &root,
        "knowledge: environment/tools/mine.md",
        (kb::HUMAN_AUTHOR_NAME, kb::HUMAN_AUTHOR_EMAIL),
        &["environment/tools/mine.md"],
    )
    .expect("commit");
    let mut update = agent_candidate(
        "environment/tools/mine.md",
        &["human:authored", "task:01J1"],
        Confidence::High,
    );
    update.op = kb::CandidateOp::Update;
    update.title = "mine".into();
    let out = apply_candidates(&root, "01JTASK", &[update]);
    assert_eq!(out.committed, vec!["environment/tools/mine.md"], "{out:?}");
    let raw = std::fs::read_to_string(root.join("environment/tools/mine.md")).expect("read");
    assert!(kb::human_authored(&raw), "{raw}");
    assert!(!raw.contains("human:instruction"), "{raw}");
}

/// H2: `celerisctl knowledge record`（run が呼ぶ）も `human` を `human:instruction` にする。
#[test]
fn record_marks_human_instruction() {
    let (_dir, root) = kb_dir();
    let ok = record(
        &root,
        &RecordRequest {
            title: "sirius の投入".into(),
            scope: "environment".into(),
            sources: vec!["task:01J1".into(), "human".into()],
            body: "qsub で投げる。".into(),
            path: Some("environment/clusters/sirius-submit.md".into()),
            ..RecordRequest::default()
        },
    )
    .expect("record");
    let item = inbox_get(&root, &ok.id).expect("item");
    assert_eq!(
        item.sources,
        vec!["task:01J1", kb::SOURCE_HUMAN_INSTRUCTION]
    );
}

/// H2: 抽出の依頼文が新しい印を指示する。
#[test]
fn maintenance_objective_asks_for_human_instruction() {
    let text = kb::maintenance_objective(&kb::MaintenanceInput {
        task_id: "01J1".into(),
        ..Default::default()
    });
    assert!(text.contains("human:instruction"), "{text}");
    assert!(text.contains("`human` 単独と `human:authored`"), "{text}");
}

fn write_commit(root: &Path, path: &str, raw: &str, author: &str, subject: &str) {
    let full = root.join(path);
    std::fs::create_dir_all(full.parent().expect("parent")).expect("dir");
    std::fs::write(&full, raw).expect("write");
    commit_paths(root, subject, (author, "celeris@local"), &[path]).expect("commit");
}

fn page(sources: &str, body: &str) -> String {
    format!("---\ntitle: t\ntags: [a]\nsources: {sources}\nconfidence: high\n---\n\n{body}\n")
}

/// H4: 一時 KB の git 履歴から判別する。人の編集（GUI の `PUT /knowledge/page`・人の直接 git）があれば
/// `human:authored`、run の書き込み（直接コミット・候補の取り込み）だけなら `human:instruction`、
/// 雛形・未コミットは保護のまま一覧に出す。dry-run は何も書かず、apply は 1 commit で冪等。
#[test]
fn migration_classifies_from_git_history_and_lists_undetermined() {
    let (_dir, root) = kb_dir();
    const AGENT: &str = kb::AGENT_AUTHOR_NAME;
    const HUMAN: &str = kb::HUMAN_AUTHOR_NAME;
    // run が作り、人が GUI で直した → 人が書いた。
    write_commit(
        &root,
        "projects/demo/gui.md",
        &page("[\"task:01M3YFCJKMNWQ13HRS52M5BSWW\", human]", "run"),
        AGENT,
        "knowledge: create projects/demo/gui.md (task 01M3YFCJKMNWQ13HRS52M5BSWW)",
    );
    write_commit(
        &root,
        "projects/demo/gui.md",
        &page("[\"task:01M3YFCJKMNWQ13HRS52M5BSWW\", human]", "人が直した"),
        HUMAN,
        "knowledge: projects/demo/gui.md",
    );
    // 人が直接 git で書いた（Celeris を通さない author）、単数形 → author: human。
    write_commit(
        &root,
        "projects/demo/direct.md",
        "---\ntitle: d\nsource: human\n---\n\n本文\n",
        "maetin0324",
        "人が書いた",
    );
    // run の直接コミットだけ（block list の sources）→ 人の指示由来。task を添える。
    write_commit(
        &root,
        "projects/demo/run.md",
        "---\ntitle: r\nsources:\n  - human\n  - \"url:https://example.org\"\nconfidence: high\n---\n\n本文\n",
        AGENT,
        "knowledge: create projects/demo/run.md (task 01M3YF3NS2EGTZD2BBWNPG1K28)",
    );
    // run の候補を人が取り込んだだけ → 人の指示由来。
    write_commit(
        &root,
        "projects/demo/accepted.md",
        &page("[human, \"task:01M3RM9HP2P6MABRNSB1N1CEHJ\"]", "候補"),
        HUMAN,
        "knowledge: projects/demo/accepted.md（候補 20261001T000000Z-x を取り込む）",
    );
    // `_inbox/` の run の候補 → 人の指示由来。
    write_commit(
        &root,
        "_inbox/20261004T000000Z-x.md",
        &page("[\"task:01M3RM9HP2P6MABRNSB1N1CEHJ\", human]", "候補"),
        AGENT,
        "knowledge: 候補 _inbox/20261004T000000Z-x.md（task 01M3RM9HP2P6MABRNSB1N1CEHJ）",
    );
    // init の雛形に run が追記 → 判別できない（雛形の human は init が付けた）。
    let seeded = std::fs::read_to_string(root.join("environment/clusters/sirius.md"))
        .expect("seed")
        .replace("human:authored", "human");
    write_commit(
        &root,
        "environment/clusters/sirius.md",
        &format!("{seeded}\nrun の追記\n"),
        AGENT,
        "knowledge: update environment/clusters/sirius.md (task 01M35X86XTHHN9C6XDAYD2FZ7T)",
    );
    // 未コミット → 判別できない。
    std::fs::write(
        root.join("projects/demo/untracked.md"),
        page("[human]", "未コミット"),
    )
    .expect("write");
    // 新しい印は対象外。
    write_commit(
        &root,
        "projects/demo/new.md",
        &page("[\"human:instruction\", \"task:01J1\"]", "new"),
        AGENT,
        "knowledge: create projects/demo/new.md (task 01M3YF3NS2EGTZD2BBWNPG1K28)",
    );

    let head_before = head(&root);
    let dry = migrate_human_sources(&root, false).expect("dry-run");
    assert!(!dry.applied);
    assert_eq!(dry.sha, None);
    assert_eq!(head(&root), head_before, "dry-run は commit しない");
    assert!(
        std::fs::read_to_string(root.join("projects/demo/run.md"))
            .expect("read")
            .contains("  - human\n"),
        "dry-run は書かない"
    );
    let class_of = |m: &HumanSourceMigration, path: &str| {
        m.entries
            .iter()
            .find(|e| e.path == path)
            .map(|e| e.class)
            .unwrap_or_else(|| panic!("{path} が無い: {m:?}"))
    };
    use HumanSourceClass::*;
    assert_eq!(class_of(&dry, "projects/demo/gui.md"), Authored);
    assert_eq!(class_of(&dry, "projects/demo/direct.md"), Authored);
    assert_eq!(class_of(&dry, "projects/demo/run.md"), Instruction);
    assert_eq!(class_of(&dry, "projects/demo/accepted.md"), Instruction);
    assert_eq!(class_of(&dry, "_inbox/20261004T000000Z-x.md"), Instruction);
    assert_eq!(
        class_of(&dry, "environment/clusters/sirius.md"),
        Undetermined
    );
    assert_eq!(class_of(&dry, "projects/demo/untracked.md"), Undetermined);
    assert!(!dry.entries.iter().any(|e| e.path == "projects/demo/new.md"));
    // init の雛形は新しい印（human:authored）で書かれるので、旧形の一覧には出ない。
    assert!(!dry.entries.iter().any(|e| e.path == "user/profile.md"));

    let applied = migrate_human_sources(&root, true).expect("apply");
    assert!(applied.sha.is_some());
    assert_eq!(applied.entries, dry.entries.clone());
    let read = |p: &str| std::fs::read_to_string(root.join(p)).expect("read");
    let gui = read("projects/demo/gui.md");
    assert!(kb::human_authored(&gui) && !kb::legacy_human(&gui), "{gui}");
    assert!(gui.contains("人が直した"));
    let direct = read("projects/demo/direct.md");
    assert!(
        direct.contains("author: human\n") && !kb::legacy_human(&direct),
        "{direct}"
    );
    let run = read("projects/demo/run.md");
    assert_eq!(
        kb::front_matter(&run).0.sources,
        vec![
            "human:instruction",
            "url:https://example.org",
            "task:01M3YF3NS2EGTZD2BBWNPG1K28"
        ],
        "{run}"
    );
    assert!(
        run.contains("confidence: high\n---\n\n本文\n"),
        "他の行と本文は変えない: {run}"
    );
    assert!(!kb::protected_page("projects/demo/run.md", &run));
    let accepted = read("projects/demo/accepted.md");
    assert_eq!(
        kb::front_matter(&accepted).0.sources,
        vec!["human:instruction", "task:01M3RM9HP2P6MABRNSB1N1CEHJ"]
    );
    // 判別できないものは書き換えず、保護のまま。
    let sirius = read("environment/clusters/sirius.md");
    assert!(kb::legacy_human(&sirius));
    assert!(kb::protected_page(
        "environment/clusters/sirius.md",
        &sirius
    ));
    let hist = history(&root, "projects/demo/run.md");
    assert_eq!(hist[0].author, kb::AGENT_AUTHOR_NAME);
    assert!(
        hist[0].subject.contains("sources の human を移行"),
        "{hist:?}"
    );

    // 冪等: 2 回目は未判別だけが残り、commit しない。
    let head_after = head(&root);
    let again = migrate_human_sources(&root, true).expect("again");
    assert_eq!(again.sha, None);
    assert!(
        again.entries.iter().all(|e| e.class == Undetermined),
        "{again:?}"
    );
    assert_eq!(head(&root), head_after);
}
