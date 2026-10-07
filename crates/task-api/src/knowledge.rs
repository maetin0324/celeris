//! 知識ベース（ADR-0047 D3 / D5。Phase 61）。**正本は `[knowledge] root` の Markdown**:
//!
//! - `GET  /knowledge/tree?scope=&q=` — ツリー（`q` は索引 ＋ `git grep -il`）。読み取り
//! - `GET  /knowledge/page?path=` — 1 ページ（raw / html / front matter / 履歴 / etag）。読み取り
//! - `PUT  /knowledge/page` — **管理系**。人の編集を 1 件 1 コミット（`etag` 必須）
//! - `GET  /knowledge/inbox` — `_inbox/` の候補。読み取り
//! - `POST /knowledge/inbox` — **管理系**。候補を 1 件作る（添付の provenance つき。CoS は
//!   `/cos/operations` の `knowledge.record`。ADR 2026-10-07 cos-live-fixes D2）
//! - `GET  /knowledge/inbox/{id}` — 候補 1 件（pin されたチャット添付の provenance つき）。読み取り
//! - `POST /knowledge/inbox/{id}/accept` / `reject` — **管理系**。取り込み・破棄
//!
//! 境界（ADR-0044 D7 / ADR-0047 D1 と同じ規則）:
//!
//! - パスは**KB の根からの相対**で、根の外に出られない（`..`・絶対パスは 403）。`.md` 以外は 422
//! - 人の編集は `etag`（中身の sha256）で衝突を見る。違えば 409 `etag_mismatch`
//! - 書き込みのあとは必ず索引（`index.json`）を作り直す
//! - 読み取りは**何も作らない**（KB が無ければ `initialized: false` を返すだけ）
//!
//! ADR-0013 は「API はコマンドを実行しない」と決めているが、ここも `task_ops::knowledge` を通して
//! **`git` だけ**を上限付きで起こす（`crate::docs` と同じ逸脱。ワーカーも LLM も起こさない）。

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::knowledge::{self as kb, Confidence, PathError};
use task_ops::docs::{self as ops_docs, DocCommit};
use task_ops::knowledge::{self as ops_kb, InboxOutcome, PageEdit, WriteOutcome};

use crate::cos::operations::{Applied, OperationAudit};
use crate::handlers::{ApiResult, Params, json_response, no_query, read_json};
use crate::middleware::require_admin;
use crate::problem::ApiProblem;
use crate::query::QueryParams;
use crate::state::ApiState;
use crate::types::ValidationError;

/// ツリーに出すページの上限（`crate::docs::MAX_TREE_PAGES` と同じ）。
pub const MAX_TREE_PAGES: usize = 500;

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
        .route("/api/v1/knowledge/tree", get(tree))
        .route("/api/v1/knowledge/page", get(page).put(put_page))
        .route("/api/v1/knowledge/inbox", get(inbox).post(record))
        .route("/api/v1/knowledge/inbox/{id}", get(inbox_detail))
        .route("/api/v1/knowledge/inbox/{id}/accept", post(accept))
        .route("/api/v1/knowledge/inbox/{id}/reject", post(reject))
}

// ---------------------------------------------------------------------------
// 応答の型（`docs/api/v1/gui-api.md` §3.98〜3.102）
// ---------------------------------------------------------------------------

/// ツリーの 1 件（`GET /knowledge/tree`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KnowledgeItem {
    /// KB の根からの相対パス（`environment/clusters/pegasus.md`）。
    pub path: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// front matter の `scope`（無ければ置き場から決めた既定）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    /// RFC 3339 か `YYYY-MM-DD`（front matter の `updated` → 最後のコミット）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<Confidence>,
}

impl From<kb::IndexItem> for KnowledgeItem {
    fn from(item: kb::IndexItem) -> Self {
        Self {
            path: item.path,
            title: item.title,
            tags: item.tags,
            scope: item.scope,
            sources: item.sources,
            updated: item.updated,
            confidence: item.confidence,
        }
    }
}

/// `GET /knowledge/tree`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KnowledgeTree {
    /// KB の根（絶対パス）。
    pub root: String,
    /// `celerisctl knowledge init` が済んでいるか。偽なら `items` は空。
    pub initialized: bool,
    /// `?scope=` で絞ったならその文字列。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// `?q=` で絞ったならその文字列。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// KB にある scope の一覧（画面のツリーの見出し。`user` / `environment/clusters` / `projects/<slug>` …）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
    pub items: Vec<KnowledgeItem>,
    /// `_inbox/` にある候補の数（画面のバッジ）。
    pub inbox_count: usize,
    /// [`MAX_TREE_PAGES`] で切った。
    pub truncated: bool,
    /// `index.json` を作った時刻（RFC 3339）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generated_at: Option<String>,
}

/// `GET /knowledge/page`。`crate::docs::DocPage` と同じ形（描画・履歴・etag）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KnowledgePage {
    pub root: String,
    pub path: String,
    pub title: String,
    /// Markdown のもと（front matter を含む）。`too_large` なら空。
    pub raw: String,
    /// サーバで描画した HTML（生 HTML は捨ててある）。
    pub html: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<Confidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
    /// 直近 20 件（新しい順）。
    pub history: Vec<DocCommit>,
    /// いまの中身の sha256。`PUT` にそのまま渡す。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    pub too_large: bool,
}

/// `PUT /knowledge/page` の本文。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KnowledgePagePutBody {
    pub path: String,
    pub body: String,
    /// 既にあるページを直すときは必須（無ければ 409 `etag_mismatch`）。
    #[serde(default)]
    pub etag: Option<String>,
    /// コミットメッセージ（既定 `knowledge: <path>`）。
    #[serde(default)]
    pub message: Option<String>,
}

/// `PUT /knowledge/page` と `inbox/{id}/accept` の応答。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KnowledgePageResult {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    /// 新しいコミットの sha。
    pub sha: String,
    /// 中身が同じだったので新しいコミットは作らなかった。
    pub unchanged: bool,
}

/// `_inbox/` の候補 1 件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KnowledgeCandidate {
    pub id: String,
    /// `_inbox/<id>.md`。
    pub path: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// `task:<id>` / `message:<id>` / `human` / `url:<…>`（GUI は出典へのリンクにする）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<Confidence>,
    /// 取り込む先（front matter の `path`、無ければ `scope` と題名からの既定）。
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    /// 本文（front matter を除く）。
    pub body: String,
    /// 本文を描画した HTML（生 HTML は捨ててある）。
    pub html: String,
    /// 取り込み先に既にページがある（`op` が無ければ accept は `overwrite` が要る）。
    pub target_exists: bool,
    /// ADR-0047 D4（Phase 62）: `create` / `update` / `merge` / `retire`、Phase K-1 の `append`。
    /// 取り込み先がまだ無い `record` の候補には無い（`null`）。`retire` の accept は `target` を
    /// `_retired/` へ動かし、`merge` の accept は `target` を必ず上書きし、`append` の accept は
    /// `target` の末尾に節として足す（`docs/guides/knowledge.md` 参照）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    /// ADR 2026-10-05 cos-chat-home D4: この候補に pin されたチャット添付（`chat_attachment_refs`
    /// owner_kind=`knowledge_inbox`）の原ファイルの出どころ。pin が古い順。添付の保存が無効なら空。
    #[serde(default)]
    pub provenance: Vec<KnowledgeCandidateProvenance>,
}

/// 候補に pin された原ファイル 1 件の provenance（SQLite の添付・参照・メッセージの表から読む。
/// チャット run の一時 file やチャットの作業場所を消しても残る）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KnowledgeCandidateProvenance {
    pub attachment_id: String,
    /// 原ファイルの名前（upload 時の名前）。
    pub name: String,
    pub media_type: String,
    pub size_bytes: u64,
    /// 原ファイルの中身の sha256（16 進）。
    pub sha256: String,
    /// 添付を upload したチャットのスレッド。
    pub thread_id: String,
    /// 添付を送ったメッセージ（最初のもの）。メッセージに載せずに pin したなら無い。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    /// そのメッセージの本文（人の依頼本文）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_text: Option<String>,
    /// pin した時刻（RFC 3339）。
    pub pinned_at: String,
}

impl From<task_core::chat::attachments::AttachmentProvenance> for KnowledgeCandidateProvenance {
    fn from(p: task_core::chat::attachments::AttachmentProvenance) -> Self {
        Self {
            attachment_id: p.attachment_id,
            name: p.name,
            media_type: p.media_type,
            size_bytes: p.size_bytes,
            sha256: p.sha256,
            thread_id: p.thread_id,
            message_id: p.message_id,
            request_text: p.request_text,
            pinned_at: p.pinned_at,
        }
    }
}

/// `GET /knowledge/inbox`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KnowledgeInbox {
    pub root: String,
    pub initialized: bool,
    pub items: Vec<KnowledgeCandidate>,
}

/// `POST /knowledge/inbox/{id}/accept` の本文（省略してよい）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeAcceptBody {
    /// 取り込む先（省略なら候補の front matter の `path`）。
    #[serde(default)]
    pub path: Option<String>,
    /// 宛先が既にあっても上書きする。
    #[serde(default)]
    pub overwrite: bool,
}

/// `POST /knowledge/inbox` の本文（ADR 2026-10-07 cos-live-fixes D2）。`celerisctl knowledge record`
/// と同じ検査・同じ候補ファイルの形（`task_ops::knowledge::record_prepare`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeRecordBody {
    pub title: String,
    /// `user` / `environment` / `environment/<分類>` / `experience` / `project:<slug>`。互換のため
    /// `projects/<slug>` も受けて `project:<slug>` に正規化する。
    pub scope: String,
    /// 本文（Markdown）。
    pub body: String,
    /// **1 件以上必須**（`message:<id>` / `task:<id>` / `human:instruction` / `url:<…>`）。
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub confidence: Option<Confidence>,
    /// 取り込む先の KB 相対パス（省略なら accept のときに scope と題名から決まる）。
    #[serde(default)]
    pub path: Option<String>,
    /// 候補に pin するチャット添付（`owner_kind: knowledge_inbox`）。CoS からは自分の thread の添付だけ。
    #[serde(default)]
    pub attachment_ids: Vec<String>,
}

/// `POST /knowledge/inbox` の応答（201）。CoS operation `knowledge.record` の `result` も同じ形。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KnowledgeRecordResult {
    /// 候補 id（`GET /knowledge/inbox/{id}` の id）。
    pub id: String,
    /// `_inbox/<id>.md`。
    pub path: String,
    /// 取り込み先（置き場のガードを通した KB 相対パス）。
    pub target: String,
    /// 正規化した scope（`project:<slug>` …）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// 取り込み先が既にあれば `append` / `merge`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    /// pin した添付の id（本文の順）。
    pub attachment_ids: Vec<String>,
    /// 候補の git commit の sha。
    pub sha: String,
}

/// `POST /knowledge/inbox/{id}/reject` の応答。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KnowledgeRejectResult {
    pub id: String,
    pub sha: String,
}

// ---------------------------------------------------------------------------
// 共通
// ---------------------------------------------------------------------------

fn knowledge_unavailable(detail: impl Into<String>) -> ApiProblem {
    ApiProblem::new(StatusCode::CONFLICT, "knowledge_unavailable", detail)
}

fn page_not_found(path: &str) -> ApiProblem {
    ApiProblem::new(
        StatusCode::NOT_FOUND,
        "page_not_found",
        format!("knowledge page not found: {path}"),
    )
}

fn candidate_not_found(id: &str) -> ApiProblem {
    ApiProblem::new(
        StatusCode::NOT_FOUND,
        "candidate_not_found",
        format!("knowledge candidate not found: {id}"),
    )
}

fn etag_mismatch(etag: Option<String>) -> ApiProblem {
    let problem = ApiProblem::new(
        StatusCode::CONFLICT,
        "etag_mismatch",
        "the page changed since it was read (reload and edit again)",
    );
    match etag {
        Some(etag) => problem.with_extra("etag", etag),
        None => problem,
    }
}

/// パスの検査（`..`・絶対パスは 403、`.md` 以外は 422）。
fn page_path(raw: &str) -> Result<String, ApiProblem> {
    kb::page_path(raw).map_err(path_problem)
}

fn path_problem(e: PathError) -> ApiProblem {
    match e {
        PathError::Forbidden => ApiProblem::path_forbidden(e.to_string()),
        PathError::NotMarkdown => ApiProblem::validation(vec![ValidationError {
            field: Some("path".into()),
            message: e.to_string(),
        }]),
        PathError::Empty => ApiProblem::bad_request("path is required"),
    }
}

/// KB の根（設定していなければ 409）。Phase 82: `crate::skills` も同じ KB を見るので `pub(crate)`。
pub(crate) fn root_of(state: &ApiState) -> Result<PathBuf, ApiProblem> {
    state.inner.knowledge_root.clone().ok_or_else(|| {
        knowledge_unavailable(
            "`[knowledge] root` が設定されていません（config.toml に `[knowledge]` を足す）",
        )
    })
}

/// 書き込み系は KB が用意されていることを要求する。Phase 82: `crate::skills` も使う。
pub(crate) fn require_kb(root: &std::path::Path) -> Result<(), ApiProblem> {
    if ops_kb::exists(root) {
        Ok(())
    } else {
        Err(knowledge_unavailable(format!(
            "{} に知識ベースがありません（`celerisctl knowledge init` で用意する）",
            root.display()
        )))
    }
}

/// ページの中の `[[…]]` のリンク先（GUI の「知識」画面）。
const LINK_BASE: &str = "/knowledge";

fn render_markdown(body: &str, from: &str) -> String {
    ops_docs::render(body, "", from, LINK_BASE)
}

// ---------------------------------------------------------------------------
// GET /knowledge/tree
// ---------------------------------------------------------------------------

async fn tree(
    axum::extract::State(state): axum::extract::State<ApiState>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> ApiResult {
    let query = QueryParams::parse(raw.as_deref(), &["scope", "q", "limit"])?;
    let scope = query
        .single("scope")?
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let q = query
        .single("q")?
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let root = root_of(&state)?;
    let view = state
        .blocking(move |_| {
            let initialized = ops_kb::exists(&root);
            if !initialized {
                return Ok(KnowledgeTree {
                    root: root.display().to_string(),
                    initialized,
                    scope,
                    q,
                    scopes: Vec::new(),
                    items: Vec::new(),
                    inbox_count: 0,
                    truncated: false,
                    generated_at: None,
                });
            }
            let index = ops_kb::ensure_index(&root);
            let mut items: Vec<KnowledgeItem> = match &q {
                Some(q) => ops_kb::search(&root, q, scope.as_deref(), 0)
                    .into_iter()
                    .map(|h| h.item.into())
                    .collect(),
                None => index
                    .items
                    .iter()
                    .filter(|i| scope.as_deref().is_none_or(|s| kb::in_scope(i, s)))
                    .cloned()
                    .map(KnowledgeItem::from)
                    .collect(),
            };
            if q.is_none() {
                items.sort_by(|a, b| a.path.cmp(&b.path));
            }
            let truncated = items.len() > MAX_TREE_PAGES;
            items.truncate(MAX_TREE_PAGES);
            // 画面のツリーの見出し（ページの置き場のディレクトリ。名前順で重複無し）。
            let mut scopes: Vec<String> = index
                .items
                .iter()
                .filter_map(|i| i.path.rsplit_once('/').map(|(dir, _)| dir.to_string()))
                .collect();
            scopes.sort();
            scopes.dedup();
            Ok(KnowledgeTree {
                root: root.display().to_string(),
                initialized,
                scope,
                q,
                scopes,
                items,
                inbox_count: ops_kb::inbox_list(&root).len(),
                truncated,
                generated_at: Some(index.generated_at),
            })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &view))
}

// ---------------------------------------------------------------------------
// GET /knowledge/page
// ---------------------------------------------------------------------------

async fn page(
    axum::extract::State(state): axum::extract::State<ApiState>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> ApiResult {
    let query = QueryParams::parse(raw.as_deref(), &["path"])?;
    let requested = query.single("path")?.unwrap_or_default().to_string();
    let root = root_of(&state)?;
    let view = state
        .blocking(move |_| {
            let path = page_path(&requested)?;
            let raw = ops_kb::read_page(&root, &path).ok_or_else(|| page_not_found(&path))?;
            let too_large = raw.len() > kb::MAX_PAGE_BYTES;
            let (front, body) = kb::front_matter(&raw);
            let html = if too_large {
                String::new()
            } else {
                render_markdown(body, &path)
            };
            Ok(KnowledgePage {
                root: root.display().to_string(),
                title: kb::title_of(&raw, &path),
                tags: front.tags,
                scope: front.scope,
                sources: front.sources,
                confidence: front.confidence,
                updated: front.updated,
                history: ops_kb::history(&root, &path),
                etag: ops_kb::etag(&root, &path),
                raw: if too_large { String::new() } else { raw },
                html,
                path,
                too_large,
            })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &view))
}

// ---------------------------------------------------------------------------
// PUT /knowledge/page（管理系）
// ---------------------------------------------------------------------------

async fn put_page(
    axum::extract::State(state): axum::extract::State<ApiState>,
    headers: HeaderMap,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let request: KnowledgePagePutBody = read_json(body, false).await?;
    let root = root_of(&state)?;
    let result = state
        .blocking(move |_| {
            require_kb(&root)?;
            let path = page_path(&request.path)?;
            if kb::is_inbox(&path) {
                return Err(ApiProblem::path_forbidden(
                    "`_inbox/` の候補は編集できません（accept か reject を使う）",
                ));
            }
            let message = request
                .message
                .as_deref()
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("knowledge: {path}"));
            // ADR-0047 付記 H1: 人の編集は『人が書いた』印を付けて保存する（整理が保護する）。
            let body = kb::mark_human_authored(&path, &request.body);
            let edit = PageEdit {
                path,
                body: Some(body),
                etag: request.etag.clone().filter(|e| !e.trim().is_empty()),
                message,
                author: (
                    kb::HUMAN_AUTHOR_NAME.to_string(),
                    kb::HUMAN_AUTHOR_EMAIL.to_string(),
                ),
            };
            match ops_kb::commit_page(&root, &edit) {
                WriteOutcome::Written {
                    sha,
                    etag,
                    unchanged,
                } => {
                    // ADR-0047 D3: 書いたら必ず索引を作り直す。
                    let _ = ops_kb::reindex(&root);
                    tracing::info!(
                        who = "admin",
                        op = "knowledge_put",
                        path = %edit.path,
                        unchanged,
                        "admin: knowledge page committed"
                    );
                    Ok(KnowledgePageResult {
                        path: edit.path,
                        etag,
                        sha,
                        unchanged,
                    })
                }
                WriteOutcome::EtagMismatch { etag } => Err(etag_mismatch(etag)),
                WriteOutcome::Missing => Err(page_not_found(&edit.path)),
                WriteOutcome::Failed { detail } => Err(knowledge_unavailable(detail)),
            }
        })
        .await?;
    Ok(json_response(StatusCode::OK, &result))
}

// ---------------------------------------------------------------------------
// GET /knowledge/inbox
// ---------------------------------------------------------------------------

/// 添付の表（チャットと同じ SQLite）を読み取り専用で開く。添付の保存が無効なら `None`。
fn open_attachment_index(
    db: Option<&std::path::Path>,
) -> Result<Option<rusqlite::Connection>, ApiProblem> {
    let Some(db) = db else {
        return Ok(None);
    };
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| ApiProblem::internal(format!("attachment index: {e}")))?;
    conn.busy_timeout(std::time::Duration::from_secs(10))
        .map_err(|e| ApiProblem::internal(format!("attachment index: {e}")))?;
    Ok(Some(conn))
}

fn candidate_provenance(
    conn: Option<&rusqlite::Connection>,
    id: &str,
) -> Result<Vec<KnowledgeCandidateProvenance>, ApiProblem> {
    let Some(conn) = conn else {
        return Ok(Vec::new());
    };
    task_core::chat::attachments::provenance_for_owner(conn, "knowledge_inbox", id)
        .map(|rows| rows.into_iter().map(Into::into).collect())
        .map_err(|e| ApiProblem::internal(format!("attachment provenance: {e}")))
}

fn candidate_view(
    root: &std::path::Path,
    item: ops_kb::InboxItem,
    provenance: Vec<KnowledgeCandidateProvenance>,
) -> KnowledgeCandidate {
    KnowledgeCandidate {
        html: render_markdown(&item.body, &item.path),
        target_exists: root.join(&item.target).exists(),
        id: item.id,
        path: item.path,
        title: item.title,
        tags: item.tags,
        scope: item.scope,
        sources: item.sources,
        confidence: item.confidence,
        target: item.target,
        created: item.created,
        body: item.body,
        op: item.op.map(|o| o.as_str().to_string()),
        provenance,
    }
}

async fn inbox(
    axum::extract::State(state): axum::extract::State<ApiState>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let root = root_of(&state)?;
    let db = state.chat.attachment_db_path.clone();
    let view = state
        .blocking(move |_| {
            let initialized = ops_kb::exists(&root);
            let items = if initialized {
                let conn = open_attachment_index(db.as_deref())?;
                ops_kb::inbox_list(&root)
                    .into_iter()
                    .map(|item| {
                        let provenance = candidate_provenance(conn.as_ref(), &item.id)?;
                        Ok(candidate_view(&root, item, provenance))
                    })
                    .collect::<Result<_, ApiProblem>>()?
            } else {
                Vec::new()
            };
            Ok(KnowledgeInbox {
                root: root.display().to_string(),
                initialized,
                items,
            })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &view))
}

// ---------------------------------------------------------------------------
// POST /knowledge/inbox（管理系。ADR 2026-10-07 cos-live-fixes D2）
// ---------------------------------------------------------------------------

async fn record(
    axum::extract::State(state): axum::extract::State<ApiState>,
    headers: HeaderMap,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let request: KnowledgeRecordBody = read_json(body, false).await?;
    let root = root_of(&state)?;
    let db = state.chat.attachment_db_path.clone();
    let result = state
        .blocking(move |store| record_op(store, &root, db.as_deref(), request, None)?.direct())
        .await?;
    Ok(json_response(StatusCode::CREATED, &result))
}

fn record_problem(e: ops_kb::RecordError) -> ApiProblem {
    use ops_kb::RecordError as E;
    let field = match &e {
        E::NoTitle => "title",
        E::NoScope | E::Placement(_) => "scope",
        E::NoSources => "sources",
        E::NoBody | E::Secret(_) => "body",
        E::Failed(detail) => return knowledge_unavailable(detail.clone()),
    };
    ApiProblem::validation(vec![ValidationError {
        field: Some(field.into()),
        message: e.to_string(),
    }])
}

/// 候補に添付を pin する（呼び手の transaction の中）。検査は D1 の task 作成と同じ
/// （`task_pin_problem_tx`: 形式・重複・件数・`ready`・期限・CoS なら自分の thread）。
fn pin_candidate_tx(
    tx: &rusqlite::Transaction<'_>,
    ids: &[String],
    candidate: &str,
    thread: Option<&str>,
) -> Result<(), ApiProblem> {
    if ids.is_empty() {
        return Ok(());
    }
    let now = time::OffsetDateTime::now_utc();
    let internal =
        |e: task_core::chat::attachments::AttachmentError| ApiProblem::internal(e.to_string());
    if let Some(why) =
        task_core::chat::attachments::task_pin_problem_tx(tx, ids, thread, now).map_err(internal)?
    {
        return Err(ApiProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_attachment",
            why,
        ));
    }
    for id in ids {
        task_core::chat::attachments::add_ref_tx(tx, id, "knowledge_inbox", candidate, now)
            .map_err(internal)?;
    }
    Ok(())
}

fn attachments_unavailable() -> ApiProblem {
    ApiProblem::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "attachments_unavailable",
        "attachment storage is not configured",
    )
}

/// `POST /knowledge/inbox` の本体。handler（`audit = None`）と CoS の `/cos/operations`
/// （action `knowledge.record`）が共有する。候補は一時名で書き（`record_prepare`）、添付の pin と
/// 監査（CoS）を SQLite に commit してから `_inbox/<id>.md` へ rename して git commit する。pin か
/// 監査が失敗すれば一時ファイルを消すので、候補ファイルは残らない。
pub(crate) fn record_op(
    store: &task_core::SqliteStore,
    root: &std::path::Path,
    attachment_db: Option<&std::path::Path>,
    body: KnowledgeRecordBody,
    audit: Option<&OperationAudit>,
) -> Result<Applied<KnowledgeRecordResult>, ApiProblem> {
    let reject = |problem: ApiProblem| match audit {
        Some(audit) => audit.reject(store, "knowledge", "new", problem),
        None => problem,
    };
    require_kb(root).map_err(reject)?;
    if let Some(path) = body
        .path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        page_path(path).map_err(reject)?;
    }
    let projects = ops_kb::project_refs(store)
        .map_err(|e| reject(ApiProblem::internal(format!("project list: {e}"))))?;
    let layout = ops_kb::layout(root, Some(projects));
    let request = ops_kb::RecordRequest {
        title: body.title,
        scope: body.scope,
        tags: body.tags,
        sources: body.sources,
        confidence: body.confidence,
        body: body.body,
        path: body.path,
        op: None,
    };
    let prepared =
        ops_kb::record_prepare(root, &request, &layout).map_err(|e| reject(record_problem(e)))?;
    let attachment_ids = body.attachment_ids;
    let view = |sha: String, prepared: &ops_kb::PreparedRecord| KnowledgeRecordResult {
        id: prepared.id.clone(),
        path: prepared.path.clone(),
        target: prepared.target.clone(),
        scope: prepared.scope.clone(),
        op: prepared.op.map(|o| o.as_str().to_string()),
        attachment_ids: attachment_ids.clone(),
        sha,
    };
    let Some(audit) = audit else {
        if !attachment_ids.is_empty() {
            let db = attachment_db.ok_or_else(attachments_unavailable)?;
            let db_error =
                |e: rusqlite::Error| ApiProblem::internal(format!("attachment pin: {e}"));
            let mut conn = rusqlite::Connection::open(db).map_err(db_error)?;
            conn.busy_timeout(std::time::Duration::from_secs(10))
                .map_err(db_error)?;
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(db_error)?;
            pin_candidate_tx(&tx, &attachment_ids, &prepared.id, None)?;
            tx.commit().map_err(db_error)?;
        }
        let template = view(String::new(), &prepared);
        let outcome = prepared.commit(root).map_err(record_problem)?;
        tracing::info!(who = "admin", op = "knowledge_record", id = %outcome.id, "admin: knowledge candidate recorded");
        return Ok(Applied::Direct(KnowledgeRecordResult {
            sha: outcome.sha,
            ..template
        }));
    };
    let thread = audit.ctx.thread_id.clone();
    let mut failure = None;
    let outcome = audit.apply(store, "knowledge", &prepared.id, "knowledge.record", |tx| {
        if let Err(problem) = pin_candidate_tx(tx, &attachment_ids, &prepared.id, Some(&thread)) {
            let detail = problem.detail().to_string();
            failure = Some(problem);
            return Err(task_core::chat::ChatError::Invalid(detail));
        }
        let mut result = serde_json::to_value(view(String::new(), &prepared))
            .map_err(|e| task_core::chat::ChatError::Invalid(e.to_string()))?;
        if let Some(map) = result.as_object_mut() {
            map.remove("sha");
        }
        Ok(result)
    });
    let operation = outcome.map_err(|problem| failure.take().unwrap_or(problem))?;
    // The audit row is committed; the candidate file becomes visible now. A failure here leaves an
    // applied operation without its file, which the API reports instead of hiding.
    prepared.commit(root).map_err(record_problem)?;
    Ok(Applied::Audited(Box::new(operation)))
}

// ---------------------------------------------------------------------------
// GET /knowledge/inbox/{id}
// ---------------------------------------------------------------------------

async fn inbox_detail(
    axum::extract::State(state): axum::extract::State<ApiState>,
    Params(id): Params<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let root = root_of(&state)?;
    let db = state.chat.attachment_db_path.clone();
    let view = state
        .blocking(move |_| {
            // id の境界（`/`・`..` は 403。accept / reject と同じ）。
            ops_kb::inbox_path(&id).map_err(path_problem)?;
            let item = ops_kb::inbox_get(&root, &id).ok_or_else(|| candidate_not_found(&id))?;
            let conn = open_attachment_index(db.as_deref())?;
            let provenance = candidate_provenance(conn.as_ref(), &item.id)?;
            Ok(candidate_view(&root, item, provenance))
        })
        .await?;
    Ok(json_response(StatusCode::OK, &view))
}

// ---------------------------------------------------------------------------
// POST /knowledge/inbox/{id}/{accept,reject}（管理系）
// ---------------------------------------------------------------------------

async fn accept(
    axum::extract::State(state): axum::extract::State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let request: KnowledgeAcceptBody = read_json(body, true).await?;
    let root = root_of(&state)?;
    let result = state
        .blocking(move |_| {
            require_kb(&root)?;
            // id の境界（`/`・`..` は 403）。
            ops_kb::inbox_path(&id).map_err(path_problem)?;
            if let Some(path) = request.path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
                page_path(path)?;
            }
            match ops_kb::inbox_accept(&root, &id, request.path.as_deref(), request.overwrite) {
                InboxOutcome::Accepted { path, sha, etag } => {
                    tracing::info!(who = "admin", op = "knowledge_accept", id = %id, path = %path, "admin: knowledge candidate accepted");
                    Ok(KnowledgePageResult {
                        path,
                        etag,
                        sha,
                        unchanged: false,
                    })
                }
                InboxOutcome::Missing => Err(candidate_not_found(&id)),
                InboxOutcome::Exists { path } => Err(ApiProblem::new(
                    StatusCode::CONFLICT,
                    "page_exists",
                    format!("page already exists: {path}（上書きするなら overwrite: true）"),
                )),
                InboxOutcome::Failed { detail } => Err(knowledge_unavailable(detail)),
                InboxOutcome::Rejected { .. } => Err(knowledge_unavailable("unexpected outcome")),
            }
        })
        .await?;
    Ok(json_response(StatusCode::OK, &result))
}

async fn reject(
    axum::extract::State(state): axum::extract::State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let root = root_of(&state)?;
    let result = state
        .blocking(move |store| reject_op(store, &root, id, None)?.direct())
        .await?;
    Ok(json_response(StatusCode::OK, &result))
}

/// `POST /knowledge/inbox/{id}/reject` の本体。handler（`audit = None`）と CoS の `/cos/operations`
/// （ADR 2026-10-05 D3）が共有する。KB は SQLite の外（git）なので、監査ありでは候補の取り下げを
/// `cos_operation_apply` の transaction の中で行い、失敗すれば `cos_operations`・監査 event・card は
/// 書かない（取り下げの git commit の後に SQLite の commit が失敗したときだけ、監査の無い取り下げが残る）。
pub(crate) fn reject_op(
    store: &task_core::SqliteStore,
    root: &std::path::Path,
    id: String,
    audit: Option<&OperationAudit>,
) -> Result<Applied<KnowledgeRejectResult>, ApiProblem> {
    let reject = |problem: ApiProblem| match audit {
        Some(audit) => audit.reject(store, "knowledge", &id, problem),
        None => problem,
    };
    require_kb(root).map_err(reject)?;
    ops_kb::inbox_path(&id).map_err(|e| reject(path_problem(e)))?;
    let Some(audit) = audit else {
        return match ops_kb::inbox_reject(root, &id) {
            InboxOutcome::Rejected { sha } => {
                tracing::info!(who = "admin", op = "knowledge_reject", id = %id, "admin: knowledge candidate rejected");
                Ok(Applied::Direct(KnowledgeRejectResult { id, sha }))
            }
            InboxOutcome::Missing => Err(candidate_not_found(&id)),
            InboxOutcome::Failed { detail } => Err(knowledge_unavailable(detail)),
            other => Err(knowledge_unavailable(format!(
                "unexpected outcome: {other:?}"
            ))),
        };
    };
    let mut failure = None;
    let outcome =
        audit.apply(
            store,
            "knowledge",
            &id,
            "knowledge.reject",
            |_tx| match ops_kb::inbox_reject(root, &id) {
                InboxOutcome::Rejected { sha } => Ok(serde_json::json!({"id": id, "sha": sha})),
                InboxOutcome::Missing => {
                    failure = Some(candidate_not_found(&id));
                    Err(task_core::chat::ChatError::NotFound {
                        kind: "knowledge candidate",
                        id: id.clone(),
                    })
                }
                InboxOutcome::Failed { detail } => {
                    failure = Some(knowledge_unavailable(detail.clone()));
                    Err(task_core::chat::ChatError::Conflict(detail))
                }
                other => {
                    let detail = format!("unexpected outcome: {other:?}");
                    failure = Some(knowledge_unavailable(detail.clone()));
                    Err(task_core::chat::ChatError::Conflict(detail))
                }
            },
        );
    match outcome {
        Ok(operation) => Ok(Applied::Audited(Box::new(operation))),
        Err(problem) => Err(failure.unwrap_or(problem)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// パスの境界は `task_core::knowledge` と同じ（403 / 422 / 400 に写す）。
    #[test]
    fn page_paths_map_to_the_right_problem() {
        assert_eq!(page_path("user/a.md").expect("ok"), "user/a.md");
        assert_eq!(
            page_path("../escape.md").err().map(|p| p.code()),
            Some("path_forbidden")
        );
        assert_eq!(
            page_path("a.txt").err().map(|p| p.code()),
            Some("validation")
        );
        assert_eq!(page_path(" ").err().map(|p| p.code()), Some("bad_request"));
        assert_eq!(
            ops_kb::inbox_path("../../etc/passwd")
                .err()
                .map(path_problem)
                .map(|p| p.code()),
            Some("path_forbidden")
        );
    }
}
