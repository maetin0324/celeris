//! admin registry and shared-operation dispatch (ADR 2026-10-09 D3).

use super::super::operations::{DispatchEnv, Matched, OperationAudit};
use super::super::operations::{
    audited, decode_optional, decode_problem, path_param, unprocessable,
};
use crate::problem::ApiProblem;
use serde_json::Value;
use task_core::chat::CosOperation;
use task_core::store::SqliteStore;

/// Registered `(method, path, action)` operations. The first match wins, so a literal segment
/// (`assignments/roles/{tier}`) is listed before the placeholder row it would also match.
pub(crate) const ALLOWED: &[(&str, &str, &str)] = &[
    (
        "PUT",
        "/api/v1/llm/models/assignments/roles/{tier}",
        "model_role.replace",
    ),
    (
        "PUT",
        "/api/v1/llm/models/assignments/{source}/{tier}",
        "model_assignment.put",
    ),
    (
        "DELETE",
        "/api/v1/llm/models/assignments/{source}/{tier}",
        "model_assignment.delete",
    ),
    (
        "POST",
        "/api/v1/llm/models/assignments/preview",
        "model_assignment.preview",
    ),
    (
        "POST",
        "/api/v1/llm/models/assignments/roles/{tier}/preview",
        "model_role.preview",
    ),
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
    (
        "PUT",
        "/api/v1/clusters/{id}/settings",
        "cluster.settings_put",
    ),
    ("POST", "/api/v1/providers", "provider.create"),
    ("PATCH", "/api/v1/providers/{id}", "provider.update"),
    ("DELETE", "/api/v1/providers/{id}", "provider.delete"),
    ("POST", "/api/v1/providers/{id}/check", "provider.check"),
    ("POST", "/api/v1/accounts", "account.create"),
    ("DELETE", "/api/v1/accounts/{id}", "account.delete"),
    ("POST", "/api/v1/accounts/{id}/check", "account.check"),
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
        "DELETE",
        "/api/v1/accounts/{id}/login",
        "secret_operations: 人の決定 secrets=exclude。秘密値を扱う系列を全て除外（取消・削除も含む）。",
    ),
    (
        "POST",
        "/api/v1/accounts/{id}/login",
        "secret_operations: 人の決定 secrets=exclude。秘密値を扱う系列を全て除外（取消・削除も含む）。",
    ),
    (
        "POST",
        "/api/v1/accounts/{id}/login/code",
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

/// Assigned mutations awaiting audited implementation (none left in admin).
pub(crate) const PENDING: &[(&str, &str)] = &[];

const ASSIGNMENT: &str = "/api/v1/llm/models/assignments/{source}/{tier}";
const ROLE: &str = "/api/v1/llm/models/assignments/roles/{tier}";
const SKILL: &str = "/api/v1/skills/{name}";
const UNMOUNT: &str = "/api/v1/org/{id}/skills/{skill}";
const PROMOTE: &str = "/api/v1/releases/{sha12}/promote";
const OVERRIDE: &str = "/api/v1/llm/models/{source}/{model_id}/override";

fn to_value<T: serde::Serialize>(value: &T) -> Result<Value, ApiProblem> {
    serde_json::to_value(value).map_err(|e| ApiProblem::internal(e.to_string()))
}

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

/// `{"adapter": "<adapter>"}` (default claude-code): the CoS form of the account routes'
/// `?adapter=` query, since an operation path carries no query.
#[derive(serde::Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct AccountTarget {
    #[serde(default)]
    adapter: Option<String>,
}

fn account_adapter(body: Value) -> Result<task_core::AccountAdapter, ApiProblem> {
    let target: AccountTarget = decode_optional(body)
        .map_err(|e| unprocessable("validation", format!("request.body: {e}")))?;
    match target.adapter {
        None => Ok(task_core::AccountAdapter::ClaudeCode),
        Some(raw) => task_core::AccountAdapter::parse(&raw).ok_or_else(|| {
            ApiProblem::bad_request("adapter must be claude-code, codex or opencode-go")
        }),
    }
}

/// Provider bodies with inline credential values (`env.OPENAI_API_KEY` …) are secret operations
/// (ADR D2, human decision secrets=exclude): refused with the body redacted from the record, so
/// the value never lands in the audit row. Credentials go through `/secrets` by a person.
fn refuse_inline_credentials(
    store: &SqliteStore,
    audit: &OperationAudit,
    path: &str,
    body: &Value,
) -> Result<(), ApiProblem> {
    let inline = body
        .get("env")
        .and_then(Value::as_object)
        .is_some_and(|env| {
            env.keys()
                .any(|k| task_core::model_routing::CREDENTIAL_KEYS.contains(&k.as_str()))
        });
    if !inline {
        return Ok(());
    }
    let mut redacted = audit.clone();
    redacted.payload["body"] = serde_json::json!({"redacted": true});
    Err(redacted.reject(
        store,
        "provider",
        path,
        unprocessable(
            "secret_operations",
            "inline credential values are secret operations; register them in /secrets (a person) and use credential_refs",
        ),
    ))
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
        "model_assignment.put" => {
            let input = serde_json::from_value(body).map_err(decode)?;
            crate::model_assignments::put_assignment_audited(
                store,
                &path_param(ASSIGNMENT, path, "{source}"),
                &path_param(ASSIGNMENT, path, "{tier}"),
                input,
                audit,
            )
        }
        "model_assignment.delete" => {
            require_empty_body(store, audit, "model_assignment", path, &body)?;
            crate::model_assignments::delete_assignment_audited(
                store,
                &path_param(ASSIGNMENT, path, "{source}"),
                &path_param(ASSIGNMENT, path, "{tier}"),
                audit,
            )
        }
        "model_assignment.preview" => {
            let input = serde_json::from_value(body).map_err(decode)?;
            let impact = crate::model_assignments::ImpactEnv::of(&env.api);
            let outcome = crate::model_assignments::preview_impact(store, &impact, &input)
                .and_then(|response| to_value(&response));
            audit.record(store, "model_assignment", path, matched.action, outcome)
        }
        "model_role.replace" | "model_role.preview" => {
            let input = serde_json::from_value(body).map_err(decode)?;
            let impact = crate::model_assignments::ImpactEnv::of(&env.api);
            crate::model_assignments::edit_role_audited(
                store,
                &impact,
                &path_param(ROLE, path, "{tier}"),
                input,
                matched.action == "model_role.preview",
                audit,
            )
        }
        "model_override.put" => {
            let input = decode_optional(body).map_err(decode)?;
            let routing = env.api.inner.routing_catalog.as_ref().map(|r| r.view());
            audited(crate::model_catalog::put_override_op(
                store,
                routing.as_ref(),
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
        "provider.create" | "provider.update" => {
            refuse_inline_credentials(store, audit, path, &body)?;
            let bytes =
                serde_json::to_vec(&body).map_err(|e| ApiProblem::internal(e.to_string()))?;
            let api = &env.api;
            if matched.action == "provider.create" {
                let input: crate::admin::ProviderCreateBody =
                    crate::handlers::providers::provider_json(&bytes, false)
                        .map_err(|p| audit.reject(store, "provider", path, p))?;
                let id = input.id.clone();
                audited(crate::cos::operations::file_op(
                    store,
                    Some(audit),
                    "provider",
                    &id,
                    matched.action,
                    || crate::handlers::providers::create_provider_op(api, input),
                )?)
            } else {
                let id = matched.id.unwrap_or_default();
                let input: crate::admin::ProviderPatchBody =
                    crate::handlers::providers::provider_json(&bytes, true)
                        .map_err(|p| audit.reject(store, "provider", &id, p))?;
                audited(crate::cos::operations::file_op(
                    store,
                    Some(audit),
                    "provider",
                    &id,
                    matched.action,
                    || crate::handlers::providers::patch_provider_op(api, &id, input),
                )?)
            }
        }
        "provider.delete" => {
            let id = matched.id.unwrap_or_default();
            require_empty_body(store, audit, "provider", path, &body)?;
            audited(crate::cos::operations::file_op(
                store,
                Some(audit),
                "provider",
                &id,
                matched.action,
                || {
                    crate::handlers::providers::delete_provider_op(&env.api, &id)
                        .map(|()| serde_json::json!({"id": id, "deleted": true}))
                },
            )?)
        }
        "account.create" => {
            let input: crate::types::AccountCreateBody =
                serde_json::from_value(body).map_err(decode)?;
            let id = input.id.clone();
            audited(crate::cos::operations::file_op(
                store,
                Some(audit),
                "account",
                &id,
                matched.action,
                || crate::handlers::accounts::create_account_op(&env.api, &input),
            )?)
        }
        "account.delete" | "account.check" => {
            let id = matched.id.unwrap_or_default();
            let adapter =
                account_adapter(body).map_err(|p| audit.reject(store, "account", &id, p))?;
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
        "provider.check" => {
            let id = matched.id.unwrap_or_default();
            require_empty_body(store, audit, "provider", path, &body)?;
            external(store, audit, ("provider", &id), matched.action, || {
                block_on(crate::handlers::providers::check_provider_effect(
                    &env.api, &id,
                ))
            })
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
                block_on(crate::handlers::providers::reload_effect(&env.api))
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
            external(store, audit, ("release", &sha12), matched.action, || {
                crate::releases::promote_effect(source.as_ref(), &sha12)
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
