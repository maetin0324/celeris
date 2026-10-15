use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

macro_rules! label_enum {
    ($(#[$m:meta])* $name:ident { $($var:ident => $s:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($var),+ }
        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$var),+];
            pub fn as_str(self) -> &'static str { match self { $($name::$var => $s),+ } }
            pub fn parse(s: &str) -> Option<Self> {
                match s.trim() { $($s => Some($name::$var),)+ _ => None }
            }
        }
    };
}

label_enum!(
    /// 手順確定度。
    Specificity {
        Specified => "specified",
        NeedsImplementation => "needs_implementation",
        NeedsInvestigationDesign => "needs_investigation_design",
        InsufficientInfo => "insufficient_info",
    }
);

label_enum!(
    /// 作業の性質。
    WorkNature {
        Implementation => "implementation",
        Investigation => "investigation",
        Design => "design",
        Documentation => "documentation",
        Verification => "verification",
        Operation => "operation",
    }
);

label_enum!(
    /// 直近失敗の種別（直近失敗があるときだけ聞く）。
    FailureKind {
        Environment => "environment",
        TestFailure => "test_failure",
        SpecAmbiguity => "spec_ambiguity",
        MissingDependency => "missing_dependency",
        Other => "other",
    }
);

/// 1 つの質問への回答。confidence・probability は「Jev の分類の確信」で、LLM の成功確率ではない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer<T> {
    pub value: T,
    pub probability: Option<f64>,
    pub classification_confidence: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Classification {
    pub specificity: Answer<Specificity>,
    pub work_nature: Answer<WorkNature>,
    pub failure_kind: Option<Answer<FailureKind>>,
    /// 応答が返した model（日付つき snapshot のことがある）。
    pub model_snapshot: Option<String>,
    /// `usage.cost`（USD）。無ければ None（0 扱いしない）。
    pub cost_usd: Option<f64>,
    pub model_id: String,
    pub schema_version: String,
    pub prompt_version: String,
}

pub const Q_SPECIFICITY: &str = "specificity";
pub const Q_WORK_NATURE: &str = "work_nature";
pub const Q_FAILURE_KIND: &str = "failure_kind";

fn choice<T: Copy>(instructions: &str, all: &[T], name: impl Fn(T) -> &'static str) -> Value {
    json!({
        "type": "choice",
        "instructions": instructions,
        "criteria": all.iter().map(|t| name(*t)).collect::<Vec<_>>(),
    })
}

/// 型付きの狭い質問。model ごとの成功率は聞かない。
pub fn questions(with_failure: bool) -> Value {
    let mut q = serde_json::Map::new();
    q.insert(
        Q_SPECIFICITY.into(),
        choice(
            "How completely does the instruction determine the procedure? specified: steps are given; \
             needs_implementation: goal is clear, code or content must be written; \
             needs_investigation_design: investigation or design decisions are needed first; \
             insufficient_info: the request cannot be acted on.",
            Specificity::ALL,
            Specificity::as_str,
        ),
    );
    q.insert(
        Q_WORK_NATURE.into(),
        choice(
            "What is the main nature of the work?",
            WorkNature::ALL,
            WorkNature::as_str,
        ),
    );
    if with_failure {
        q.insert(
            Q_FAILURE_KIND.into(),
            choice(
                "What kind of failure is the recent_failure?",
                FailureKind::ALL,
                FailureKind::as_str,
            ),
        );
    }
    Value::Object(q)
}

/// 応答の解釈失敗。値は含めない。
#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    Invalid,
    UnknownLabel,
}

struct RawAnswer {
    label: String,
    probability: Option<f64>,
    confidence: Option<f64>,
}

fn raw_answer(v: &Value) -> Option<RawAnswer> {
    let label = match v {
        Value::String(s) => s.clone(),
        Value::Object(o) => ["choice", "answer", "selected"]
            .iter()
            .find_map(|k| o.get(*k)?.as_str().map(String::from))?,
        _ => return None,
    };
    let probability = v
        .get("probabilities")
        .and_then(|p| p.get(&label))
        .and_then(Value::as_f64);
    Some(RawAnswer {
        label,
        probability,
        confidence: v.get("confidence").and_then(Value::as_f64),
    })
}

fn typed<T>(
    answers: &serde_json::Map<String, Value>,
    name: &str,
    parse: fn(&str) -> Option<T>,
) -> Result<Answer<T>, ParseError> {
    let raw = answers
        .get(name)
        .and_then(raw_answer)
        .ok_or(ParseError::Invalid)?;
    let value = parse(&raw.label).ok_or(ParseError::UnknownLabel)?;
    Ok(Answer {
        value,
        probability: raw.probability,
        classification_confidence: raw.confidence,
    })
}

pub struct Meta<'a> {
    pub model_id: &'a str,
    pub schema_version: &'a str,
    pub prompt_version: &'a str,
}

/// 応答 body を `Classification` にする。想定形: `{model, answers:{name:{choice,probabilities,confidence}}, usage:{cost}}`
/// （live 呼び出し前に公式の形を再確認する。進捗の未解決事項）。
pub fn parse_response(
    body: &str,
    with_failure: bool,
    meta: Meta<'_>,
) -> Result<Classification, ParseError> {
    let v: Value = serde_json::from_str(body).map_err(|_| ParseError::Invalid)?;
    let answers = v
        .get("answers")
        .and_then(Value::as_object)
        .ok_or(ParseError::Invalid)?;
    let failure_kind = if with_failure {
        Some(typed(answers, Q_FAILURE_KIND, FailureKind::parse)?)
    } else {
        None
    };
    Ok(Classification {
        specificity: typed(answers, Q_SPECIFICITY, Specificity::parse)?,
        work_nature: typed(answers, Q_WORK_NATURE, WorkNature::parse)?,
        failure_kind,
        model_snapshot: v.get("model").and_then(Value::as_str).map(String::from),
        cost_usd: v
            .get("usage")
            .and_then(|u| u.get("cost"))
            .and_then(Value::as_f64),
        model_id: meta.model_id.to_string(),
        schema_version: meta.schema_version.to_string(),
        prompt_version: meta.prompt_version.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> Meta<'static> {
        Meta {
            model_id: "typesafe/jev-1.14",
            schema_version: "s",
            prompt_version: "p",
        }
    }

    const OK: &str = r#"{"model":"typesafe/jev-1.14-20261001","answers":{
        "specificity":{"choice":"specified","probabilities":{"specified":0.8},"confidence":0.7},
        "work_nature":{"choice":"design"}},"usage":{"cost":0.0002}}"#;

    #[test]
    fn parses_success() {
        let c = parse_response(OK, false, meta()).unwrap_or_else(|_| unreachable!());
        assert_eq!(c.specificity.value, Specificity::Specified);
        assert_eq!(c.specificity.probability, Some(0.8));
        assert_eq!(c.work_nature.value, WorkNature::Design);
        assert_eq!(c.cost_usd, Some(0.0002));
        assert_eq!(
            c.model_snapshot.as_deref(),
            Some("typesafe/jev-1.14-20261001")
        );
    }

    #[test]
    fn missing_cost_is_none_not_zero() {
        let b = r#"{"answers":{"specificity":"specified","work_nature":"design"}}"#;
        let c = parse_response(b, false, meta()).unwrap_or_else(|_| unreachable!());
        assert_eq!(c.cost_usd, None);
    }

    #[test]
    fn invalid_and_unknown_are_distinguished() {
        assert_eq!(
            parse_response("not json", false, meta()).err(),
            Some(ParseError::Invalid)
        );
        assert_eq!(
            parse_response(r#"{"answers":{}}"#, false, meta()).err(),
            Some(ParseError::Invalid)
        );
        let unk = OK.replace("\"specified\",\"prob", "\"weird\",\"prob");
        assert_eq!(
            parse_response(&unk, false, meta()).err(),
            Some(ParseError::UnknownLabel)
        );
    }

    #[test]
    fn failure_question_only_when_asked() {
        assert!(questions(false).get(Q_FAILURE_KIND).is_none());
        assert!(questions(true).get(Q_FAILURE_KIND).is_some());
        assert_eq!(
            parse_response(OK, true, meta()).err(),
            Some(ParseError::Invalid)
        );
    }

    #[test]
    fn no_success_rate_question() {
        let q = questions(true).to_string().to_lowercase();
        assert!(!q.contains("success rate") && !q.contains("probability of success"));
    }
}
