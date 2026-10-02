use super::*;

/// `<root>/<sha>/` を作り、渡した JSON を置く（`None` はファイルを作らない）。
fn release(
    root: &Path,
    sha: &str,
    manifest: Option<&str>,
    gate: Option<&str>,
    verify: Option<&str>,
) {
    let dir = root.join(sha);
    std::fs::create_dir_all(&dir).expect("mkdir");
    for (name, body) in [
        ("manifest.json", manifest),
        ("gate.json", gate),
        ("verify.json", verify),
    ] {
        if let Some(body) = body {
            std::fs::write(dir.join(name), body).expect("write");
        }
    }
}

/// 実行ビット付きの偽 `scripts/promote.sh`（1 行書いて眠るだけ。本物の昇格は起きない）。
fn fake_script(root: &Path, sha: &str, marker: &str) {
    use std::os::unix::fs::PermissionsExt;
    let scripts = root.join(sha).join("scripts");
    std::fs::create_dir_all(&scripts).expect("mkdir");
    let script = scripts.join("promote.sh");
    std::fs::write(
        &script,
        format!("#!/bin/sh\nprintf '{marker} %s\\n' \"$1\"\nsleep 2\n"),
    )
    .expect("write");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

/// tempdir に `main` を持つ git リポジトリを作り、(main の sha, main に居ない sha) を返す。
/// git が無い環境では `None`（テストは飛ばす）。
fn git_repo(dir: &Path) -> Option<(String, String)> {
    let git = |args: &[&str]| -> Option<String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.email=t@example.invalid", "-c", "user.name=t"])
            .args(args)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    std::fs::create_dir_all(dir).expect("mkdir");
    git(&["init", "-q", "-b", "main"])?;
    git(&["commit", "-q", "--allow-empty", "-m", "on main"])?;
    let on_main_sha = git(&["rev-parse", "HEAD"])?;
    git(&["checkout", "-q", "-b", "side"])?;
    git(&["commit", "-q", "--allow-empty", "-m", "not on main"])?;
    let off_main_sha = git(&["rev-parse", "HEAD"])?;
    git(&["checkout", "-q", "main"])?;
    Some((on_main_sha, off_main_sha))
}

fn env() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("releases");
    std::fs::create_dir_all(&root).expect("mkdir");
    (dir, root)
}

#[test]
fn scanning_an_empty_or_missing_directory_yields_nothing_and_does_not_fail() {
    let (dir, root) = env();
    let scanned = scan(&root, None);
    assert!(scanned.items.is_empty());
    assert_eq!(scanned.current, None);
    assert_eq!(scanned.previous, None);
    // ディレクトリごと無いとき（初回）も同じ。
    let missing = dir.path().join("nope");
    assert!(scan(&missing, None).items.is_empty());
}

#[test]
fn two_releases_are_sorted_newest_first_with_the_symlinks_applied() {
    let (dir, root) = env();
    release(
        &root,
        "aaaaaaaaaaaa",
        Some(r#"{"ref":"main","built_at":"2026-09-18T00:00:00Z","schema_version":10}"#),
        Some(r#"{"ok":true}"#),
        Some(r#"{"ok":true,"live_ok":false,"at":"2026-09-18T01:00:00Z"}"#),
    );
    release(
        &root,
        "bbbbbbbbbbbb",
        Some(r#"{"ref":"self/01M","built_at":"2026-09-19T00:00:00Z","schema_version":11}"#),
        Some(r#"{"ok":true}"#),
        None,
    );
    // `~/.local/celeris/current -> releases/aaaaaaaaaaaa`（releases_dir の親に張る）。
    std::os::unix::fs::symlink("releases/aaaaaaaaaaaa", dir.path().join("current"))
        .expect("symlink");
    std::os::unix::fs::symlink("releases/bbbbbbbbbbbb", dir.path().join("previous"))
        .expect("symlink");

    let scanned = scan(&root, None);
    assert_eq!(scanned.current.as_deref(), Some("aaaaaaaaaaaa"));
    assert_eq!(scanned.previous.as_deref(), Some("bbbbbbbbbbbb"));
    assert_eq!(
        scanned
            .items
            .iter()
            .map(|i| i.sha12.as_str())
            .collect::<Vec<_>>(),
        ["bbbbbbbbbbbb", "aaaaaaaaaaaa"],
        "built_at の新しい順"
    );

    let newest = &scanned.items[0];
    assert_eq!(newest.r#ref.as_deref(), Some("self/01M"));
    assert_eq!(newest.schema_version, Some(11));
    assert!(newest.gate_ok);
    assert!(newest.verify.is_none(), "verify.json が無ければ未検証");
    assert!(newest.is_previous && !newest.is_current);
    assert!(!newest.promoting);
    assert_eq!(newest.problem, None);

    let older = &scanned.items[1];
    let verify = older.verify.as_ref().expect("verify");
    assert!(verify.ok && !verify.live_ok);
    assert_eq!(verify.at.as_deref(), Some("2026-09-18T01:00:00Z"));
    assert!(older.is_current);
}

#[test]
fn broken_json_and_build_directories_do_not_break_the_scan() {
    let (_dir, root) = env();
    release(
        &root,
        "cccccccccccc",
        Some("{ this is not json"),
        None,
        None,
    );
    // `.build` / `.cargo-target` / `<sha>.partial` は一覧に出ない。
    std::fs::create_dir_all(root.join(".build").join("dddddddddddd")).expect("mkdir");
    std::fs::create_dir_all(root.join(".cargo-target")).expect("mkdir");
    std::fs::create_dir_all(root.join("eeeeeeeeeeee.partial")).expect("mkdir");
    std::fs::write(root.join("stray.txt"), "x").expect("write");

    let scanned = scan(&root, None);
    assert_eq!(scanned.items.len(), 1, "{:?}", scanned.items);
    let item = &scanned.items[0];
    assert_eq!(item.sha12, "cccccccccccc");
    assert!(!item.gate_ok);
    assert_eq!(item.built_at, None);
    let problem = item.problem.as_deref().expect("problem");
    assert!(problem.contains("manifest.json"), "{problem}");
    assert!(problem.contains("gate.json"), "{problem}");
}

#[test]
fn sha12_validation_rejects_path_traversal() {
    assert!(valid_sha12("aaaaaaaaaaaa"));
    assert!(valid_sha12("0123456789ab"));
    assert!(!valid_sha12(""));
    assert!(!valid_sha12("../escape"));
    assert!(!valid_sha12("aaaa"));
    assert!(!valid_sha12("zzzzzzzzzzzz"));
    assert!(!valid_sha12(&"a".repeat(41)));
}

#[test]
fn promotion_is_refused_for_unknown_unverified_current_and_running_releases() {
    let (dir, root) = env();
    // 知らない sha / 形が違う sha。
    assert_eq!(
        start_promote(&root, "aaaaaaaaaaaa", "inline"),
        Err(ReleasePromoteError::NotFound)
    );
    assert_eq!(
        start_promote(&root, "../etc", "inline"),
        Err(ReleasePromoteError::NotFound)
    );

    // verify.json が無い。
    release(
        &root,
        "aaaaaaaaaaaa",
        Some(r#"{"built_at":"2026-09-18T00:00:00Z"}"#),
        Some(r#"{"ok":true}"#),
        None,
    );
    assert!(matches!(
        start_promote(&root, "aaaaaaaaaaaa", "inline"),
        Err(ReleasePromoteError::NotVerified(_))
    ));
    // verify.json は在るが ok ではない。
    std::fs::write(
        root.join("aaaaaaaaaaaa").join("verify.json"),
        r#"{"ok":false,"live_ok":false}"#,
    )
    .expect("write");
    assert!(matches!(
        start_promote(&root, "aaaaaaaaaaaa", "inline"),
        Err(ReleasePromoteError::NotVerified(_))
    ));

    // 検証済みだが既に current。
    std::fs::write(
        root.join("aaaaaaaaaaaa").join("verify.json"),
        r#"{"ok":true,"live_ok":true}"#,
    )
    .expect("write");
    std::os::unix::fs::symlink("releases/aaaaaaaaaaaa", dir.path().join("current"))
        .expect("symlink");
    assert_eq!(
        start_promote(&root, "aaaaaaaaaaaa", "inline"),
        Err(ReleasePromoteError::AlreadyCurrent)
    );

    // current ではないが `scripts/promote.sh` が無い（Phase 48 より前に作られたリリース）。
    release(
        &root,
        "bbbbbbbbbbbb",
        Some(r#"{"built_at":"2026-09-19T00:00:00Z"}"#),
        Some(r#"{"ok":true}"#),
        Some(r#"{"ok":true,"live_ok":true}"#),
    );
    assert!(matches!(
        start_promote(&root, "bbbbbbbbbbbb", "inline"),
        Err(ReleasePromoteError::Unavailable(_))
    ));

    // 昇格中（生きている pid の promote.lock）。自分自身の pid を使う。
    let scripts = root.join("bbbbbbbbbbbb").join("scripts");
    std::fs::create_dir_all(&scripts).expect("mkdir");
    std::fs::write(scripts.join("promote.sh"), "#!/bin/sh\nexit 0\n").expect("write");
    std::fs::write(
        root.join("bbbbbbbbbbbb").join("promote.lock"),
        format!("{}\n", std::process::id()),
    )
    .expect("write");
    assert_eq!(
        start_promote(&root, "bbbbbbbbbbbb", "inline"),
        Err(ReleasePromoteError::AlreadyPromoting)
    );
}

/// 偽の `scripts/promote.sh`（ログに 1 行書いて少し眠る）を detached で起こす。
/// **本物の昇格は起きない**（本番のパスにもプロセスにも触れない）。
#[test]
fn promoting_spawns_the_bundled_script_detached_and_writes_the_lock() {
    use std::os::unix::fs::PermissionsExt;

    let (_dir, root) = env();
    release(
        &root,
        "abcdef123456",
        Some(r#"{"built_at":"2026-09-19T00:00:00Z"}"#),
        Some(r#"{"ok":true}"#),
        Some(r#"{"ok":true,"live_ok":true}"#),
    );
    let scripts = root.join("abcdef123456").join("scripts");
    std::fs::create_dir_all(&scripts).expect("mkdir");
    let script = scripts.join("promote.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\nprintf 'promote %s\\n' \"$1\"\nsleep 2\n",
    )
    .expect("write");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    // `Inline` を明示して、この開発環境に `systemd-run`/`XDG_RUNTIME_DIR` があっても
    // 本物の systemd-run を呼ばないようにする（実行は禁止。テストは常に偽物か `Inline`）。
    let accepted =
        start_promote_with_launcher(&root, "abcdef123456", &DetachLauncher::Inline).expect("202");
    assert_eq!(accepted.sha12, "abcdef123456");
    assert!(
        accepted.log.ends_with("abcdef123456/promote.log"),
        "{}",
        accepted.log
    );
    assert!(accepted.started_at.contains('T'));

    // lock に pid が書かれ、ログに 1 行出る（少し待つ）。
    let lock = root.join("abcdef123456").join("promote.lock");
    let log = root.join("abcdef123456").join("promote.log");
    let mut logged = String::new();
    for _ in 0..100 {
        logged = std::fs::read_to_string(&log).unwrap_or_default();
        if logged.contains("promote abcdef123456") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(logged.contains("promote abcdef123456"), "{logged:?}");
    let pid: u32 = std::fs::read_to_string(&lock)
        .expect("lock")
        .trim()
        .parse()
        .expect("pid");
    assert!(pid > 0);

    // 走っている間は 409（二重に起こさない）。
    assert_eq!(
        start_promote(&root, "abcdef123456", "inline"),
        Err(ReleasePromoteError::AlreadyPromoting)
    );
    // 一覧にも `promoting = true` で出る。
    let scanned = scan(&root, None);
    assert!(
        scanned
            .items
            .iter()
            .any(|i| i.sha12 == "abcdef123456" && i.promoting)
    );
}

/// ADR-0041 D4: 昇格に使うのは **`current` に同梱された** `promote.sh`。
/// `current` がそれを持っていないとき（Phase 48 以前・初回）だけ昇格先のものを使う。
#[test]
fn promotion_runs_the_promote_script_of_the_current_release() {
    let (dir, root) = env();
    release(
        &root,
        "aaaaaaaaaaaa",
        Some(r#"{"built_at":"2026-09-18T00:00:00Z"}"#),
        Some(r#"{"ok":true}"#),
        Some(r#"{"ok":true,"live_ok":true}"#),
    );
    release(
        &root,
        "bbbbbbbbbbbb",
        Some(r#"{"built_at":"2026-09-19T00:00:00Z"}"#),
        Some(r#"{"ok":true}"#),
        Some(r#"{"ok":true,"live_ok":true}"#),
    );
    std::os::unix::fs::symlink("releases/aaaaaaaaaaaa", dir.path().join("current"))
        .expect("symlink");

    // (1) current にも昇格先にも `scripts/` が無い → 409。
    assert!(matches!(
        start_promote(&root, "bbbbbbbbbbbb", "inline"),
        Err(ReleasePromoteError::Unavailable(_))
    ));

    // (2) 昇格先にだけある（current は Phase 48 以前）→ 昇格先のものを使う。
    // `Inline` を明示（本物の systemd-run を呼ばない）。
    fake_script(&root, "bbbbbbbbbbbb", "target-script");
    let accepted =
        start_promote_with_launcher(&root, "bbbbbbbbbbbb", &DetachLauncher::Inline).expect("202");
    assert_eq!(accepted.script_from, "target");
    let log = root.join("bbbbbbbbbbbb").join("promote.log");
    let mut logged = String::new();
    for _ in 0..100 {
        logged = std::fs::read_to_string(&log).unwrap_or_default();
        if logged.contains("target-script") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(logged.contains("target-script bbbbbbbbbbbb"), "{logged:?}");

    // (3) current も持っている → **current のもの**を使う（実装者が昇格先の promote.sh を
    //     壊しても、それは走らない）。前の昇格のロックは消してから。
    std::fs::remove_file(root.join("bbbbbbbbbbbb").join("promote.lock")).expect("rm lock");
    std::fs::remove_file(&log).expect("rm log");
    fake_script(&root, "aaaaaaaaaaaa", "current-script");
    let accepted =
        start_promote_with_launcher(&root, "bbbbbbbbbbbb", &DetachLauncher::Inline).expect("202");
    assert_eq!(accepted.script_from, "current");
    // ログは**昇格先**の promote.log（人が見る場所は変わらない）。
    assert!(
        accepted.log.ends_with("bbbbbbbbbbbb/promote.log"),
        "{}",
        accepted.log
    );
    for _ in 0..100 {
        logged = std::fs::read_to_string(&log).unwrap_or_default();
        if logged.contains("current-script") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(logged.contains("current-script bbbbbbbbbbbb"), "{logged:?}");
}

/// ADR-0041 D3: `promoted.json` が `promoted_at` になる。無ければ `null`。
#[test]
fn promoted_json_becomes_promoted_at() {
    let (_dir, root) = env();
    release(
        &root,
        "aaaaaaaaaaaa",
        Some(r#"{"built_at":"2026-09-18T00:00:00Z"}"#),
        Some(r#"{"ok":true}"#),
        None,
    );
    std::fs::write(
        root.join("aaaaaaaaaaaa").join("promoted.json"),
        r#"{"promoted_at":"2026-09-19T12:31:36Z","mode":"stop-start","from":null}"#,
    )
    .expect("write");
    release(
        &root,
        "bbbbbbbbbbbb",
        Some(r#"{"built_at":"2026-09-19T00:00:00Z"}"#),
        Some(r#"{"ok":true}"#),
        None,
    );

    let scanned = scan(&root, None);
    let by = |sha: &str| {
        scanned
            .items
            .iter()
            .find(|i| i.sha12 == sha)
            .expect("item")
            .clone()
    };
    assert_eq!(
        by("aaaaaaaaaaaa").promoted_at.as_deref(),
        Some("2026-09-19T12:31:36Z")
    );
    assert_eq!(
        by("bbbbbbbbbbbb").promoted_at,
        None,
        "昇格していないリリースは null"
    );
    // 壊れた promoted.json でも落ちない。
    std::fs::write(root.join("bbbbbbbbbbbb").join("promoted.json"), "{ broken").expect("write");
    assert_eq!(scan(&root, None).items.len(), 2);
}

/// バグ報告（2026-09-21）: 昇格ボタンを押したあと、失敗しても GUI に何も出なかった。
/// `promote.sh` は `set -e` で `sd_die` した瞬間にただ死ぬだけで、成否をどこにも残していなかった
/// （`promoted.json` は成功のときだけ）。`promote.lock` の pid が消えれば `promoting` は偽に戻るので、
/// 「昇格が終わった」ようにしか見えなかった。`promote_failed.json` を読んで `promote_failed` に写す。
#[test]
fn promote_failed_json_becomes_promote_failed() {
    let (_dir, root) = env();
    release(
        &root,
        "aaaaaaaaaaaa",
        Some(r#"{"built_at":"2026-09-18T00:00:00Z"}"#),
        Some(r#"{"ok":true}"#),
        None,
    );
    std::fs::write(
            root.join("aaaaaaaaaaaa").join("promote_failed.json"),
            r#"{"failed_at":"2026-09-19T12:31:36Z","error":"promote.sh: ERROR: old celeris is still serving"}"#,
        )
        .expect("write");
    release(
        &root,
        "bbbbbbbbbbbb",
        Some(r#"{"built_at":"2026-09-19T00:00:00Z"}"#),
        Some(r#"{"ok":true}"#),
        None,
    );

    let scanned = scan(&root, None);
    let by = |sha: &str| {
        scanned
            .items
            .iter()
            .find(|i| i.sha12 == sha)
            .expect("item")
            .clone()
    };
    let failed = by("aaaaaaaaaaaa").promote_failed.expect("promote_failed");
    assert_eq!(failed.failed_at, "2026-09-19T12:31:36Z");
    assert!(failed.error.contains("old celeris is still serving"));
    assert_eq!(
        by("bbbbbbbbbbbb").promote_failed,
        None,
        "失敗したことが無いリリースは null"
    );
    // 壊れた promote_failed.json でも落ちない。
    std::fs::write(
        root.join("bbbbbbbbbbbb").join("promote_failed.json"),
        "{ broken",
    )
    .expect("write");
    assert_eq!(scan(&root, None).items.len(), 2);
}

/// 前回の失敗の印は、次の昇格の試みが始まると消える（残ったままだと新しい試みが進行中でも
/// 赤いバナーが出続けてしまう）。
#[test]
fn starting_a_new_promotion_clears_the_previous_failure_marker() {
    let (_dir, root) = env();
    release(
        &root,
        "abcdef123456",
        Some(r#"{"built_at":"2026-09-19T00:00:00Z"}"#),
        Some(r#"{"ok":true}"#),
        Some(r#"{"ok":true,"live_ok":true}"#),
    );
    std::fs::write(
        root.join("abcdef123456").join("promote_failed.json"),
        r#"{"failed_at":"2026-09-19T12:00:00Z","error":"boom"}"#,
    )
    .expect("write");
    fake_script(&root, "abcdef123456", "retry");

    start_promote_with_launcher(&root, "abcdef123456", &DetachLauncher::Inline).expect("202");

    assert!(
        !root
            .join("abcdef123456")
            .join("promote_failed.json")
            .exists(),
        "start_promote は前回の失敗の印を消す"
    );
}

/// ADR-0041 D4: `changes.json` が `changes` になる。`stale` は**いまの** `current` と比べて決める。
/// `changes.json` が無いリリース（Phase 48 以前）は `null`。
#[test]
fn changes_json_becomes_changes_with_stale_computed_against_current() {
    let (dir, root) = env();
    release(
        &root,
        "aaaaaaaaaaaa",
        Some(r#"{"built_at":"2026-09-18T00:00:00Z"}"#),
        Some(r#"{"ok":true}"#),
        None,
    );
    release(
        &root,
        "bbbbbbbbbbbb",
        Some(r#"{"built_at":"2026-09-19T00:00:00Z"}"#),
        Some(r#"{"ok":true}"#),
        None,
    );
    std::fs::write(
        root.join("bbbbbbbbbbbb").join("changes.json"),
        r#"{"base":"aaaaaaaaaaaa",
                "commits":[{"sha":"1111111111111111111111111111111111111111","subject":"phase 50"},
                           {"sha":"2222222222222222222222222222222222222222","subject":"adr-0041"}],
                "files":["crates/celeris/src/releases.rs","docs/PROGRESS.md","README.md"],
                "sensitive":["crates/celeris/src/releases.rs"]}"#,
    )
    .expect("write");
    std::os::unix::fs::symlink("releases/aaaaaaaaaaaa", dir.path().join("current"))
        .expect("symlink");

    let scanned = scan(&root, None);
    let newest = &scanned.items[0];
    assert_eq!(newest.sha12, "bbbbbbbbbbbb");
    let changes = newest.changes.as_ref().expect("changes");
    assert_eq!(changes.base.as_deref(), Some("aaaaaaaaaaaa"));
    assert!(!changes.stale, "base == current なら stale ではない");
    assert_eq!(changes.commit_count, 2);
    assert_eq!(changes.file_count, 3);
    assert_eq!(changes.sensitive, ["crates/celeris/src/releases.rs"]);
    assert_eq!(
        changes.commits[0].sha,
        "1111111111111111111111111111111111111111"
    );
    assert_eq!(changes.commits[0].subject, "phase 50");
    // `changes.json` が無いリリース（Phase 48 以前）は `null`。
    assert!(scanned.items[1].changes.is_none());

    // `current` が動いたら stale になる（差分の起点がもう「いま」ではない）。
    std::fs::remove_file(dir.path().join("current")).expect("rm");
    std::os::unix::fs::symlink("releases/cccccccccccc", dir.path().join("current"))
        .expect("symlink");
    let stale = scan(&root, None).items[0].changes.clone().expect("changes");
    assert!(stale.stale);
}

/// ADR-0058: `verify.json` の `checks[]` と `gate.json` の `steps[]` が `ReleaseVerify.checks` /
/// `ReleaseItem.gate` にそのまま写る。
#[test]
fn verify_checks_and_gate_steps_are_carried_through() {
    let (_dir, root) = env();
    release(
        &root,
        "aaaaaaaaaaaa",
        Some(r#"{"built_at":"2026-09-22T00:00:00Z"}"#),
        None,
        None,
    );
    std::fs::write(
        root.join("aaaaaaaaaaaa").join("gate.json"),
        r#"{"ok":false,"failed_step":"cargo-test",
                "steps":[{"step":"cargo-workspace-clean","exit":0,"secs":1.2,"log":".gate-x.log"},
                         {"step":"cargo-test","exit":101,"secs":42.5,"log":".gate-y.log"}]}"#,
    )
    .expect("write");
    std::fs::write(
            root.join("aaaaaaaaaaaa").join("verify.json"),
            r#"{"ok":false,"live_ok":false,"at":"2026-09-22T01:00:00Z",
                "checks":[{"id":"1","name":"boot","ok":true,"detail":"started","task_id":"","elapsed_s":0},
                          {"id":"6","name":"smoke","ok":false,"detail":"timed out","task_id":"01K…","elapsed_s":60.3}]}"#,
        )
        .expect("write");

    let scanned = scan(&root, None);
    let item = scanned.items.first().expect("item");

    let gate = item.gate.as_ref().expect("gate");
    assert!(!gate.ok);
    assert_eq!(gate.failed_step.as_deref(), Some("cargo-test"));
    assert_eq!(gate.steps.len(), 2);
    assert_eq!(gate.steps[0].step, "cargo-workspace-clean");
    assert_eq!(gate.steps[0].exit, 0);
    assert!((gate.steps[0].secs - 1.2).abs() < f64::EPSILON);
    assert_eq!(gate.steps[1].step, "cargo-test");
    assert_eq!(gate.steps[1].exit, 101);

    let verify = item.verify.as_ref().expect("verify");
    assert_eq!(verify.checks.len(), 2);
    assert_eq!(verify.checks[0].id, "1");
    assert_eq!(verify.checks[0].name, "boot");
    assert!(verify.checks[0].ok);
    assert_eq!(verify.checks[1].id, "6");
    assert!(!verify.checks[1].ok);
    assert_eq!(verify.checks[1].detail, "timed out");
    assert_eq!(verify.checks[1].elapsed_s, Some(60.3));
}

/// ADR-0058 D3: `checks`/`steps` が無い（この Phase 以前に作られたリリース、または壊れた
/// `gate.json`）ときは、一覧を落とさずに空配列 / `None` になる。
#[test]
fn missing_checks_and_gate_default_to_empty_without_breaking_the_scan() {
    let (_dir, root) = env();
    // 検証済みだが `checks` を持たない古い形の verify.json。
    release(
        &root,
        "aaaaaaaaaaaa",
        Some(r#"{"built_at":"2026-09-18T00:00:00Z"}"#),
        Some(r#"{"ok":true}"#),
        Some(r#"{"ok":true,"live_ok":true,"at":"2026-09-18T01:00:00Z"}"#),
    );
    // 壊れた gate.json（`ok` が読めない）。
    release(
        &root,
        "bbbbbbbbbbbb",
        Some(r#"{"built_at":"2026-09-19T00:00:00Z"}"#),
        Some("{ not json"),
        None,
    );

    let scanned = scan(&root, None);
    let by = |sha: &str| {
        scanned
            .items
            .iter()
            .find(|i| i.sha12 == sha)
            .expect("item")
            .clone()
    };

    let old = by("aaaaaaaaaaaa");
    assert!(
        old.verify.as_ref().expect("verify").checks.is_empty(),
        "checks キーが無い verify.json は空配列"
    );
    // gate.json 自体（`{"ok":true}`）は壊れていない（`steps` キーが無いだけ）ので、
    // `gate` は Some のまま、`steps` だけが空配列になる。
    let old_gate = old.gate.as_ref().expect("gate.json の ok は読める");
    assert!(old_gate.ok);
    assert!(old_gate.steps.is_empty());
    assert_eq!(old_gate.failed_step, None);

    let broken = by("bbbbbbbbbbbb");
    assert!(!broken.gate_ok, "壊れた gate.json は gate_ok=false のまま");
    assert!(broken.gate.is_none(), "壊れた gate.json は gate も None");
    assert!(
        broken.verify.is_none(),
        "verify.json が無ければ verify は None"
    );
}

/// P-94-1: `release.sh` はゲート成功時に `gate.json` に `failed_step: ""` を書く
/// （本番で確認済み）。空文字（トリム後に空でも）は `None` に正規化する。失敗時の非空文字列は
/// そのまま運び、キー自体が無いときも（既存の後方互換どおり）`None` になる。
#[test]
fn gate_failed_step_empty_string_is_normalized_to_none() {
    let (_dir, root) = env();
    // 成功時: `failed_step: ""`。
    release(
        &root,
        "aaaaaaaaaaaa",
        Some(r#"{"built_at":"2026-09-22T00:00:00Z"}"#),
        Some(r#"{"ok":true,"failed_step":"","steps":[]}"#),
        None,
    );
    // 空白のみ（トリム後に空）。
    release(
        &root,
        "bbbbbbbbbbbb",
        Some(r#"{"built_at":"2026-09-22T00:00:01Z"}"#),
        Some(r#"{"ok":true,"failed_step":"   ","steps":[]}"#),
        None,
    );
    // 失敗時: 非空文字列はそのまま `Some`。
    release(
        &root,
        "cccccccccccc",
        Some(r#"{"built_at":"2026-09-22T00:00:02Z"}"#),
        Some(r#"{"ok":false,"failed_step":"cargo-test","steps":[]}"#),
        None,
    );
    // キー自体が無い（従来どおり `None`）。
    release(
        &root,
        "dddddddddddd",
        Some(r#"{"built_at":"2026-09-22T00:00:03Z"}"#),
        Some(r#"{"ok":true,"steps":[]}"#),
        None,
    );

    let scanned = scan(&root, None);
    let gate_of = |sha: &str| {
        scanned
            .items
            .iter()
            .find(|i| i.sha12 == sha)
            .and_then(|i| i.gate.clone())
            .expect("gate")
    };

    assert_eq!(
        gate_of("aaaaaaaaaaaa").failed_step,
        None,
        "空文字は None に正規化される"
    );
    assert_eq!(
        gate_of("bbbbbbbbbbbb").failed_step,
        None,
        "空白のみもトリム後に空なので None"
    );
    assert_eq!(
        gate_of("cccccccccccc").failed_step,
        Some("cargo-test".to_string()),
        "非空文字列はそのまま運ぶ"
    );
    assert_eq!(
        gate_of("dddddddddddd").failed_step,
        None,
        "キー欠落は従来どおり None"
    );
}

/// ADR-0041 D3: `on_main` は `git merge-base --is-ancestor <sha> main`。
/// リポジトリが無い・その sha を知らないときは `null`（一覧は落ちない）。
#[test]
fn on_main_is_true_false_or_null() {
    let (dir, root) = env();
    let repo = dir.path().join("repo");
    let Some((merged, unmerged)) = git_repo(&repo) else {
        eprintln!("git is not usable here; skipping");
        return;
    };

    release(
        &root,
        "aaaaaaaaaaaa",
        Some(&format!(
            r#"{{"sha":"{merged}","built_at":"2026-09-18T00:00:00Z"}}"#
        )),
        Some(r#"{"ok":true}"#),
        None,
    );
    release(
        &root,
        "bbbbbbbbbbbb",
        Some(&format!(
            r#"{{"sha":"{unmerged}","built_at":"2026-09-19T00:00:00Z"}}"#
        )),
        Some(r#"{"ok":true}"#),
        None,
    );
    // このリポジトリが知らない sha（別のチェックアウトでビルドした版）。
    release(
        &root,
        "cccccccccccc",
        Some(
            r#"{"sha":"deadbeefdeadbeefdeadbeefdeadbeefdeadbeef","built_at":"2026-09-17T00:00:00Z"}"#,
        ),
        Some(r#"{"ok":true}"#),
        None,
    );

    let scanned = scan(&root, Some(&repo));
    let by = |sha: &str| {
        scanned
            .items
            .iter()
            .find(|i| i.sha12 == sha)
            .expect("item")
            .clone()
    };
    assert_eq!(by("aaaaaaaaaaaa").on_main, Some(true), "main の祖先");
    assert_eq!(
        by("bbbbbbbbbbbb").on_main,
        Some(false),
        "main に入っていない"
    );
    assert_eq!(
        by("cccccccccccc").on_main,
        None,
        "このリポジトリが知らない sha"
    );

    // リポジトリが無ければ全部 null（`git` を 1 回試して諦める）。
    let missing = dir.path().join("no-such-repo");
    assert!(
        scan(&root, Some(&missing))
            .items
            .iter()
            .all(|i| i.on_main.is_none())
    );
    // `repo` を渡さなければそもそも見ない。
    assert!(scan(&root, None).items.iter().all(|i| i.on_main.is_none()));
}

// ---- Phase 105（ADR-0060 D1 追記）: `promote.sh` を systemd-run --scope で起こす ----

/// `promote_exec_command` が組み立てる argv の形（純関数。プロセスは起こさない）。
#[test]
fn promote_exec_command_wraps_with_systemd_run() {
    let script = Path::new("/releases/current/scripts/promote.sh");
    let launcher = DetachLauncher::SystemdRun {
        program: "systemd-run".to_string(),
    };
    let (program, args) = promote_exec_command(&launcher, script, "abcdef123456");
    assert_eq!(program, "systemd-run");
    assert_eq!(
        &args[..3],
        &[
            "--user".to_string(),
            "--scope".to_string(),
            "--quiet".to_string()
        ]
    );
    assert_eq!(args[3], "--unit");
    assert!(
        args[4].starts_with("celeris-promote-abcdef123456-"),
        "{}",
        args[4]
    );
    assert_eq!(args[5], "--description");
    assert_eq!(args[6], "celeris promote abcdef123456");
    assert_eq!(args[7], "--");
    assert_eq!(args[8], "sh");
    assert_eq!(args[9], "-c");
    assert_eq!(
        args[10],
        "'/releases/current/scripts/promote.sh' 'abcdef123456'"
    );
}

/// `Inline` は従来どおり（`setsid <script> <sha12>`）。
#[test]
fn promote_exec_command_inline_is_setsid() {
    let script = Path::new("/releases/current/scripts/promote.sh");
    let (program, args) = promote_exec_command(&DetachLauncher::Inline, script, "abcdef123456");
    assert_eq!(program, "setsid");
    assert_eq!(
        args,
        vec![
            "/releases/current/scripts/promote.sh".to_string(),
            "abcdef123456".to_string()
        ]
    );
}

/// 偽の `systemd-run`（`--` の後を `exec` するだけのスクリプト。`cluster_login.rs` の偽物と
/// 同じ流儀）を経由して、実際に `promote.sh` が detached で起こり、`promote.lock` に子の pid が
/// 書かれ、`promote.log` に出力が流れる。**本物の `systemd-run` は一切呼ばない**。
#[test]
fn promoting_via_fake_systemd_run_writes_the_lock_and_streams_the_log() {
    use std::os::unix::fs::PermissionsExt;

    let (dir, root) = env();
    release(
        &root,
        "abcdef123456",
        Some(r#"{"built_at":"2026-09-19T00:00:00Z"}"#),
        Some(r#"{"ok":true}"#),
        Some(r#"{"ok":true,"live_ok":true}"#),
    );
    fake_script(&root, "abcdef123456", "promote");

    let fake_systemd_run = dir.path().join("systemd-run");
    std::fs::write(
        &fake_systemd_run,
        "#!/bin/sh\n\
             while [ $# -gt 0 ]; do\n  \
               if [ \"$1\" = \"--\" ]; then\n    \
                 shift\n    \
                 exec \"$@\"\n  \
               fi\n  \
               shift\n\
             done\n\
             exit 1\n",
    )
    .expect("write fake systemd-run");
    std::fs::set_permissions(&fake_systemd_run, std::fs::Permissions::from_mode(0o755))
        .expect("chmod");

    let launcher = DetachLauncher::SystemdRun {
        program: fake_systemd_run.to_string_lossy().into_owned(),
    };
    let accepted = start_promote_with_launcher(&root, "abcdef123456", &launcher).expect("202");
    assert_eq!(accepted.sha12, "abcdef123456");

    let lock = root.join("abcdef123456").join("promote.lock");
    let log = root.join("abcdef123456").join("promote.log");
    let mut logged = String::new();
    for _ in 0..100 {
        logged = std::fs::read_to_string(&log).unwrap_or_default();
        if logged.contains("promote abcdef123456") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(logged.contains("promote abcdef123456"), "{logged:?}");
    let pid: u32 = std::fs::read_to_string(&lock)
        .expect("lock")
        .trim()
        .parse()
        .expect("pid");
    assert!(pid > 0);

    // 走っている間は 409（二重に起こさない。launcher に依らず同じ判定）。
    assert_eq!(
        start_promote_with_launcher(&root, "abcdef123456", &launcher),
        Err(ReleasePromoteError::AlreadyPromoting)
    );
}

/// `promote_stale`: pid が死んでいて `promoted.json` も無く、`promote.log` が「promoted」まで
/// 進んでいなければ stale。
#[test]
fn promote_stale_when_lock_pid_is_dead_and_not_promoted() {
    let (_dir, root) = env();
    release(
        &root,
        "abcdef123456",
        Some(r#"{"built_at":"2026-09-19T00:00:00Z"}"#),
        Some(r#"{"ok":true}"#),
        Some(r#"{"ok":true,"live_ok":true}"#),
    );
    let rdir = root.join("abcdef123456");
    // 死んでいる（存在しない）pid。
    std::fs::write(rdir.join("promote.lock"), "999999999\n").expect("lock");
    std::fs::write(
        rdir.join("promote.log"),
        "2026-09-22T21:55:43Z [promote] systemctl --user start celeris@abcdef123456\n",
    )
    .expect("log");

    let scanned = scan(&root, None);
    let item = scanned
        .items
        .iter()
        .find(|i| i.sha12 == "abcdef123456")
        .expect("item");
    assert!(item.promote_stale, "{item:?}");
    assert_eq!(
        item.promote_last_line.as_deref(),
        Some("2026-09-22T21:55:43Z [promote] systemctl --user start celeris@abcdef123456")
    );
}

/// `promoted.json` があれば、pid が死んでいても stale ではない（成功済み）。
#[test]
fn promote_not_stale_when_promoted_json_exists() {
    let (_dir, root) = env();
    release(
        &root,
        "abcdef123456",
        Some(r#"{"built_at":"2026-09-19T00:00:00Z"}"#),
        Some(r#"{"ok":true}"#),
        Some(r#"{"ok":true,"live_ok":true}"#),
    );
    let rdir = root.join("abcdef123456");
    std::fs::write(rdir.join("promote.lock"), "999999999\n").expect("lock");
    std::fs::write(
        rdir.join("promoted.json"),
        r#"{"promoted_at":"2026-09-22T21:56:00Z","mode":"live","from":null}"#,
    )
    .expect("promoted.json");

    let scanned = scan(&root, None);
    let item = scanned
        .items
        .iter()
        .find(|i| i.sha12 == "abcdef123456")
        .expect("item");
    assert!(!item.promote_stale, "{item:?}");
}

/// pid がまだ生きていれば（走行中）stale ではない。
#[test]
fn promote_not_stale_when_lock_pid_is_alive() {
    let (_dir, root) = env();
    release(
        &root,
        "abcdef123456",
        Some(r#"{"built_at":"2026-09-19T00:00:00Z"}"#),
        Some(r#"{"ok":true}"#),
        Some(r#"{"ok":true,"live_ok":true}"#),
    );
    let rdir = root.join("abcdef123456");
    std::fs::write(
        rdir.join("promote.lock"),
        format!("{}\n", std::process::id()),
    )
    .expect("lock");

    let scanned = scan(&root, None);
    let item = scanned
        .items
        .iter()
        .find(|i| i.sha12 == "abcdef123456")
        .expect("item");
    assert!(!item.promote_stale, "{item:?}");
    assert!(item.promoting);
}
