//! ADR-0047 追記（Phase K-1）: 知識の**置き場のガード**（決定的。I/O・git・LLM 無し）。
//!
//! `celerisctl knowledge record`・MCP の `knowledge_propose`・知識整理 run の
//! `apply_candidates` が**同じ 1 つの関数** [`place`] を通す。入力は「候補が言った置き場」
//! （`path` / `scope` / `title` / `tags`）と、呼び出し側が集めた [`Layout`]（案件の slug の一覧・
//! `environment/` の分類・既存のページの索引・今日の日付）だけ。
//!
//! 規則（`docs/knowledge.md` §2.1）:
//!
//! - `project:<x>` の `<x>` は**案件の slug**。案件 ID（ULID）が来たら slug に直す。どちらでも
//!   なければ拒否する（案件を知らない `celerisctl` では「ULID でない正しい綴りの slug」だけ通す）
//! - ページは `user/…`・`environment/<分類>/…`・`projects/<slug>/…`・`experience/YYYY/MM/…` に置く。
//!   `environment/` と `projects/` の**直下**には README 以外を置かない。`environment/` の分類は
//!   [`ENVIRONMENT_CATEGORIES`] と既にあるディレクトリ
//! - パスのどの段にも ULID を使わない
//! - `scope` のラベルは置き場と一致させる（無ければ置き場から決める。食い違えば拒否する）
//! - 同じ scope に同じ題名のページがあれば、新しいページを作らずそのページに入れる。`user/` の
//!   正準ページ（[`USER_CANONICAL`]）は題名・ファイル名・タグのどれかが当たればそこへ入れる

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{CandidateOp, IndexItem, PathError, page_path, slugify};

/// `environment/<分類>/` の既定の分類（D1 の `clusters` / `servers` / `tools` に、実際に使われて
/// きた `hosts`（手元の機械）と `celeris`（Celeris 自身の運用の癖）を足したもの）。既にある
/// ディレクトリも分類として通る（[`Layout::new`]）。
pub const ENVIRONMENT_CATEGORIES: [&str; 5] = ["celeris", "clusters", "hosts", "servers", "tools"];

/// 知識のページを置いてよい最上位のディレクトリ（`skills/`・`_inbox/`・`_retired/` は別の道具の場所）。
pub const TOP_DIRS: [&str; 4] = ["user", "environment", "projects", "experience"];

/// `user/` の正準ページ（`celerisctl knowledge init` の雛形。ファイル名の stem と題名）。
/// ここに当たる候補は**必ずこのページに入れる**（別のページを作らない）。
pub const USER_CANONICAL: [(&str, &str); 4] = [
    ("profile", "人のプロフィール"),
    ("expertise", "人の専門"),
    ("preferences", "人の好み"),
    ("goals", "人の目標"),
];

/// 案件 1 件（ID と、その知識の置き場 `projects/<slug>` の slug）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProjectRef {
    pub id: String,
    pub slug: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
}

/// [`place`] が見る置き場の状況（呼び出し側が集める）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layout {
    /// 案件の一覧。`None` は「案件を知らない」（DB を開かない `celerisctl knowledge record`）。
    pub projects: Option<Vec<ProjectRef>>,
    /// `environment/<分類>/` として通す分類（[`ENVIRONMENT_CATEGORIES`] ∪ 既にあるディレクトリ）。
    pub environment_categories: BTreeSet<String>,
    /// 既存のページ（`index.json` の中身）。同じ題名の検出と、タグからの分類の推定に使う。
    pub pages: Vec<IndexItem>,
    /// 今日（`YYYY-MM-DD`）。`experience/YYYY/MM/` を決めるのに使う。
    pub today: String,
}

impl Layout {
    /// `existing_environment_dirs` は `environment/` の下に既にあるディレクトリ名。
    pub fn new(
        projects: Option<Vec<ProjectRef>>,
        existing_environment_dirs: impl IntoIterator<Item = String>,
        pages: Vec<IndexItem>,
        today: impl Into<String>,
    ) -> Self {
        let mut environment_categories: BTreeSet<String> = ENVIRONMENT_CATEGORIES
            .iter()
            .map(|c| (*c).to_string())
            .collect();
        for dir in existing_environment_dirs {
            let dir = dir.trim().to_string();
            if !dir.is_empty() && !dir.starts_with('.') && !looks_like_ulid(&dir) {
                environment_categories.insert(dir);
            }
        }
        Self {
            projects,
            environment_categories,
            pages,
            today: today.into(),
        }
    }

    /// 知っている案件の slug（`None` は案件を知らない）。
    pub fn project_slugs(&self) -> Option<Vec<String>> {
        self.projects
            .as_ref()
            .map(|ps| ps.iter().map(|p| p.slug.clone()).collect())
    }
}

/// 26 文字の Crockford base32（ULID）に見えるか（大文字小文字は問わない）。
pub fn looks_like_ulid(raw: &str) -> bool {
    let raw = raw.trim();
    raw.len() == 26
        && raw.chars().next().is_some_and(|c| ('0'..='7').contains(&c))
        && raw.chars().all(|c| {
            let c = c.to_ascii_uppercase();
            c.is_ascii_digit() || (c.is_ascii_uppercase() && !matches!(c, 'I' | 'L' | 'O' | 'U'))
        })
}

/// 案件の slug の綴り（`[a-z0-9]` で始まり終わる `[a-z0-9-]`、64 文字まで、ULID に見えない）。
pub fn is_valid_project_slug(raw: &str) -> bool {
    !raw.is_empty()
        && raw.len() <= 64
        && raw
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !raw.starts_with('-')
        && !raw.ends_with('-')
        && !raw.contains("--")
        && !looks_like_ulid(raw)
}

/// 案件の slug の既定（ADR-0044 D7 追記）: 題名の slug → primary リポジトリの名前の slug →
/// `project-<id の末尾 8 文字>`。`taken` が真を返す（他の案件が使っている）slug は避け、
/// `<slug>-<id の末尾 8 文字>`、さらに `-2`、`-3` … と足す。
pub fn derive_project_slug(
    title: &str,
    project_id: &str,
    primary_repo: Option<&str>,
    taken: &dyn Fn(&str) -> bool,
) -> String {
    let tail: String = project_id
        .trim()
        .chars()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>()
        .to_ascii_lowercase();
    let usable = |s: Option<String>| s.filter(|s| is_valid_project_slug(s));
    let base = usable(slugify(title))
        .or_else(|| usable(primary_repo.and_then(slugify)))
        .unwrap_or_else(|| format!("project-{tail}"));
    if !taken(&base) {
        return base;
    }
    let with_tail = format!("{base}-{tail}");
    if is_valid_project_slug(&with_tail) && !taken(&with_tail) {
        return with_tail;
    }
    let mut n = 2u32;
    loop {
        let next = format!("{with_tail}-{n}");
        if !taken(&next) {
            return next;
        }
        n += 1;
    }
}

/// 置き場の scope（ラベルに直す前の形）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Scope {
    User,
    Environment(Option<String>),
    Experience,
    Project(String),
    /// `projects/README.md`（置き場の説明。scope を持たない）。
    ProjectsReadme,
}

impl Scope {
    fn label(&self) -> Option<String> {
        match self {
            Scope::User => Some("user".into()),
            Scope::Environment(_) => Some("environment".into()),
            Scope::Experience => Some("experience".into()),
            Scope::Project(slug) => Some(format!("project:{slug}")),
            Scope::ProjectsReadme => None,
        }
    }

    fn same_place(&self, other: &Scope) -> bool {
        match (self, other) {
            (Scope::Environment(a), Scope::Environment(b)) => match (a, b) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            },
            (a, b) => a == b,
        }
    }
}

/// [`place`] の入力。
#[derive(Debug, Clone, Copy, Default)]
pub struct PlacementRequest<'a> {
    /// 知識整理 run の候補の `op`。`record` / `propose` は `None`。
    pub op: Option<CandidateOp>,
    pub path: Option<&'a str>,
    pub scope: Option<&'a str>,
    pub title: &'a str,
    pub tags: &'a [String],
}

/// 候補の置き場を別のページに向け直した理由。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Redirect {
    /// 同じ scope に同じ題名のページがあった（`from` は元の置き場）。
    SameTitle { from: String },
    /// `user/` の正準ページに当たった。
    UserCanonical { from: String },
}

/// [`place`] の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// KB 相対のパス（正規化済み。案件 ID は slug に直してある）。
    pub path: String,
    /// front matter に書く `scope` のラベル（`projects/README.md` だけ `None`）。
    pub scope: Option<String>,
    pub redirect: Option<Redirect>,
}

/// 置き場のガードに落ちた理由。**候補を書いた側（ChatGPT・ワーカー・知識整理 run）がそのまま
/// 直せる**ように、何が正しいかを文面に入れる。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlacementError {
    #[error("path: {0}")]
    Path(#[from] PathError),
    #[error(
        "scope {0:?} is not valid: use `user`, `environment`, `experience` or `project:<slug>`"
    )]
    UnknownScope(String),
    #[error("{}", unknown_project_message(.value, .known.as_deref()))]
    UnknownProject {
        value: String,
        known: Option<Vec<String>>,
    },
    #[error(
        "{0:?}: a path segment looks like a ULID (task/project id). Use the project slug (`projects/<slug>/…`) and a descriptive file name"
    )]
    UlidSegment(String),
    #[error("{0:?}: `_inbox/`, `_retired/` and `skills/` are not places for knowledge pages")]
    Reserved(String),
    #[error(
        "{0:?}: knowledge pages live under `user/`, `environment/<category>/`, `projects/<slug>/` or `experience/YYYY/MM/`"
    )]
    UnknownTop(String),
    #[error(
        "{path:?}: no pages directly under `{dir}/` (only README.md). Use `{dir}/{hint}/<name>.md`"
    )]
    ScopeRoot {
        path: String,
        dir: &'static str,
        hint: String,
    },
    #[error("{path:?}: unknown environment category {category:?}. Use one of: {}", .known.join(", "))]
    UnknownEnvironmentCategory {
        path: String,
        category: String,
        known: Vec<String>,
    },
    #[error(
        "scope `environment` needs a category: pass `path = environment/<category>/<name>.md` (categories: {})",
        .known.join(", ")
    )]
    NeedEnvironmentCategory { known: Vec<String> },
    #[error("{0:?}: experience pages live at `experience/YYYY/MM/<name>.md`")]
    ExperienceLayout(String),
    #[error("scope {scope:?} does not match the place {path:?} (whose scope is {expected:?})")]
    ScopeMismatch {
        scope: String,
        path: String,
        expected: String,
    },
    #[error("scope is required when no path is given")]
    NoScope,
    #[error(
        "cannot make a file name from the title or tags (non-ASCII only): pass `path` with a descriptive ASCII file name"
    )]
    NoName,
}

fn unknown_project_message(value: &str, known: Option<&[String]>) -> String {
    match known {
        Some(known) => format!(
            "project {value:?} is neither a known project slug nor a project id. Use `project:<slug>` with one of: {}",
            if known.is_empty() {
                "(no projects)".to_string()
            } else {
                known.join(", ")
            }
        ),
        None => format!(
            "project {value:?} is not a project slug (lowercase `[a-z0-9-]`, not an id). Use `project:<slug>` — the slug is the `projects/<slug>` directory this task's knowledge is mounted from"
        ),
    }
}

/// `project:<x>` の `<x>` を slug に直す。
pub fn resolve_project(value: &str, layout: &Layout) -> Result<String, PlacementError> {
    let value = value.trim();
    match &layout.projects {
        Some(projects) => projects
            .iter()
            .find(|p| p.slug == value)
            .or_else(|| projects.iter().find(|p| p.id.eq_ignore_ascii_case(value)))
            .map(|p| p.slug.clone())
            .ok_or_else(|| PlacementError::UnknownProject {
                value: value.to_string(),
                known: Some(projects.iter().map(|p| p.slug.clone()).collect()),
            }),
        None if is_valid_project_slug(value) => Ok(value.to_string()),
        None => Err(PlacementError::UnknownProject {
            value: value.to_string(),
            known: None,
        }),
    }
}

/// `scope` の値を正規化する（`project:<ULID>` は `project:<slug>` に。`projects/<x>`・
/// `environment/<分類>` のようなパスの形も受ける）。ラベルを返す。
pub fn resolve_scope_label(raw: &str, layout: &Layout) -> Result<String, PlacementError> {
    let scope = parse_scope(raw, layout)?;
    Ok(scope.label().unwrap_or_default())
}

fn parse_scope(raw: &str, layout: &Layout) -> Result<Scope, PlacementError> {
    let s = raw.trim().trim_matches('/');
    if let Some(x) = s.strip_prefix("project:") {
        return resolve_project(x, layout).map(Scope::Project);
    }
    let parts: Vec<&str> = s.split('/').filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        ["user", ..] => Ok(Scope::User),
        ["experience", ..] => Ok(Scope::Experience),
        ["environment"] => Ok(Scope::Environment(None)),
        ["environment", category, ..] => {
            let category = category.to_string();
            if layout.environment_categories.contains(&category) {
                Ok(Scope::Environment(Some(category)))
            } else {
                Err(PlacementError::UnknownEnvironmentCategory {
                    path: s.to_string(),
                    category,
                    known: layout.environment_categories.iter().cloned().collect(),
                })
            }
        }
        ["projects", slug, ..] => resolve_project(slug, layout).map(Scope::Project),
        _ => Err(PlacementError::UnknownScope(raw.trim().to_string())),
    }
}

/// パスの形を検査し、そのパスの scope と（案件 ID を slug に直した）正規化後のパスを返す。
fn scope_of_path(path: &str, layout: &Layout) -> Result<(Scope, String), PlacementError> {
    let mut parts: Vec<String> = path.split('/').map(str::to_string).collect();
    let top = parts.first().cloned().unwrap_or_default();
    let is_readme = |p: &[String]| {
        p.last()
            .is_some_and(|l| l.eq_ignore_ascii_case("README.md"))
    };
    let scope = match top.as_str() {
        "_inbox" | "_retired" | "skills" => return Err(PlacementError::Reserved(path.to_string())),
        _ if parts.len() == 1 => return Err(PlacementError::UnknownTop(path.to_string())),
        "user" => Scope::User,
        "environment" => {
            if parts.len() == 2 {
                if !is_readme(&parts) {
                    return Err(PlacementError::ScopeRoot {
                        path: path.to_string(),
                        dir: "environment",
                        hint: format!(
                            "{{{}}}",
                            layout
                                .environment_categories
                                .iter()
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(",")
                        ),
                    });
                }
                Scope::Environment(None)
            } else {
                let category = parts[1].clone();
                if !layout.environment_categories.contains(&category) {
                    return Err(PlacementError::UnknownEnvironmentCategory {
                        path: path.to_string(),
                        category,
                        known: layout.environment_categories.iter().cloned().collect(),
                    });
                }
                Scope::Environment(Some(category))
            }
        }
        "experience" => {
            let dated = parts.len() == 4
                && parts[1].len() == 4
                && parts[1].chars().all(|c| c.is_ascii_digit())
                && parts[2].len() == 2
                && parts[2].chars().all(|c| c.is_ascii_digit());
            if !(dated || (parts.len() == 2 && is_readme(&parts))) {
                return Err(PlacementError::ExperienceLayout(path.to_string()));
            }
            Scope::Experience
        }
        "projects" => {
            if parts.len() == 2 {
                if !is_readme(&parts) {
                    return Err(PlacementError::ScopeRoot {
                        path: path.to_string(),
                        dir: "projects",
                        hint: "<slug>".to_string(),
                    });
                }
                Scope::ProjectsReadme
            } else {
                let slug = resolve_project(&parts[1], layout)?;
                parts[1] = slug.clone();
                Scope::Project(slug)
            }
        }
        _ => return Err(PlacementError::UnknownTop(path.to_string())),
    };
    let path = parts.join("/");
    if parts.iter().any(|p| {
        let stem = p.strip_suffix(".md").unwrap_or(p);
        looks_like_ulid(stem) || stem.split('-').any(looks_like_ulid)
    }) {
        return Err(PlacementError::UlidSegment(path));
    }
    Ok((scope, path))
}

/// 既存のページの scope（置き場から。置き場が規則に合わない古いページはラベルから）。
fn scope_of_existing(item: &IndexItem, layout: &Layout) -> Option<Scope> {
    match scope_of_path(&item.path, layout) {
        Ok((scope, _)) => Some(scope),
        Err(_) => item
            .scope
            .as_deref()
            .and_then(|s| parse_scope(s, layout).ok()),
    }
}

fn same_title(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// 題名（無ければ最初の使えるタグ）からファイル名の stem を作る。
fn file_stem(title: &str, tags: &[String], avoid: &[&str]) -> Option<String> {
    slugify(title).or_else(|| {
        tags.iter()
            .filter_map(|t| slugify(t))
            .find(|t| !avoid.contains(&t.as_str()))
    })
}

/// `environment` のタグから分類を当てる（分類名そのもの・単数形・既存ページの stem）。
fn category_from_tags(tags: &[String], layout: &Layout) -> Option<String> {
    for tag in tags {
        let tag = tag.trim().to_ascii_lowercase();
        if tag.is_empty() {
            continue;
        }
        for category in &layout.environment_categories {
            if *category == tag || *category == format!("{tag}s") {
                return Some(category.clone());
            }
        }
    }
    for tag in tags {
        let tag = tag.trim().to_ascii_lowercase();
        for item in &layout.pages {
            let parts: Vec<&str> = item.path.split('/').collect();
            if let ["environment", category, file] = parts.as_slice()
                && file.strip_suffix(".md") == Some(tag.as_str())
                && layout.environment_categories.contains(*category)
            {
                return Some((*category).to_string());
            }
        }
    }
    None
}

fn user_canonical(path: &str, title: &str, tags: &[String]) -> Option<&'static str> {
    let stem = path
        .strip_prefix("user/")
        .and_then(|p| p.strip_suffix(".md"))
        .unwrap_or_default();
    if let Some((c, _)) = USER_CANONICAL.iter().find(|(c, _)| *c == stem) {
        return Some(c);
    }
    if let Some((c, _)) = USER_CANONICAL.iter().find(|(_, t)| same_title(t, title)) {
        return Some(c);
    }
    let hits: BTreeSet<&'static str> = tags
        .iter()
        .filter_map(|t| {
            let t = t.trim().to_ascii_lowercase();
            USER_CANONICAL
                .iter()
                .find(|(c, _)| *c == t || format!("{c}s") == t || *c == format!("{t}s"))
                .map(|(c, _)| *c)
        })
        .collect();
    if hits.len() == 1 {
        hits.into_iter().next()
    } else {
        None
    }
}

/// 候補の置き場を決める（ガード）。規則はモジュールの説明のとおり。
pub fn place(req: &PlacementRequest<'_>, layout: &Layout) -> Result<Placement, PlacementError> {
    let asked_scope = req
        .scope
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| parse_scope(s, layout))
        .transpose()?;
    let asked_path = req.path.map(str::trim).filter(|p| !p.is_empty());
    // `retire` は既存のページを退役させるだけなので、置き場の規則に合わない古いページも対象にできる。
    if req.op == Some(CandidateOp::Retire)
        && let Some(raw) = asked_path
    {
        let path = page_path(raw)?;
        let scope = scope_of_path(&path, layout).ok().map(|(s, _)| s);
        return Ok(Placement {
            scope: scope
                .or(asked_scope)
                .and_then(|s| s.label())
                .or_else(|| req.scope.map(|s| s.trim().to_string())),
            path,
            redirect: None,
        });
    }
    let (scope, mut path) = match asked_path {
        Some(raw) => {
            let cleaned = page_path(raw)?;
            let (path_scope, path) = scope_of_path(&cleaned, layout)?;
            if let Some(asked) = &asked_scope
                && !asked.same_place(&path_scope)
            {
                return Err(PlacementError::ScopeMismatch {
                    scope: req.scope.unwrap_or_default().trim().to_string(),
                    path,
                    expected: path_scope.label().unwrap_or_else(|| "(none)".into()),
                });
            }
            (path_scope, path)
        }
        None => {
            let scope = asked_scope.ok_or(PlacementError::NoScope)?;
            let (dir, avoid): (String, Vec<&str>) = match &scope {
                Scope::User => ("user".into(), vec!["user"]),
                Scope::Experience => {
                    let (y, m) = (
                        layout.today.get(0..4).unwrap_or("0000"),
                        layout.today.get(5..7).unwrap_or("00"),
                    );
                    (format!("experience/{y}/{m}"), vec!["experience"])
                }
                Scope::Project(slug) => (format!("projects/{slug}"), vec!["project"]),
                Scope::Environment(Some(c)) => (format!("environment/{c}"), vec!["environment"]),
                Scope::Environment(None) => match category_from_tags(req.tags, layout) {
                    Some(c) => (format!("environment/{c}"), vec!["environment"]),
                    None => {
                        return Err(PlacementError::NeedEnvironmentCategory {
                            known: layout.environment_categories.iter().cloned().collect(),
                        });
                    }
                },
                Scope::ProjectsReadme => ("projects".into(), vec![]),
            };
            let stem = if scope == Scope::User {
                user_canonical("", req.title, req.tags)
                    .map(str::to_string)
                    .or_else(|| file_stem(req.title, req.tags, &avoid))
            } else {
                file_stem(req.title, req.tags, &avoid)
            }
            .ok_or(PlacementError::NoName)?;
            let path = format!("{dir}/{stem}.md");
            // 組み立てたパスも同じ検査に通す（ULID の段など）。
            let (_, path) = scope_of_path(&path, layout)?;
            (scope, path)
        }
    };
    let mut redirect = None;
    let creates = matches!(req.op, None | Some(CandidateOp::Create));
    if creates && scope == Scope::User {
        let from = path.clone();
        if let Some(c) = user_canonical(&path, req.title, req.tags) {
            let canonical = format!("user/{c}.md");
            if canonical != path {
                path = canonical;
                redirect = Some(Redirect::UserCanonical { from });
            }
        }
    }
    if creates
        && redirect.is_none()
        && let Some(existing) = layout.pages.iter().find(|item| {
            item.path != path
                && same_title(&item.title, req.title)
                && scope_of_existing(item, layout).is_some_and(|s| s.same_place(&scope))
        })
    {
        redirect = Some(Redirect::SameTitle { from: path.clone() });
        path = existing.path.clone();
    }
    Ok(Placement {
        scope: scope.label(),
        path,
        redirect,
    })
}

/// MCP の `knowledge_propose` の説明に足す「今の置き場」（決定的な文面）。
pub fn layout_hint(layout: &Layout) -> String {
    let categories = layout
        .environment_categories
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let projects = match &layout.projects {
        Some(ps) if !ps.is_empty() => ps
            .iter()
            .map(|p| {
                if p.title.is_empty() {
                    format!("`project:{}` (id {})", p.slug, p.id)
                } else {
                    format!("`project:{}` = {} (id {})", p.slug, p.title, p.id)
                }
            })
            .collect::<Vec<_>>()
            .join("; "),
        Some(_) => "(no projects yet)".to_string(),
        None => "(unknown)".to_string(),
    };
    format!(
        "Layout: `user/<name>.md` (canonical: profile / expertise / preferences / goals — facts about the person go INTO these), \
         `environment/<category>/<name>.md` (categories: {categories}; nothing directly under environment/), \
         `projects/<slug>/<name>.md` (scope `project:<slug>`; never a project id), \
         `experience/YYYY/MM/<name>.md`. Known projects: {projects}. \
         If a page with the same title already exists in the scope, the proposal is merged into that page."
    )
}

#[cfg(test)]
#[path = "layout/tests.rs"]
mod tests;
