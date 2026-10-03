//! ADR 2026-10-04-release-notes: リリースの説明（`<release>/notes.json` と `notes.md`）と昇格の要約。
//!
//! - [`generate`]: `base..sha` の first-parent の範囲を Celeris の task 単位にまとめる。task の判別は
//!   配送記録（`GET /deliveries` の head。配送は早送りなので head が first-parent に載る）と branch 名
//!   （`celeris/<ULID>` の先端・merge commit の題）。題と要約は `GET /tasks/{id}`（取れなければ commit 題だけ）。
//!   ほかに task に属さない直接の commit、migration と schema_version の変化、ADR、
//!   `config/celeris.example.toml` の変更、gate で飛ばした段を書く。
//! - [`aggregate`]: `current` から対象リリースまでに入る全リリースの notes を 1 つにまとめる
//!   （同じ task は 1 回、schema は `current` と対象の値、mode は対象の `verify.json`）。
//!
//! どちらも**決定的**（git と JSON だけ。LLM もワーカーも関与しない）。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use task_api::types::{
    DeliveryHead, RELEASE_NOTES_FIRST_PARENT_LIMIT, ReleaseNoteChild, ReleaseNoteCommit,
    ReleaseNoteConfig, ReleaseNoteFile, ReleaseNoteGateSkip, ReleaseNoteSchema, ReleaseNoteTask,
    ReleaseNotes, ReleasePromotionPreview, ReleasePromotionRelease,
};

/// git を待つ上限（release.sh の中で 1 回走るだけなので長め）。
const GIT_TIMEOUT: Duration = Duration::from_secs(120);
pub const MIGRATIONS_DIR: &str = "crates/task-core/migrations/";
pub const ADR_DIRS: [&str; 2] = ["agent-docs/adr/", "docs/adr/"];
pub const CONFIG_EXAMPLE: &str = "config/celeris.example.toml";
const SUMMARY_MAX_CHARS: usize = 600;
const CONFIG_LINES_MAX: usize = 40;

/// `GET /tasks/{id}` から取る task の情報。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaskInfo {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub status: Option<String>,
    pub parent_id: Option<String>,
}

/// task id → 情報。API に届かない・知らない task は `None`（commit 題だけで書く）。
pub trait TaskLookup {
    fn task(&self, task_id: &str) -> Option<TaskInfo>;
}

/// 何も引かない（API が無いとき・試験用）。
pub struct NoLookup;

impl TaskLookup for NoLookup {
    fn task(&self, _task_id: &str) -> Option<TaskInfo> {
        None
    }
}

impl TaskLookup for HashMap<String, TaskInfo> {
    fn task(&self, task_id: &str) -> Option<TaskInfo> {
        self.get(task_id).cloned()
    }
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// `GET /tasks/{id}`（`TaskDetail`）の JSON から題・要約・status・親を取る。要約は最後に `done` で
/// 終わった worker run の `outcome_text`。
pub fn task_info_from_detail(v: &serde_json::Value) -> TaskInfo {
    let task = &v["task"];
    let text = |x: &serde_json::Value| x.as_str().map(str::to_string);
    let summary = v["runs"].as_array().and_then(|runs| {
        runs.iter()
            .rev()
            .filter(|r| r["role"] == "worker" && r["outcome"] == "done")
            .filter_map(|r| r["outcome_text"].as_str())
            .map(str::trim)
            .find(|t| !t.is_empty())
            .map(|t| clip(t, SUMMARY_MAX_CHARS))
    });
    TaskInfo {
        title: text(&task["title"]),
        summary,
        status: text(&task["status"]),
        parent_id: text(&task["parent_id"]),
    }
}

/// 文字列の中の `celeris/<ULID>` / `celeris-wu/<ULID>/…` の ULID（最初の 1 つ）。
pub fn task_id_in(s: &str) -> Option<String> {
    for prefix in ["celeris/", "celeris-wu/"] {
        let mut rest = s;
        while let Some(i) = rest.find(prefix) {
            let after = &rest[i + prefix.len()..];
            let cand: String = after.chars().take(26).collect();
            let boundary_ok = after[cand.len()..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_ascii_alphanumeric());
            if is_ulid(&cand) && boundary_ok {
                return Some(cand);
            }
            rest = after;
        }
    }
    None
}

fn is_ulid(s: &str) -> bool {
    s.len() == 26
        && s.chars()
            .all(|c| c.is_ascii_digit() || (c.is_ascii_uppercase() && !"ILOU".contains(c)))
}

fn git_text(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = task_ops::changes::git(repo, args, GIT_TIMEOUT)
        .ok_or_else(|| "git を起動できません".to_string())?;
    if out.ok {
        Ok(out.stdout)
    } else {
        Err(format!("git {}: {}", args.join(" "), out.why()))
    }
}

fn resolve(repo: &Path, rev: &str) -> Option<String> {
    let spec = format!("{rev}^{{commit}}");
    git_text(repo, &["rev-parse", "--verify", "--quiet", &spec])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// [`generate`] の入力。
pub struct NotesInput<'a> {
    pub repo: &'a Path,
    /// 起点（ビルド時の `current` の sha。sha12 でもよい）。無い・repo が知らないなら空の説明。
    pub base: Option<&'a str>,
    pub sha: &'a str,
    pub schema_from: Option<u32>,
    pub schema_to: Option<u32>,
    /// このリリースの `gate.json`（飛ばした段を拾う）。
    pub gate: Option<&'a serde_json::Value>,
    /// 配送記録。`None` は「読めなかった」（branch 名だけで判別する）。
    pub deliveries: Option<&'a [DeliveryHead]>,
    /// RFC 3339。
    pub generated_at: String,
}

fn schema(from: Option<u32>, to: Option<u32>) -> ReleaseNoteSchema {
    ReleaseNoteSchema {
        from,
        to,
        changed: match (from, to) {
            (Some(a), Some(b)) => Some(a != b),
            _ => None,
        },
    }
}

/// `gate.json` の `steps[]` から `skipped: true` の段。
pub fn gate_skips(gate: Option<&serde_json::Value>) -> Vec<ReleaseNoteGateSkip> {
    gate.and_then(|g| g["steps"].as_array())
        .map(|steps| {
            steps
                .iter()
                .filter(|s| s["skipped"] == true)
                .map(|s| ReleaseNoteGateSkip {
                    step: s["step"].as_str().unwrap_or("").to_string(),
                    reason: s["reason"].as_str().unwrap_or("").to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

struct Head {
    task_id: String,
    source: &'static str,
    /// 配送の `base`（配送時の取り込み先の先端）。ここに着いたらその task の区間は終わり。
    stop: Option<String>,
}

#[derive(Default)]
struct TaskAcc {
    source: &'static str,
    commits: Vec<ReleaseNoteCommit>,
}

/// `base..sha` の説明を作る。git が失敗したら `Err`（release.sh は警告だけにしてリリースは作る）。
pub fn generate(input: &NotesInput<'_>, lookup: &dyn TaskLookup) -> Result<ReleaseNotes, String> {
    let repo = input.repo;
    let sha = resolve(repo, input.sha).ok_or_else(|| format!("unknown commit {}", input.sha))?;
    let base = input
        .base
        .filter(|b| !b.trim().is_empty())
        .and_then(|b| resolve(repo, b.trim()));
    let mut notes = ReleaseNotes {
        version: 1,
        sha12: sha.chars().take(12).collect(),
        sha: sha.clone(),
        base: base.clone(),
        generated_at: input.generated_at.clone(),
        first_parent: Vec::new(),
        truncated: false,
        deliveries_known: input.deliveries.is_some(),
        tasks: Vec::new(),
        direct_commits: Vec::new(),
        migrations: Vec::new(),
        schema: schema(input.schema_from, input.schema_to),
        adrs: Vec::new(),
        config_example: None,
        gate_skips: gate_skips(input.gate),
    };
    let Some(base) = base else {
        return Ok(notes);
    };
    let range = format!("{base}..{sha}");

    // first-parent（新しい順）: sha, 親の数, 題。
    let fp: Vec<(String, usize, String)> = git_text(
        repo,
        &["log", "--first-parent", "--format=%H%x09%P%x09%s", &range],
    )?
    .lines()
    .filter_map(|l| {
        let mut it = l.splitn(3, '\t');
        let sha = it.next()?.to_string();
        let parents = it.next()?.split_whitespace().count();
        let subject = it.next().unwrap_or("").to_string();
        (!sha.is_empty()).then_some((sha, parents, subject))
    })
    .collect();
    let fp_set: HashSet<&str> = fp.iter().map(|c| c.0.as_str()).collect();
    // 範囲の全 commit（second parent の側も。新しい順）: 早送りでない取り込みの配送 head を拾う。
    let all: Vec<(String, String)> = git_text(repo, &["log", "--format=%H%x09%s", &range])?
        .lines()
        .filter_map(|l| {
            let (s, subj) = l.split_once('\t')?;
            Some((s.to_string(), subj.to_string()))
        })
        .collect();

    // commit → task。branch の先端を先に入れ、配送記録で上書きする（配送の方が確か）。
    let mut heads: HashMap<String, Head> = HashMap::new();
    if let Ok(refs) = git_text(
        repo,
        &[
            "for-each-ref",
            "--format=%(objectname)%09%(refname:short)",
            "refs/heads/celeris/",
        ],
    ) {
        for line in refs.lines() {
            let Some((obj, name)) = line.split_once('\t') else {
                continue;
            };
            if let Some(id) = name.strip_prefix("celeris/").filter(|r| is_ulid(r)) {
                heads.insert(
                    obj.to_string(),
                    Head {
                        task_id: id.to_string(),
                        source: "branch",
                        stop: None,
                    },
                );
            }
        }
    }
    for d in input.deliveries.unwrap_or_default() {
        for s in [
            Some(&d.head),
            d.reviewed_sha.as_ref(),
            d.merge_candidate_sha.as_ref(),
        ]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        {
            let full = if s.len() == 40 {
                Some(s.clone())
            } else {
                resolve(repo, s)
            };
            if let Some(full) = full {
                heads.insert(
                    full,
                    Head {
                        task_id: d.task_id.clone(),
                        source: "delivery",
                        stop: d.base.clone().filter(|b| !b.is_empty()),
                    },
                );
            }
        }
    }

    let mut order: Vec<String> = Vec::new();
    let mut acc: HashMap<String, TaskAcc> = HashMap::new();
    fn add(
        acc: &mut HashMap<String, TaskAcc>,
        order: &mut Vec<String>,
        id: &str,
        source: &'static str,
        c: ReleaseNoteCommit,
    ) {
        let entry = acc.entry(id.to_string()).or_insert_with(|| {
            order.push(id.to_string());
            TaskAcc {
                source,
                commits: Vec::new(),
            }
        });
        if source == "delivery" {
            entry.source = "delivery";
        }
        if !entry.commits.iter().any(|x| x.sha == c.sha) {
            entry.commits.push(c);
        }
    }
    let mut cur: Option<(String, &'static str, Option<String>)> = None;
    for (c, parents, subject) in &fp {
        if cur
            .as_ref()
            .and_then(|x| x.2.as_deref())
            .is_some_and(|stop| c.starts_with(stop) || stop.starts_with(c.as_str()))
        {
            cur = None;
        }
        if let Some(h) = heads.get(c) {
            cur = Some((h.task_id.clone(), h.source, h.stop.clone()));
        }
        let commit = ReleaseNoteCommit {
            sha: c.clone(),
            subject: subject.clone(),
        };
        if let Some((id, source, _)) = &cur {
            add(&mut acc, &mut order, id, source, commit);
            continue;
        }
        if *parents > 1
            && let Some(id) = task_id_in(subject)
        {
            add(&mut acc, &mut order, &id, "branch", commit);
            continue;
        }
        notes.direct_commits.push(commit);
    }
    // first-parent に載らない（merge の second parent 側の）配送 head・branch 先端: その task を 1 件で足す。
    for (c, subject) in &all {
        if fp_set.contains(c.as_str()) {
            continue;
        }
        if let Some(h) = heads.get(c)
            && !acc.contains_key(&h.task_id)
        {
            add(
                &mut acc,
                &mut order,
                &h.task_id,
                h.source,
                ReleaseNoteCommit {
                    sha: c.clone(),
                    subject: subject.clone(),
                },
            );
        }
    }

    // 題と要約を引き、親が一覧に居る子 task は親の下へ。
    let infos: HashMap<String, TaskInfo> = order
        .iter()
        .filter_map(|id| lookup.task(id).map(|i| (id.clone(), i)))
        .collect();
    let present: HashSet<&str> = order.iter().map(String::as_str).collect();
    let top_of = |id: &str| -> Option<String> {
        let mut at = id.to_string();
        let mut found = None;
        for _ in 0..8 {
            let Some(p) = infos.get(&at).and_then(|i| i.parent_id.clone()) else {
                break;
            };
            if present.contains(p.as_str()) {
                found = Some(p.clone());
            }
            at = p;
        }
        found
    };
    let mut tasks: Vec<ReleaseNoteTask> = Vec::new();
    let mut folded: Vec<(String, String)> = Vec::new();
    for id in &order {
        if let Some(parent) = top_of(id) {
            folded.push((parent, id.clone()));
            continue;
        }
        let info = infos.get(id).cloned().unwrap_or_default();
        let a = acc.remove(id).unwrap_or_default();
        tasks.push(ReleaseNoteTask {
            task_id: id.clone(),
            title: info.title,
            summary: info.summary,
            status: info.status,
            source: a.source.to_string(),
            commits: a.commits,
            children: Vec::new(),
        });
    }
    for (parent, child) in folded {
        if let Some(t) = tasks.iter_mut().find(|t| t.task_id == parent) {
            t.children.push(ReleaseNoteChild {
                title: infos.get(&child).and_then(|i| i.title.clone()),
                task_id: child,
            });
        }
    }
    notes.tasks = tasks;

    // 変わったファイル。
    let name_status = git_text(repo, &["diff", "--name-status", "-M", &base, &sha])?;
    let last_commit = |path: &str| -> Option<String> {
        git_text(
            repo,
            &[
                "log",
                "--first-parent",
                "-n",
                "1",
                "--format=%H",
                &range,
                "--",
                path,
            ],
        )
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    };
    for line in name_status.lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        let (status, path) = match cols.as_slice() {
            [st, _old, new] if st.starts_with('R') || st.starts_with('C') => ("added", *new),
            [st, path] => (
                match st.chars().next() {
                    Some('A') => "added",
                    Some('D') => "deleted",
                    _ => "modified",
                },
                *path,
            ),
            _ => continue,
        };
        let file = |title: Option<String>| ReleaseNoteFile {
            path: path.to_string(),
            status: status.to_string(),
            commit: last_commit(path),
            title,
        };
        if path.starts_with(MIGRATIONS_DIR) {
            notes.migrations.push(file(None));
        } else if ADR_DIRS.iter().any(|d| path.starts_with(d)) && path.ends_with(".md") {
            let title = (status != "deleted")
                .then(|| git_text(repo, &["show", &format!("{sha}:{path}")]).ok())
                .flatten()
                .and_then(|text| {
                    text.lines()
                        .find_map(|l| l.strip_prefix("# ").map(|t| t.trim().to_string()))
                });
            notes.adrs.push(file(title));
        } else if path == CONFIG_EXAMPLE {
            let diff =
                git_text(repo, &["diff", &base, &sha, "--", CONFIG_EXAMPLE]).unwrap_or_default();
            let added: Vec<String> = diff
                .lines()
                .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
                .map(|l| l[1..].trim().to_string())
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .collect();
            let sections = added
                .iter()
                .filter(|l| l.starts_with('[') && l.ends_with(']'))
                .cloned()
                .collect();
            notes.config_example = Some(ReleaseNoteConfig {
                path: path.to_string(),
                status: status.to_string(),
                commit: last_commit(path),
                needs_review: !added.is_empty(),
                added_lines: added.into_iter().take(CONFIG_LINES_MAX).collect(),
                added_sections: sections,
            });
        }
    }
    notes.truncated = fp.len() > RELEASE_NOTES_FIRST_PARENT_LIMIT;
    notes.first_parent = fp
        .into_iter()
        .take(RELEASE_NOTES_FIRST_PARENT_LIMIT)
        .map(|c| c.0)
        .collect();
    Ok(notes)
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

fn task_line(t: &ReleaseNoteTask) -> String {
    let title = t
        .title
        .clone()
        .or_else(|| t.commits.first().map(|c| c.subject.clone()))
        .unwrap_or_else(|| "(題なし)".into());
    let mut s = format!("- **{title}** (`{}`)", t.task_id);
    if let Some(sum) = &t.summary {
        s.push_str(&format!("\n  - 要約: {}", sum.replace('\n', " ")));
    }
    for c in &t.children {
        s.push_str(&format!(
            "\n  - 子 task: {} (`{}`)",
            c.title.as_deref().unwrap_or("(題なし)"),
            c.task_id
        ));
    }
    s
}

fn schema_line(s: &ReleaseNoteSchema) -> String {
    match (s.from, s.to, s.changed) {
        (Some(a), Some(b), Some(true)) => {
            format!("schema_version {a} → {b}（DB の移行あり。旧 daemon が読めなければ停止→起動）")
        }
        (_, Some(b), Some(false)) => format!("schema_version {b}（変化なし）"),
        (a, b, _) => format!(
            "schema_version {} → {}（起点が分からない）",
            a.map_or("?".into(), |v| v.to_string()),
            b.map_or("?".into(), |v| v.to_string())
        ),
    }
}

fn push_common(
    out: &mut String,
    direct: &[ReleaseNoteCommit],
    migrations: &[ReleaseNoteFile],
    schema: &ReleaseNoteSchema,
    adrs: &[ReleaseNoteFile],
    configs: &[&ReleaseNoteConfig],
    skips: &[ReleaseNoteGateSkip],
) {
    out.push_str(&format!(
        "\n## task に属さない commit（{}）\n\n",
        direct.len()
    ));
    for c in direct {
        out.push_str(&format!("- `{}` {}\n", short(&c.sha), c.subject));
    }
    out.push_str("\n## DB migration と schema\n\n");
    out.push_str(&format!("- {}\n", schema_line(schema)));
    for m in migrations {
        out.push_str(&format!("- {} `{}`\n", m.status, m.path));
    }
    out.push_str(&format!("\n## ADR（{}）\n\n", adrs.len()));
    for a in adrs {
        out.push_str(&format!(
            "- {} `{}`{}\n",
            a.status,
            a.path,
            a.title
                .as_ref()
                .map(|t| format!(" — {t}"))
                .unwrap_or_default()
        ));
    }
    out.push_str("\n## config 例（config/celeris.example.toml）\n\n");
    if configs.is_empty() {
        out.push_str("- 変更なし\n");
    }
    for c in configs {
        if c.needs_review {
            out.push_str(&format!(
                "- 足された設定あり（本番 config に要るか人が確かめる）: {}\n",
                c.added_lines.join(" / ")
            ));
        } else {
            out.push_str("- 変更あり（足された設定行はなし）\n");
        }
    }
    out.push_str(&format!("\n## gate で飛ばした段（{}）\n\n", skips.len()));
    for s in skips {
        out.push_str(&format!("- {}: {}\n", s.step, s.reason));
    }
}

/// `notes.md`（人が読む版）。
pub fn render_markdown(n: &ReleaseNotes) -> String {
    let mut out = format!("# リリース {}\n\n", n.sha12);
    out.push_str(&format!(
        "起点: {}。作成: {}。task の判別: {}\n",
        n.base.as_deref().map(short).unwrap_or("（なし）"),
        n.generated_at,
        if n.deliveries_known {
            "配送記録と branch 名"
        } else {
            "branch 名だけ（配送記録を読めなかった）"
        }
    ));
    out.push_str(&format!("\n## 入った task（{}）\n\n", n.tasks.len()));
    for t in &n.tasks {
        out.push_str(&task_line(t));
        out.push('\n');
    }
    let configs: Vec<&ReleaseNoteConfig> = n.config_example.iter().collect();
    push_common(
        &mut out,
        &n.direct_commits,
        &n.migrations,
        &n.schema,
        &n.adrs,
        &configs,
        &n.gate_skips,
    );
    out
}

/// 昇格の要約の人向けの版（promote.sh がログに出す）。
pub fn render_preview_markdown(p: &ReleasePromotionPreview) -> String {
    let mut out = format!(
        "# 昇格の要約 {} → {}\n\n",
        p.from.as_deref().unwrap_or("（なし）"),
        p.to
    );
    out.push_str(&format!(
        "- 含まれるリリース: {}\n- 方式: {}\n",
        p.releases
            .iter()
            .map(|r| r.sha12.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        p.mode.as_deref().unwrap_or("未検証")
    ));
    if !p.complete {
        out.push_str(&format!(
            "- 注意: current まで辿り切れていない（{}）\n",
            p.problem.as_deref().unwrap_or("理由不明")
        ));
    }
    out.push_str(&format!("\n## 入る task（{}）\n\n", p.tasks.len()));
    for t in &p.tasks {
        out.push_str(&task_line(t));
        out.push('\n');
    }
    let configs: Vec<&ReleaseNoteConfig> = p.config_examples.iter().collect();
    push_common(
        &mut out,
        &p.direct_commits,
        &p.migrations,
        &p.schema,
        &p.adrs,
        &configs,
        &p.gate_skips,
    );
    out
}

/// [`aggregate`] に渡すリリース 1 件（`manifest.json` / `verify.json` / `notes.json` から）。
#[derive(Debug, Clone, PartialEq)]
pub struct ReleaseMeta {
    pub sha12: String,
    /// 完全な sha（manifest の `sha`。読めなければ sha12）。
    pub sha: String,
    pub built_at: Option<String>,
    pub schema_version: Option<u32>,
    /// `verify.json` の `live_ok`（未検証なら `None`）。
    pub live_ok: Option<bool>,
    pub notes: Option<ReleaseNotes>,
}

fn same_sha(a: &str, b: &str) -> bool {
    !a.is_empty() && !b.is_empty() && (a.starts_with(b) || b.starts_with(a))
}

/// `current` から `target` へ昇格したら入るものの要約。`all` は手元の全リリース（中間のリリースを
/// 見つけるのと、notes の `base` を辿るのに使う）。
///
/// 辿り方: 対象の notes の `base` が `current` なら 1 段で終わり。`current` が対象の `first_parent` に
/// あればそこで切る。どちらでもなければ `base` のリリースの notes へ進む（rollback 後など）。
pub fn aggregate(
    current: Option<&ReleaseMeta>,
    target: &ReleaseMeta,
    all: &[ReleaseMeta],
) -> ReleasePromotionPreview {
    // (リリース, 切る位置)。切る位置は `first_parent` の index（そこから古い方は current に入っている）。
    let mut segments: Vec<(&ReleaseMeta, Option<usize>)> = Vec::new();
    let mut complete = false;
    let mut problem = None;
    let mut node = target;
    for _ in 0..=all.len() {
        let Some(notes) = &node.notes else {
            problem = Some(format!("{} に notes.json が無い", node.sha12));
            break;
        };
        let Some(cur) = current else {
            segments.push((node, None));
            complete = true;
            break;
        };
        if notes.base.as_deref().is_some_and(|b| same_sha(b, &cur.sha)) {
            segments.push((node, None));
            complete = true;
            break;
        }
        if let Some(i) = notes
            .first_parent
            .iter()
            .position(|c| same_sha(c, &cur.sha))
        {
            segments.push((node, Some(i)));
            complete = true;
            break;
        }
        segments.push((node, None));
        let next = notes
            .base
            .as_deref()
            .and_then(|b| all.iter().find(|m| same_sha(&m.sha, b)));
        match next {
            Some(m) if !segments.iter().any(|(s, _)| s.sha12 == m.sha12) => node = m,
            _ => {
                problem = Some(format!(
                    "{} の起点 {} から current {} まで notes で辿れない",
                    node.sha12,
                    notes.base.as_deref().map(short).unwrap_or("（なし）"),
                    cur.sha12
                ));
                break;
            }
        }
    }

    let mut tasks: Vec<ReleaseNoteTask> = Vec::new();
    let mut direct: Vec<ReleaseNoteCommit> = Vec::new();
    let mut migrations: Vec<ReleaseNoteFile> = Vec::new();
    let mut adrs: Vec<ReleaseNoteFile> = Vec::new();
    let mut configs: Vec<ReleaseNoteConfig> = Vec::new();
    let mut included: HashSet<String> = HashSet::new();
    let mut seg_task_count: HashMap<String, usize> = HashMap::new();
    let merge_file = |list: &mut Vec<ReleaseNoteFile>, f: &ReleaseNoteFile| {
        match list.iter_mut().find(|x| x.path == f.path) {
            // 新しい方が先に入っている。古い方で足されていれば「足された」。
            Some(x) => {
                if f.status == "added" && x.status != "deleted" {
                    x.status = "added".into();
                }
            }
            None => list.push(f.clone()),
        }
    };
    for (meta, cut) in &segments {
        let Some(n) = &meta.notes else { continue };
        let fp: &[String] = match cut {
            Some(i) => &n.first_parent[..*i],
            None => &n.first_parent,
        };
        let fp_set: HashSet<&str> = fp.iter().map(String::as_str).collect();
        included.extend(fp.iter().cloned());
        let keep = |sha: Option<&String>| {
            cut.is_none() || sha.is_some_and(|s| fp_set.contains(s.as_str()))
        };
        let mut count = 0;
        for t in &n.tasks {
            if cut.is_some() && !t.commits.iter().any(|c| fp_set.contains(c.sha.as_str())) {
                continue;
            }
            count += 1;
            match tasks.iter_mut().find(|x| x.task_id == t.task_id) {
                Some(x) => {
                    for c in &t.commits {
                        if !x.commits.iter().any(|y| y.sha == c.sha) {
                            x.commits.push(c.clone());
                        }
                    }
                    for ch in &t.children {
                        if !x.children.iter().any(|y| y.task_id == ch.task_id) {
                            x.children.push(ch.clone());
                        }
                    }
                    if x.title.is_none() {
                        x.title = t.title.clone();
                    }
                    if x.summary.is_none() {
                        x.summary = t.summary.clone();
                    }
                }
                None => tasks.push(t.clone()),
            }
        }
        seg_task_count.insert(meta.sha12.clone(), count);
        for c in &n.direct_commits {
            if keep(Some(&c.sha)) && !direct.iter().any(|x| x.sha == c.sha) {
                direct.push(c.clone());
            }
        }
        for f in n.migrations.iter().filter(|f| keep(f.commit.as_ref())) {
            merge_file(&mut migrations, f);
        }
        for f in n.adrs.iter().filter(|f| keep(f.commit.as_ref())) {
            merge_file(&mut adrs, f);
        }
        if let Some(c) = n
            .config_example
            .as_ref()
            .filter(|c| keep(c.commit.as_ref()))
        {
            configs.push(c.clone());
        }
    }
    // 子として載った task は一覧の上から消す（同じ task は 1 回）。
    let child_ids: HashSet<String> = tasks
        .iter()
        .flat_map(|t| t.children.iter().map(|c| c.task_id.clone()))
        .collect();
    tasks.retain(|t| !child_ids.contains(&t.task_id));
    let task_ids: HashSet<&str> = tasks
        .iter()
        .map(|t| t.task_id.as_str())
        .chain(child_ids.iter().map(String::as_str))
        .collect();

    // 含まれるリリース: 辿った段と、その範囲に sha がある中間のリリース。
    let mut releases: Vec<&ReleaseMeta> = segments.iter().map(|(m, _)| *m).collect();
    for m in all {
        if releases.iter().any(|r| r.sha12 == m.sha12) {
            continue;
        }
        if current.is_some_and(|c| c.sha12 == m.sha12) {
            continue;
        }
        if included.iter().any(|s| same_sha(s, &m.sha)) {
            releases.push(m);
        }
    }
    releases.sort_by(|a, b| {
        (a.sha12 != target.sha12)
            .cmp(&(b.sha12 != target.sha12))
            .then_with(|| b.built_at.cmp(&a.built_at))
            .then_with(|| a.sha12.cmp(&b.sha12))
    });
    let releases = releases
        .into_iter()
        .map(|m| ReleasePromotionRelease {
            sha12: m.sha12.clone(),
            built_at: m.built_at.clone(),
            task_count: seg_task_count.get(&m.sha12).copied().unwrap_or_else(|| {
                m.notes.as_ref().map_or(0, |n| {
                    n.tasks
                        .iter()
                        .filter(|t| task_ids.contains(t.task_id.as_str()))
                        .count()
                })
            }),
        })
        .collect();

    ReleasePromotionPreview {
        from: current.map(|c| c.sha12.clone()),
        to: target.sha12.clone(),
        complete,
        problem,
        releases,
        tasks,
        direct_commits: direct,
        migrations,
        schema: schema(
            current.and_then(|c| c.schema_version),
            target.schema_version,
        ),
        mode: target
            .live_ok
            .map(|ok| if ok { "live" } else { "stop-start" }.to_string()),
        adrs,
        config_examples: configs,
        gate_skips: target
            .notes
            .as_ref()
            .map(|n| n.gate_skips.clone())
            .unwrap_or_default(),
    }
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// `<releases_dir>/<sha12>/` を 1 件読む（notes.json が壊れていれば `notes: None`）。
pub fn read_meta(dir: &Path, sha12: &str) -> ReleaseMeta {
    let manifest = read_json(&dir.join("manifest.json"));
    let verify = read_json(&dir.join("verify.json"));
    let notes = std::fs::read_to_string(dir.join("notes.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<ReleaseNotes>(&t).ok());
    let m = |k: &str| manifest.as_ref().and_then(|v| v.get(k)).cloned();
    ReleaseMeta {
        sha12: sha12.to_string(),
        sha: m("sha")
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| sha12.to_string()),
        built_at: m("built_at").and_then(|v| v.as_str().map(str::to_string)),
        schema_version: m("schema_version")
            .and_then(|v| v.as_u64())
            .and_then(|v| u32::try_from(v).ok()),
        live_ok: verify
            .as_ref()
            .and_then(|v| v.get("live_ok"))
            .and_then(serde_json::Value::as_bool),
        notes,
    }
}

/// `<releases_dir>` の全リリース（`.` で始まる名前と `*.partial` は飛ばす）。
pub fn read_all_meta(root: &Path) -> Vec<ReleaseMeta> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for e in entries.flatten() {
        let Ok(name) = e.file_name().into_string() else {
            continue;
        };
        if name.starts_with('.') || name.ends_with(".partial") || !e.path().is_dir() {
            continue;
        }
        out.push(read_meta(&e.path(), &name));
    }
    out.sort_by(|a, b| a.sha12.cmp(&b.sha12));
    out
}

/// `GET /releases/{sha12}/promotion-preview` と `celerisctl release preview`。`current` は
/// `<releases_dir>/../current` の指す先。対象が無ければ `None`。
pub fn preview_from_dir(root: &Path, target_sha12: &str) -> Option<ReleasePromotionPreview> {
    let all = read_all_meta(root);
    let current = root
        .parent()
        .and_then(|h| std::fs::read_link(h.join("current")).ok())
        .and_then(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string));
    preview_among(&all, current.as_deref(), target_sha12)
}

/// [`preview_from_dir`] の中身（読んだ一覧から）。
pub fn preview_among(
    all: &[ReleaseMeta],
    current: Option<&str>,
    target_sha12: &str,
) -> Option<ReleasePromotionPreview> {
    let target = all.iter().find(|m| m.sha12 == target_sha12)?;
    let cur = current.and_then(|c| all.iter().find(|m| m.sha12 == c));
    // current のディレクトリが掃除で消えていても sha12 で辿れるようにする。
    let fallback;
    let cur = match (cur, current) {
        (Some(c), _) => Some(c),
        (None, Some(c)) => {
            fallback = ReleaseMeta {
                sha12: c.to_string(),
                sha: c.to_string(),
                built_at: None,
                schema_version: None,
                live_ok: None,
                notes: None,
            };
            Some(&fallback)
        }
        (None, None) => None,
    };
    Some(aggregate(cur, target, all))
}

/// API から task の題と要約・配送記録を引く（`celerisctl release notes --api`）。外に出るのは
/// 渡された URL（本番 daemon の localhost）だけ。
pub struct HttpLookup {
    rt: tokio::runtime::Runtime,
    client: reqwest::Client,
    api: String,
    token: Option<String>,
}

impl HttpLookup {
    pub fn new(api: &str, token: Option<String>) -> Result<Self, String> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .no_proxy()
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            rt,
            client,
            api: api.trim_end_matches('/').to_string(),
            token,
        })
    }

    fn get(&self, path: &str) -> Option<serde_json::Value> {
        let url = format!("{}{path}", self.api);
        self.rt.block_on(async {
            let mut req = self.client.get(&url);
            if let Some(t) = &self.token {
                req = req.bearer_auth(t);
            }
            let resp = req.send().await.ok()?;
            if !resp.status().is_success() {
                return None;
            }
            resp.json::<serde_json::Value>().await.ok()
        })
    }

    /// `GET /api/v1/deliveries`。読めなければ `None`。
    pub fn deliveries(&self) -> Option<Vec<DeliveryHead>> {
        let v = self.get("/api/v1/deliveries")?;
        serde_json::from_value(v.get("items")?.clone()).ok()
    }
}

impl TaskLookup for HttpLookup {
    fn task(&self, task_id: &str) -> Option<TaskInfo> {
        if !is_ulid(task_id) {
            return None;
        }
        self.get(&format!("/api/v1/tasks/{task_id}"))
            .map(|v| task_info_from_detail(&v))
    }
}

#[cfg(test)]
#[path = "release_notes/tests.rs"]
mod tests;
