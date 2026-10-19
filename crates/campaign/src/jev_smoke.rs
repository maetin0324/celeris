//! Jev smoke の判定器: `routing_decided` の `lane_assist` と Jev の消費行から、実際に Jev が働いたかを判定する。
//! `real_api_verified`（本体側の定数）は証拠にしない: 証拠は classifier_status・費用・版だけ。実 API は呼ばない。
use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClassifierKind {
    JevOk,
    Fallback {
        reason: Option<String>,
    },
    NotAttempted,
    /// lane_assist 欄が無い・status が読めない。fallback とは区別する。
    Missing,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum Attribution {
    /// lane を上げていない、または basis が記録と整合している。
    Consistent,
    /// base→final の引き上げが fixed_rule だが、fixed 規則の lane が観測に無く、Jev と区別できない。
    Unverifiable,
    /// basis が事実と食い違う（理由つき）。
    Wrong(String),
}

/// 1 件の観測。`event` は `routing_decided` event（`record.lane_assist` または `lane_assist`）。
#[derive(Clone, Debug)]
pub struct Observation {
    pub task_id: String,
    pub event: Value,
    /// 許可 lane（空は制限なし）と天井。
    pub allowed: Vec<String>,
    pub max_lane: Option<String>,
    /// Jev の消費行があるか・費用（USD、欠測は None）。
    pub usage_present: bool,
    pub cost_usd: Option<f64>,
    /// Jev の版（model_snapshot）。欠測は None。
    pub version: Option<String>,
    /// fixed 規則だけが出す lane（観測できたときだけ）。
    pub fixed_lane: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Verdict {
    pub task_id: String,
    pub classifier: ClassifierKind,
    pub lane_in_limits: bool,
    pub cost_recorded: bool,
    pub version_recorded: bool,
    pub attribution: Attribution,
    /// jev_ok かつ費用・版が揃い、lane が許可内で、帰属が矛盾しない（記録の整合。live の証明ではない）。
    pub chain_ok: bool,
    pub violations: Vec<String>,
}

fn rank(l: &str) -> Option<u8> {
    match l {
        "cheap" => Some(0),
        "standard" => Some(1),
        "frontier" => Some(2),
        _ => None,
    }
}

fn lane_assist(event: &Value) -> Option<&Value> {
    event
        .pointer("/record/lane_assist")
        .or_else(|| event.get("lane_assist"))
        .filter(|v| v.is_object())
}

fn classifier(la: Option<&Value>) -> ClassifierKind {
    let Some(st) = la.and_then(|v| v.get("classifier_status")) else {
        return ClassifierKind::Missing;
    };
    match st.get("status").and_then(Value::as_str) {
        Some("jev_ok") => ClassifierKind::JevOk,
        Some("fell_back_to_fixed") => ClassifierKind::Fallback {
            reason: st.get("reason").and_then(Value::as_str).map(str::to_owned),
        },
        Some("not_attempted") => ClassifierKind::NotAttempted,
        _ => ClassifierKind::Missing,
    }
}

fn attribution(la: &Value, classifier: &ClassifierKind, fixed_lane: Option<&str>) -> Attribution {
    let s = |k: &str| la.get(k).and_then(Value::as_str);
    let (Some(base), Some(fin), Some(basis)) = (s("base_lane"), s("final_lane"), s("basis")) else {
        return Attribution::Wrong("base_lane/final_lane/basis が欠けている".into());
    };
    let (Some(b), Some(f)) = (rank(base), rank(fin)) else {
        return Attribution::Wrong(format!("未知の lane: {base} / {fin}"));
    };
    if basis == "classification_raise" && *classifier != ClassifierKind::JevOk {
        return Attribution::Wrong("classification_raise だが Jev の分類は成功していない".into());
    }
    if f < b {
        return Attribution::Wrong("final_lane が base_lane より低い（引き下げ権限は無い）".into());
    }
    if f == b {
        return if basis == "classification_raise" {
            Attribution::Wrong("lane が動いていないのに classification_raise".into())
        } else {
            Attribution::Consistent
        };
    }
    // 引き上げあり。
    match (basis, fixed_lane.and_then(rank)) {
        ("classification_raise", Some(x)) if x >= f => Attribution::Wrong(
            "fixed 規則だけで final_lane に届くのに classification_raise（Jev に帰属）".into(),
        ),
        ("classification_raise", _) => Attribution::Consistent,
        (_, Some(x)) if x >= f => Attribution::Consistent,
        (_, Some(_)) => Attribution::Wrong(format!(
            "fixed 規則だけでは final_lane に届かないのに basis={basis}（Jev の引き上げが fixed 側に帰属）"
        )),
        ("fixed_rule", None) if *classifier == ClassifierKind::JevOk => Attribution::Unverifiable,
        _ => Attribution::Consistent,
    }
}

pub fn judge(o: &Observation) -> Verdict {
    let la = lane_assist(&o.event);
    let classifier = classifier(la);
    let mut violations = Vec::new();
    let cost_recorded = o.usage_present && o.cost_usd.is_some();
    let version_recorded = o.version.as_deref().is_some_and(|v| !v.is_empty());
    let mut lane_in_limits = true;
    let mut attr = Attribution::Consistent;
    if let Some(la) = la {
        let fin = la.get("final_lane").and_then(Value::as_str).unwrap_or("");
        let permitted = (o.allowed.is_empty() || o.allowed.iter().any(|a| a == fin))
            && o.max_lane
                .as_deref()
                .is_none_or(|m| matches!((rank(fin), rank(m)), (Some(f), Some(m)) if f <= m));
        lane_in_limits = permitted && rank(fin).is_some();
        if !lane_in_limits {
            violations.push(format!("final_lane {fin} が許可範囲の外"));
        }
        attr = attribution(la, &classifier, o.fixed_lane.as_deref());
        if let Attribution::Wrong(w) = &attr {
            violations.push(format!("basis の帰属: {w}"));
        }
    } else {
        violations.push("lane_assist 欄が無い（decision_assisted の記録ではない）".into());
    }
    if classifier == ClassifierKind::JevOk {
        if !o.usage_present {
            violations.push("Jev の消費行が無い".into());
        } else if o.cost_usd.is_none() {
            violations.push("Jev の費用が欠測".into());
        }
        if !version_recorded {
            violations.push("Jev の版（model_snapshot）が欠測".into());
        }
    }
    let chain_ok = classifier == ClassifierKind::JevOk
        && cost_recorded
        && version_recorded
        && lane_in_limits
        && !matches!(attr, Attribution::Wrong(_));
    Verdict {
        task_id: o.task_id.clone(),
        classifier,
        lane_in_limits,
        cost_recorded,
        version_recorded,
        attribution: attr,
        chain_ok,
        violations,
    }
}

/// smoke 全体の判定。jev_ok が 1 件も無ければ「実経路は未確認」として失敗にする（fallback だけで合格にしない）。
pub fn check_smoke(obs: &[Observation]) -> (Vec<Verdict>, Result<(), Vec<String>>) {
    let verdicts: Vec<Verdict> = obs.iter().map(judge).collect();
    let mut errs: Vec<String> = verdicts
        .iter()
        .flat_map(|v| v.violations.iter().map(|m| format!("{}: {m}", v.task_id)))
        .collect();
    if !verdicts.iter().any(|v| v.chain_ok) {
        errs.push("jev_ok・費用・版が揃った観測が 1 件も無い（実経路は未確認）".into());
    }
    (verdicts, if errs.is_empty() { Ok(()) } else { Err(errs) })
}
