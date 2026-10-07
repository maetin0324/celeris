//! 知識ベースのファイル操作（ADR-0047 D1〜D3。Phase 61）。
//!
//! **正本は `~/.local/share/celeris/knowledge/` の Markdown**（`[knowledge] root` で変えられる）で、DB には何も持たない。
//! `task_ops::docs`（ADR-0044 D7）と同じ流儀で **`git` を起こすだけ**（判断も LLM も無い。DESIGN 原則 1）。
//! 文書との違いは「正本が作業ツリーそのもの」という一点で、読み取りは常にファイルを読み、書き込みは
//! 作業ツリーに書いてから 1 件ずつコミットする（一時 worktree は要らない。人も同じファイルを直接編集する）。
//!
//! - [`init`] — `git init` + 骨組み + 雛形 + `README.md` + `_inbox/` + `index.json`（冪等）
//! - [`reindex`] / [`load_index`] / [`ensure_index`] — `index.json`（派生物。`_inbox` は入れない）
//! - [`read_page`] / [`etag`] / [`history`] — ページ 1 枚
//! - [`commit_page`] — 1 件 1 コミット（`etag` で衝突を見る）
//! - [`search`] — 索引 ＋ `git grep -il`（無ければ `grep -ril`）→ [`task_core::knowledge::search`] で順位付け
//! - [`record`] — 候補を `_inbox/<ts>-<slug>.md` に書く（`sources` 必須、秘密は拒否）
//! - [`inbox_list`] / [`inbox_accept`] / [`inbox_reject`] — 候補の一覧と取り込み・破棄
//!
//! 子プロセス（`git` / `grep`）には全部待ち時間の上限がある。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use task_core::knowledge::{
    self as kb, Confidence, FrontMatter, INBOX_DIR, INDEX_FILE, Index, IndexItem, PathError,
    SearchHit,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::changes::{CmdOutput, GIT_TIMEOUT, GIT_WRITE_TIMEOUT, git};
use crate::docs::DocCommit;

/// 索引が古いと見なす閾値（daemon は起動時にこれを超えていれば作り直す）。
pub const INDEX_STALE_SECS: i64 = 6 * 60 * 60;
/// `grep` にかける上限（`git` が使えないときのフォールバック）。
const GREP_TIMEOUT: std::time::Duration = GIT_TIMEOUT;
/// ADR-0047 D4（Phase 62）: `op = retire` の取り込み先（`_inbox` accept が動かす。P-61-k の答え:
/// `DELETE /knowledge/page` は足さず、「捨てる」は retire 一本にする。`docs/guides/knowledge.md` に明記）。
/// `_inbox` と同じく索引にも検索にも出ない。
pub const RETIRED_DIR: &str = "_retired";

fn is_retired(path: &str) -> bool {
    path == RETIRED_DIR || path.starts_with(&format!("{RETIRED_DIR}/"))
}

// ---------------------------------------------------------------------------
// 根の解決（ADR-0047 D3: `--root` > `CELERIS_KNOWLEDGE_ROOT` > `[knowledge] root` > `~/.local/share/celeris/knowledge`）
// ---------------------------------------------------------------------------

/// 知識ベースの根を決める。`~` は展開する。
pub fn resolve_root(explicit: Option<&Path>, configured: Option<&Path>) -> PathBuf {
    let home = task_core::home_dir();
    let raw = explicit
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("CELERIS_KNOWLEDGE_ROOT").map(PathBuf::from))
        .or_else(|| configured.map(Path::to_path_buf))
        .unwrap_or_else(kb::default_root);
    task_core::expand_home(&raw, home.as_deref())
}

/// KB がそこにあるか（`init` 済みか）。
pub fn exists(root: &Path) -> bool {
    root.join(".git").exists()
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default()
}

fn today() -> String {
    now_rfc3339()
        .split('T')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn author_args(name: &str, email: &str) -> (String, String) {
    (format!("user.name={name}"), format!("user.email={email}"))
}

/// P-G46-5: タグの重複を**源で**除く（`celerisctl knowledge record --tags a,a` の人手入力や、
/// LangMem 整理 run が書く `Candidate.tags` に同じ値が並んでいた場合の保険）。順序を保ち、
/// 大文字小文字は区別する（正規化はしない）。
fn dedup_tags_preserve_order(tags: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    tags.into_iter()
        .filter(|t| seen.insert(t.clone()))
        .collect()
}

fn commit_paths(
    root: &Path,
    message: &str,
    author: (&str, &str),
    paths: &[&str],
) -> Result<String, String> {
    let mut add: Vec<&str> = vec!["add", "-A", "--"];
    add.extend_from_slice(paths);
    if !git(root, &add, GIT_WRITE_TIMEOUT).is_some_and(|o| o.ok) {
        return Err("git add に失敗しました".to_string());
    }
    // 書いた内容が既存のコミットと一字一句同じ（`skills_put` の冪等な書き直し等）なら、
    // ステージに何も乗らない。その場合は「新しいコミットは作らず、今の HEAD を返す」
    // （`git commit` は空のコミットを拒否するので、ここで先に見ておく）。
    let mut diff_check: Vec<&str> = vec!["diff", "--cached", "--quiet", "--"];
    diff_check.extend_from_slice(paths);
    if git(root, &diff_check, GIT_TIMEOUT).is_some_and(|o| o.ok) {
        return Ok(head(root).unwrap_or_default());
    }
    let (name, email) = author_args(author.0, author.1);
    let mut args: Vec<&str> = vec![
        "-c", &name, "-c", &email, "commit", "-q", "-m", message, "--",
    ];
    args.extend_from_slice(paths);
    match git(root, &args, GIT_WRITE_TIMEOUT) {
        Some(o) if o.ok => {}
        Some(o) => return Err(format!("コミットできませんでした: {}", o.why())),
        None => return Err("git を起動できませんでした".to_string()),
    }
    Ok(head(root).unwrap_or_default())
}

fn head(root: &Path) -> Option<String> {
    git(root, &["rev-parse", "HEAD"], GIT_TIMEOUT)
        .filter(|o| o.ok)
        .map(|o| o.stdout.trim().to_string())
        .filter(|s| !s.is_empty())
}

// ---------------------------------------------------------------------------
// 初期化（ADR-0047 D1）
// ---------------------------------------------------------------------------

/// `init` の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitOutcome {
    pub root: PathBuf,
    /// この呼び出しで `git init` した。
    pub created: bool,
    /// この呼び出しで新しく書いた雛形（KB 相対）。既にあるページは**触らない**。
    pub added: Vec<String>,
}

/// ADR-0047 D1: `~/.local/share/celeris/knowledge` を git のリポジトリとして用意する（**冪等**）。
///
/// 既にあるファイルは 1 バイトも触らない。足りないディレクトリ・雛形・`README.md`・`.gitignore`・
/// `_inbox/.gitkeep` だけを書き、変わったものがあれば 1 回コミットして `index.json` を作り直す。
pub fn init(root: &Path) -> Result<InitOutcome, String> {
    std::fs::create_dir_all(root)
        .map_err(|e| format!("{} を作れませんでした: {e}", root.display()))?;
    let created = !root.join(".git").exists();
    if created {
        match git(root, &["init", "-q", "-b", "main"], GIT_WRITE_TIMEOUT) {
            Some(o) if o.ok => {}
            Some(o) => return Err(format!("git init に失敗しました: {}", o.why())),
            None => return Err("git を起動できませんでした".to_string()),
        }
    }
    for dir in kb::SKELETON_DIRS.iter().chain([INBOX_DIR].iter()) {
        std::fs::create_dir_all(root.join(dir))
            .map_err(|e| format!("{dir} を作れませんでした: {e}"))?;
    }
    let mut added = Vec::new();
    for (path, body) in seed_files() {
        let target = root.join(&path);
        if target.exists() {
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("{} を作れませんでした: {e}", parent.display()))?;
        }
        std::fs::write(&target, body.as_bytes())
            .map_err(|e| format!("{path} を書けませんでした: {e}"))?;
        added.push(path);
    }
    if !added.is_empty() || created {
        let message = if created {
            "knowledge: 知識ベースを作る（ADR-0047 D1）".to_string()
        } else {
            format!("knowledge: 雛形を追加（{} 件）", added.len())
        };
        commit_paths(
            root,
            &message,
            (kb::HUMAN_AUTHOR_NAME, kb::HUMAN_AUTHOR_EMAIL),
            &["."],
        )?;
    }
    reindex(root)?;
    Ok(InitOutcome {
        root: root.to_path_buf(),
        created,
        added,
    })
}

/// ADR-0047 D1 の雛形（**人が埋める前提のテンプレート**。中身は空欄と書き方の説明だけ）。
fn seed_files() -> Vec<(String, String)> {
    let today = today();
    let page = |title: &str, tags: &[&str], scope: &str, body: &str| {
        kb::render_page(
            &FrontMatter {
                title: Some(title.to_string()),
                tags: tags.iter().map(|t| (*t).to_string()).collect(),
                // Phase K-1: `projects/README.md` は置き場の説明なので scope を持たない（空で渡す）。
                scope: Some(scope.to_string()).filter(|s| !s.is_empty()),
                sources: vec![kb::SOURCE_HUMAN_AUTHORED.to_string()],
                created: Some(today.clone()),
                updated: Some(today.clone()),
                confidence: Some(Confidence::Medium),
                path: None,
                op: None,
            },
            body,
        )
    };
    vec![
        (".gitignore".to_string(), GITIGNORE.to_string()),
        ("README.md".to_string(), readme()),
        (format!("{INBOX_DIR}/.gitkeep"), String::new()),
        (
            "user/profile.md".to_string(),
            page(
                "人のプロフィール",
                &["user"],
                "user",
                "# 人のプロフィール\n\n- 所属・役割:\n- 呼び方・言語:\n- 連絡の好み:\n\n\
                 （celeris が人のことで繰り返し確かめている事実をここに書く。一時的な予定は書かない）\n",
            ),
        ),
        (
            "user/expertise.md".to_string(),
            page(
                "人の専門",
                &["user"],
                "user",
                "# 人の専門\n\n- 得意:\n- 前提として説明が要らないこと:\n- 説明が要ること:\n",
            ),
        ),
        (
            "user/preferences.md".to_string(),
            page(
                "人の好み",
                &["user"],
                "user",
                "# 人の好み\n\n- 文書の書き方:\n- 実装の進め方:\n- 確認を取ってほしい場面:\n",
            ),
        ),
        (
            "user/goals.md".to_string(),
            page(
                "人の目標",
                &["user"],
                "user",
                "# 人の目標\n\n- いま追っていること:\n- 中期の目標:\n",
            ),
        ),
        (
            "environment/clusters/pegasus.md".to_string(),
            cluster_page(&today, "pegasus"),
        ),
        (
            "environment/clusters/sirius.md".to_string(),
            cluster_page(&today, "sirius"),
        ),
        (
            "environment/clusters/fern03.md".to_string(),
            cluster_page(&today, "fern03"),
        ),
        (
            "experience/README.md".to_string(),
            page(
                "経験",
                &["experience"],
                "experience",
                "# 経験\n\n`YYYY/MM/<slug>.md` に 1 件ずつ。**問題・解法・結果・採らなかった案と理由**を書く。\n",
            ),
        ),
        (
            "projects/README.md".to_string(),
            page(
                "案件の知識",
                &["projects"],
                "",
                "# 案件の知識\n\n`projects/<slug>/` に 1 案件ずつ（`design.md` / `decisions.md` / `status.md` …）。\n\
                 `<slug>` は案件の slug（ADR-0044 D7 追記。`GET /projects` の `slug`）。案件のタスクの前置きには\n\
                 この `projects/<slug>` が自動でマウントされる。**案件 ID をディレクトリ名に使わない**。\n",
            ),
        ),
    ]
}

/// クラスタ 1 台分の雛形（**人が埋める**。`docs/` と設定に書いてあることを書き写す場所）。
fn cluster_page(today: &str, id: &str) -> String {
    kb::render_page(
        &FrontMatter {
            title: Some(format!("{id} の使い方")),
            tags: vec!["environment".into(), "cluster".into(), id.to_string()],
            scope: Some("environment".into()),
            sources: vec![kb::SOURCE_HUMAN_AUTHORED.into()],
            created: Some(today.to_string()),
            updated: Some(today.to_string()),
            confidence: Some(Confidence::Low),
            path: None,
            op: None,
        },
        &format!(
            "# {id} の使い方\n\n\
             > 雛形。**人が埋める**（`docs/` と `~/.config/celeris/config.toml` の `[[clusters]]` に\n\
             > 既に書いてあることを書き写す。埋めたら `confidence: high` にする）。\n\n\
             ## 接続\n\n- ホスト（`ssh` の別名）:\n- 踏み台:\n- 認証:\n\n\
             ## 作業場所\n\n- 作業ツリーの根:\n- 共有ディレクトリ:\n\n\
             ## ジョブ\n\n- 投げ方:\n- 待ち行列 / 資源の単位:\n- よくある落とし穴:\n\n\
             ## 環境\n\n- module / コンパイラ:\n- 使えるネットワーク・ストレージ:\n"
        ),
    )
}

const GITIGNORE: &str =
    "# ADR-0047 D1 / D6: 索引は再生成できる派生物（正本は Markdown）。\nindex.json\nindex.*/\n";

fn readme() -> String {
    format!(
        "# 知識ベース（Celeris）\n\n\
         正本はこのディレクトリの **Markdown**（ADR-0047）。DB も外部サービスも正本ではない。\n\
         `index.json` は再生成できる派生物なので git には入れない。\n\n\
         ```\n\
         user/                 人のこと（profile / expertise / preferences / goals）\n\
         environment/          環境（<分類>/<name>.md。分類は celeris/ clusters/ hosts/ servers/ tools/）\n\
         projects/<slug>/      案件の知識（design.md / decisions.md / status.md …）\n\
         experience/           経験（YYYY/MM/<slug>.md。問題・解法・結果・採らなかった案）\n\
         {INBOX_DIR}/               抽出された候補。まだ索引に入らない（人が accept / reject する）\n\
         {INDEX_FILE}            派生物\n\
         ```\n\n\
         1 ファイル = 1 トピック。先頭に front matter を付ける:\n\n\
         ```\n\
         ---\n\
         title: pegasus の使い方\n\
         tags: [hpc, cluster, pegasus]\n\
         scope: environment\n\
         sources: [\"human:authored\", \"task:01J…\"]\n\
         created: 2026-09-20\n\
         updated: 2026-09-20\n\
         confidence: high\n\
         ---\n\
         ```\n\n\
         `scope` は `user` / `environment` / `project:<slug>` / `experience`（置き場と一致させる）。\n\
         `<slug>` は案件の slug（`GET /projects` の `slug`）。**案件 ID を置き場に使わない**。\n\
         `environment/` と `projects/` の直下には README 以外を置かない。同じ scope に同じ題名の\n\
         ページがあれば、新しいページは作らずそのページへの追記・統合の候補になる（Phase K-1）。\n\
         `sources` は `task:<id>` / `message:<id>` / `human:authored` / `human:instruction` / `url:<…>`。\n\
         `human:authored`（または `author: human`）は人が書いたページの印で、日次整理は自動で消さない。\n\
         `human:instruction` は人の指示・発言に由来する事実の印で、通常の整理の対象（ADR-0047 付記 2026-10-04）。\n\n\
         ## 道具\n\n\
         ```\n\
         celerisctl knowledge search <語> [--scope …] [--limit N] [--json]\n\
         celerisctl knowledge get <path>\n\
         celerisctl knowledge record --title … --scope … --tags … --source task:<id> < body.md\n\
         celerisctl knowledge reindex\n\
         ```\n\n\
         `record` は**候補**を `{INBOX_DIR}/` に書く（正本には直接書かない）。人が GUI の「知識」画面で\n\
         accept / reject する。秘密（API キー・トークン・秘密鍵）は決定的な検査で弾かれる。\n"
    )
}

// ---------------------------------------------------------------------------
// 索引（`index.json`。派生物）
// ---------------------------------------------------------------------------

/// KB の下の `*.md`（`_inbox` と `.git` は除く。名前順、最大 [`kb::MAX_INDEX_ITEMS`] 件）。
pub fn list_pages(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out.truncate(kb::MAX_INDEX_ITEMS);
    out
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        if rel.starts_with('.') || kb::is_inbox(&rel) || is_retired(&rel) || kb::is_skills(&rel) {
            continue;
        }
        if path.is_dir() {
            walk(root, &path, out);
        } else if rel.to_ascii_lowercase().ends_with(".md") {
            out.push(rel);
        }
    }
}

/// ADR-0047 D1 / D3: `index.json` を作り直す（`_inbox` は入れない）。
pub fn reindex(root: &Path) -> Result<Index, String> {
    let last = last_commits(root);
    let mut items = Vec::new();
    for path in list_pages(root) {
        let Ok(raw) = std::fs::read_to_string(root.join(&path)) else {
            continue;
        };
        items.push(index_item(&path, &raw, last.get(&path)));
    }
    let index = Index {
        generated_at: now_rfc3339(),
        items,
    };
    write_index(root, &index)?;
    Ok(index)
}

fn index_item(path: &str, raw: &str, commit: Option<&DocCommit>) -> IndexItem {
    let (front, _) = kb::front_matter(raw);
    IndexItem {
        title: kb::title_of(raw, path),
        tags: front.tags,
        scope: front.scope.or_else(|| default_scope(path)),
        sources: front.sources,
        updated: front.updated.or_else(|| commit.map(|c| c.at.clone())),
        confidence: front.confidence,
        path: path.to_string(),
    }
}

/// front matter に `scope` が無いページの既定（置き場から決める）。
fn default_scope(path: &str) -> Option<String> {
    let top = path.split('/').next()?;
    match top {
        "user" | "environment" | "experience" => Some(top.to_string()),
        // Phase K-1: `projects/README.md`（置き場の説明）は案件ではない。
        "projects" => {
            let parts: Vec<&str> = path.split('/').collect();
            (parts.len() > 2).then(|| format!("project:{}", parts[1]))
        }
        _ => None,
    }
}

fn write_index(root: &Path, index: &Index) -> Result<(), String> {
    let json = serde_json::to_string_pretty(index)
        .map_err(|e| format!("索引を組み立てられませんでした: {e}"))?;
    std::fs::write(root.join(INDEX_FILE), format!("{json}\n").as_bytes())
        .map_err(|e| format!("{INDEX_FILE} を書けませんでした: {e}"))
}

/// `index.json` を読む（無い・壊れていれば `None`）。
pub fn load_index(root: &Path) -> Option<Index> {
    let raw = std::fs::read_to_string(root.join(INDEX_FILE)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// 索引が無い・古い（[`INDEX_STALE_SECS`] より前）か。
pub fn index_is_stale(root: &Path) -> bool {
    let Some(index) = load_index(root) else {
        return true;
    };
    let Ok(at) = OffsetDateTime::parse(&index.generated_at, &Rfc3339) else {
        return true;
    };
    (OffsetDateTime::now_utc() - at).whole_seconds() > INDEX_STALE_SECS
}

/// 索引を読む。無い・古ければ作り直す（daemon が起動時に呼ぶ。ADR-0047 D3）。
pub fn ensure_index(root: &Path) -> Index {
    if index_is_stale(root)
        && let Ok(index) = reindex(root)
    {
        return index;
    }
    load_index(root).unwrap_or_default()
}

/// ページごとの**最後のコミット**（`git log --name-only` を 1 回だけ起こす）。
pub fn last_commits(root: &Path) -> BTreeMap<String, DocCommit> {
    let format = format!("--format={RECORD}%H{UNIT}%aI{UNIT}%an{UNIT}%s");
    let mut out = BTreeMap::new();
    let Some(result) = git(
        root,
        &["log", "--no-merges", "--name-only", &format],
        GIT_TIMEOUT,
    )
    .filter(|o| o.ok) else {
        return out;
    };
    for record in result.stdout.split(RECORD).skip(1) {
        let mut lines = record.lines();
        let Some(header) = lines.next() else { continue };
        let Some(commit) = parse_commit(header) else {
            continue;
        };
        for path in lines.map(str::trim).filter(|p| !p.is_empty()) {
            out.entry(path.to_string())
                .or_insert_with(|| commit.clone());
        }
    }
    out
}

const UNIT: char = '\u{1f}';
const RECORD: char = '\u{1e}';

fn parse_commit(line: &str) -> Option<DocCommit> {
    let mut parts = line.split(UNIT);
    let sha = parts.next()?.trim().to_string();
    if sha.is_empty() {
        return None;
    }
    Some(DocCommit {
        sha,
        at: parts.next().unwrap_or_default().trim().to_string(),
        author: parts.next().unwrap_or_default().trim().to_string(),
        subject: parts.next().unwrap_or_default().trim().to_string(),
    })
}

// ---------------------------------------------------------------------------
// ページ 1 枚
// ---------------------------------------------------------------------------

/// KB のファイルの絶対パス（境界の検査を通したものだけ）。
pub fn page_file(root: &Path, path: &str) -> Result<PathBuf, PathError> {
    let path = kb::page_path(path)?;
    Ok(root.join(path))
}

/// ページの中身（**作業ツリーのファイル**。人が編集中のものもそのまま見える）。
pub fn read_page(root: &Path, path: &str) -> Option<String> {
    std::fs::read_to_string(root.join(path)).ok()
}

/// ページの `etag`（中身の sha256。作業ツリーのファイルに対して決まる）。無ければ `None`。
pub fn etag(root: &Path, path: &str) -> Option<String> {
    let bytes = std::fs::read(root.join(path)).ok()?;
    Some(content_etag(&bytes))
}

fn content_etag(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// ページ 1 枚の履歴（新しい順、直近 [`kb::HISTORY_LIMIT`] 件）。
pub fn history(root: &Path, path: &str) -> Vec<DocCommit> {
    let format = format!("--format=%H{UNIT}%aI{UNIT}%an{UNIT}%s");
    let limit = format!("-{}", kb::HISTORY_LIMIT);
    let Some(out) = git(root, &["log", &limit, &format, "--", path], GIT_TIMEOUT).filter(|o| o.ok)
    else {
        return Vec::new();
    };
    out.stdout.lines().filter_map(parse_commit).collect()
}

/// 1 回の書き込み。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageEdit {
    /// KB 相対のパス（[`kb::page_path`] を通したもの）。
    pub path: String,
    /// 本文。`None` なら削除。
    pub body: Option<String>,
    /// 期待する `etag`。新規作成のときだけ `None` でよい。
    pub etag: Option<String>,
    pub message: String,
    /// 作った人（`Celeris (human)` / `Celeris (knowledge)`）。
    pub author: (String, String),
}

/// 書き込みの結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOutcome {
    Written {
        sha: String,
        etag: Option<String>,
        unchanged: bool,
    },
    /// `etag` が現在の中身と違う（409）。`etag` はいまの値。
    EtagMismatch {
        etag: Option<String>,
    },
    /// 消そうとしたページが無い（404）。
    Missing,
    Failed {
        detail: String,
    },
}

/// ADR-0047 D1: **1 件 1 コミット**。作業ツリーに書いてからそのパスだけをコミットする。
pub fn commit_page(root: &Path, edit: &PageEdit) -> WriteOutcome {
    if !exists(root) {
        return WriteOutcome::Failed {
            detail: format!(
                "{} は知識ベースではありません（celerisctl knowledge init）",
                root.display()
            ),
        };
    }
    let current = etag(root, &edit.path);
    if edit.body.is_none() && current.is_none() {
        return WriteOutcome::Missing;
    }
    if edit.etag.as_deref() != current.as_deref() {
        return WriteOutcome::EtagMismatch { etag: current };
    }
    let target = root.join(&edit.path);
    match &edit.body {
        Some(body) => {
            if let Some(parent) = target.parent()
                && std::fs::create_dir_all(parent).is_err()
            {
                return WriteOutcome::Failed {
                    detail: format!("{} を作れませんでした", edit.path),
                };
            }
            let mut text = body.replace("\r\n", "\n");
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            if current.as_deref() == Some(content_etag(text.as_bytes()).as_str()) {
                return WriteOutcome::Written {
                    sha: head(root).unwrap_or_default(),
                    etag: current,
                    unchanged: true,
                };
            }
            if std::fs::write(&target, text.as_bytes()).is_err() {
                return WriteOutcome::Failed {
                    detail: format!("{} を書けませんでした", edit.path),
                };
            }
        }
        None => {
            if std::fs::remove_file(&target).is_err() {
                return WriteOutcome::Failed {
                    detail: format!("{} を消せませんでした", edit.path),
                };
            }
        }
    }
    match commit_paths(
        root,
        &edit.message,
        (edit.author.0.as_str(), edit.author.1.as_str()),
        &[edit.path.as_str()],
    ) {
        Ok(sha) => WriteOutcome::Written {
            sha,
            etag: etag(root, &edit.path),
            unchanged: false,
        },
        Err(detail) => WriteOutcome::Failed { detail },
    }
}

// ---------------------------------------------------------------------------
// 検索（ADR-0047 D3）
// ---------------------------------------------------------------------------

/// 本文の全文一致（`git grep -il`、無ければ `grep -ril`）。当たった KB 相対パス（`_inbox` は除く）。
pub fn grep(root: &Path, needle: &str) -> Vec<String> {
    let needle = needle.trim();
    if needle.is_empty() {
        return Vec::new();
    }
    // `--untracked` は「人がまだコミットしていないページ」も見る（正本は作業ツリーなので）。
    let out = git(
        root,
        &[
            "grep",
            "-I",
            "-i",
            "-l",
            "-F",
            "--untracked",
            "-e",
            needle,
            "--",
            "*.md",
        ],
        GIT_TIMEOUT,
    );
    let text = match out {
        // 「見つからない」は exit 1（エラーではない）。
        Some(o) => o.stdout,
        // `git` そのものが無い環境では `grep -ril`（ADR-0047 D3）。
        None => plain_grep(root, needle),
    };
    let mut paths: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| l.trim_start_matches("./").to_string())
        .filter(|p| {
            p.to_ascii_lowercase().ends_with(".md")
                && !kb::is_inbox(p)
                && !is_retired(p)
                && !kb::is_skills(p)
        })
        .collect();
    paths.sort();
    paths.dedup();
    paths.truncate(kb::MAX_INDEX_ITEMS);
    paths
}

/// git が動かない（KB がまだ git でない）ときの `grep -ril`。
fn plain_grep(root: &Path, needle: &str) -> String {
    let out = run(
        root,
        "grep",
        &["-r", "-i", "-l", "-F", "--include=*.md", "-e", needle, "."],
        GREP_TIMEOUT,
    );
    out.map(|o| o.stdout).unwrap_or_default()
}

/// 子プロセスを 1 つ起こす（`crate::changes::git` と同じ上限の付け方）。
fn run(
    dir: &Path,
    program: &str,
    args: &[&str],
    timeout: std::time::Duration,
) -> Option<CmdOutput> {
    crate::changes::run_with_timeout(dir, program, args, timeout)
}

/// ADR-0047 D3 の検索（索引 ＋ 本文一致 → [`kb::search`] の順位付け）。
pub fn search(root: &Path, query: &str, scope: Option<&str>, limit: usize) -> Vec<SearchHit> {
    let index = ensure_index(root);
    let body_hits = if query.trim().is_empty() {
        Vec::new()
    } else {
        grep(root, query.trim())
    };
    kb::search(&index, query, scope, limit, &body_hits)
}

// ---------------------------------------------------------------------------
// 候補（`_inbox/`。ADR-0047 D3 の `record` と D4 の適用）
// ---------------------------------------------------------------------------

/// `record` の入力（CLI と、Phase 62 の知識整理 run が使う）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecordRequest {
    pub title: String,
    pub scope: String,
    pub tags: Vec<String>,
    /// **1 件以上必須**（ADR-0047 D3）。
    pub sources: Vec<String>,
    pub confidence: Option<Confidence>,
    pub body: String,
    /// 取り込む先の KB 相対パス（省略なら `scope` と `title` から決める。Phase K-1: どちらも
    /// [`kb::place`] のガードを通す）。
    pub path: Option<String>,
    /// Phase K-1: 取り込み先が既にあるときの扱い。`Some(Merge)` は「本文は既存ページを統合した
    /// 完全な版」（accept で上書き）、それ以外は `Append`（accept で末尾に節として足す）。
    pub op: Option<kb::CandidateOp>,
}

/// `record` の失敗（CLI は 1、API は 422）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RecordError {
    #[error("title must not be blank")]
    NoTitle,
    #[error("scope must not be blank (user | environment | project:<slug> | experience)")]
    NoScope,
    #[error(
        "at least one --source is required (task:<id> / message:<id> / human:instruction / url:<…>)"
    )]
    NoSources,
    #[error("body must not be blank")]
    NoBody,
    /// ADR-0047 D4: 秘密は保存しない。
    #[error("refused: the candidate contains a secret（{0}）")]
    Secret(&'static str),
    /// Phase K-1: 置き場のガード（[`kb::place`]）に落ちた。文面に正しい置き場の書き方が入る。
    #[error("refused: {0}")]
    Placement(#[from] kb::PlacementError),
    #[error("{0}")]
    Failed(String),
}

/// `record` の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordOutcome {
    /// `_inbox/<ts>-<slug>.md`。
    pub path: String,
    /// `_inbox` の中での id（ファイル名から `.md` を取ったもの）。
    pub id: String,
    pub sha: String,
    /// Phase K-1: 取り込み先（ガードを通した KB 相対パス）。
    pub target: String,
    /// Phase K-1: 取り込み先が既にあれば `Append` か `Merge`。新しいページなら `None`。
    pub op: Option<kb::CandidateOp>,
    /// Phase K-1: 同じ題名のページ・`user/` の正準ページへ向け直したとき。
    pub redirect: Option<kb::Redirect>,
}

/// Phase K-1: [`kb::place`] に渡す置き場の状況を集める（`environment/` の下のディレクトリ・索引・今日）。
/// `projects` は案件の一覧（DB を開ける呼び出し側だけが渡せる。`None` は「案件を知らない」）。
pub fn layout(root: &Path, projects: Option<Vec<kb::ProjectRef>>) -> kb::Layout {
    let env_dirs = std::fs::read_dir(root.join("environment"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let pages = if exists(root) {
        ensure_index(root).items
    } else {
        Vec::new()
    };
    kb::Layout::new(projects, env_dirs, pages, today())
}

/// Phase K-1: 案件の一覧を [`kb::ProjectRef`] にする（slug は [`task_core::Project::kb_slug`]）。
pub fn project_refs<S: task_core::TaskStore + ?Sized>(
    store: &S,
) -> Result<Vec<kb::ProjectRef>, task_core::StoreError> {
    Ok(store
        .project_list()?
        .into_iter()
        .map(|p| kb::ProjectRef {
            id: p.id.to_string(),
            slug: p.kb_slug(),
            title: p.title,
        })
        .collect())
}

/// ADR-0047 D3: **候補**を `_inbox/` に書く（正本には直接書かない）。案件を知らない呼び出し
/// （`celerisctl knowledge record`）の形。案件の一覧を持つ側は [`record_in`] を使う。
pub fn record(root: &Path, request: &RecordRequest) -> Result<RecordOutcome, RecordError> {
    record_in(root, request, &layout(root, None))
}

/// [`record`] に置き場の状況（[`layout`]）を渡す形（MCP の `knowledge_propose` が案件の一覧つきで呼ぶ）。
pub fn record_in(
    root: &Path,
    request: &RecordRequest,
    layout: &kb::Layout,
) -> Result<RecordOutcome, RecordError> {
    record_prepare(root, request, layout)?.commit(root)
}

/// [`record_prepare`] の結果: 検証を通り、id を決め、`_inbox/.<id>.md.tmp` に書いた候補。
/// [`PreparedRecord::commit`] で `_inbox/<id>.md` へ rename して git commit する。commit しないで
/// 捨てる（drop する）と一時ファイルは消える。ADR 2026-10-07 cos-live-fixes D2: API は添付の pin
/// （SQLite の commit）の後に `commit` するので、pin に失敗した候補は残らない。
#[derive(Debug)]
pub struct PreparedRecord {
    /// `_inbox` の中での id（ファイル名から `.md` を取ったもの）。
    pub id: String,
    /// `_inbox/<id>.md`。
    pub path: String,
    /// 取り込み先（ガードを通した KB 相対パス）。
    pub target: String,
    /// 正規化した scope（`project:<slug>` など）。
    pub scope: Option<String>,
    pub op: Option<kb::CandidateOp>,
    pub redirect: Option<kb::Redirect>,
    temp: Option<PathBuf>,
}

impl PreparedRecord {
    /// 一時ファイルを `_inbox/<id>.md` にして git commit する。
    pub fn commit(mut self, root: &Path) -> Result<RecordOutcome, RecordError> {
        let Some(temp) = self.temp.take() else {
            return Err(RecordError::Failed(
                "the candidate was already committed".into(),
            ));
        };
        let final_path = root.join(&self.path);
        if let Err(e) = std::fs::rename(&temp, &final_path) {
            let _ = std::fs::remove_file(&temp);
            return Err(RecordError::Failed(format!(
                "{} を書けませんでした: {e}",
                self.path
            )));
        }
        let sha = commit_paths(
            root,
            &format!("knowledge: 候補 {}", self.path),
            (kb::AGENT_AUTHOR_NAME, kb::AGENT_AUTHOR_EMAIL),
            &[self.path.as_str()],
        )
        .map_err(RecordError::Failed)?;
        Ok(RecordOutcome {
            path: std::mem::take(&mut self.path),
            id: std::mem::take(&mut self.id),
            sha,
            target: std::mem::take(&mut self.target),
            op: self.op,
            redirect: self.redirect.take(),
        })
    }
}

impl Drop for PreparedRecord {
    fn drop(&mut self) {
        if let Some(temp) = self.temp.take() {
            let _ = std::fs::remove_file(temp);
        }
    }
}

/// [`record_in`] の前半: 検証・置き場のガード・id の決定をして、候補を一時名で書く（git には入れない）。
pub fn record_prepare(
    root: &Path,
    request: &RecordRequest,
    layout: &kb::Layout,
) -> Result<PreparedRecord, RecordError> {
    let title = request.title.trim();
    let scope = request.scope.trim();
    let body = request.body.trim();
    if title.is_empty() {
        return Err(RecordError::NoTitle);
    }
    if scope.is_empty() {
        return Err(RecordError::NoScope);
    }
    let sources: Vec<String> = request
        .sources
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if sources.is_empty() {
        return Err(RecordError::NoSources);
    }
    if body.is_empty() {
        return Err(RecordError::NoBody);
    }
    // ADR-0047 D4: 秘密（API キー・トークン・秘密鍵）は決定的な検査で弾く。
    let haystack = format!("{title}\n{body}\n{}", sources.join("\n"));
    if let Some(why) = kb::secret_finding(&haystack) {
        return Err(RecordError::Secret(why));
    }
    if !exists(root) {
        return Err(RecordError::Failed(format!(
            "{} は知識ベースではありません（celerisctl knowledge init）",
            root.display()
        )));
    }
    let tags: Vec<String> = request
        .tags
        .iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    // Phase K-1: 置き場のガード（apply_candidates と同じ関数）。
    let placement = kb::place(
        &kb::PlacementRequest {
            op: None,
            path: request.path.as_deref(),
            scope: Some(scope),
            title,
            tags: &tags,
        },
        layout,
    )?;
    let target = placement.path.clone();
    // ADR-0047 付記 H2: `record` は run（celerisctl・MCP）が書く。人が書いた印は作れない。
    let existing_sources = read_page(root, &target)
        .map(|raw| kb::front_matter(&raw).0.sources)
        .unwrap_or_default();
    let sources = kb::normalize_agent_sources(&sources, &existing_sources, None);
    let op = root.join(&target).exists().then(|| {
        if request.op == Some(kb::CandidateOp::Merge) {
            kb::CandidateOp::Merge
        } else {
            kb::CandidateOp::Append
        }
    });
    let now = OffsetDateTime::now_utc();
    let stamp = format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    );
    let slug = kb::slugify(title).unwrap_or_else(|| "note".to_string());
    let mut id = format!("{stamp}-{slug}");
    let mut n = 2;
    while root.join(INBOX_DIR).join(format!("{id}.md")).exists()
        || root.join(INBOX_DIR).join(format!(".{id}.md.tmp")).exists()
    {
        id = format!("{stamp}-{slug}-{n}");
        n += 1;
    }
    let path = format!("{INBOX_DIR}/{id}.md");
    let page = kb::render_page(
        &FrontMatter {
            title: Some(title.to_string()),
            tags: dedup_tags_preserve_order(tags),
            scope: placement.scope.clone(),
            sources,
            created: Some(today()),
            updated: Some(today()),
            confidence: request.confidence,
            path: Some(target.clone()),
            op: op.map(|o| o.as_str().to_string()),
        },
        body,
    );
    let dir = root.join(INBOX_DIR);
    std::fs::create_dir_all(&dir)
        .map_err(|e| RecordError::Failed(format!("{INBOX_DIR} を作れませんでした: {e}")))?;
    let temp = dir.join(format!(".{id}.md.tmp"));
    std::fs::write(&temp, page.as_bytes())
        .map_err(|e| RecordError::Failed(format!("{path} を書けませんでした: {e}")))?;
    Ok(PreparedRecord {
        id,
        path,
        target,
        scope: placement.scope,
        op,
        redirect: placement.redirect,
        temp: Some(temp),
    })
}

/// `_inbox/` の候補 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxItem {
    /// ファイル名から `.md` を取ったもの（API のパスに使う）。
    pub id: String,
    /// `_inbox/<id>.md`。
    pub path: String,
    pub title: String,
    pub tags: Vec<String>,
    pub scope: Option<String>,
    pub sources: Vec<String>,
    pub confidence: Option<Confidence>,
    /// 取り込む先（front matter の `path`、無ければ `scope` と `title` からの既定）。
    pub target: String,
    pub created: Option<String>,
    /// 本文（front matter を除く）。
    pub body: String,
    /// ADR-0047 D4（Phase 62）: `create` / `update` / `merge` / `retire`。`record`（Phase 61）が書いた
    /// 候補（人・ワーカーの `celerisctl knowledge record`）は `None`（従来どおりの素の accept/reject）。
    pub op: Option<kb::CandidateOp>,
}

/// `_inbox/` の候補（新しい順 = id の降順）。
pub fn inbox_list(root: &Path) -> Vec<InboxItem> {
    let dir = root.join(INBOX_DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".md").map(str::to_string)
        })
        .collect();
    ids.sort();
    ids.reverse();
    ids.iter().filter_map(|id| inbox_get(root, id)).collect()
}

/// `_inbox` の id の検査（`/`・`..`・空は通さない）。
pub fn inbox_path(id: &str) -> Result<String, PathError> {
    let id = id.trim();
    if id.is_empty() {
        return Err(PathError::Empty);
    }
    if id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err(PathError::Forbidden);
    }
    kb::page_path(&format!("{INBOX_DIR}/{id}.md"))
}

/// 候補 1 件。
pub fn inbox_get(root: &Path, id: &str) -> Option<InboxItem> {
    let path = inbox_path(id).ok()?;
    let raw = std::fs::read_to_string(root.join(&path)).ok()?;
    let (front, body) = kb::front_matter(&raw);
    let title = kb::title_of(&raw, &path);
    let target = front
        .path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .and_then(|p| kb::page_path(p).ok())
        .unwrap_or_else(|| default_target(root, &front, &title, id));
    let op = front.op.as_deref().and_then(|o| o.parse().ok());
    Some(InboxItem {
        id: id.trim().to_string(),
        path,
        title,
        tags: front.tags,
        scope: front.scope,
        sources: front.sources,
        confidence: front.confidence,
        target,
        created: front.created,
        body: body.trim_start_matches(['\n', '\r']).to_string(),
        op,
    })
}

/// 取り込み先の既定（Phase K-1 より前の、`path` を持たない候補のため）。置き場のガード
/// （[`kb::place`]）が通ればそれ、通らなければ従来どおり `scope` のディレクトリ ＋ 題名の slug
/// （その場合 accept は [`accept_target_problem`] で止まることがある）。
fn default_target(root: &Path, front: &FrontMatter, title: &str, id: &str) -> String {
    if let Ok(p) = kb::place(
        &kb::PlacementRequest {
            op: None,
            path: None,
            scope: front.scope.as_deref(),
            title,
            tags: &front.tags,
        },
        &layout(root, None),
    ) {
        return p.path;
    }
    let dir = front
        .scope
        .as_deref()
        .and_then(kb::scope_dir)
        .unwrap_or_else(|| "experience".to_string());
    let slug = kb::slugify(title).unwrap_or_else(|| id.to_ascii_lowercase());
    format!("{dir}/{slug}.md")
}

/// accept / reject の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboxOutcome {
    /// 取り込んだ（`path` は正本の中での置き場）。
    Accepted {
        path: String,
        sha: String,
        etag: Option<String>,
    },
    /// 破棄した。
    Rejected {
        sha: String,
    },
    /// その id が無い（404）。
    Missing,
    /// 宛先が既にある（409）。
    Exists {
        path: String,
    },
    Failed {
        detail: String,
    },
}

/// ADR-0047 D3 / D5: 候補を正本に取り込む（`_inbox` から消して、`path` にコミットする）。
///
/// `path` を渡せばそこへ、渡さなければ front matter の `path`（無ければ `scope` と題名からの既定）へ。
/// 宛先が既にあるときは `overwrite` が無ければ 409。
pub fn inbox_accept(root: &Path, id: &str, path: Option<&str>, overwrite: bool) -> InboxOutcome {
    let Some(item) = inbox_get(root, id) else {
        return InboxOutcome::Missing;
    };
    // ADR-0047 D4（Phase 62）: `op = retire` は候補の中身を書くのではなく、`target`（対象の既存ページ）を
    // `_retired/` へ動かす。`op = merge` は候補の本文（= 書き直した完全な版）で `target` を**必ず上書き**する
    // （P-61-k: `DELETE /knowledge/page` は足さず、捨てるのは retire に一本化。`docs/guides/knowledge.md` に明記）。
    if item.op == Some(kb::CandidateOp::Retire) {
        return inbox_accept_retire(root, &item);
    }
    let target = match path.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => match kb::page_path(p) {
            Ok(p) => p,
            Err(e) => {
                return InboxOutcome::Failed {
                    detail: e.to_string(),
                };
            }
        },
        None => item.target.clone(),
    };
    if kb::is_inbox(&target) || is_retired(&target) {
        return InboxOutcome::Failed {
            detail: "取り込み先を `_inbox/`・`_retired/` にはできません".to_string(),
        };
    }
    if let Some(detail) = accept_target_problem(&target) {
        return InboxOutcome::Failed { detail };
    }
    // Phase K-1: `op = append` は既存のページの末尾に節として足す（取り込み先を人が変えたときは従来どおり）。
    // 候補を作ったあとで取り込み先が消えていれば、新しいページとして書く（下の従来の経路）。
    if item.op == Some(kb::CandidateOp::Append)
        && target == item.target
        && root.join(&target).exists()
    {
        return inbox_accept_append(root, &item);
    }
    let overwrite = overwrite || item.op == Some(kb::CandidateOp::Merge);
    if !overwrite && root.join(&target).exists() {
        return InboxOutcome::Exists { path: target };
    }
    // 取り込んだページは `_inbox` 専用の `path:` を落とし、`updated` を今日にする。
    let Some(raw) = std::fs::read_to_string(root.join(&item.path)).ok() else {
        return InboxOutcome::Missing;
    };
    let (mut front, body) = kb::front_matter(&raw);
    front.path = None;
    front.updated = Some(today());
    let page = kb::render_page(&front, body);
    let target_file = root.join(&target);
    if let Some(parent) = target_file.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return InboxOutcome::Failed {
            detail: format!("{target} を作れませんでした"),
        };
    }
    if std::fs::write(&target_file, page.as_bytes()).is_err() {
        return InboxOutcome::Failed {
            detail: format!("{target} を書けませんでした"),
        };
    }
    if std::fs::remove_file(root.join(&item.path)).is_err() {
        return InboxOutcome::Failed {
            detail: format!("{} を消せませんでした", item.path),
        };
    }
    match commit_paths(
        root,
        &format!("knowledge: {target}（候補 {} を取り込む）", item.id),
        (kb::HUMAN_AUTHOR_NAME, kb::HUMAN_AUTHOR_EMAIL),
        &[target.as_str(), item.path.as_str()],
    ) {
        Ok(sha) => {
            let _ = reindex(root);
            InboxOutcome::Accepted {
                etag: etag(root, &target),
                path: target,
                sha,
            }
        }
        Err(detail) => InboxOutcome::Failed { detail },
    }
}

/// Phase K-1: 取り込み先に案件 ID（ULID）の段があれば止める（`projects/<ULID>/` を二度と作らない）。
/// 人が GUI で取り込み先を書き換えたときも同じ（案件 ID のディレクトリは他のどこからも読まれない）。
fn accept_target_problem(target: &str) -> Option<String> {
    target
        .split('/')
        .any(|seg| {
            let stem = seg.strip_suffix(".md").unwrap_or(seg);
            kb::looks_like_ulid(stem)
        })
        .then(|| {
            format!(
                "取り込み先 {target} に案件 ID（ULID）の段があります。`projects/<slug>/…` のような置き場を指定してください"
            )
        })
}

/// Phase K-1: `op = append` の accept。取り込み先の既存ページに、候補の本文を**節として足す**
/// （既存の本文は消さない）。`tags` / `sources` は和、`updated` は今日。取り込み先が `init` の雛形の
/// ままなら（[`is_seed_body`]）、雛形の本文を候補の本文で置き換える。
fn inbox_accept_append(root: &Path, item: &InboxItem) -> InboxOutcome {
    let Some(existing) = read_page(root, &item.target) else {
        return InboxOutcome::Failed {
            detail: format!("{} を読めませんでした", item.target),
        };
    };
    let page = append_page(&existing, &item.target, item, &today());
    if std::fs::write(root.join(&item.target), page.as_bytes()).is_err() {
        return InboxOutcome::Failed {
            detail: format!("{} を書けませんでした", item.target),
        };
    }
    if std::fs::remove_file(root.join(&item.path)).is_err() {
        return InboxOutcome::Failed {
            detail: format!("{} を消せませんでした", item.path),
        };
    }
    match commit_paths(
        root,
        &format!(
            "knowledge: {}（候補 {} を追記で取り込む）",
            item.target, item.id
        ),
        (kb::HUMAN_AUTHOR_NAME, kb::HUMAN_AUTHOR_EMAIL),
        &[item.target.as_str(), item.path.as_str()],
    ) {
        Ok(sha) => {
            let _ = reindex(root);
            InboxOutcome::Accepted {
                etag: etag(root, &item.target),
                path: item.target.clone(),
                sha,
            }
        }
        Err(detail) => InboxOutcome::Failed { detail },
    }
}

/// `init` の雛形のままの本文か（空欄と書き方だけで、人も celeris もまだ何も書いていない）。
fn is_seed_body(target: &str, body: &str) -> bool {
    seed_files()
        .into_iter()
        .find(|(path, _)| path == target)
        .is_some_and(|(_, raw)| kb::front_matter(&raw).1.trim() == body.trim())
}

/// 既存のページ `existing` に候補 `item` を足した 1 枚（純粋。`today` は `YYYY-MM-DD`）。
fn append_page(existing: &str, target: &str, item: &InboxItem, today: &str) -> String {
    let (mut front, body) = kb::front_matter(existing);
    let mut tags = front.tags.clone();
    tags.extend(item.tags.iter().cloned());
    front.tags = dedup_tags_preserve_order(tags);
    for s in &item.sources {
        if !front.sources.contains(s) {
            front.sources.push(s.clone());
        }
    }
    front.updated = Some(today.to_string());
    front.path = None;
    front.op = None;
    let title = front.title.clone().unwrap_or_default();
    let body = if is_seed_body(target, body) {
        // 雛形の空欄は情報を持たないので、候補の本文で置き換える（created は候補のものにしない:
        // ページそのものは init のときからある）。
        if front.confidence.is_none() || front.confidence == Some(Confidence::Medium) {
            front.confidence = item.confidence.or(front.confidence);
        }
        format!("{}\n", item.body.trim())
    } else {
        // 候補の本文の先頭の `# <題名>` は節の見出しと重なるので落とす。
        let mut added = item.body.trim();
        if let Some(rest) = added.strip_prefix("# ") {
            let (first, tail) = rest.split_once('\n').unwrap_or((rest, ""));
            if first.trim() == item.title.trim() || first.trim() == title.trim() {
                added = tail.trim_start();
            }
        }
        let heading = if item.title.trim() == title.trim() || item.title.trim().is_empty() {
            format!("追記（{today}）")
        } else {
            format!("{}（{today} 追記）", item.title.trim())
        };
        format!("{}\n\n## {heading}\n\n{}\n", body.trim_end(), added.trim())
    };
    kb::render_page(&front, &body)
}

/// ADR-0047 D4（Phase 62）: `op = retire` の accept。候補の本文は書かず、`item.target`
/// （退役させる既存ページ）を `_retired/<target>` へ動かす。`target` が無ければ何もできない。
fn inbox_accept_retire(root: &Path, item: &InboxItem) -> InboxOutcome {
    if !root.join(&item.target).exists() {
        return InboxOutcome::Failed {
            detail: format!("退役させるページがありません: {}", item.target),
        };
    }
    let Some(raw) = std::fs::read_to_string(root.join(&item.target)).ok() else {
        return InboxOutcome::Failed {
            detail: format!("{} を読めませんでした", item.target),
        };
    };
    let retired_path = format!("{RETIRED_DIR}/{}", item.target);
    let retired_file = root.join(&retired_path);
    if let Some(parent) = retired_file.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return InboxOutcome::Failed {
            detail: format!("{retired_path} を作れませんでした"),
        };
    }
    if std::fs::write(&retired_file, raw.as_bytes()).is_err() {
        return InboxOutcome::Failed {
            detail: format!("{retired_path} を書けませんでした"),
        };
    }
    if std::fs::remove_file(root.join(&item.target)).is_err() {
        return InboxOutcome::Failed {
            detail: format!("{} を消せませんでした", item.target),
        };
    }
    if std::fs::remove_file(root.join(&item.path)).is_err() {
        return InboxOutcome::Failed {
            detail: format!("{} を消せませんでした", item.path),
        };
    }
    match commit_paths(
        root,
        &format!(
            "knowledge: retire {}（候補 {} を取り込む）",
            item.target, item.id
        ),
        (kb::HUMAN_AUTHOR_NAME, kb::HUMAN_AUTHOR_EMAIL),
        &[
            retired_path.as_str(),
            item.target.as_str(),
            item.path.as_str(),
        ],
    ) {
        Ok(sha) => {
            let _ = reindex(root);
            InboxOutcome::Accepted {
                etag: None,
                path: retired_path,
                sha,
            }
        }
        Err(detail) => InboxOutcome::Failed { detail },
    }
}

/// ADR-0047 D5: 候補を捨てる（`_inbox` から消してコミットする。git に履歴は残る）。
pub fn inbox_reject(root: &Path, id: &str) -> InboxOutcome {
    let Ok(path) = inbox_path(id) else {
        return InboxOutcome::Missing;
    };
    if !root.join(&path).exists() {
        return InboxOutcome::Missing;
    }
    if std::fs::remove_file(root.join(&path)).is_err() {
        return InboxOutcome::Failed {
            detail: format!("{path} を消せませんでした"),
        };
    }
    match commit_paths(
        root,
        &format!("knowledge: 候補 {path} を捨てる"),
        (kb::HUMAN_AUTHOR_NAME, kb::HUMAN_AUTHOR_EMAIL),
        &[path.as_str()],
    ) {
        Ok(sha) => InboxOutcome::Rejected { sha },
        Err(detail) => InboxOutcome::Failed { detail },
    }
}

// ---------------------------------------------------------------------------
// 候補の適用（ADR-0047 D4。Phase 62）。`crates/celeris` が知識整理 run の終端で 1 度だけ呼ぶ。
// ---------------------------------------------------------------------------

/// [`apply_candidates`] の結果。`task_core::KnowledgeRunSummary` と対になる件数を持つ。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplyOutcome {
    /// KB へ直接コミットした KB 相対パス。
    pub committed: Vec<String>,
    /// `_inbox/` へ送った候補の KB 相対パス（`_inbox/<id>.md`）。
    pub inboxed: Vec<String>,
    /// 検査で落とした候補（元の `candidate.path` と理由）。
    pub dropped: Vec<(String, String)>,
}

impl ApplyOutcome {
    /// `knowledge_runs.summary_json` に書く形（Console・タイムラインが読む）。
    pub fn summary(&self) -> task_core::KnowledgeRunSummary {
        task_core::KnowledgeRunSummary {
            candidates: (self.committed.len() + self.inboxed.len() + self.dropped.len()) as u32,
            ingested: self.committed.len() as u32,
            inbox: self.inboxed.len() as u32,
            discarded: self.dropped.len() as u32,
            discarded_reasons: self
                .dropped
                .iter()
                .map(|(path, why)| format!("{path}: {why}"))
                .collect(),
            // ADR-0052 D2: どの経路で抽出したかは呼び出し側（`celeris::knowledge_maint`）が run の
            // `WorkerStarted` から決めて埋める（適用そのものは経路に関係なく同じ）。
            via: None,
        }
    }
}

/// ADR-0047 D4: 知識整理 run（`langmem` アダプタ）が書いた候補を適用する。
///
/// - 検査を通らない候補（path 境界・`.md`・題名/出典/本文なし・サイズ超過・秘密。
///   [`kb::validate_candidate`]）は**落とす**（どこにも書かない。`dropped` に理由を残す）。
/// - `confidence = high` かつ `op ∈ {create, update}` で、対象に人の未コミット編集が無ければ
///   KB へ**直接**コミットする（author [`kb::AGENT_AUTHOR_NAME`]、message
///   `knowledge: <op> <path> (task <task_id>)`。`update` は既存の `sources`/`created` を引き継ぐ）。
/// - それ以外（`merge`/`retire`/`medium`/`low`/人の編集と衝突/`create` なのに既にある/`update` なのに
///   まだ無い）は `_inbox/` へ（front matter に取り込み先 `path` と `op` を持たせる。人が GUI で
///   accept/reject する）。
///
/// 適用のあとに 1 度だけ [`reindex`] する。
pub fn apply_candidates(root: &Path, task_id: &str, candidates: &[kb::Candidate]) -> ApplyOutcome {
    apply_candidates_with_policy(root, task_id, candidates, ApplyPolicy::Task)
}

/// GC may only propose edits to existing pages, always through human review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyPolicy {
    Task,
    Gc,
}

pub fn apply_candidates_with_policy(
    root: &Path,
    task_id: &str,
    candidates: &[kb::Candidate],
    policy: ApplyPolicy,
) -> ApplyOutcome {
    apply_candidates_in(root, task_id, candidates, policy, &layout(root, None))
}

/// [`apply_candidates_with_policy`] に置き場の状況（[`layout`]。案件の一覧つき）を渡す形。
///
/// Phase K-1: 検査（[`kb::validate_candidate`]）のあとに**置き場のガード**（[`kb::place`]。
/// `record` / MCP `knowledge_propose` と同じ関数）を通す。落ちた候補は `dropped` に理由を残す。
/// `project:<案件 ID>` は `project:<slug>` に、`projects/<案件 ID>/…` は `projects/<slug>/…` に直す。
/// `create` の置き場が既にあるか、同じ scope に同じ題名のページがあれば、そのページへの
/// `append`（`_inbox/`。人が accept すると末尾に節として足す）にする。
pub fn apply_candidates_in(
    root: &Path,
    task_id: &str,
    candidates: &[kb::Candidate],
    policy: ApplyPolicy,
    layout: &kb::Layout,
) -> ApplyOutcome {
    let mut out = ApplyOutcome::default();
    for candidate in candidates {
        let path = match kb::validate_candidate(candidate) {
            Ok(p) => p,
            Err(e) => {
                out.dropped.push((candidate.path.clone(), e.to_string()));
                continue;
            }
        };
        if policy == ApplyPolicy::Gc
            && (candidate.op == kb::CandidateOp::Create
                || path
                    .split('/')
                    .any(|part| matches!(part, "skills" | "_inbox" | "_retired"))
                || !root
                    .canonicalize()
                    .ok()
                    .zip(root.join(&path).canonicalize().ok())
                    .is_some_and(|(base, target)| target.starts_with(base))
                || read_page(root, &path).is_none())
        {
            out.dropped
                .push((candidate.path.clone(), "GC cannot create pages".into()));
            continue;
        }
        let placement = match kb::place(
            &kb::PlacementRequest {
                op: Some(candidate.op),
                path: Some(&path),
                scope: Some(candidate.scope.as_str()),
                title: &candidate.title,
                tags: &candidate.tags,
            },
            layout,
        ) {
            Ok(p) => p,
            Err(e) => {
                out.dropped
                    .push((candidate.path.clone(), format!("placement: {e}")));
                continue;
            }
        };
        let mut placed = candidate.clone();
        placed.path = placement.path.clone();
        // ADR-0047 付記 H2: run の候補は人が書いた印を作れない（`human:instruction` に正し、task を添える）。
        let existing_sources = read_page(root, &placement.path)
            .map(|raw| kb::front_matter(&raw).0.sources)
            .unwrap_or_default();
        placed.sources =
            kb::normalize_agent_sources(&candidate.sources, &existing_sources, Some(task_id));
        placed.scope = placement.scope.clone().unwrap_or_default();
        if candidate.op == kb::CandidateOp::Create
            && (placement.redirect.is_some() || root.join(&placement.path).exists())
        {
            placed.op = kb::CandidateOp::Append;
        }
        let candidate = &placed;
        let path = placement.path;
        let eligible = policy == ApplyPolicy::Task
            && candidate.confidence == Confidence::High
            && candidate.op.direct_commit_eligible()
            && direct_commit_fits(root, &path, candidate.op);
        if eligible && commit_candidate_directly(root, task_id, candidate, &path).is_ok() {
            out.committed.push(path);
            continue;
        }
        match write_inbox_candidate(root, task_id, candidate, &path) {
            Ok(inbox_path) => out.inboxed.push(inbox_path),
            Err(detail) => out.dropped.push((candidate.path.clone(), detail)),
        }
    }
    let _ = reindex(root);
    out
}

/// 直接コミットしてよい形か: `create` は対象がまだ無いこと、`update` は対象があって
/// 人の未コミット編集が無いこと（[`has_uncommitted_changes`]）。それ以外の組は `_inbox/` へ。
fn direct_commit_fits(root: &Path, path: &str, op: kb::CandidateOp) -> bool {
    let exists = root.join(path).exists();
    match op {
        kb::CandidateOp::Create => !exists,
        kb::CandidateOp::Update => exists && !has_uncommitted_changes(root, path),
        kb::CandidateOp::Merge | kb::CandidateOp::Retire | kb::CandidateOp::Append => false,
    }
}

/// 対象ページに人の未コミット編集（変更・未追跡）があるか。無ければ `false`（`git` が使えない
/// 環境でも安全側＝直接コミットを妨げない）。
fn has_uncommitted_changes(root: &Path, path: &str) -> bool {
    match git(root, &["status", "--porcelain", "--", path], GIT_TIMEOUT) {
        Some(o) if o.ok => !o.stdout.trim().is_empty(),
        _ => false,
    }
}

/// `confidence = high` の `create`/`update` を KB へ直接コミットする。
fn commit_candidate_directly(
    root: &Path,
    task_id: &str,
    candidate: &kb::Candidate,
    path: &str,
) -> Result<(), ()> {
    let current_etag = etag(root, path);
    let existing_front = read_page(root, path).map(|raw| kb::front_matter(&raw).0);
    let mut sources: Vec<String> = existing_front
        .as_ref()
        .map(|f| f.sources.clone())
        .unwrap_or_default();
    for s in &candidate.sources {
        let s = s.trim().to_string();
        if !s.is_empty() && !sources.contains(&s) {
            sources.push(s);
        }
    }
    let created = existing_front
        .as_ref()
        .and_then(|f| f.created.clone())
        .unwrap_or_else(today);
    let front = FrontMatter {
        title: Some(candidate.title.trim().to_string()),
        tags: dedup_tags_preserve_order(candidate.tags.clone()),
        scope: Some(candidate.scope.trim().to_string()).filter(|s| !s.is_empty()),
        sources,
        created: Some(created),
        updated: Some(today()),
        confidence: Some(candidate.confidence),
        path: None,
        op: None,
    };
    let page = kb::render_page(&front, candidate.body.trim());
    let edit = PageEdit {
        path: path.to_string(),
        body: Some(page),
        etag: current_etag,
        message: format!("knowledge: {} {path} (task {task_id})", candidate.op),
        author: (
            kb::AGENT_AUTHOR_NAME.to_string(),
            kb::AGENT_AUTHOR_EMAIL.to_string(),
        ),
    };
    match commit_page(root, &edit) {
        WriteOutcome::Written { .. } => Ok(()),
        _ => Err(()),
    }
}

/// `_inbox/` へ候補を書く（[`record`] と同じファイル名の作り方。front matter に取り込み先 `path` と
/// `op` を持たせる）。
fn write_inbox_candidate(
    root: &Path,
    task_id: &str,
    candidate: &kb::Candidate,
    target_path: &str,
) -> Result<String, String> {
    let now = OffsetDateTime::now_utc();
    let stamp = format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    );
    let slug = kb::slugify(&candidate.title).unwrap_or_else(|| "candidate".to_string());
    let mut id = format!("{stamp}-{slug}");
    let mut n = 2;
    while root.join(INBOX_DIR).join(format!("{id}.md")).exists() {
        id = format!("{stamp}-{slug}-{n}");
        n += 1;
    }
    let path = format!("{INBOX_DIR}/{id}.md");
    let mut sources = candidate.sources.clone();
    let task_source = format!("task:{task_id}");
    if !sources.iter().any(|s| s == &task_source) {
        sources.push(task_source);
    }
    let front = FrontMatter {
        title: Some(candidate.title.trim().to_string()),
        tags: dedup_tags_preserve_order(candidate.tags.clone()),
        scope: Some(candidate.scope.trim().to_string()).filter(|s| !s.is_empty()),
        sources,
        created: Some(today()),
        updated: Some(today()),
        confidence: Some(candidate.confidence),
        path: Some(target_path.to_string()),
        op: Some(candidate.op.as_str().to_string()),
    };
    let page = kb::render_page(&front, candidate.body.trim());
    let dir = root.join(INBOX_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{INBOX_DIR} を作れませんでした: {e}"))?;
    std::fs::write(dir.join(format!("{id}.md")), page.as_bytes())
        .map_err(|e| format!("{path} を書けませんでした: {e}"))?;
    commit_paths(
        root,
        &format!("knowledge: 候補 {path}（task {task_id}）"),
        (kb::AGENT_AUTHOR_NAME, kb::AGENT_AUTHOR_EMAIL),
        &[path.as_str()],
    )
    .map(|_| path)
}

// ADR-0047 付記（2026-10-04）H4: 旧形の `sources: human` の移行。
mod human_sources;
pub use human_sources::*;

// Skill の永続化はページ・候補管理と独立して変更できる。
mod skills;
pub use skills::*;
// ADR-0122 D1: repo に写した skill のディレクトリから `skills_put` で取り込む。
mod skill_import;
pub use skill_import::*;

// ---------------------------------------------------------------------------
// ADR-0131 付記（2026-10-04、日次整理の自動適用）: 適用後の 1 commit と KB の remote への push。
// daemon が再検証に通った計画を適用した後に呼ぶ部品。判断も LLM も無く `git` を起こすだけ。
// push はネットワークに出るが LLM 呼び出しではない（配送の `git push` と同じ扱い）。

/// 日次整理 1 回の件数（commit の題に書く）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CurationCounts {
    /// 重複の統合。
    pub merged: usize,
    /// 新規ページ。
    pub new: usize,
    /// 削除（retire を含む）。
    pub deleted: usize,
    /// 古い記述の修正。
    pub fixed: usize,
}

/// commit の題: `knowledge curation 2026-10-04: 統合 2・新規 1・削除 0・修正 3`。
pub fn curation_commit_subject(date: &str, counts: CurationCounts) -> String {
    format!(
        "knowledge curation {date}: 統合 {}・新規 {}・削除 {}・修正 {}",
        counts.merged, counts.new, counts.deleted, counts.fixed
    )
}

/// 適用で変えた `paths`（KB 相対。`_curation/YYYY-MM-DD.md`・`index.json`・`README.md` を含めてよい）を
/// 1 commit にまとめる。作者は [`commit_paths`] と同じ設定（`kb::AGENT_AUTHOR_*`）。
///
/// `.gitignore` で除かれた path（`index.json` は派生物で追跡しない）と、作業ツリーにも index にも
/// 無い path は commit の対象から外す。変更が無ければ新しい commit を作らず HEAD を返す。
pub fn commit_curation(
    root: &Path,
    date: &str,
    counts: CurationCounts,
    task_id: &str,
    paths: &[&str],
) -> Result<String, String> {
    let mut kept: Vec<&str> = Vec::new();
    for path in paths {
        if path.is_empty() || kept.contains(path) {
            continue;
        }
        if git(root, &["check-ignore", "-q", "--", path], GIT_TIMEOUT).is_some_and(|o| o.ok) {
            continue;
        }
        let tracked = git(
            root,
            &["ls-files", "--error-unmatch", "--", path],
            GIT_TIMEOUT,
        )
        .is_some_and(|o| o.ok);
        if !tracked && !root.join(path).exists() {
            continue;
        }
        kept.push(path);
    }
    if kept.is_empty() {
        return head(root).ok_or_else(|| "HEAD を読めませんでした".to_string());
    }
    let message = format!(
        "{}\n\ntask: {task_id}\n",
        curation_commit_subject(date, counts)
    );
    commit_paths(
        root,
        &message,
        (kb::AGENT_AUTHOR_NAME, kb::AGENT_AUTHOR_EMAIL),
        &kept,
    )
}

/// [`push_remote`] の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushOutcome {
    /// remote が無い（push を省いた。記録だけ残す）。
    NoRemote,
    /// push した（既に最新だった場合も含む）。
    Pushed { remote: String, branch: String },
    /// push できなかった。apply は失敗にせず、報告と event に残して次回の push でまとめて送る。
    Failed(String),
}

fn git_line(root: &Path, args: &[&str]) -> Option<String> {
    git(root, args, GIT_TIMEOUT)
        .filter(|o| o.ok)
        .map(|o| o.stdout.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 現在の branch を、その upstream の remote（無ければ `origin`）へ `git push` する。
/// force push はしない。非対話（`GIT_TERMINAL_PROMPT=0`・ssh の `BatchMode=yes`）で、時間の上限がある。
pub fn push_remote(root: &Path) -> PushOutcome {
    let Some(branch) = git_line(root, &["symbolic-ref", "--short", "-q", "HEAD"]) else {
        return PushOutcome::Failed("現在の branch を決められません（detached HEAD）".to_string());
    };
    let remotes: Vec<String> = git_line(root, &["remote"])
        .map(|s| s.lines().map(|l| l.trim().to_string()).collect())
        .unwrap_or_default();
    if remotes.is_empty() {
        return PushOutcome::NoRemote;
    }
    let upstream_remote = git_line(
        root,
        &["config", "--get", &format!("branch.{branch}.remote")],
    )
    .filter(|r| remotes.contains(r));
    let (remote, dest) = match upstream_remote {
        Some(remote) => {
            let merge = git_line(
                root,
                &["config", "--get", &format!("branch.{branch}.merge")],
            )
            .unwrap_or_else(|| format!("refs/heads/{branch}"));
            (remote, merge)
        }
        None if remotes.iter().any(|r| r == "origin") => {
            ("origin".to_string(), format!("refs/heads/{branch}"))
        }
        None => return PushOutcome::NoRemote,
    };
    let ssh_command = match std::env::var("GIT_SSH_COMMAND") {
        Ok(existing) if !existing.trim().is_empty() => format!("{existing} -o BatchMode=yes"),
        _ => "ssh -o BatchMode=yes".to_string(),
    };
    let refspec = format!("refs/heads/{branch}:{dest}");
    match crate::changes::git_with_env(
        root,
        &["push", "-q", &remote, &refspec],
        &[("GIT_SSH_COMMAND", ssh_command.as_str())],
        GIT_WRITE_TIMEOUT,
    ) {
        Some(o) if o.ok => PushOutcome::Pushed { remote, branch },
        Some(o) => PushOutcome::Failed(format!("{remote} への push に失敗: {}", o.why())),
        None => PushOutcome::Failed("git を起動できませんでした".to_string()),
    }
}

#[cfg(test)]
mod curation_git_tests;
#[cfg(test)]
mod tests;
