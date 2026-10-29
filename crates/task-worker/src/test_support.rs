//! テスト専用ヘルパ（`#[cfg(test)]`）。`claude_code`/`codex` のテストが起動する `sh` スタブを
//! 書き込むために使う。
//!
//! ETXTBSY 対策（ADR-0010 D10）: `std::fs::write` でテストプロセス自身がスタブファイルを書き込むと、
//! その書き込みで開いた fd はテストプロセス（tokio マルチスレッドランタイム、複数テストが並行実行）が
//! 保持したままになりうる。並行に走る別のテストが `Command::spawn` で fork するとき、fork は親プロセス
//! （＝テストプロセスそのもの）の全ファイルディスクリプタを継承するため、書き込み直後でまだ閉じられて
//! いないスタブファイルの fd を子が継承してしまうことがあり、その状態でスタブを exec しようとすると
//! `ETXTBSY`（書き込み用に開かれているファイルへの exec は失敗する）が発生しうる。
//! テストプロセス自身がスタブへの書き込み fd を一切保持しなければこの競合は起こらないので、
//! 内容を **別プロセス**（`sh -c 'cat > "$1" && chmod 755 "$1"'`）に標準入力経由で渡して書かせる。

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// ADR-0126 B2/B3 付記: lib 内（`src/` の `#[cfg(test)]`）の user namespace（unshare/`CLONE_NEWUSER`/
/// 実 browser・runtime の sandbox 化）前提の試験を `tests/userns_gate/mod.rs` と同じ規則で既定 skip にする。
/// 優先順位（上が強い）:
/// 1. `CELERIS_ISOLATION_TESTS=skip` → skip。
/// 2. `CELERIS_USERNS_TESTS=1`、または従来の `CELERIS_ISOLATION_TESTS=require` / `CELERIS_DB_GUARD_TESTS=require`
///    → 走らせる（環境が無ければ skip ではなく、呼び出し側の assert/panic で fail させる）。
/// 3. どれも無い → skip（既定）。
pub(crate) fn skip_unless_userns_tests() -> bool {
    if std::env::var("CELERIS_ISOLATION_TESTS").as_deref() == Ok("skip") {
        eprintln!("SKIPPED (not passed): CELERIS_ISOLATION_TESTS=skip");
        return true;
    }
    if std::env::var("CELERIS_USERNS_TESTS").as_deref() == Ok("1")
        || std::env::var("CELERIS_ISOLATION_TESTS").as_deref() == Ok("require")
        || std::env::var("CELERIS_DB_GUARD_TESTS").as_deref() == Ok("require")
    {
        return false;
    }
    eprintln!("SKIPPED (userns test, not passed): set CELERIS_USERNS_TESTS=1 to run (ADR-0126)");
    true
}

/// `path` に `contents` を実行可能（0o755）として書き込む。書き込みは別プロセスで行う（上記コメント参照）。
pub(crate) fn write_executable(path: &Path, contents: &str) {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(r#"cat > "$1" && chmod 755 "$1""#)
        .arg("sh") // "$0"
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn stub writer for {}: {e}", path.display()));

    let mut stdin = child
        .stdin
        .take()
        .unwrap_or_else(|| panic!("stub writer stdin was not piped"));
    stdin
        .write_all(contents.as_bytes())
        .unwrap_or_else(|e| panic!("failed to write stub contents for {}: {e}", path.display()));
    drop(stdin);

    let status = child
        .wait()
        .unwrap_or_else(|e| panic!("failed to wait for stub writer for {}: {e}", path.display()));
    assert!(
        status.success(),
        "stub writer failed for {}: {status}",
        path.display()
    );
}

/// 必須規則の配布試験（ADR 2026-10-23-required-rules-prompt-delivery）の作業場所の形。
#[derive(Clone, Copy, Debug)]
pub(crate) enum AgentsMdFixture {
    /// `AGENTS.md` なし（agent-platform と同じ）。
    Absent,
    /// 人が書いた `AGENTS.md` を追跡している repo。
    TrackedUserFile,
    /// `AGENTS.md` を `CLAUDE.md` への symlink として追跡している repo（よくある慣習）。
    SymlinkToClaudeMd,
    /// 規則の実体を `AGENTS.md` に置き、`CLAUDE.md` をそこへの symlink として追跡している repo。
    ClaudeMdLinksToAgentsMd,
}

/// 各形を 1 度ずつ試す。
pub(crate) const AGENTS_MD_FIXTURES: [AgentsMdFixture; 4] = [
    AgentsMdFixture::Absent,
    AgentsMdFixture::TrackedUserFile,
    AgentsMdFixture::SymlinkToClaudeMd,
    AgentsMdFixture::ClaudeMdLinksToAgentsMd,
];

/// 人が書いた `AGENTS.md` の中身（区切りの外のユーザー指示）。
pub(crate) const USER_AGENTS_MD: &str = "# user notes\n\nkeep me: build with make\n";

/// この repo の実際の `CLAUDE.md`（正本）。
pub(crate) fn repo_claude_md() -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../CLAUDE.md"))
        .unwrap_or_else(|e| panic!("read repo CLAUDE.md: {e}"))
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// `root/origin` に repo の `CLAUDE.md` を追跡した repo を作り、`root/wt` に新しい worktree を切って返す
/// （celeris が task ごとに切る fresh worktree と同じ形）。
pub(crate) fn fresh_worktree_with_rules(
    root: &Path,
    agents: AgentsMdFixture,
) -> std::path::PathBuf {
    let origin = root.join("origin");
    std::fs::create_dir_all(&origin).unwrap_or_else(|e| panic!("mkdir origin: {e}"));
    git(&origin, &["init", "-q", "-b", "main"]);
    let write = |name: &str, body: &str| {
        std::fs::write(origin.join(name), body).unwrap_or_else(|e| panic!("write {name}: {e}"))
    };
    let link = |name: &str, target: &str| {
        std::os::unix::fs::symlink(target, origin.join(name))
            .unwrap_or_else(|e| panic!("symlink {name}: {e}"))
    };
    match agents {
        AgentsMdFixture::ClaudeMdLinksToAgentsMd => {
            write("AGENTS.md", &repo_claude_md());
            link("CLAUDE.md", "AGENTS.md");
        }
        _ => write("CLAUDE.md", &repo_claude_md()),
    }
    match agents {
        AgentsMdFixture::Absent | AgentsMdFixture::ClaudeMdLinksToAgentsMd => {}
        AgentsMdFixture::TrackedUserFile => write("AGENTS.md", USER_AGENTS_MD),
        AgentsMdFixture::SymlinkToClaudeMd => link("AGENTS.md", "CLAUDE.md"),
    }
    git(&origin, &["add", "-A"]);
    git(&origin, &["commit", "-q", "-m", "seed"]);
    let wt = root.join("wt");
    let wt_arg = wt.to_string_lossy().into_owned();
    git(&origin, &["worktree", "add", "-q", "-b", "task", &wt_arg]);
    wt
}

/// 作業ツリーの未 commit の変更・未追跡 file（ignore されたものも含む）。空なら何も書かれていない。
pub(crate) fn worktree_changes(wt: &Path) -> String {
    git(
        wt,
        &[
            "status",
            "--porcelain",
            "--ignored",
            "--untracked-files=all",
        ],
    )
}

/// 作業場所の `AGENTS.md`（無ければ `None`。symlink は辿らずに中身の代わりに参照先を返す）。
pub(crate) fn agents_md_state(wt: &Path) -> Option<String> {
    let path = wt.join("AGENTS.md");
    match std::fs::read_link(&path) {
        Ok(target) => Some(format!("-> {}", target.display())),
        Err(_) => std::fs::read_to_string(&path).ok(),
    }
}

/// harness が受け取った prompt に、正本 `CLAUDE.md` の全文がちょうど 1 度だけ前置きとして入っている。
pub(crate) fn assert_required_rules_in(prompt: &str) {
    let rules = repo_claude_md();
    assert_eq!(
        prompt.matches(crate::required_rules::RULES_HEADING).count(),
        1,
        "{prompt}"
    );
    assert_eq!(prompt.matches(rules.trim_end()).count(), 1, "{prompt}");
    // 現行の禁止事項（例）が実際に届く。
    assert!(prompt.contains("`unwrap()`（テスト以外）"), "{prompt}");
}

/// Unix socket を置く試験用の一時 dir。socket の path は `sun_path`（108 byte）に収まる必要があり、
/// run の `TMPDIR` のような深い path の下では bind できないので、そのときは `/tmp` に作る。
pub(crate) fn short_tempdir() -> std::io::Result<tempfile::TempDir> {
    let base = std::env::temp_dir();
    let base = if base.as_os_str().len() > 40 {
        std::path::PathBuf::from("/tmp")
    } else {
        base
    };
    tempfile::Builder::new().prefix("cz").tempdir_in(base)
}
