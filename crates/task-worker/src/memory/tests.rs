use super::*;

fn dir() -> (tempfile::TempDir, MemoryDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let memory = MemoryDir::new(tmp.path().join("memory"));
    (tmp, memory)
}

#[test]
fn nothing_written_yet_reads_as_empty() {
    let (_tmp, memory) = dir();
    assert_eq!(
        memory.load("secretary", Some("P1")),
        MemoryContext::default()
    );
    assert_eq!(memory.load("secretary", None), MemoryContext::default());
}

#[test]
fn the_result_file_memory_is_appended_as_dated_bullets() {
    let (_tmp, memory) = dir();
    let update = MemoryUpdate {
        notes: vec!["pegasus は pjsub で投げる".into(), "  ".into()],
        project: vec!["Pluvio は非同期ランタイム基盤\nらしい".into()],
    };
    memory
        .append("secretary", Some("P1"), &update, "2026-09-17")
        .expect("append");
    let loaded = memory.load("secretary", Some("P1"));
    assert_eq!(
        loaded.notes, "- 2026-09-17: pegasus は pjsub で投げる\n",
        "空白だけの項目は捨てる"
    );
    assert_eq!(
        loaded.project,
        "- 2026-09-17: Pluvio は非同期ランタイム基盤 らしい\n"
    );

    // 2 回目は追記（上書きしない）。
    memory
        .append(
            "secretary",
            Some("P1"),
            &MemoryUpdate {
                notes: vec!["人は図より表が好き".into()],
                project: vec![],
            },
            "2026-09-18",
        )
        .expect("append");
    let loaded = memory.load("secretary", Some("P1"));
    assert_eq!(loaded.notes.lines().count(), 2);
    assert!(loaded.notes.ends_with("- 2026-09-18: 人は図より表が好き\n"));
    // 別の案件の引き出しは混ざらない。
    assert_eq!(memory.load("secretary", Some("P2")).project, "");
    // 別のノードの記憶も混ざらない。
    assert_eq!(memory.load("research-survey", Some("P1")).notes, "");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(memory.root())
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700, "記憶のディレクトリは 0700");
    }
}

#[test]
fn an_empty_or_missing_memory_writes_nothing() {
    let (_tmp, memory) = dir();
    memory
        .append(
            "secretary",
            Some("P1"),
            &MemoryUpdate::default(),
            "2026-09-17",
        )
        .expect("append");
    memory
        .append(
            "secretary",
            Some("P1"),
            &MemoryUpdate {
                notes: vec![" ".into()],
                project: vec![],
            },
            "2026-09-17",
        )
        .expect("append");
    assert!(!memory.notes_path("secretary").exists());
    assert!(!memory.project_path("secretary", "P1").exists());
    // 案件が無い run の `project` は行き先が無いので捨てる（notes だけ残る）。
    memory
        .append(
            "secretary",
            None,
            &MemoryUpdate {
                notes: vec!["a".into()],
                project: vec!["b".into()],
            },
            "2026-09-17",
        )
        .expect("append");
    assert_eq!(memory.load("secretary", None).notes, "- 2026-09-17: a\n");
}

#[test]
fn the_preamble_is_cut_at_eight_thousand_characters_keeping_the_newest() {
    let (_tmp, memory) = dir();
    let items: Vec<String> = (0..500)
        .map(|i| format!("{i:03} {}", "あ".repeat(40)))
        .collect();
    memory
        .append(
            "secretary",
            Some("P1"),
            &MemoryUpdate {
                notes: items,
                project: vec![],
            },
            "2026-09-17",
        )
        .expect("append");
    let raw = std::fs::read_to_string(memory.notes_path("secretary")).expect("read");
    assert!(raw.chars().count() > MEMORY_MAX_CHARS);
    let loaded = memory.load("secretary", Some("P1"));
    assert!(loaded.notes.chars().count() <= MEMORY_MAX_CHARS + TRUNCATED_MARK.chars().count());
    assert!(
        loaded.notes.starts_with(TRUNCATED_MARK),
        "切ったことが分かる"
    );
    let last = loaded.notes.lines().next_back().unwrap_or_default();
    assert!(last.starts_with("- 2026-09-17: 499 "), "{last}");
    assert!(
        !loaded.notes.contains("- 2026-09-17: 000 "),
        "古い方から落ちる"
    );
}

#[test]
fn the_result_file_is_read_leniently() {
    let (tmp, _memory) = dir();
    // ADR-0036 D2: 読むのは成果物ディレクトリの `result.json`。
    let artifacts = tmp.path().join("ws").join("artifacts");
    std::fs::create_dir_all(&artifacts).expect("mkdir");
    // ファイルが無い
    assert_eq!(read_result_memory(&tmp.path().join("nowhere")), None);
    // `memory` が無い
    std::fs::write(
        artifacts.join("result.json"),
        r#"{"summary":"ok","evidence":[]}"#,
    )
    .expect("write");
    assert_eq!(read_result_memory(&artifacts), None);
    // 形が違う（配列でない）
    std::fs::write(
        artifacts.join("result.json"),
        r#"{"summary":"ok","memory":{"notes":"x"}}"#,
    )
    .expect("write");
    assert_eq!(read_result_memory(&artifacts), None);
    // 空の配列は「何も無い」
    std::fs::write(
        artifacts.join("result.json"),
        r#"{"summary":"ok","memory":{"notes":[],"project":[]}}"#,
    )
    .expect("write");
    assert_eq!(read_result_memory(&artifacts), None);
    // 正常
    std::fs::write(
        artifacts.join("result.json"),
        r#"{"summary":"ok","memory":{"notes":["a"],"project":["b","c"]}}"#,
    )
    .expect("write");
    let update = read_result_memory(&artifacts).expect("memory");
    assert_eq!(update.notes, vec!["a".to_string()]);
    assert_eq!(update.project, vec!["b".to_string(), "c".to_string()]);
    // JSON ですらない
    std::fs::write(artifacts.join("result.json"), "not json").expect("write");
    assert_eq!(read_result_memory(&artifacts), None);
}
