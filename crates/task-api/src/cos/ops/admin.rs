//! admin registry and shared-operation dispatch (ADR 2026-10-09 D3).

use super::super::operations::{DispatchEnv, Matched, OperationAudit};
use super::super::operations::{
    audited, decode_optional, decode_problem, path_param, unprocessable,
};
use crate::problem::ApiProblem;
use serde_json::Value;
use task_core::chat::CosOperation;
use task_core::store::SqliteStore;

/// Registered `(method, path, action)` operations.
pub(crate) const ALLOWED: &[(&str, &str, &str)] = &[
    (
        "PUT",
        "/api/v1/llm/models/{source}/{model_id}/override",
        "model_override.put",
    ),
    (
        "DELETE",
        "/api/v1/llm/models/{source}/{model_id}/override",
        "model_override.delete",
    ),
    ("POST", "/api/v1/org", "org.create"),
    ("PATCH", "/api/v1/org/{id}", "org.update"),
    ("DELETE", "/api/v1/org/{id}", "org.delete"),
    (
        "PATCH",
        "/api/v1/org/{id}/browser-settings",
        "org.browser_settings",
    ),
    ("POST", "/api/v1/org/{id}/skills", "org.skill_mount"),
    (
        "DELETE",
        "/api/v1/org/{id}/skills/{skill}",
        "org.skill_unmount",
    ),
    ("PUT", "/api/v1/skills/{name}", "skill.put"),
    ("DELETE", "/api/v1/skills/{name}", "skill.delete"),
    ("POST", "/api/v1/projects/{id}/repos", "repo.create"),
    ("PATCH", "/api/v1/repos/{id}", "repo.update"),
    ("DELETE", "/api/v1/repos/{id}", "repo.delete"),
    // ADR 2026-10-14-local-machine-settings D4: 先に一致させる（`{id}` の行より前）。
    (
        "PUT",
        "/api/v1/clusters/local/settings",
        "local.settings_put",
    ),
    (
        "PUT",
        "/api/v1/clusters/{id}/settings",
        "cluster.settings_put",
    ),
    (
        "POST",
        "/api/v1/llm/sources/{source_id}/accounts",
        "account.create",
    ),
    (
        "DELETE",
        "/api/v1/llm/sources/{source_id}/accounts/{account_id}",
        "account.delete",
    ),
    (
        "POST",
        "/api/v1/llm/sources/{source_id}/accounts/{account_id}/check",
        "account.check",
    ),
    (
        "PATCH",
        "/api/v1/llm/sources/{source_id}/accounts/{account_id}",
        "account.limit_set",
    ),
    (
        "POST",
        "/api/v1/llm/models/discover",
        "model_catalog.discover",
    ),
    ("POST", "/api/v1/notify/test", "notify.test"),
    ("POST", "/api/v1/reload", "daemon.reload"),
    ("POST", "/api/v1/replay", "daemon.replay"),
    (
        "POST",
        "/api/v1/releases/{sha12}/promote",
        "release.promote",
    ),
    ("POST", "/api/v1/cron-jobs", "cron_job.create"),
    ("PATCH", "/api/v1/cron-jobs/{id}", "cron_job.update"),
    ("DELETE", "/api/v1/cron-jobs/{id}", "cron_job.delete"),
    ("POST", "/api/v1/cron-jobs/{id}/pause", "cron_job.pause"),
    ("POST", "/api/v1/cron-jobs/{id}/resume", "cron_job.resume"),
    ("POST", "/api/v1/cron-jobs/{id}/run", "cron_job.run"),
];

/// ADR D2 exclusions: `(method, path, reason code and detail)`.
pub(crate) const EXCLUDED: &[(&str, &str, &str)] = &[
    (
        "POST",
        "/api/v1/terminals",
        "human_only: ADR 2026-10-27-web-terminal D3。端末は web に login し step-up した人だけが開く（主体 human:web 以外は 403）。",
    ),
    (
        "POST",
        "/api/v1/terminals/{id}/close",
        "human_only: ADR 2026-10-27-web-terminal D3。端末は web に login し step-up した人だけが開く（主体 human:web 以外は 403）。",
    ),
    (
        "PATCH",
        "/api/v1/config/fields",
        "human_only: ADR 2026-10-22-dynamic-config D5。config.toml の欄の変更（書き戻しと稼働中の差し替え）は人が設定画面で行う。",
    ),
    (
        "POST",
        "/api/v1/routing/stages",
        "human_only: ADR 2026-10-11-llm-source-model-routing-evaluation D3。enforce へ進めるのは人の承認だけ（戻すのは daemon）。",
    ),
    (
        "DELETE",
        "/api/v1/llm/sources/{source_id}/accounts/{account_id}/login",
        "secret_operations: 人の決定 secrets=exclude。秘密値を扱う系列を全て除外（取消・削除も含む）。",
    ),
    (
        "POST",
        "/api/v1/llm/sources/{source_id}/accounts/{account_id}/login",
        "secret_operations: 人の決定 secrets=exclude。秘密値を扱う系列を全て除外（取消・削除も含む）。",
    ),
    (
        "POST",
        "/api/v1/llm/sources/{source_id}/accounts/{account_id}/login/code",
        "secret_operations: 人の決定 secrets=exclude。秘密値を扱う系列を全て除外（取消・削除も含む）。",
    ),
    (
        "DELETE",
        "/api/v1/clusters/{id}/connect",
        "secret_operations: 人の決定 secrets=exclude。秘密値を扱う系列を全て除外（取消・削除も含む）。",
    ),
    (
        "POST",
        "/api/v1/clusters/{id}/connect",
        "secret_operations: 人の決定 secrets=exclude。秘密値を扱う系列を全て除外（取消・削除も含む）。",
    ),
    (
        "POST",
        "/api/v1/clusters/{id}/connect/code",
        "secret_operations: 人の決定 secrets=exclude。秘密値を扱う系列を全て除外（取消・削除も含む）。",
    ),
    (
        "DELETE",
        "/api/v1/secrets/{id}",
        "secret_operations: 人の決定 secrets=exclude。秘密値を扱う系列を全て除外（取消・削除も含む）。",
    ),
    (
        "PUT",
        "/api/v1/secrets/{id}",
        "secret_operations: 人の決定 secrets=exclude。秘密値を扱う系列を全て除外（取消・削除も含む）。",
    ),
];

const SKILL: &str = "/api/v1/skills/{name}";
const UNMOUNT: &str = "/api/v1/org/{id}/skills/{skill}";
const PROMOTE: &str = "/api/v1/releases/{sha12}/promote";
const OVERRIDE: &str = "/api/v1/llm/models/{source}/{model_id}/override";

/// The CoS form of a route whose domain request has no body: `null` or `{}` only.
fn require_empty_body(
    store: &SqliteStore,
    audit: &OperationAudit,
    kind: &str,
    path: &str,
    body: &Value,
) -> Result<(), ApiProblem> {
    if body.is_null() || *body == serde_json::json!({}) {
        return Ok(());
    }
    Err(audit.reject(
        store,
        kind,
        path,
        unprocessable("validation", "request.body must be empty"),
    ))
}

/// Run an async route effect from the blocking dispatch thread (`spawn_blocking` keeps the
/// runtime context, and the request task drives the runtime while it waits).
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Handle::current().block_on(future)
}

/// Run a C operation whose effect is a route's [`crate::cos::operations::Effect`].
fn external<T: serde::Serialize>(
    store: &SqliteStore,
    audit: &OperationAudit,
    target: (&str, &str),
    action: &str,
    effect: impl FnOnce() -> crate::cos::operations::Effect<T>,
) -> Result<CosOperation, ApiProblem> {
    audit.external(
        store,
        target.0,
        target.1,
        action,
        time::OffsetDateTime::now_utc(),
        || crate::cos::operations::effect_value(effect()),
    )
}

/// A body the route reads as optional JSON: `null` is `{}`.
fn decode_null(body: Value) -> Value {
    if body.is_null() {
        serde_json::json!({})
    } else {
        body
    }
}

fn empty_body(body: &Value) -> Result<(), ApiProblem> {
    if body.is_null() || *body == serde_json::json!({}) {
        Ok(())
    } else {
        Err(unprocessable("validation", "request.body must be empty"))
    }
}

pub(crate) fn dispatch(
    store: &SqliteStore,
    env: &DispatchEnv,
    audit: &OperationAudit,
    matched: Matched,
    path: &str,
    body: Value,
) -> Result<CosOperation, ApiProblem> {
    let decode = |error| decode_problem(store, audit, path, error);
    match matched.action {
        "model_override.put" => {
            let input = decode_optional(body).map_err(decode)?;
            audited(crate::model_catalog::put_override_op(
                store,
                &path_param(OVERRIDE, path, "{source}"),
                &path_param(OVERRIDE, path, "{model_id}"),
                input,
                Some(audit),
            )?)
        }
        "model_override.delete" => {
            require_empty_body(store, audit, "model_override", path, &body)?;
            audited(crate::model_catalog::delete_override_op(
                store,
                &path_param(OVERRIDE, path, "{source}"),
                &path_param(OVERRIDE, path, "{model_id}"),
                Some(audit),
            )?)
        }
        "org.create"
        | "org.update"
        | "org.delete"
        | "org.browser_settings"
        | "org.skill_mount"
        | "org.skill_unmount" => {
            use crate::handlers::org;
            let id = matched.id.clone().unwrap_or_default();
            let planned = match matched.action {
                "org.create" => serde_json::from_value(body)
                    .map_err(|e| unprocessable("validation", format!("request.body: {e}")))
                    .and_then(|input| org::plan_create(store, &env.genres, input)),
                "org.update" => org::plan_patch(store, &env.genres, &id, decode_null(body)),
                "org.browser_settings" => {
                    org::plan_browser_settings(store, &env.genres, &id, decode_null(body))
                }
                "org.skill_mount" => {
                    serde_json::from_value::<crate::skills::OrgSkillMountBody>(body)
                        .map_err(|e| unprocessable("validation", format!("request.body: {e}")))
                        .and_then(|input| {
                            crate::skills::plan_skill_mount(store, &id, &input.skill, true)
                        })
                }
                "org.skill_unmount" => empty_body(&body).and_then(|()| {
                    crate::skills::plan_skill_mount(
                        store,
                        &id,
                        &path_param(UNMOUNT, path, "{skill}"),
                        false,
                    )
                }),
                _ => empty_body(&body).and_then(|()| {
                    crate::handlers::load_org_node(store, &id)?;
                    Ok(org::OrgWrite::Delete(id.clone()))
                }),
            };
            let write = planned.map_err(|p| audit.reject(store, "org", &id, p))?;
            audited(org::org_commit(store, write, matched.action, Some(audit))?)
        }
        "skill.put" | "skill.delete" => {
            let name = path_param(SKILL, path, "{name}");
            let root = env
                .kb_root
                .clone()
                .map_err(|p| audit.reject(store, "skill", &name, p))?;
            if matched.action == "skill.put" {
                let input = serde_json::from_value(body).map_err(decode)?;
                audited(crate::skills::put_skill_op(
                    store,
                    &root,
                    &name,
                    input,
                    Some(audit),
                )?)
            } else {
                require_empty_body(store, audit, "skill", path, &body)?;
                audited(crate::skills::delete_skill_op(
                    store,
                    &root,
                    &name,
                    Some(audit),
                )?)
            }
        }
        "repo.create" => {
            let id = matched.id.unwrap_or_default();
            let repo = serde_json::from_value(body)
                .map_err(|e| unprocessable("validation", format!("request.body: {e}")))
                .and_then(|input| crate::repos::plan_create(&env.api, &id, input))
                .map_err(|p| audit.reject(store, "repo", &id, p))?;
            audited(crate::repos::create_repo_op(store, repo, Some(audit))?)
        }
        "repo.update" => {
            let id = matched.id.unwrap_or_default();
            let next = serde_json::from_value::<crate::types::RepoPatchBody>(body)
                .map_err(|e| unprocessable("validation", format!("request.body: {e}")))
                .and_then(|input| {
                    let repo_id = crate::repos::repo_id_of(&id)?;
                    let location = crate::repos::check_patch(&env.api, &input)?;
                    crate::repos::plan_patch(store, repo_id, input, location)
                })
                .map_err(|p| audit.reject(store, "repo", &id, p))?;
            audited(crate::repos::patch_repo_op(store, next, Some(audit))?)
        }
        "repo.delete" => {
            let id = matched.id.unwrap_or_default();
            let repo_id = empty_body(&body)
                .and_then(|()| crate::repos::repo_id_of(&id))
                .map_err(|p| audit.reject(store, "repo", &id, p))?;
            audited(crate::repos::delete_repo_op(store, repo_id, Some(audit))?)
        }
        "local.settings_put" => {
            let input = serde_json::from_value(body).map_err(decode)?;
            audited(crate::handlers::clusters::put_local_settings_op(
                &env.api,
                store,
                input,
                Some(audit),
            )?)
        }
        "cluster.settings_put" => {
            let id = matched.id.unwrap_or_default();
            let input = serde_json::from_value(body).map_err(decode)?;
            audited(crate::handlers::clusters::put_cluster_settings_op(
                store,
                &env.clusters,
                &id,
                input,
                Some(audit),
            )?)
        }
        "account.create" => {
            let input: crate::types::AccountCreateBody =
                serde_json::from_value(body).map_err(decode)?;
            let id = input.id.clone();
            let source = path_param(
                "/api/v1/llm/sources/{source_id}/accounts",
                path,
                "{source_id}",
            );
            let adapter = super::super::super::handlers::accounts::pool_adapter(&source)
                .ok_or_else(|| {
                    audit.reject(
                        store,
                        "account",
                        &id,
                        ApiProblem::bad_request("source does not have a managed account pool"),
                    )
                })?;
            // ADR 2026-10-08 account-concurrency-limits §(d): a re-created account must not inherit
            // an old stored limit. The clear goes through the daemon (or the store), so it runs after
            // the audit transaction, and only when this request created the directory.
            let created = std::cell::Cell::new(None::<String>);
            let op = audited(crate::cos::operations::file_op(
                store,
                Some(audit),
                "account",
                &id,
                matched.action,
                || {
                    let view =
                        crate::handlers::accounts::create_account_op(&env.api, adapter, &input)?;
                    created.set(Some(adapter.as_str().to_string()));
                    Ok(view)
                },
            )?)?;
            if let Some(adapter) = created.take().filter(|_| op.state == "applied") {
                block_on(crate::handlers::accounts::clear_stale_limit(
                    &env.api, &adapter, &id,
                ));
            }
            Ok(op)
        }
        "account.limit_set" => {
            let source = path_param(
                "/api/v1/llm/sources/{source_id}/accounts/{account_id}",
                path,
                "{source_id}",
            );
            let account = path_param(
                "/api/v1/llm/sources/{source_id}/accounts/{account_id}",
                path,
                "{account_id}",
            );
            let adapter = if let Some(a) =
                super::super::super::handlers::accounts::pool_adapter(&source)
            {
                if !env.api.inner.accounts_roots.contains_key(&a) {
                    return Err(audit.reject(
                        store,
                        "account",
                        &source,
                        ApiProblem::account_not_found(&source),
                    ));
                }
                a.as_str().to_string()
            } else if super::super::super::handlers::accounts::is_implicit_source(&env.api, &source)
            {
                "implicit".to_string()
            } else {
                return Err(audit.reject(
                    store,
                    "account",
                    &source,
                    ApiProblem::account_not_found(&source),
                ));
            };
            let account = if adapter == "implicit" && account != source {
                return Err(audit.reject(
                    store,
                    "account",
                    &account,
                    ApiProblem::account_not_found(&account),
                ));
            } else {
                account
            };
            let api = &env.api;
            external(store, audit, ("account", &account), matched.action, || {
                block_on(crate::handlers::accounts::set_account_limit_effect(
                    api, &adapter, &account, body,
                ))
            })
        }
        "account.delete" | "account.check" => {
            let template = "/api/v1/llm/sources/{source_id}/accounts/{account_id}";
            let source = path_param(template, path, "{source_id}");
            let id = path_param(template, path, "{account_id}");
            let adapter = super::super::super::handlers::accounts::resolve_pool_account(
                &env.api,
                &source,
                &id,
                if matched.action == "account.delete" {
                    "deleted"
                } else {
                    "checked"
                },
            )
            .map_err(|p| audit.reject(store, "account", &id, p))?;
            let api = &env.api;
            if matched.action == "account.delete" {
                external(store, audit, ("account", &id), matched.action, || {
                    block_on(crate::handlers::accounts::delete_account_effect(api, adapter, &id))
                        .map(|r| r.map(|()| serde_json::json!({"id": id, "adapter": adapter.as_str(), "removed": true})))
                })
            } else {
                external(store, audit, ("account", &id), matched.action, || {
                    block_on(crate::handlers::accounts::check_account_effect(
                        api, adapter, &id,
                    ))
                })
            }
        }
        "model_catalog.discover" => {
            let input: crate::model_catalog::DiscoverBody =
                decode_optional(body).map_err(decode)?;
            let target = input.source.clone().unwrap_or_else(|| "all".into());
            external(
                store,
                audit,
                ("model_catalog", &target),
                matched.action,
                || block_on(crate::model_catalog::discover_effect(&env.api, input)),
            )
        }
        "notify.test" => {
            require_empty_body(store, audit, "notify", path, &body)?;
            external(store, audit, ("notify", "discord"), matched.action, || {
                block_on(crate::notify::test_effect(&env.api))
            })
        }
        "daemon.reload" => {
            require_empty_body(store, audit, "daemon", path, &body)?;
            external(store, audit, ("daemon", "reload"), matched.action, || {
                block_on(crate::handlers::reload::reload_effect(&env.api))
            })
        }
        "daemon.replay" => {
            require_empty_body(store, audit, "daemon", path, &body)?;
            external(store, audit, ("daemon", "replay"), matched.action, || {
                crate::handlers::system::replay_effect(&env.api, store)
            })
        }
        "release.promote" => {
            let sha12 = path_param(PROMOTE, path, "{sha12}");
            require_empty_body(store, audit, "release", path, &body)?;
            let Some(source) = env.api.inner.releases.clone() else {
                return Err(audit.reject(
                    store,
                    "release",
                    &sha12,
                    ApiProblem::release_not_promotable(
                        "the [selfdeploy] section is not configured in config.toml",
                    ),
                ));
            };
            let thread_kind = store
                .chat_thread_get(&audit.ctx.thread_id)
                .ok()
                .flatten()
                .map(|thread| thread.kind);
            let actor = crate::releases::cos_deployment_actor(&audit.ctx.thread_id, thread_kind)
                .map_err(|reason| {
                    audit.reject(
                        store,
                        "release",
                        &sha12,
                        ApiProblem::release_not_promotable(reason),
                    )
                })?;
            external(store, audit, ("release", &sha12), matched.action, || {
                crate::releases::promote_effect(
                    source.as_ref(),
                    &sha12,
                    &crate::releases::DeploymentContext {
                        actor,
                        reason: audit.ctx.reason.clone(),
                        policy_ref: audit.ctx.policy_version.clone(),
                        task_refs: vec![],
                        decision_refs: vec![format!("cos_operation:{}", audit.ctx.operation_id)],
                    },
                )
            })
        }
        "cron_job.create" => {
            let input = serde_json::from_value(body).map_err(decode)?;
            audited(crate::cron_jobs::create_job_op(
                store,
                &env.roles,
                &env.genres,
                input,
                Some(audit),
            )?)
        }
        "cron_job.update" => {
            let input = decode_optional(body).map_err(decode)?;
            audited(crate::cron_jobs::update_job_op(
                store,
                &env.roles,
                &env.genres,
                &matched.id.unwrap_or_default(),
                input,
                Some(audit),
            )?)
        }
        "cron_job.delete" | "cron_job.pause" | "cron_job.resume" | "cron_job.run" => {
            require_empty_body(store, audit, "cron_job", path, &body)?;
            let key = matched.id.unwrap_or_default();
            match matched.action {
                "cron_job.delete" => {
                    audited(crate::cron_jobs::delete_job_op(store, &key, Some(audit))?)
                }
                "cron_job.pause" => {
                    audited(crate::cron_jobs::pause_job_op(store, &key, Some(audit))?)
                }
                "cron_job.resume" => {
                    audited(crate::cron_jobs::resume_job_op(store, &key, Some(audit))?)
                }
                _ => audited(crate::cron_jobs::run_job_op(
                    store,
                    &env.roles,
                    &env.genres,
                    &key,
                    Some(audit),
                    time::OffsetDateTime::now_utc(),
                )?),
            }
        }
        other => Err(ApiProblem::internal(format!(
            "registered CoS operation {other} has no implementation in admin"
        ))),
    }
}
