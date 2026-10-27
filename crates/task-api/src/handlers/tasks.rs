//! タスクの一覧・作成・詳細（`GET/POST /tasks`、`GET /tasks/{id}`）。

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::HashMap;
use task_core::{
    ListFilter, ListOrder, MilestoneId, ProjectId, Status, StoreError, Task, TaskKind, TaskStore,
};
use task_ops::OpsError;
use task_ops::add::NewTaskSpec;
use time::OffsetDateTime;

use crate::cos::operations::{Applied, OperationAudit};
use crate::files;
use crate::middleware::require_admin;
use crate::problem::{ApiProblem, ops_problem};
use crate::query::{QueryParams, parse_snake, parse_task_id};
use crate::state::ApiState;

use super::{ApiResult, Params, idempotency, json_response, no_query, read_json};

const TITLE_QUERY_MAX_CHARS: usize = 200;

/// 取り込み状態の付加（ADR 2026-10-21-task-list-opt-in-integrations D2）。要求ごとに
/// `task_integration_list` を 1 回だけ読み、repo の解決（`repo_get` と `resolve_default_branch`）は
/// repo ごと、promotion の最新 release は要求ごとに 1 回だけ行う。付加は行数に比例して store・git を叩かない。
struct IntegrationAttacher<'a> {
    store: &'a dyn TaskStore,
    states: HashMap<task_core::TaskId, Vec<task_core::TaskRepoIntegrationState>>,
    repositories: HashMap<task_core::RepoId, Option<task_ops::stranded::IntegrationRepoContext>>,
    release: Option<task_ops::stranded::NewestRelease>,
}

impl<'a> IntegrationAttacher<'a> {
    fn new(
        store: &'a dyn TaskStore,
        releases_dir: Option<&std::path::Path>,
    ) -> Result<Self, ApiProblem> {
        let mut states: HashMap<_, Vec<_>> = HashMap::new();
        for state in store
            .task_integration_list(None)
            .map_err(crate::problem::store_problem)?
        {
            states.entry(state.task_id).or_default().push(state);
        }
        Ok(Self {
            store,
            states,
            repositories: HashMap::new(),
            release: releases_dir.map(task_ops::stranded::NewestRelease::read),
        })
    }

    fn attach(
        &mut self,
        id: task_core::TaskId,
        integrations: &mut Option<Vec<task_ops::view::IntegrationView>>,
        promotion: &mut Option<task_ops::stranded::PromotionView>,
    ) -> Result<(), ApiProblem> {
        let states = self.states.get(&id).cloned().unwrap_or_default();
        // 状態の行が名指す repo だけが view になる。task の案件の repo を全部引く必要はない。
        for state in &states {
            if !self.repositories.contains_key(&state.repo_id) {
                let context = self.resolve_repo(state.repo_id)?;
                self.repositories.insert(state.repo_id, context);
            }
        }
        let context = task_ops::stranded::IntegrationContext {
            repositories: self
                .repositories
                .iter()
                .filter_map(|(id, repo)| Some((*id, repo.clone()?)))
                .collect(),
            release: self.release.clone(),
        };
        task_ops::view::attach_integrations(states, integrations, promotion, &context);
        Ok(())
    }

    fn resolve_repo(
        &self,
        repo_id: task_core::RepoId,
    ) -> Result<Option<task_ops::stranded::IntegrationRepoContext>, ApiProblem> {
        let Some(repo) = self
            .store
            .repo_get(repo_id)
            .map_err(crate::problem::store_problem)?
        else {
            return Ok(None);
        };
        let task_core::WorkspaceSpec::Local { path, .. } = repo.location else {
            return Ok(None);
        };
        // ADR 2026-10-11-stranded-done-branches F1: 解決できない repo は context に入れない。
        match task_ops::changes::resolve_default_branch(&path, repo.default_branch.as_deref()) {
            Ok(default_branch) => Ok(Some(task_ops::stranded::IntegrationRepoContext {
                name: repo.name,
                path,
                default_branch,
            })),
            Err(error) => {
                tracing::warn!(repo = %repo.name, %error, "integration context skips repository");
                Ok(None)
            }
        }
    }
}

/// POST /tasks accepts the task specification and an optional write path hint.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewTaskBody {
    #[serde(flatten)]
    pub task: NewTaskSpec,
    #[serde(default)]
    pub expected_write_paths: Option<Vec<String>>,
    /// ADR 2026-10-07 cos-live-fixes D1: chat attachment ids to pin to the new task
    /// (`owner_kind: "task"`) in the transaction that creates it, so the first run's input manifest
    /// already has them. An unknown, deleted, expired or duplicate id (or, from a CoS run, one from
    /// another thread) is 422 `invalid_attachment` and creates nothing.
    #[serde(default)]
    pub attachment_ids: Vec<String>,
    /// ADR 2026-10-16-mlsys-measure-task-routing: 試行ごとの routing 設定（mode・下段・根拠表・
    /// オンライン更新）を task に固定する。`task_routing_overrides` に作成と同じ transaction で 1 行書き、dispatcher は config の
    /// `[routing] mode` より優先する。省略時は従来（config に従う）。未知の値は 400。
    #[serde(default)]
    pub routing_override: Option<task_core::route_select::RoutingOverride>,
    /// ADR 2026-10-16-campaign-identity-propagation: campaign の root task に付ける識別
    /// （campaign_id / case_id / arm / repetition / manifest_hash / routing_setting）。task 作成と
    /// 同じ transaction で保存する。子孫・再試行・review は祖先の鎖から継ぐ。省略時は campaign 無し。
    #[serde(default)]
    pub campaign: Option<task_core::decision_log::CampaignIdentity>,
    /// ADR 2026-10-27-task-idempotency-keys: 再送をまとめる冪等キー（1〜200 文字）。同じキーで
    /// 中身（キーを除いた本文）も同じ再送は既存 task を 200 で返し、中身が違えば 409
    /// `idempotency_key_conflict`。キーは `POST /tasks/{id}/retry` と同じ名前空間。CoS の
    /// `/cos/operations` からは 400（操作自身の `idempotency_key` を使う）。
    #[serde(default)]
    pub idempotency_key: Option<String>,
    /// ADR 2026-10-27-unblocks-reopen-on-promotion D1: この task（修正）を待っている task の id（最大 8 件・
    /// 重複不可・存在する task だけ。違反は 422 `validation`）。task と同じ transaction で `task_unblocks` に
    /// 書く。この task を含む release が昇格したら、daemon が待っている `failed` の task を 1 度だけ reopen し、
    /// `blocked` の task にはコメントを残す。
    #[serde(default)]
    pub unblocks: Vec<task_core::TaskId>,
}

fn created_task(task: &Task) -> Response {
    let mut response = json_response(StatusCode::CREATED, task);
    if let Ok(location) = HeaderValue::from_str(&format!("/api/v1/tasks/{}", task.id)) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    response
}

// ---- 3. GET /tasks ----

/// `GET /tasks/counts` は一覧の重い行取得をせず、状態別件数だけを返す。
pub(super) async fn task_counts(
    State(state): State<ApiState>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let counts = state
        .blocking(|store| {
            task_ops::view::task_status_counts(store).map_err(|e| ops_problem(store, e, None))
        })
        .await?;
    Ok(json_response(StatusCode::OK, &counts))
}

pub(super) async fn list_tasks(
    State(state): State<ApiState>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    let query = QueryParams::parse(
        raw.as_deref(),
        &[
            "status",
            "kind",
            "genre",
            "parent",
            "project",
            "root_only",
            "q",
            "order",
            "limit",
            "cursor",
            // ADR-0044 D4（Phase 53）: ボードと検索のフィルタ。複数指定は AND。
            "label",
            "category",
            "assignee",
            "milestone",
            "tier",
            "priority",
            // ADR-0044 D6（Phase 55）: アーカイブされた案件のタスクは既定で隠す。
            "archived",
            // ADR 2026-10-11-task-wait-reasons D5: 走れる／待ち。
            "run_state",
            // ADR 2026-10-21-task-list-opt-in-integrations D1: 付加する欄（`integrations`）。
            "with",
        ],
    )?;
    let mut filter = ListFilter::default();
    for status in query.list("status") {
        filter
            .statuses
            .push(parse_snake::<Status>("status", status)?);
    }
    for kind in query.list("kind") {
        filter.kinds.push(parse_snake::<TaskKind>("kind", kind)?);
    }
    // ADR-0027 D1: `genre` は自由記述なので `kind` と違って列挙型の検証はしない（完全一致だけ）。
    for genre in query.list("genre") {
        filter.genres.push(genre.to_string());
    }
    filter.parent_id = query.task_id("parent")?;
    // ADR-0033 D2: 案件で絞る（案件の仕事の木。`GET /projects/{id}` は同じ絞り込みを使う）。
    if let Some(raw) = query.single("project")? {
        filter.project_id =
            Some(raw.parse::<ProjectId>().map_err(|_| {
                ApiProblem::bad_request("query parameter `project` must be a ULID")
            })?);
    }
    filter.root_only = query.bool("root_only")?.unwrap_or(false);
    // ADR-0044 D6（Phase 55）: `?archived=1` を付けたときだけアーカイブされた案件のタスクも返す。
    filter.hide_archived = !query.bool("archived")?.unwrap_or(false);
    // ---- ADR-0044 D4（Phase 53）----
    for label in query.list("label") {
        if !task_core::is_valid_label(label) {
            return Err(ApiProblem::bad_request(format!(
                "query parameter `label` must match [a-z0-9-] (got {label:?})"
            )));
        }
        filter.labels.push(label.to_string());
    }
    for category in query.list("category") {
        let Some(parsed) = task_core::TaskCategory::parse(category) else {
            return Err(ApiProblem::bad_request(format!(
                "unknown category `{category}`"
            )));
        };
        filter.categories.push(parsed);
    }
    if let Some(assignee) = query.single("assignee")?.filter(|a| !a.is_empty()) {
        filter.assignee = Some(assignee.to_string());
    }
    if let Some(raw) = query.single("milestone")? {
        filter.milestone_id =
            Some(raw.parse::<MilestoneId>().map_err(|_| {
                ApiProblem::bad_request("query parameter `milestone` must be a ULID")
            })?);
    }
    for tier in query.list("tier") {
        filter
            .tiers
            .push(parse_snake::<task_core::Tier>("tier", tier)?);
    }
    for priority in query.list("priority") {
        // `P0`〜`P3` でも生の整数でも受ける（`priority_label` と対）。
        let value = match task_core::priority_from_label(priority) {
            Some(v) => v,
            None => priority
                .parse::<i32>()
                .map_err(|_| ApiProblem::bad_request(format!("unknown priority `{priority}`")))?,
        };
        filter.priorities.push(value);
    }
    if let Some(text) = query.single("q")? {
        if text.chars().count() > TITLE_QUERY_MAX_CHARS {
            return Err(ApiProblem::bad_request(
                "query parameter `q` must be at most 200 characters",
            ));
        }
        if !text.is_empty() {
            filter.text_contains = Some(text.to_string());
            // ADR-0044 D4: `GET /tasks?q=` は title / objective に加えて**コメント本文**も見る。
            filter.text_includes_comments = true;
        }
    }
    let order = match query.single("order")? {
        None | Some("updated_desc") => ListOrder::UpdatedDesc,
        Some("dispatch") => ListOrder::Dispatch,
        Some("created_desc") => ListOrder::CreatedDesc,
        Some(other) => return Err(ApiProblem::bad_request(format!("unknown order `{other}`"))),
    };
    let run_state = match query.single("run_state")? {
        None | Some("") => None,
        Some("runnable") => Some(task_ops::view::RunState::Runnable),
        Some("waiting") => Some(task_ops::view::RunState::Waiting),
        Some(other) => {
            return Err(ApiProblem::bad_request(format!(
                "unknown run_state `{other}` (runnable | waiting)"
            )));
        }
    };
    let limit = query.limit("limit", 100, 500)?;
    let cursor = query
        .single("cursor")?
        .filter(|c| !c.is_empty())
        .map(str::to_string);
    // ADR 2026-10-21-task-list-opt-in-integrations D1: 取り込み状態は `?with=integrations` のときだけ。
    let mut with_integrations = false;
    for value in query.list("with") {
        match value {
            "integrations" => with_integrations = true,
            other => {
                return Err(ApiProblem::bad_request(format!(
                    "unknown with `{other}` (integrations)"
                )));
            }
        }
    }
    let ctx = state.inner.view.clone();
    let releases_dir = state.inner.integration_releases_dir.clone();
    let memory = state.memory_waits();
    let list = state
        .blocking(move |store| {
            let mut list = task_ops::view::task_list(
                store,
                &filter,
                order,
                cursor.as_deref(),
                limit,
                run_state,
                &ctx,
                &memory,
                OffsetDateTime::now_utc(),
            )
            .map_err(|e| match e {
                OpsError::Store(StoreError::Invalid(message))
                    if message.starts_with("invalid cursor") =>
                {
                    ApiProblem::bad_request("invalid cursor")
                }
                other => ops_problem(store, other, None),
            })?;
            if with_integrations {
                let mut attacher = IntegrationAttacher::new(store, releases_dir.as_deref())?;
                for item in &mut list.items {
                    attacher.attach(item.id, &mut item.integrations, &mut item.promotion)?;
                }
            }
            Ok(list)
        })
        .await?;
    Ok(json_response(StatusCode::OK, &list))
}

// ---- 4. POST /tasks ----

pub(super) async fn create_task(
    State(state): State<ApiState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let raw: serde_json::Value = read_json(body, false).await?;
    let claim = idempotency::claim("task.create", "POST", "/api/v1/tasks", &raw)?;
    let body: NewTaskBody = idempotency::decode(raw)?;
    let roles = state.inner.roles.clone();
    let genres = state.inner.genres.clone();
    let (task, resent) = state
        .blocking(move |store| {
            let keyed = match &claim {
                Some(claim) => idempotency::lookup(store, claim)?,
                None => None,
            };
            if let Some(task) = campaign_resend(store, &body)? {
                if let Some(claim) = &claim {
                    let found = store
                        .idempotency_bind_existing(
                            claim,
                            &task.id.to_string(),
                            &OffsetDateTime::now_utc().to_string(),
                        )
                        .map_err(crate::problem::store_problem)?;
                    if let Some(existing) = idempotency::settle(found)? {
                        return Ok((idempotency::existing_task(store, &existing)?, true));
                    }
                }
                return Ok((task, true));
            }
            if let Some(existing) = keyed {
                return Ok((idempotency::existing_task(store, &existing)?, true));
            }
            match create_task_op(store, &roles, &genres, body, claim.as_ref(), None) {
                Ok(applied) => Ok((applied.direct()?, false)),
                Err(problem) => {
                    // 別の要求が先に commit した場合、rollback 後に結果を照合する。
                    if let Some(claim) = &claim
                        && let Some(existing) = idempotency::lookup(store, claim)?
                    {
                        return Ok((idempotency::existing_task(store, &existing)?, true));
                    }
                    Err(problem)
                }
            }
        })
        .await?;
    if resent {
        // 同じキー・同じ識別の再送: 既存 task をそのまま返す（作り直さない）。
        return Ok(json_response(StatusCode::OK, &task));
    }
    let mut response = created_task(&task);
    // ADR 2026-10-09-cos-task-repository-required D2: 人の起票は拒否せず、header で警告する
    // （後から `PATCH /tasks/{id}` の `project_id`・`repos` で付けられる）。
    if task_ops::repo_requirement::missing_repository_reason(&task).is_some() {
        response.headers_mut().insert(
            REPOSITORY_WARNING_HEADER,
            HeaderValue::from_static(REPOSITORY_WARNING),
        );
    }
    Ok(response)
}

/// campaign 識別付きの再送なら既存 task を返す。識別が食い違えば 409（別の試行を上書きしない）。
fn campaign_resend(
    store: &task_core::store::SqliteStore,
    body: &NewTaskBody,
) -> Result<Option<Task>, ApiProblem> {
    let Some(identity) = &body.campaign else {
        return Ok(None);
    };
    identity.validate().map_err(ApiProblem::bad_request)?;
    let internal = |e: StoreError| ApiProblem::internal(e.to_string());
    match store
        .task_campaign_resend_org_checked(identity)
        .map_err(|e| match e {
            StoreError::Invalid(why) => org_drift(&why),
            e => internal(e),
        })? {
        task_core::store::CampaignResend::New => Ok(None),
        task_core::store::CampaignResend::Same(id) => {
            let id = id
                .parse()
                .map_err(|_| ApiProblem::internal("bad task id"))?;
            store
                .get(id)
                .map_err(internal)?
                .map(Some)
                .ok_or_else(|| ApiProblem::internal("task vanished"))
        }
        task_core::store::CampaignResend::Conflict(id) => Err(campaign_conflict(&id)),
    }
}

fn org_drift(why: &str) -> ApiProblem {
    ApiProblem::new(
        StatusCode::CONFLICT,
        "campaign_org_drift",
        format!("campaign.variant_org does not match the current org: {why}"),
    )
}

fn campaign_conflict(existing: &str) -> ApiProblem {
    ApiProblem::new(
        StatusCode::CONFLICT,
        "campaign_identity_conflict",
        format!("task {existing} already holds this trial with a different campaign identity"),
    )
}

/// ADR 2026-10-09-cos-task-repository-required D2: 人の `POST /tasks` への警告の header。
pub const REPOSITORY_WARNING_HEADER: &str = "celeris-warning";
const REPOSITORY_WARNING: &str = "repository_required: the task uses a repository but has no project_id/repos; \
     attach them with PATCH /tasks/{id} (project_id, repos) before it runs";

/// `POST /tasks` の本体。handler（`audit = None`）と CoS の `/cos/operations`（ADR 2026-10-05 D3。
/// `audit` の transaction で task・`cos_operations`・監査 event を一緒に書く）が共有する。
pub(crate) fn create_task_op(
    store: &task_core::store::SqliteStore,
    roles: &[task_core::RoleSpec],
    genres: &[task_core::GenreSpec],
    body: NewTaskBody,
    claim: Option<&task_core::store::IdempotencyClaim>,
    audit: Option<&OperationAudit>,
) -> Result<Applied<Task>, ApiProblem> {
    let NewTaskBody {
        mut task,
        expected_write_paths,
        attachment_ids,
        routing_override,
        campaign,
        idempotency_key,
        unblocks,
    } = body;
    let reject = |problem: ApiProblem| match audit {
        Some(audit) => audit.reject(store, "task", "new", problem),
        None => problem,
    };
    if audit.is_some() && idempotency_key.is_some() {
        return Err(reject(idempotency::cos_key_rejected()));
    }
    // repo 名だけがあり案件が無い要求にも、CoS へは同じ修正可能な理由を返す。
    if audit.is_some() && task.project_id.is_none() && !task.repos.is_empty() {
        return Err(reject(ApiProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            task_ops::repo_requirement::REPOSITORY_REQUIRED_CODE,
            task_ops::repo_requirement::REPOSITORY_REQUIRED,
        )));
    }
    let paths = expected_write_paths
        .map(|paths| task_core::write_set::normalize_write_paths(&paths))
        .transpose()
        .map_err(|e| reject(ApiProblem::bad_request(e)))?;
    // ADR-0044 D1（Phase 53）: **人が作ったタスクは `ready`**（人は Go を出す側なので draft を挟まない）。
    // `draft` にしたければ `status: "draft"` を明示する。計画・委譲で作られる子（`draft` → Go）の経路は
    // ここを通らないので変わらない。
    if task.status.is_none() {
        task.status = Some(task_core::Status::Ready);
    }
    // ADR-0016 M3 / ADR-0027 D1: 省略された tier / adapter / 予算は `[[roles]]` の既定 → `[[genres]]` の
    // `default_role` の既定 → 全体の既定で埋める。API は常に完全な設定を持つので、`genres` が設定されて
    // いれば知らない `genre` / `genre` と `role` の不整合は常に検証する（celerisctl の「`--config` 無し」の
    // 緩さはここには無い）。
    if let Some(o) = &routing_override {
        o.validate()
            .map_err(|e| reject(ApiProblem::bad_request(e.to_string())))?;
    }
    task_ops::unblocks::validate_unblocks(store, &unblocks)
        .map_err(|e| reject(ops_problem(store, e, None)))?;
    let built =
        task_ops::add::build_task_with_roles(store, task, roles, genres, OffsetDateTime::now_utc())
            .map_err(|e| reject(ops_problem(store, e, None)))?;
    // ADR 2026-10-16-mlsys-measure-task-routing D2: 固定は task と同じ transaction で 1 行 + 1 event。
    let pinned_events: Vec<task_core::Event> = routing_override
        .iter()
        .map(|o| task_core::Event::RoutingOverridePinned {
            routing_override: o.clone(),
        })
        .collect();
    // ADR 2026-10-09-cos-task-repository-required D1: CoS の起票は、リポジトリを使う task に案件・
    // リポジトリが無ければ 422（worker の workspace が空のまま走らせない）。
    if audit.is_some()
        && let Some(reason) = task_ops::repo_requirement::missing_repository_reason(&built)
    {
        return Err(reject(ApiProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            task_ops::repo_requirement::REPOSITORY_REQUIRED_CODE,
            reason,
        )));
    }
    let paths = paths.filter(|paths| !paths.is_empty());
    let thread = audit.map(|audit| audit.ctx.thread_id.clone());
    // ADR 2026-10-07 cos-live-fixes D1: the task row, its `ready` status and the attachment pins are
    // one transaction, so the dispatcher never picks the task up before its pins exist.
    let mut failure = None;
    let mut write = |tx: &rusqlite::Transaction<'_>| -> Result<(), StoreError> {
        if let Some(o) = &routing_override {
            task_core::store::SqliteStore::set_task_routing_override_tx(
                tx,
                built.id,
                o,
                &OffsetDateTime::now_utc().to_string(),
            )?;
        }
        if let Some(identity) = &campaign {
            // 同じ transaction で再確認する（先の確認と保存の間に同じ試行が入った競合も 409 にする）。
            if !matches!(
                task_core::store::SqliteStore::task_campaign_resend_tx(tx, identity)?,
                task_core::store::CampaignResend::New
            ) {
                failure = Some(campaign_conflict("(concurrent)"));
                return Err(StoreError::Invalid("campaign identity conflict".into()));
            }
            if let Some(pin) = &identity.variant_org
                && let Err(e) = task_core::store::SqliteStore::campaign_org_check_tx(tx, pin)
            {
                let why = e.to_string();
                failure = Some(org_drift(&why));
                return Err(StoreError::Invalid(why));
            }
            task_core::store::SqliteStore::task_campaign_set_tx(
                tx,
                &built.id.to_string(),
                identity,
                OffsetDateTime::now_utc(),
            )?;
        }
        if let Some(claim) = claim {
            // 同じ transaction で照合し直す（先の照合と保存の間に同じキーが入った競合も 409 にする）。
            if let Err(problem) = idempotency::record_tx(tx, claim, &built.id.to_string(), None) {
                let detail = problem.detail().to_string();
                failure = Some(problem);
                return Err(StoreError::Invalid(detail));
            }
        }
        if paths.is_some() {
            task_core::store::SqliteStore::set_task_expected_write_paths_tx(
                tx,
                built.id,
                paths.clone(),
                &OffsetDateTime::now_utc().to_string(),
            )?;
        }
        // ADR 2026-10-27-unblocks-reopen-on-promotion D1: 結びは task と同じ transaction で書く。
        task_core::store::SqliteStore::unblocks_insert_tx(
            tx,
            built.id,
            &unblocks,
            OffsetDateTime::now_utc(),
        )?;
        if let Err(problem) = pin_attachments_tx(tx, &attachment_ids, built.id, thread.as_deref()) {
            let detail = problem.detail().to_string();
            failure = Some(problem);
            return Err(StoreError::Invalid(detail));
        }
        Ok(())
    };
    let Some(audit) = audit else {
        let outcome = store.create_task_with(&built, pinned_events.clone(), write);
        if let Some(problem) = failure {
            return Err(problem);
        }
        outcome.map_err(|e| ops_problem(store, OpsError::Store(e), None))?;
        return Ok(Applied::Direct(built));
    };
    let target_id = built.id.to_string();
    let outcome = audit.apply(store, "task", &target_id, "task.create", |tx| {
        task_core::store::SqliteStore::create_task_tx(tx, &built, None, pinned_events.clone())?;
        write(tx).map_err(|e| task_core::chat::ChatError::Invalid(e.to_string()))?;
        Ok(serde_json::json!({
            "task_id": built.id.to_string(),
            "attachment_ids": attachment_ids,
        }))
    });
    let operation = outcome.map_err(|problem| failure.take().unwrap_or(problem))?;
    Ok(Applied::Audited(Box::new(operation)))
}

/// Pin `ids` to the task being created inside its creation transaction (ADR 2026-10-07
/// cos-live-fixes D1). Any invalid id fails the whole request with 422 `invalid_attachment`.
fn pin_attachments_tx(
    tx: &rusqlite::Transaction<'_>,
    ids: &[String],
    task: task_core::TaskId,
    thread: Option<&str>,
) -> Result<(), ApiProblem> {
    if ids.is_empty() {
        return Ok(());
    }
    let now = OffsetDateTime::now_utc();
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
    let owner = task.to_string();
    for id in ids {
        task_core::chat::attachments::add_ref_tx(tx, id, "task", &owner, now).map_err(internal)?;
    }
    Ok(())
}

// ---- 5. GET /tasks/{id} ----

pub(super) async fn task_detail(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let id = parse_task_id(&id)?;
    let ctx = state.inner.view.clone();
    let releases_dir = state.inner.integration_releases_dir.clone();
    let memory = state.memory_waits();
    let detail = state
        .blocking(move |store| {
            let mut detail =
                task_ops::view::task_detail(store, id, &ctx, &memory, OffsetDateTime::now_utc())
                    .map_err(|e| ops_problem(store, e, None))?;
            IntegrationAttacher::new(store, releases_dir.as_deref())?.attach(
                id,
                &mut detail.integrations,
                &mut detail.promotion,
            )?;
            let dir = files::task_local_dir(store, &ctx.workspace_roots, &detail.task)?;
            for run in &mut detail.runs {
                run.files = Some(files::run_files(&dir, &run.run_id));
            }
            Ok(detail)
        })
        .await?;
    Ok(json_response(StatusCode::OK, &detail))
}
