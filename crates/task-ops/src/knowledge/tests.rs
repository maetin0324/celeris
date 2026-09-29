#[cfg(test)]
mod tests {
    use super::*;

    fn kb_dir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("knowledge");
        init(&root).expect("init");
        (dir, root)
    }

    /// ADR-0047 D1: `init` は git のリポジトリ・骨組み・雛形・`index.json` を作り、**2 回目は何もしない**。
    #[test]
    fn init_creates_the_skeleton_and_is_idempotent() {
        let (_dir, root) = kb_dir();
        assert!(root.join(".git").exists());
        for d in kb::SKELETON_DIRS {
            assert!(root.join(d).is_dir(), "{d}");
        }
        assert!(root.join(INBOX_DIR).is_dir());
        assert!(root.join("README.md").exists());
        assert!(root.join("user/profile.md").exists());
        assert!(root.join("environment/clusters/pegasus.md").exists());
        assert!(root.join(INDEX_FILE).exists());
        // 索引は雛形を拾い、`_inbox` は入らない。
        let index = load_index(&root).expect("index");
        assert!(index.items.iter().any(|i| i.path == "user/profile.md"));
        assert!(!index.items.iter().any(|i| kb::is_inbox(&i.path)));
        assert!(
            index
                .items
                .iter()
                .any(|i| i.path == "environment/clusters/pegasus.md")
        );
        // `index.json` は派生物なので git には入れない。
        let tracked = git(&root, &["ls-files"], GIT_TIMEOUT)
            .expect("ls-files")
            .stdout;
        assert!(!tracked.contains(INDEX_FILE), "{tracked}");
        let commits_before = history(&root, "README.md").len();

        // 人が書き換えたページは 2 回目の `init` で上書きされない。
        std::fs::write(
            root.join("user/profile.md"),
            "---\ntitle: 私\n---\n\n# 私\n".as_bytes(),
        )
        .expect("write");
        let again = init(&root).expect("again");
        assert!(!again.created);
        assert!(again.added.is_empty(), "{again:?}");
        assert!(
            std::fs::read_to_string(root.join("user/profile.md"))
                .expect("read")
                .contains("# 私"),
        );
        assert_eq!(history(&root, "README.md").len(), commits_before);
    }

    /// 索引は front matter を読み、`scope` が無いページは置き場から決める。
    #[test]
    fn reindex_reads_front_matter_and_defaults_the_scope() {
        let (_dir, root) = kb_dir();
        std::fs::create_dir_all(root.join("projects/pluvio")).expect("mkdir");
        std::fs::write(
            root.join("projects/pluvio/design.md"),
            "# Pluvio の設計\n".as_bytes(),
        )
        .expect("write");
        std::fs::write(
            root.join("environment/tools/git.md"),
            "---\ntitle: git\ntags: [tool, vcs]\nconfidence: high\n---\n\n本文\n".as_bytes(),
        )
        .expect("write");
        let index = reindex(&root).expect("reindex");
        let design = index.get("projects/pluvio/design.md").expect("design");
        assert_eq!(design.scope.as_deref(), Some("project:pluvio"));
        assert_eq!(design.title, "Pluvio の設計");
        let tool = index.get("environment/tools/git.md").expect("tool");
        assert_eq!(tool.tags, vec!["tool", "vcs"]);
        assert_eq!(tool.scope.as_deref(), Some("environment"));
        assert_eq!(tool.confidence, Some(Confidence::High));
        assert!(!index.generated_at.is_empty());
        assert!(!index_is_stale(&root));
    }

    /// ADR-0047 D3: 検索は索引の tag / title と、本文の `git grep` を合わせる。
    #[test]
    fn search_finds_pages_by_tag_title_and_body() {
        let (_dir, root) = kb_dir();
        std::fs::write(
            root.join("environment/clusters/pegasus.md"),
            "---\ntitle: pegasus\ntags: [hpc, cluster]\nscope: environment\n---\n\npjsub で投げる。\n".as_bytes(),
        )
        .expect("write");
        std::fs::write(
            root.join("experience/2026-pjsub.md"),
            "---\ntitle: 計測の記録\nscope: experience\n---\n\npjsub の待ち行列が長い。\n"
                .as_bytes(),
        )
        .expect("write");
        reindex(&root).expect("reindex");

        let hits = search(&root, "cluster", None, 10);
        assert_eq!(
            hits.first().map(|h| h.item.path.as_str()),
            Some("environment/clusters/pegasus.md")
        );
        // 本文にしか無い語も当たる（`git grep`）。
        let body = search(&root, "pjsub", None, 10);
        let paths: Vec<&str> = body.iter().map(|h| h.item.path.as_str()).collect();
        assert!(paths.contains(&"experience/2026-pjsub.md"), "{paths:?}");
        assert!(
            paths.contains(&"environment/clusters/pegasus.md"),
            "{paths:?}"
        );
        // scope で絞る。
        let only = search(&root, "pjsub", Some("experience"), 10);
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].item.path, "experience/2026-pjsub.md");
        // limit。
        assert_eq!(search(&root, "pjsub", None, 1).len(), 1);
        assert!(grep(&root, "みつからないはず").is_empty());
    }

    /// ADR-0047 D3 / D4: `record` は `_inbox` に書き、`sources` が無ければ拒否、秘密も拒否。
    #[test]
    fn record_writes_a_candidate_and_refuses_secrets_and_missing_sources() {
        let (_dir, root) = kb_dir();
        let ok = record(
            &root,
            &RecordRequest {
                title: "pegasus の投げ方".into(),
                scope: "environment".into(),
                // Phase K-1: `environment` は分類が要る（タグ `cluster` → `clusters/`）。
                tags: vec!["hpc".into(), "cluster".into()],
                sources: vec!["task:01J1".into()],
                confidence: Some(Confidence::High),
                body: "pjsub -L node=1 で投げる。".into(),
                path: None,
                op: None,
            },
        )
        .expect("record");
        assert!(ok.path.starts_with("_inbox/"), "{ok:?}");
        let raw = std::fs::read_to_string(root.join(&ok.path)).expect("read");
        assert!(raw.contains("title: pegasus の投げ方"), "{raw}");
        assert!(raw.contains("sources: [\"task:01J1\"]"), "{raw}");
        assert!(raw.contains("confidence: high"), "{raw}");
        // 候補は索引に入らない。
        let index = reindex(&root).expect("reindex");
        assert!(!index.items.iter().any(|i| i.path == ok.path));
        // コミットされている。
        assert_eq!(history(&root, &ok.path).len(), 1);
        assert_eq!(history(&root, &ok.path)[0].author, kb::AGENT_AUTHOR_NAME);

        // `sources` が無ければエラー。
        let no_source = record(
            &root,
            &RecordRequest {
                title: "x".into(),
                scope: "user".into(),
                sources: vec![],
                body: "y".into(),
                ..RecordRequest::default()
            },
        );
        assert_eq!(no_source, Err(RecordError::NoSources));
        // 秘密は拒否（ADR-0047 D4）。
        let secret = record(
            &root,
            &RecordRequest {
                title: "鍵".into(),
                scope: "user".into(),
                sources: vec!["human".into()],
                body: "API キーは sk-abc123def456 です".into(),
                ..RecordRequest::default()
            },
        );
        assert!(matches!(secret, Err(RecordError::Secret(_))), "{secret:?}");
        assert_eq!(inbox_list(&root).len(), 1);
    }

    /// P-G46-5: `celerisctl knowledge record --tags a,a` のように人が同じタグを重ねて渡しても、
    /// `_inbox/` に書く時点（保存時）で順序を保ったまま重複を除く。大文字小文字は区別する。
    #[test]
    fn record_deduplicates_tags_preserving_order() {
        let (_dir, root) = kb_dir();
        let ok = record(
            &root,
            &RecordRequest {
                title: "重複タグ".into(),
                scope: "environment".into(),
                tags: vec![
                    "environment".into(),
                    "hpc".into(),
                    "environment".into(),
                    "Environment".into(),
                ],
                sources: vec!["human".into()],
                body: "本文。".into(),
                path: Some("environment/tools/dup-tags.md".into()),
                ..RecordRequest::default()
            },
        )
        .expect("record");
        let item = inbox_get(&root, &ok.id).expect("inbox item");
        assert_eq!(
            item.tags,
            vec![
                "environment".to_string(),
                "hpc".to_string(),
                "Environment".to_string()
            ],
            "順序を保ったまま重複だけ除く。大文字小文字は別のタグとして残る"
        );
    }

    /// ADR-0047 D5: accept は正本へ移してコミットし、reject は捨てる。
    #[test]
    fn inbox_accept_and_reject_commit_to_git() {
        let (_dir, root) = kb_dir();
        let one = record(
            &root,
            &RecordRequest {
                title: "fern03 の使い方".into(),
                scope: "environment".into(),
                sources: vec!["human".into()],
                body: "ssh fern03。".into(),
                path: Some("environment/servers/fern03.md".into()),
                ..RecordRequest::default()
            },
        )
        .expect("record");
        let listed = inbox_list(&root);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, one.id);
        assert_eq!(listed[0].target, "environment/servers/fern03.md");
        assert_eq!(listed[0].sources, vec!["human"]);

        let accepted = inbox_accept(&root, &one.id, None, false);
        let path = match &accepted {
            InboxOutcome::Accepted { path, etag, .. } => {
                assert!(etag.is_some());
                path.clone()
            }
            other => panic!("{other:?}"),
        };
        assert_eq!(path, "environment/servers/fern03.md");
        assert!(root.join(&path).exists());
        assert!(!root.join(&one.path).exists());
        let raw = std::fs::read_to_string(root.join(&path)).expect("read");
        // `_inbox` 専用の `path:` は落ちる。
        assert!(!raw.contains("\npath:"), "{raw}");
        assert!(raw.contains("title: fern03 の使い方"), "{raw}");
        assert!(load_index(&root).expect("index").get(&path).is_some());
        assert_eq!(history(&root, &path).len(), 1);
        assert_eq!(history(&root, &path)[0].author, kb::HUMAN_AUTHOR_NAME);
        // 同じ id はもう無い。
        assert_eq!(
            inbox_accept(&root, &one.id, None, false),
            InboxOutcome::Missing
        );

        // Phase K-1: 宛先が既にあれば `append`（accept で末尾に節として足す。既存の本文は残る）。
        let two = record(
            &root,
            &RecordRequest {
                title: "fern03 の使い方".into(),
                scope: "environment".into(),
                sources: vec!["task:01J2".into()],
                body: "別の版。".into(),
                path: Some("environment/servers/fern03.md".into()),
                ..RecordRequest::default()
            },
        )
        .expect("record");
        assert_eq!(two.op, Some(kb::CandidateOp::Append));
        assert!(matches!(
            inbox_accept(&root, &two.id, None, false),
            InboxOutcome::Accepted { .. }
        ));
        let raw = std::fs::read_to_string(root.join(&path)).expect("read");
        assert!(raw.contains("ssh fern03。"), "{raw}");
        assert!(raw.contains("## 追記（"), "{raw}");
        assert!(raw.contains("別の版。"), "{raw}");
        assert!(raw.contains("\"task:01J2\""), "{raw}");
        assert_eq!(history(&root, &path).len(), 2);
        // 取り込み先を人が既存のページに変えたときは従来どおり 409。
        let two = record(
            &root,
            &RecordRequest {
                title: "fern03 の別件".into(),
                scope: "environment".into(),
                sources: vec!["human".into()],
                body: "別の件。".into(),
                path: Some("environment/servers/fern03-other.md".into()),
                ..RecordRequest::default()
            },
        )
        .expect("record");
        assert_eq!(two.op, None);
        assert_eq!(
            inbox_accept(&root, &two.id, Some("environment/servers/fern03.md"), false),
            InboxOutcome::Exists {
                path: "environment/servers/fern03.md".into()
            }
        );
        // reject は捨てる。
        assert!(matches!(
            inbox_reject(&root, &two.id),
            InboxOutcome::Rejected { .. }
        ));
        assert!(inbox_list(&root).is_empty());
        assert_eq!(inbox_reject(&root, &two.id), InboxOutcome::Missing);
        // id の境界。
        assert_eq!(inbox_path("../../etc/passwd"), Err(PathError::Forbidden));
        assert_eq!(inbox_path("  "), Err(PathError::Empty));
    }

    /// 人の編集は `etag` で衝突を見て、1 件 1 コミットになる。
    #[test]
    fn commit_page_checks_the_etag_and_makes_one_commit_per_change() {
        let (_dir, root) = kb_dir();
        let human = (
            kb::HUMAN_AUTHOR_NAME.to_string(),
            kb::HUMAN_AUTHOR_EMAIL.to_string(),
        );
        let created = commit_page(
            &root,
            &PageEdit {
                path: "user/notes.md".into(),
                body: Some("# メモ\n".into()),
                etag: None,
                message: "knowledge: user/notes.md".into(),
                author: human.clone(),
            },
        );
        let tag = match created {
            WriteOutcome::Written {
                etag, unchanged, ..
            } => {
                assert!(!unchanged);
                etag.expect("etag")
            }
            other => panic!("{other:?}"),
        };
        assert_eq!(
            read_page(&root, "user/notes.md").as_deref(),
            Some("# メモ\n")
        );
        // 既にあるのに etag 無しは 409。
        assert!(matches!(
            commit_page(
                &root,
                &PageEdit {
                    path: "user/notes.md".into(),
                    body: Some("# 別\n".into()),
                    etag: None,
                    message: "x".into(),
                    author: human.clone(),
                }
            ),
            WriteOutcome::EtagMismatch { .. }
        ));
        // 同じ中身なら新しいコミットは作らない。
        let same = commit_page(
            &root,
            &PageEdit {
                path: "user/notes.md".into(),
                body: Some("# メモ\n".into()),
                etag: Some(tag.clone()),
                message: "x".into(),
                author: human.clone(),
            },
        );
        assert!(
            matches!(
                same,
                WriteOutcome::Written {
                    unchanged: true,
                    ..
                }
            ),
            "{same:?}"
        );
        assert_eq!(history(&root, "user/notes.md").len(), 1);
        // 正しい etag なら通る。
        let updated = commit_page(
            &root,
            &PageEdit {
                path: "user/notes.md".into(),
                body: Some("# メモ\n\n続き\n".into()),
                etag: Some(tag),
                message: "knowledge: user/notes.md".into(),
                author: human.clone(),
            },
        );
        let next = match updated {
            WriteOutcome::Written { etag, .. } => etag.expect("etag"),
            other => panic!("{other:?}"),
        };
        assert_eq!(history(&root, "user/notes.md").len(), 2);
        // 削除。
        let deleted = commit_page(
            &root,
            &PageEdit {
                path: "user/notes.md".into(),
                body: None,
                etag: Some(next),
                message: "knowledge: remove".into(),
                author: human.clone(),
            },
        );
        assert!(
            matches!(deleted, WriteOutcome::Written { etag: None, .. }),
            "{deleted:?}"
        );
        assert!(read_page(&root, "user/notes.md").is_none());
        assert_eq!(
            commit_page(
                &root,
                &PageEdit {
                    path: "user/notes.md".into(),
                    body: None,
                    etag: None,
                    message: "x".into(),
                    author: human,
                }
            ),
            WriteOutcome::Missing
        );
    }

    /// 根の決め方（`--root` > `CELERIS_KNOWLEDGE_ROOT` > `[knowledge] root` > `~/.local/share/celeris/knowledge`）。
    /// 環境変数は他のテストと干渉しないよう、この 1 本の中だけで設定する。
    #[test]
    fn the_root_comes_from_the_flag_then_the_env_then_the_config() {
        let explicit = PathBuf::from("/tmp/kb-explicit");
        let configured = PathBuf::from("/tmp/kb-config");
        assert_eq!(resolve_root(Some(&explicit), Some(&configured)), explicit);
        assert_eq!(resolve_root(None, Some(&configured)), configured);
        // 既定は `~/.local/share/celeris/knowledge`（`~` は展開される）。
        let fallback = resolve_root(None, None);
        assert!(fallback.ends_with("knowledge"), "{}", fallback.display());
        assert!(
            !fallback.to_string_lossy().starts_with('~'),
            "{}",
            fallback.display()
        );
    }

    fn candidate(op: kb::CandidateOp, path: &str, confidence: Confidence) -> kb::Candidate {
        kb::Candidate {
            op,
            path: path.to_string(),
            title: "pegasus の使い方".into(),
            tags: vec!["hpc".into()],
            scope: "environment".into(),
            body: "pjsub -L node=1 で投げる。".into(),
            sources: vec!["task:01J1".into()],
            confidence,
        }
    }

    /// ADR-0047 D4: `confidence = high` の `create`/`update` は KB へ直接コミットする。
    #[test]
    fn apply_candidates_commits_high_confidence_create_and_update_directly() {
        let (_dir, root) = kb_dir();
        let created = candidate(
            kb::CandidateOp::Create,
            "environment/tools/newtool.md",
            Confidence::High,
        );
        let out = apply_candidates(&root, "01JTASK", &[created]);
        assert_eq!(out.committed, vec!["environment/tools/newtool.md"]);
        assert!(out.inboxed.is_empty());
        assert!(out.dropped.is_empty());
        assert_eq!(out.summary().ingested, 1);
        let raw = std::fs::read_to_string(root.join("environment/tools/newtool.md")).expect("read");
        assert!(raw.contains("title: pegasus の使い方"), "{raw}");
        assert!(raw.contains("sources: [\"task:01J1\"]"), "{raw}");
        let hist = history(&root, "environment/tools/newtool.md");
        assert_eq!(hist.len(), 1);
        assert_eq!(hist[0].author, kb::AGENT_AUTHOR_NAME);
        assert!(hist[0].subject.contains("knowledge: create"), "{hist:?}");
        assert!(hist[0].subject.contains("01JTASK"), "{hist:?}");

        // update: 既存の sources を引き継ぎ、和集合にする。
        let mut update = candidate(
            kb::CandidateOp::Update,
            "environment/tools/newtool.md",
            Confidence::High,
        );
        update.sources = vec!["task:01J2".into()];
        update.body = "続き。".into();
        let out2 = apply_candidates(&root, "01JTASK2", &[update]);
        assert_eq!(out2.committed, vec!["environment/tools/newtool.md"]);
        let raw2 =
            std::fs::read_to_string(root.join("environment/tools/newtool.md")).expect("read");
        assert!(raw2.contains("task:01J1"), "{raw2}");
        assert!(raw2.contains("task:01J2"), "{raw2}");
        assert_eq!(history(&root, "environment/tools/newtool.md").len(), 2);
    }

    /// P-G46-5: LangMem 整理 run が書く `Candidate.tags` に同じ値が並んでいても、直接コミット
    /// （`commit_candidate_directly`）と `_inbox/` 行き（`write_inbox_candidate`）のどちらでも
    /// 保存時に順序を保ったまま重複を除く。
    #[test]
    fn apply_candidates_deduplicates_candidate_tags_on_both_write_paths() {
        let (_dir, root) = kb_dir();
        let mut direct = candidate(
            kb::CandidateOp::Create,
            "environment/tools/newtool.md",
            Confidence::High,
        );
        direct.tags = vec!["hpc".into(), "tool".into(), "hpc".into()];
        let out = apply_candidates(&root, "01JTASK", &[direct]);
        assert_eq!(out.committed, vec!["environment/tools/newtool.md"]);
        let raw = std::fs::read_to_string(root.join("environment/tools/newtool.md")).expect("read");
        assert!(raw.contains("tags: [hpc, tool]"), "{raw}");

        let mut inboxed = candidate(
            kb::CandidateOp::Create,
            "environment/tools/other.md",
            Confidence::Medium,
        );
        inboxed.tags = vec!["hpc".into(), "tool".into(), "hpc".into()];
        let out2 = apply_candidates(&root, "01JTASK2", &[inboxed]);
        assert_eq!(out2.inboxed.len(), 1, "{out2:?}");
        let raw2 = std::fs::read_to_string(root.join(&out2.inboxed[0])).expect("read");
        assert!(raw2.contains("tags: [hpc, tool]"), "{raw2}");
    }

    /// ADR-0047 D4: `merge`/`retire` は常に `_inbox/` へ（`confidence = high` でも直接コミットしない）。
    /// `medium`/`low` の `create`/`update` も同様。
    #[test]
    fn apply_candidates_routes_merge_retire_and_low_confidence_to_the_inbox() {
        let (_dir, root) = kb_dir();
        let merge = candidate(
            kb::CandidateOp::Merge,
            "environment/clusters/pegasus.md",
            Confidence::High,
        );
        let retire = candidate(
            kb::CandidateOp::Retire,
            "environment/clusters/pegasus.md",
            Confidence::High,
        );
        let mut medium = candidate(
            kb::CandidateOp::Create,
            "environment/tools/other.md",
            Confidence::Medium,
        );
        medium.path = "environment/tools/other.md".into();
        let out = apply_candidates(&root, "01JTASK", &[merge, retire, medium]);
        assert!(out.committed.is_empty(), "{out:?}");
        assert_eq!(out.inboxed.len(), 3, "{out:?}");
        assert_eq!(out.summary().inbox, 3);
        for path in &out.inboxed {
            assert!(kb::is_inbox(path), "{path}");
            let raw = std::fs::read_to_string(root.join(path)).expect("read");
            assert!(raw.contains("path: environment/"), "{raw}");
        }
    }

    /// ADR-0047 D4: 対象ページに人の未コミット編集があれば、`confidence = high` の `update` でも
    /// 直接コミットせず `_inbox/` へ。
    #[test]
    fn apply_candidates_sends_a_human_dirty_target_to_the_inbox() {
        let (_dir, root) = kb_dir();
        std::fs::write(
            root.join("environment/clusters/pegasus.md"),
            "---\ntitle: 人が今書いている\n---\n\n下書き\n",
        )
        .expect("write");
        let update = candidate(
            kb::CandidateOp::Update,
            "environment/clusters/pegasus.md",
            Confidence::High,
        );
        let out = apply_candidates(&root, "01JTASK", &[update]);
        assert!(out.committed.is_empty(), "{out:?}");
        assert_eq!(out.inboxed.len(), 1, "{out:?}");
        // 人の下書きは触らない。
        assert_eq!(
            std::fs::read_to_string(root.join("environment/clusters/pegasus.md")).expect("read"),
            "---\ntitle: 人が今書いている\n---\n\n下書き\n"
        );
    }

    /// ADR-0047 D4: 検査を通らない候補（出典なし・秘密）は落とす（どこにも書かない）。
    #[test]
    fn apply_candidates_drops_hard_invalid_candidates() {
        let (_dir, root) = kb_dir();
        let mut no_sources = candidate(
            kb::CandidateOp::Create,
            "environment/tools/x.md",
            Confidence::High,
        );
        no_sources.sources = vec![];
        let mut secret = candidate(
            kb::CandidateOp::Create,
            "environment/tools/y.md",
            Confidence::High,
        );
        secret.body = "API キーは sk-abc123def456 です".into();
        let escapes = candidate(
            kb::CandidateOp::Create,
            "../../etc/passwd",
            Confidence::High,
        );
        let out = apply_candidates(&root, "01JTASK", &[no_sources, secret, escapes]);
        assert!(out.committed.is_empty());
        assert!(out.inboxed.is_empty());
        assert_eq!(out.dropped.len(), 3, "{out:?}");
        assert_eq!(out.summary().discarded, 3);
        assert_eq!(out.summary().candidates, 3);
    }

    /// ADR-0047 D4: 適用は冪等（同じ high の候補をもう一度適用しても、中身が同じなら新しいコミットは作らない）。
    #[test]
    fn apply_candidates_is_idempotent_for_an_unchanged_high_confidence_candidate() {
        let (_dir, root) = kb_dir();
        let c = candidate(
            kb::CandidateOp::Create,
            "environment/tools/idempotent.md",
            Confidence::High,
        );
        let first = apply_candidates(&root, "01JTASK", std::slice::from_ref(&c));
        assert_eq!(first.committed.len(), 1);
        assert_eq!(history(&root, "environment/tools/idempotent.md").len(), 1);
        // 2 回目: op は create のままだが対象が既にあるので「create なのに既にある」= inbox へ。
        let second = apply_candidates(&root, "01JTASK", &[c]);
        assert!(second.committed.is_empty(), "{second:?}");
        assert_eq!(second.inboxed.len(), 1);
    }

    /// ADR-0047 D4: `_inbox` の accept は `op = merge` なら対象を必ず上書きし、`op = retire` なら
    /// 対象を `_retired/` へ動かす。
    #[test]
    fn inbox_accept_handles_merge_and_retire() {
        let (_dir, root) = kb_dir();
        // merge: 対象は既にある。上書きされる。
        let merge = candidate(
            kb::CandidateOp::Merge,
            "environment/clusters/pegasus.md",
            Confidence::Medium,
        );
        let out = apply_candidates(&root, "01JTASK", &[merge]);
        assert_eq!(out.inboxed.len(), 1);
        let merge_id = out.inboxed[0]
            .strip_prefix(&format!("{INBOX_DIR}/"))
            .and_then(|s| s.strip_suffix(".md"))
            .expect("id")
            .to_string();
        let accepted = inbox_accept(&root, &merge_id, None, false);
        match accepted {
            InboxOutcome::Accepted { path, .. } => {
                assert_eq!(path, "environment/clusters/pegasus.md")
            }
            other => panic!("{other:?}"),
        }
        let raw =
            std::fs::read_to_string(root.join("environment/clusters/pegasus.md")).expect("read");
        assert!(raw.contains("pjsub -L node=1"), "{raw}");

        // retire: 対象ページが `_retired/` へ動く。
        let retire = candidate(
            kb::CandidateOp::Retire,
            "environment/clusters/pegasus.md",
            Confidence::Medium,
        );
        let out2 = apply_candidates(&root, "01JTASK2", &[retire]);
        assert_eq!(out2.inboxed.len(), 1);
        let retire_id = out2.inboxed[0]
            .strip_prefix(&format!("{INBOX_DIR}/"))
            .and_then(|s| s.strip_suffix(".md"))
            .expect("id")
            .to_string();
        let retired = inbox_accept(&root, &retire_id, None, false);
        match retired {
            InboxOutcome::Accepted { path, .. } => {
                assert_eq!(
                    path,
                    format!("{RETIRED_DIR}/environment/clusters/pegasus.md")
                )
            }
            other => panic!("{other:?}"),
        }
        assert!(!root.join("environment/clusters/pegasus.md").exists());
        assert!(
            root.join(format!("{RETIRED_DIR}/environment/clusters/pegasus.md"))
                .exists()
        );
        // 退役したページは索引にも `_inbox` にも出ない。
        assert!(inbox_list(&root).is_empty());
        let index = reindex(&root).expect("reindex");
        assert!(
            !index.items.iter().any(|i| i.path.contains("pegasus")),
            "{index:?}"
        );
    }

    // -----------------------------------------------------------------
    // skills（ADR-0056 D3。Phase 78）
    // -----------------------------------------------------------------

    const SAMPLE_SKILL_MD: &str = "---\nname: rust-review\ndescription: Rust のコードレビューの手順\n---\n\n# rust-review\n\n手順...\n";

    #[test]
    fn skills_put_writes_the_frontmatter_name_and_description_and_adds_the_source() {
        let (_dir, root) = kb_dir();
        let path = skills_put(
            &root,
            "rust-review",
            SAMPLE_SKILL_MD,
            &[],
            Some("mcp:chatgpt"),
        )
        .expect("put");
        assert_eq!(path, "skills/rust-review/SKILL.md");
        let raw = std::fs::read_to_string(root.join(&path)).expect("read");
        assert!(raw.contains("name: rust-review"));
        assert!(raw.contains("source: mcp:chatgpt"), "{raw}");

        // 索引・検索・`list_pages` から見えない（ADR-0056 D3: 専用ディレクトリ）。
        let index = reindex(&root).expect("reindex");
        assert!(!index.items.iter().any(|i| i.path.starts_with("skills/")));
        assert!(list_pages(&root).iter().all(|p| !p.starts_with("skills/")));

        // 2 回目（source 込みで書いても）冪等: 既に `source:` があれば足さない。
        let path2 = skills_put(&root, "rust-review", &raw, &[], Some("mcp:other")).expect("put 2");
        let raw2 = std::fs::read_to_string(root.join(&path2)).expect("read 2");
        assert_eq!(raw2.matches("source:").count(), 1);
        assert!(raw2.contains("source: mcp:chatgpt"), "{raw2}");
    }

    #[test]
    fn skills_put_writes_attached_files_and_skills_get_lists_them() {
        let (_dir, root) = kb_dir();
        skills_put(
            &root,
            "rust-review",
            SAMPLE_SKILL_MD,
            &[("checklist.md".to_string(), "- fmt\n- clippy\n".to_string())],
            None,
        )
        .expect("put");
        let detail = skills_get(&root, "rust-review").expect("get");
        assert_eq!(detail.name, "rust-review");
        assert!(detail.skill_md.contains("rust-review"));
        assert_eq!(detail.files, vec!["checklist.md".to_string()]);

        let list = skills_list(&root);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "rust-review");
        assert_eq!(list[0].description, "Rust のコードレビューの手順");

        assert!(skills_get(&root, "does-not-exist").is_none());
    }

    #[test]
    fn skills_put_rejects_bad_names_missing_frontmatter_and_path_traversal() {
        let (_dir, root) = kb_dir();
        assert!(matches!(
            skills_put(&root, "Bad Name", SAMPLE_SKILL_MD, &[], None),
            Err(SkillError::InvalidName { .. })
        ));
        assert!(matches!(
            skills_put(&root, "rust-review", "no frontmatter here", &[], None),
            Err(SkillError::MissingFrontmatter)
        ));
        assert!(matches!(
            skills_put(
                &root,
                "rust-review",
                "---\nname: other\ndescription: x\n---\n",
                &[],
                None
            ),
            Err(SkillError::NameMismatch { .. })
        ));
        assert!(matches!(
            skills_put(
                &root,
                "rust-review",
                SAMPLE_SKILL_MD,
                &[("../escape.md".to_string(), "x".to_string())],
                None
            ),
            Err(SkillError::BadFilePath { .. })
        ));
    }

    /// Phase 82: `skills_delete` は `skills/<name>/` をまるごと消してコミットし、消えたことが
    /// `skills_get`/`skills_list` から見えなくなる。無い skill は `Failed`。
    #[test]
    fn skills_delete_removes_the_directory_and_commits() {
        let (_dir, root) = kb_dir();
        skills_put(
            &root,
            "rust-review",
            SAMPLE_SKILL_MD,
            &[("checklist.md".to_string(), "- fmt\n".to_string())],
            None,
        )
        .expect("put");
        assert!(root.join("skills/rust-review/SKILL.md").exists());

        let sha = skills_delete(&root, "rust-review").expect("delete");
        assert!(!sha.is_empty());
        assert!(!root.join("skills/rust-review").exists());
        assert!(skills_get(&root, "rust-review").is_none());
        assert!(skills_list(&root).is_empty());

        assert!(matches!(
            skills_delete(&root, "rust-review"),
            Err(SkillError::Failed(_))
        ));
    }

    /// Phase 82: `set_skill_mount` は名前を検証し、重複を足さず、外すと消える。
    #[test]
    fn set_skill_mount_toggles_without_duplicates_and_validates_the_name() {
        let mut mounts = vec!["writing".to_string()];
        set_skill_mount(&mut mounts, "rust-review", true).expect("mount");
        set_skill_mount(&mut mounts, "rust-review", true).expect("mount again");
        assert_eq!(
            mounts,
            vec!["writing".to_string(), "rust-review".to_string()]
        );

        set_skill_mount(&mut mounts, "writing", false).expect("unmount");
        set_skill_mount(&mut mounts, "writing", false).expect("unmount again (no-op)");
        assert_eq!(mounts, vec!["rust-review".to_string()]);

        let err = set_skill_mount(&mut mounts, "Not Valid", true).unwrap_err();
        assert!(matches!(err, SkillError::InvalidName { .. }));
        assert_eq!(
            mounts,
            vec!["rust-review".to_string()],
            "不正な名前は触らない"
        );
    }

    // ---- Phase K-1: 置き場のガード ----

    fn guard_layout(root: &Path) -> kb::Layout {
        layout(
            root,
            Some(vec![kb::ProjectRef {
                id: "01M2WTS3DKNZBSZ2JMVB4CZMBW".into(),
                slug: "agent-platform".into(),
                title: "agent-platform の自己改善".into(),
            }]),
        )
    }

    /// 知識整理 run の候補も `record` と同じガードを通る: environment 直下は落とし、案件 ID は slug に、
    /// 同じ題名の `create` は既存のページへの `append`（`_inbox/`）にする。
    #[test]
    fn apply_candidates_guard_the_placement() {
        let (_dir, root) = kb_dir();
        let layout = guard_layout(&root);
        let mut root_level = candidate(
            kb::CandidateOp::Create,
            "environment/pegasus-qwen.md",
            Confidence::High,
        );
        root_level.title = "Qwen forward target".into();
        let mut by_id = candidate(
            kb::CandidateOp::Create,
            "projects/01M2WTS3DKNZBSZ2JMVB4CZMBW/celeris.md",
            Confidence::High,
        );
        by_id.title = "Celeris direction".into();
        by_id.scope = "project:01M2WTS3DKNZBSZ2JMVB4CZMBW".into();
        let mut mismatch = candidate(kb::CandidateOp::Create, "user/x.md", Confidence::High);
        mismatch.title = "Mismatch".into();
        mismatch.scope = "project:agent-platform".into();
        // seed の `environment/clusters/pegasus.md` と同じ題名（「pegasus の使い方」）。
        let same_title = candidate(
            kb::CandidateOp::Create,
            "environment/clusters/pegasus-howto.md",
            Confidence::High,
        );
        let out = apply_candidates_in(
            &root,
            "01JTASK",
            &[root_level, by_id, mismatch, same_title],
            ApplyPolicy::Task,
            &layout,
        );
        assert_eq!(out.dropped.len(), 2, "{out:?}");
        assert!(out.dropped[0].1.contains("placement:"), "{out:?}");
        assert!(out.dropped[0].1.contains("environment/{"), "{out:?}");
        assert!(out.dropped[1].1.contains("does not match"), "{out:?}");
        assert_eq!(
            out.committed,
            vec!["projects/agent-platform/celeris.md".to_string()]
        );
        let raw =
            std::fs::read_to_string(root.join("projects/agent-platform/celeris.md")).expect("read");
        assert!(raw.contains("scope: \"project:agent-platform\""), "{raw}");
        assert!(!root.join("projects/01M2WTS3DKNZBSZ2JMVB4CZMBW").exists());
        assert_eq!(out.inboxed.len(), 1, "{out:?}");
        let id = out.inboxed[0]
            .strip_prefix("_inbox/")
            .and_then(|p| p.strip_suffix(".md"))
            .expect("id")
            .to_string();
        let item = inbox_get(&root, &id).expect("inbox");
        assert_eq!(item.op, Some(kb::CandidateOp::Append));
        assert_eq!(item.target, "environment/clusters/pegasus.md");
        assert!(!root.join("environment/clusters/pegasus-howto.md").exists());
    }

    /// `append` の accept は既存の本文を残して末尾に節を足す（tags / sources は和）。
    #[test]
    fn append_accept_keeps_the_existing_body() {
        let (_dir, root) = kb_dir();
        let layout = guard_layout(&root);
        let first = record_in(
            &root,
            &RecordRequest {
                title: "Celeris direction".into(),
                scope: "project:01M2WTS3DKNZBSZ2JMVB4CZMBW".into(),
                tags: vec!["celeris".into()],
                sources: vec!["human".into()],
                body: "# Celeris direction\n\n最初の方針。".into(),
                ..RecordRequest::default()
            },
            &layout,
        )
        .expect("record");
        assert_eq!(first.target, "projects/agent-platform/celeris-direction.md");
        assert_eq!(first.op, None);
        assert!(matches!(
            inbox_accept(&root, &first.id, None, false),
            InboxOutcome::Accepted { .. }
        ));
        let layout = guard_layout(&root);
        let second = record_in(
            &root,
            &RecordRequest {
                title: "Celeris direction".into(),
                scope: "project:agent-platform".into(),
                tags: vec!["research".into()],
                sources: vec!["mcp:chatgpt-rdc".into()],
                body: "# Celeris direction\n\n研究としての方針。".into(),
                ..RecordRequest::default()
            },
            &layout,
        )
        .expect("record");
        assert_eq!(second.target, first.target);
        assert_eq!(second.op, Some(kb::CandidateOp::Append));
        assert!(matches!(
            inbox_accept(&root, &second.id, None, false),
            InboxOutcome::Accepted { .. }
        ));
        let raw = std::fs::read_to_string(root.join(&first.target)).expect("read");
        assert!(raw.contains("最初の方針。"), "{raw}");
        assert!(raw.contains("## 追記（"), "{raw}");
        assert!(raw.contains("研究としての方針。"), "{raw}");
        assert_eq!(raw.matches("# Celeris direction").count(), 1, "{raw}");
        assert!(raw.contains("tags: [celeris, research]"), "{raw}");
        assert!(raw.contains("\"mcp:chatgpt-rdc\""), "{raw}");
        // 案件 ID の段がある取り込み先は、人が指定しても止める。
        let third = record_in(
            &root,
            &RecordRequest {
                title: "Other".into(),
                scope: "project:agent-platform".into(),
                sources: vec!["human".into()],
                body: "x".into(),
                ..RecordRequest::default()
            },
            &layout,
        )
        .expect("record");
        assert!(matches!(
            inbox_accept(
                &root,
                &third.id,
                Some("projects/01M2WTS3DKNZBSZ2JMVB4CZMBW/other.md"),
                false
            ),
            InboxOutcome::Failed { .. }
        ));
        // `record`（案件を知らない）でも案件 ID は拒否する。
        let offline = record(
            &root,
            &RecordRequest {
                title: "x".into(),
                scope: "project:01M2WTS3DKNZBSZ2JMVB4CZMBW".into(),
                sources: vec!["human".into()],
                body: "y".into(),
                ..RecordRequest::default()
            },
        );
        assert!(
            matches!(offline, Err(RecordError::Placement(_))),
            "{offline:?}"
        );
    }

    #[test]
    fn projects_readme_has_no_project_scope() {
        assert_eq!(default_scope("projects/README.md"), None);
        assert_eq!(
            default_scope("projects/agent-platform/design.md").as_deref(),
            Some("project:agent-platform")
        );
    }
}
