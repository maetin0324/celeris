//! run 観測証拠と全非対象の封印照合（propagation §8.6 の L2）。
//! 入力は既存の GET 応答だけ（`GET /tasks/{id}` の木、`GET /api/v1/org`、`GET /api/v1/config`）。daemon の L1 が通っても
//! 適用済みにはせず、欠落・null・不一致・再起動・非対象の drift はすべて拒否（`Refused`、集計しない）。
//! reload で変わり得る構成（役割・genre・harness 選択）は identity に出ないので、run ごとの selected 値と
//! 専用環境の固定（承認済みの instance_id・config_sha256）で立証できなければ fail closed とする。
use serde_json::Value;

use crate::{
    env_lock::CatalogRow,
    manifest::Config,
    variant::{VariantError, VariantRef, VariantSpec},
    variant_pin::{OrgSnapshot, check_b0_org, check_live_org},
};
use task_core::decision_log::{CampaignOrgPin, CampaignVariantRef};

/// 専用 daemon の固定（env check を通った起動時の値）。これが無い観測は固定構成を立証できないので拒否する。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixedEnv {
    pub instance_id: String,
    pub config_sha256: String,
}

/// 封印証拠から作る run 単位の期待値。
pub struct RunProofExpectation<'a> {
    pub variant: &'a VariantRef,
    pub spec: &'a VariantSpec,
    /// 承認済み sha256 で読んだ B0 の org（全 node の比較元）。
    pub b0_org: &'a OrgSnapshot,
    /// この試行の arm の config（lock `execution.configs`）。
    pub arm: &'a Config,
    /// env-lock のこの arm の catalog（source × model の許可集合）。
    pub catalog: &'a [CatalogRow],
    pub env: Option<&'a FixedEnv>,
}

fn refuse(m: impl Into<String>) -> VariantError {
    VariantError::Missing(m.into())
}

fn text<'v>(v: &'v Value, what: &str) -> Result<&'v str, VariantError> {
    match v.as_str() {
        Some(s) if !s.trim().is_empty() => Ok(s),
        _ => Err(refuse(format!("{what} is missing or null"))),
    }
}

/// 専用環境の固定: daemon の identity が承認済みの起動と同一（再起動・別 config を検出）で、上書きが無いこと。
fn check_env(env: Option<&FixedEnv>, config: &Value) -> Result<(), VariantError> {
    let env = env.ok_or_else(|| refuse("fixed dedicated environment is not proven"))?;
    let id = &config["identity"];
    if text(&id["instance_id"], "identity.instance_id")? != env.instance_id {
        return Err(VariantError::Baseline(
            "daemon instance_id differs from the approved one (restarted or another daemon)".into(),
        ));
    }
    if text(&id["config_sha256"], "identity.config_sha256")? != env.config_sha256 {
        return Err(VariantError::Baseline(
            "daemon config_sha256 differs from the approved measurement config".into(),
        ));
    }
    let o = &id["overrides"];
    let empty = o.is_null()
        || o.as_array().is_some_and(Vec::is_empty)
        || o.as_object().is_some_and(|m| m.is_empty());
    if !empty {
        return Err(VariantError::Baseline(
            "daemon was started with command-line overrides".into(),
        ));
    }
    Ok(())
}

fn check_identity(exp: &RunProofExpectation, detail: &Value) -> Result<(), VariantError> {
    let got = &detail["campaign"];
    if !got.is_object() {
        return Err(refuse("task campaign is missing in the response"));
    }
    let want_v = serde_json::to_value(CampaignVariantRef::from(exp.variant))
        .map_err(|e| VariantError::Baseline(e.to_string()))?;
    let want_o = serde_json::to_value(CampaignOrgPin::from(&exp.spec.org))
        .map_err(|e| VariantError::Baseline(e.to_string()))?;
    for (name, w) in [("variant", &want_v), ("variant_org", &want_o)] {
        if got[name].is_null() {
            return Err(refuse(format!("campaign.{name} is missing")));
        }
        if &got[name] != w {
            return Err(VariantError::Baseline(format!(
                "campaign.{name} differs from the sealed one"
            )));
        }
    }
    Ok(())
}

fn check_runs(exp: &RunProofExpectation, detail: &Value) -> Result<usize, VariantError> {
    let nt = &exp.spec.non_target;
    let mut n = 0;
    for r in detail["runs"].as_array().into_iter().flatten() {
        n += 1;
        let role = text(&r["role"], "run.role")?;
        let sel = &r["selection"];
        if sel["status"] != "selected" {
            return Err(refuse(format!("run {role}: selection is not recorded")));
        }
        let harness = text(&sel["harness_id"], "selected harness")?;
        let source = text(&sel["source_id"], "selected source")?;
        let model = text(&sel["model_id"], "selected model")?;
        let want_model = nt
            .role_models
            .get(role)
            .ok_or_else(|| refuse(format!("no sealed model for role {role}")))?;
        if model != want_model {
            return Err(VariantError::NonTargetDiff(format!(
                "run role {role}: model {model} != sealed {want_model}"
            )));
        }
        let allowed: Vec<String> = match role {
            "worker" => exp.arm.worker_harnesses(),
            "reviewer" => vec![nt.review.harness.clone()],
            "planner" => vec![exp.arm.harness.clone()],
            _ => return Err(refuse(format!("no sealed harness for role {role}"))),
        };
        if !allowed.iter().any(|h| h == harness) {
            return Err(VariantError::NonTargetDiff(format!(
                "run role {role}: harness {harness} not in sealed {allowed:?}"
            )));
        }
        if !exp
            .catalog
            .iter()
            .any(|c| c.source_id == source && c.model_id == model)
        {
            return Err(VariantError::NonTargetDiff(format!(
                "run role {role}: source {source} x model {model} is not in the sealed catalog"
            )));
        }
    }
    Ok(n)
}

/// 木の全 task 応答・稼働 org・daemon config を封印期待値に照合する。1 件でも欠落・不一致なら Err（集計しない）。
pub fn prove_runs(
    exp: &RunProofExpectation,
    task_details: &[Value],
    live_org: &Value,
    live_config: &Value,
) -> Result<(), VariantError> {
    check_env(exp.env, live_config)?;
    let live = OrgSnapshot::from_get_org(live_org)?;
    check_live_org(exp.spec, &live)?;
    check_b0_org(exp.spec, exp.b0_org, &live)?;
    let mut runs = 0;
    for d in task_details {
        check_identity(exp, d)?;
        runs += check_runs(exp, d)?;
    }
    if runs == 0 {
        return Err(refuse("no run was observed"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        variant::VariantId,
        variant_pin::tests::{b0, org_json, org_with, world},
    };
    use serde_json::json;

    struct World {
        spec: VariantSpec,
        vref: VariantRef,
        b0: OrgSnapshot,
        arm: Config,
        catalog: Vec<CatalogRow>,
        env: FixedEnv,
        org: Value,
        config: Value,
        task: Value,
    }

    fn setup(id: VariantId) -> World {
        let (m, _, spec) = world(id);
        let nodes: Vec<&str> = spec.org.nodes.iter().map(String::as_str).collect();
        let arm = spec.non_target.configs[0].clone();
        let model = |r: &str| spec.non_target.role_models[r].clone();
        let run = |role: &str, harness: &str| {
            json!({"role": role, "selection": {"status": "selected",
                "harness_id": harness, "source_id": "src", "model_id": model(role)}})
        };
        let task = json!({
            "campaign": {
                "variant": serde_json::to_value(CampaignVariantRef::from(m.variant.as_ref().unwrap())).unwrap(),
                "variant_org": serde_json::to_value(CampaignOrgPin::from(&spec.org)).unwrap(),
            },
            "runs": [run("worker", &arm.worker_harnesses()[0]), run("reviewer", &spec.non_target.review.harness)],
        });
        World {
            vref: m.variant.clone().unwrap(),
            b0: OrgSnapshot::from_get_org(&b0().org).unwrap(),
            catalog: ["worker", "reviewer"]
                .iter()
                .map(|r| CatalogRow {
                    model_id: model(r),
                    tier: "standard".into(),
                    source_id: "src".into(),
                })
                .collect(),
            env: FixedEnv {
                instance_id: "i-1".into(),
                config_sha256: "c".repeat(64),
            },
            org: org_json(&nodes),
            config: json!({"identity": {"instance_id": "i-1", "config_sha256": "c".repeat(64)}}),
            arm,
            task,
            spec,
        }
    }

    fn prove(w: &World, tasks: &[Value], env: bool) -> Result<(), VariantError> {
        let exp = RunProofExpectation {
            variant: &w.vref,
            spec: &w.spec,
            b0_org: &w.b0,
            arm: &w.arm,
            catalog: &w.catalog,
            env: env.then_some(&w.env),
        };
        prove_runs(&exp, tasks, &w.org, &w.config)
    }

    #[test]
    fn sealed_observation_passes_for_both_variants() {
        for id in [VariantId::SpecOn, VariantId::SpecOff] {
            let w = setup(id);
            prove(&w, std::slice::from_ref(&w.task), true).expect("proof");
        }
    }

    #[test]
    fn every_non_target_drift_missing_or_null_is_refused() {
        let w = setup(VariantId::SpecOn);
        let one = |edit: &dyn Fn(&mut Value)| {
            let mut t = w.task.clone();
            edit(&mut t);
            prove(&w, &[t], true).expect_err("must refuse")
        };
        one(&|t| t["campaign"] = Value::Null);
        one(&|t| t["campaign"]["variant"] = Value::Null);
        one(&|t| t["campaign"]["variant_org"]["coding_node"] = json!("other"));
        one(&|t| t["runs"] = json!([]));
        one(&|t| t["runs"][0]["selection"]["model_id"] = Value::Null);
        one(&|t| t["runs"][0]["selection"]["status"] = json!("pending"));
        one(&|t| t["runs"][0]["selection"]["model_id"] = json!("other-model"));
        one(&|t| t["runs"][1]["selection"]["model_id"] = json!("other-model"));
        one(&|t| t["runs"][0]["selection"]["harness_id"] = json!("builtin_smoke"));
        one(&|t| t["runs"][1]["selection"]["harness_id"] = json!("builtin_reviewer"));
        one(&|t| t["runs"][0]["selection"]["source_id"] = json!("other-source"));
        one(&|t| t["runs"][0]["role"] = json!("conversation"));
        // 木の 2 つ目の task（子）が矛盾していても拒否する。
        let mut child = w.task.clone();
        child["campaign"]["variant"]["content_sha256"] = json!("f".repeat(64));
        assert!(prove(&w, &[w.task.clone(), child], true).is_err());
    }

    #[test]
    fn live_org_drift_in_a_non_target_node_is_refused() {
        let mut w = setup(VariantId::SpecOn);
        let nodes: Vec<&str> = w.spec.org.nodes.iter().map(String::as_str).collect();
        for n in ["cos", "research"] {
            w.org = org_with(&nodes, Some((n, 2)));
            assert!(
                prove(&w, std::slice::from_ref(&w.task), true).is_err(),
                "{n}"
            );
        }
    }

    #[test]
    fn restart_override_or_unproven_fixed_environment_is_refused() {
        let mut w = setup(VariantId::SpecOn);
        let t = [w.task.clone()];
        assert!(prove(&w, &t, false).is_err(), "no fixed-env proof");
        let ok = w.config.clone();
        w.config["identity"]["instance_id"] = json!("i-2");
        assert!(prove(&w, &t, true).is_err(), "restart");
        w.config = ok.clone();
        w.config["identity"]["config_sha256"] = Value::Null;
        assert!(prove(&w, &t, true).is_err(), "null config hash");
        w.config = ok.clone();
        w.config["identity"]["config_sha256"] = json!("d".repeat(64));
        assert!(prove(&w, &t, true).is_err(), "other config");
        w.config = ok;
        w.config["identity"]["overrides"] = json!(["--x"]);
        assert!(prove(&w, &t, true).is_err(), "overrides");
    }

    struct Get(std::collections::BTreeMap<String, Value>);

    impl crate::celeris_exec::Transport for Get {
        fn request(
            &self,
            method: &str,
            path: &str,
            _: Option<&Value>,
        ) -> Result<crate::celeris_exec::HttpResponse, String> {
            assert_eq!(method, "GET", "the proof only reads");
            match self.0.get(path) {
                Some(b) => Ok(crate::celeris_exec::HttpResponse {
                    status: 200,
                    body: b.clone(),
                }),
                None => Err(format!("GET {path}: no route")),
            }
        }
    }

    #[test]
    fn executor_reads_only_existing_gets_and_refuses_with_refused_kind() {
        let w = setup(VariantId::SpecOn);
        let mut detail = w.task.clone();
        detail["task"] = json!({"status": "done"});
        let routes = |detail: Value, config: Value| {
            Get([
                ("/api/v1/tasks/T1".to_string(), detail),
                ("/api/v1/org".to_string(), w.org.clone()),
                ("/api/v1/config".to_string(), config),
            ]
            .into())
        };
        let exec = |g: Get| {
            crate::celeris_exec::CelerisExecutor::new(
                g,
                Default::default(),
                60,
                std::path::PathBuf::from("unused"),
            )
        };
        let exp = RunProofExpectation {
            variant: &w.vref,
            spec: &w.spec,
            b0_org: &w.b0,
            arm: &w.arm,
            catalog: &w.catalog,
            env: Some(&w.env),
        };
        let t = crate::executor::TaskRef {
            task_id: "T1".into(),
            ..Default::default()
        };
        exec(routes(detail.clone(), w.config.clone()))
            .prove_variant(&t, &exp)
            .expect("proved");
        let mut bad = w.config.clone();
        bad["identity"]["instance_id"] = json!("restarted");
        let e = exec(routes(detail, bad))
            .prove_variant(&t, &exp)
            .unwrap_err();
        assert_eq!(e.kind, crate::executor::ErrKind::Refused);
    }
}
