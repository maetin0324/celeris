//! 組織のノードの書き込みの共有 use-case（ADR 2026-11-18-org-write-shared-use-case D1）。
//!
//! plan（読み取りと検証だけ。DB を書かない）と、呼び出し元の transaction の中で書く [`apply`] を置く。
//! HTTP の route と監査付きの操作はこの module を一方向に呼び、transaction の開閉と response の形を持つ。
//! 依存は `task_core`・`crate::problem`・`crate::types` と外部 crate だけ。

use axum::http::StatusCode;
use rusqlite::Transaction;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use task_core::OrgNode;
use task_core::store::SqliteStore;
use time::OffsetDateTime;

use crate::problem::{ApiProblem, store_problem};
use crate::types::{OrgCreateBody, OrgPatchBody, ValidationError};

/// PATCH /org/{id}/browser-settings. Only browser-related profile fields are editable.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserSettingsPatch {
    pub allowed_domains: Option<Vec<String>>,
    /// ADR 2026-10-08-browser-click-download-approval-policy D1: business actions that need a
    /// per-run human approval in every task of this grant (default empty = click/download run
    /// without approval). `credential_use` is always approved regardless of this list.
    pub approval_actions: Option<Vec<task_core::BrowserAction>>,
    pub credential_policy_ids: Option<Vec<String>>,
    pub credential_identity_ids: Option<BTreeMap<String, String>>,
    /// ADR 2026-10-08-browser-prod-enablement D3: `true` は grant の `allowed_actions` に `credential_use` を
    /// 加え（欄が無い grant は Phase 1 の集合を実体化してから）、`false` は外す。使用は毎回人の承認（変わらない）。
    pub credential_use: Option<bool>,
    pub harnesses: Option<task_core::HarnessPrefs>,
    pub budget: Option<task_core::BudgetPrefs>,
}

/// One org write planned by a `plan_*` (validation and reads done), written by [`apply`].
pub(crate) enum OrgWrite {
    /// Upsert the node; `browser` also records the browser settings audit row.
    Upsert {
        node: Box<OrgNode>,
        browser: bool,
    },
    Delete(String),
}

impl OrgWrite {
    /// The id of the node this write targets.
    pub(crate) fn target_id(&self) -> &str {
        match self {
            OrgWrite::Upsert { node, .. } => &node.id,
            OrgWrite::Delete(id) => id,
        }
    }
}

/// What [`apply`] wrote: the stored node, or whether the deleted node's row existed.
pub(crate) enum OrgWritten {
    Node(Box<OrgNode>),
    Deleted { id: String, existed: bool },
}

impl OrgWritten {
    /// The stored node of an upsert (a delete has none).
    pub(crate) fn into_node(self) -> Result<OrgNode, ApiProblem> {
        match self {
            OrgWritten::Node(node) => Ok(*node),
            OrgWritten::Deleted { .. } => Err(ApiProblem::internal("org write returned no node")),
        }
    }
}

/// Write `write` in the caller's transaction (no commit here). A browser upsert records the
/// browser settings audit row with `actor`; other upserts record none.
pub(crate) fn apply(
    tx: &Transaction<'_>,
    write: &OrgWrite,
    actor: &str,
) -> Result<OrgWritten, ApiProblem> {
    match write {
        OrgWrite::Upsert { node, browser } => {
            SqliteStore::org_upsert_tx(tx, node, browser.then_some(actor))
                .map(|stored| OrgWritten::Node(Box::new(stored)))
                .map_err(store_problem)
        }
        OrgWrite::Delete(id) => Ok(OrgWritten::Deleted {
            id: id.clone(),
            existed: SqliteStore::org_delete_tx(tx, id).map_err(store_problem)?,
        }),
    }
}

/// `id` を組織のノードとして読む（存在しなければ 404）。
pub(crate) fn load_org_node(store: &SqliteStore, id: &str) -> Result<OrgNode, ApiProblem> {
    store
        .org_get(id)
        .map_err(store_problem)?
        .ok_or_else(|| ApiProblem::org_node_not_found(id))
}

fn parse_browser_settings_json<T: serde::de::DeserializeOwned>(
    value: Value,
    domains: Option<&Value>,
) -> Result<T, ApiProblem> {
    if let Some(domains) = domains {
        let valid = domains.as_array().is_some_and(|values| {
            values.iter().all(|value| {
                value
                    .as_str()
                    .is_some_and(|origin| task_core::AllowedOrigin::parse(origin).is_ok())
            })
        });
        if !valid {
            return Err(ApiProblem::validation(vec![ValidationError {
                field: Some("browser.allowed_domains".into()),
                message: "expected valid browser origins".into(),
            }]));
        }
    }
    serde_json::from_value(value)
        .map_err(|e| ApiProblem::bad_request(format!("invalid JSON body: {e}")))
}

/// `PATCH /org/{id}/browser-settings`: the node with only the browser-related profile changed.
pub(crate) fn plan_browser_settings(
    store: &SqliteStore,
    genres: &[task_core::GenreSpec],
    id: &str,
    raw: Value,
) -> Result<OrgWrite, ApiProblem> {
    let patch: BrowserSettingsPatch =
        parse_browser_settings_json(raw.clone(), raw.get("allowed_domains"))?;
    let known = task_core::known_harness_ids(genres);
    let mut node = load_org_node(store, id)?;
    let browser = node.profile.browser.as_mut().ok_or_else(|| {
        ApiProblem::validation(vec![ValidationError {
            field: Some("browser".into()),
            message: "node has no browser grant".into(),
        }])
    })?;
    if let Some(domains) = patch.allowed_domains {
        browser.allowed_domains = domains;
    }
    if let Some(actions) = patch.approval_actions {
        browser.approval_actions = actions;
    }
    if let Some(ids) = patch.credential_policy_ids {
        // ADR 2026-10-08 D3: 実在する site policy だけを grant に入れる。
        let mut missing = Vec::new();
        for id in &ids {
            if store
                .browser_site_policy_get(id)
                .map_err(store_problem)?
                .is_none()
            {
                missing.push(id.clone());
            }
        }
        if !missing.is_empty() {
            return Err(ApiProblem::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "unknown_site_policy",
                format!("unknown site policy: {}", missing.join(", ")),
            )
            .with_extra("policy_ids", missing));
        }
        browser.credential_policy_ids = ids;
    }
    if let Some(ids) = patch.credential_identity_ids {
        browser.credential_identity_ids = ids;
    }
    if let Some(enabled) = patch.credential_use {
        set_credential_use(browser, enabled);
    }
    if let Some(harnesses) = patch.harnesses {
        node.profile.harnesses = harnesses;
    }
    if let Some(budget) = patch.budget {
        node.profile.budget = budget;
    }
    task_core::validate_profile(&node.profile, &known).map_err(|e| {
        ApiProblem::validation(vec![ValidationError {
            field: Some("profile".into()),
            message: e.to_string(),
        }])
    })?;
    node.updated_at = OffsetDateTime::now_utc();
    Ok(OrgWrite::Upsert {
        node: Box::new(node),
        browser: true,
    })
}

/// grant の `allowed_actions` に `credential_use` を入れる・外す（他の action は変えない）。
fn set_credential_use(browser: &mut task_core::BrowserCapability, enabled: bool) {
    use task_core::BrowserAction;
    if !enabled && browser.allowed_actions.is_none() {
        // 欄が無い grant は Phase 1 の集合（`credential_use` を含まない）。
        return;
    }
    let actions = browser
        .allowed_actions
        .get_or_insert_with(|| BrowserAction::PHASE1.to_vec());
    actions.retain(|a| *a != BrowserAction::CredentialUse);
    if enabled {
        actions.push(BrowserAction::CredentialUse);
    }
}

/// 監査 L-1: 組織のノードの `genre` は設定の `[[genres]]` にあるものだけ（分野を 1 つも設定していない
/// 構成では検証しない。`POST /tasks` の `genre` と同じ規律。ADR-0027 D1）。
fn validate_genre(genres: &[task_core::GenreSpec], genre: Option<&str>) -> Result<(), ApiProblem> {
    let Some(genre) = genre else { return Ok(()) };
    if genres.is_empty() || genres.iter().any(|g| g.id == genre) {
        return Ok(());
    }
    Err(ApiProblem::validation(vec![ValidationError {
        field: Some("genre".into()),
        message: format!("unknown genre: {genre:?}"),
    }]))
}

/// ADR-0046 D1（Phase 59）: profile の決定的な検証（知らない道具・知らないハーネス・skill の綴り）。
/// ハーネスの集合は設定の `[[genres]]`（= `[[harnesses]]` の射影）＋ 組み込み。分野を 1 つも設定して
/// いない構成では検証しない（`validate_genre` と同じ規律）。
fn validate_profile_body(
    genres: &[task_core::GenreSpec],
    profile: Option<&task_core::Profile>,
) -> Result<(), ApiProblem> {
    let Some(profile) = profile else {
        return Ok(());
    };
    let known = task_core::known_harness_ids(genres);
    task_core::validate_profile(profile, &known).map_err(|e| {
        ApiProblem::validation(vec![ValidationError {
            field: Some("profile".into()),
            message: e.to_string(),
        }])
    })
}

/// `POST /org`: validate the body and build the new node (409 when the id exists).
pub(crate) fn plan_create(
    store: &SqliteStore,
    genres: &[task_core::GenreSpec],
    create: OrgCreateBody,
) -> Result<OrgWrite, ApiProblem> {
    // 監査 L-1: `genre` は `[[genres]]` にあるものだけ受ける（`genres` が空の設定では検証しない）。
    validate_genre(genres, create.genre.as_deref())?;
    validate_profile_body(genres, create.profile.as_ref())?;
    if store.org_get(&create.id).map_err(store_problem)?.is_some() {
        return Err(ApiProblem::org_node_exists(&create.id));
    }
    let now = OffsetDateTime::now_utc();
    Ok(OrgWrite::Upsert {
        node: Box::new(OrgNode {
            id: create.id,
            parent_id: create.parent_id,
            name: create.name,
            kind: create.kind,
            genre: create.genre,
            brief: create.brief.unwrap_or_default(),
            // ADR-0046 D1（Phase 59）: 省略時は空の profile。
            profile: create.profile.unwrap_or_default(),
            position: create.position.unwrap_or(0),
            created_at: now,
            updated_at: now,
        }),
        browser: false,
    })
}

/// `PATCH /org/{id}`: the node with the patch applied (profile is replaced whole).
pub(crate) fn plan_patch(
    store: &SqliteStore,
    genres: &[task_core::GenreSpec],
    id: &str,
    raw: Value,
) -> Result<OrgWrite, ApiProblem> {
    let domains = raw.pointer("/profile/browser/allowed_domains").cloned();
    let patch: OrgPatchBody = parse_browser_settings_json(raw, domains.as_ref())?;
    if let Some(genre) = &patch.genre {
        validate_genre(genres, genre.as_deref())?;
    }
    validate_profile_body(genres, patch.profile.as_ref())?;
    let mut node = load_org_node(store, id)?;
    if let Some(name) = patch.name {
        node.name = name;
    }
    if let Some(kind) = patch.kind {
        node.kind = kind;
    }
    if let Some(parent_id) = patch.parent_id {
        node.parent_id = Some(parent_id);
    }
    if let Some(genre) = patch.genre {
        node.genre = genre;
    }
    if let Some(brief) = patch.brief {
        node.brief = brief;
    }
    if let Some(position) = patch.position {
        node.position = position;
    }
    // ADR-0046 D1（Phase 59）: profile は**丸ごと差し替え**（書かなければ今のまま）。
    let browser = patch
        .profile
        .as_ref()
        .is_some_and(|profile| profile.browser != node.profile.browser);
    if let Some(profile) = patch.profile {
        node.profile = profile;
    }
    node.updated_at = OffsetDateTime::now_utc();
    Ok(OrgWrite::Upsert {
        node: Box::new(node),
        browser,
    })
}

/// `DELETE /org/{id}`: the delete of an existing node (404 when absent).
pub(crate) fn plan_delete(store: &SqliteStore, id: &str) -> Result<OrgWrite, ApiProblem> {
    load_org_node(store, id)?;
    Ok(OrgWrite::Delete(id.to_owned()))
}
