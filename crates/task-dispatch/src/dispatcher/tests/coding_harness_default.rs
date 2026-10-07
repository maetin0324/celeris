//! ADR 2026-10-07（coding harness の既定）: `adapter_policy = "model_family"` のハーネスで、
//! adapter の明示が無い run は行の model family の既定ハーネス（Claude → claude-code、他 → pi）の
//! 行を優先し、使えなければ絞る前の候補へ倒れる。実 CLI・ネットワークは使わない。

use super::*;
use crate::dispatcher::CodingHarnessDefault;
use task_core::model_routing::ProviderSelectionReason;
use task_core::{AdapterChoice, FamilyBasis, LlmSourceRef, ModelFamily};

/// 1 行: (provider id, adapter, model, LLM source, catalog の family)。
struct Row {
    id: &'static str,
    adapter: &'static str,
    model: &'static str,
    source: LlmSourceRef,
    family: Option<&'static str>,
}

fn row(
    id: &'static str,
    adapter: &'static str,
    model: &'static str,
    source: LlmSourceRef,
    family: Option<&'static str>,
) -> Row {
    Row {
        id,
        adapter,
        model,
        source,
        family,
    }
}

fn instant() -> Arc<dyn WorkerAdapter> {
    Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    })
}

/// `rows` を設定順に持ち、`coding` ハーネスを `adapter_policy = "model_family"` にした dispatcher。
fn family_dispatcher(store: Arc<dyn TaskStore>, rows: &[Row]) -> Dispatcher {
    let mut d = dispatcher(store, instant(), 4);
    d.policy = Box::new(StaticPolicy::new(
        rows.iter()
            .map(|r| ProviderSpec {
                id: r.id.into(),
                adapter: r.adapter.into(),
                tiers: vec![Tier::Frontier, Tier::Standard, Tier::Cheap],
                concurrency: 2,
                model: r.model.into(),
            })
            .collect(),
        Duration::from_secs(5),
    ));
    d.adapters = rows.iter().map(|r| (r.id.to_string(), instant())).collect();
    d.set_coding_harness_default(CodingHarnessDefault {
        harnesses: vec!["coding".into()],
        sources: rows
            .iter()
            .map(|r| (r.id.to_string(), r.source.clone()))
            .collect(),
        model_families: rows
            .iter()
            .filter_map(|r| r.family.map(|f| (r.model.to_string(), f.to_string())))
            .collect(),
    });
    d
}

fn hint(tier: Tier, adapter: Option<&str>) -> WorkerHint {
    WorkerHint {
        tier,
        adapter: adapter.map(str::to_string),
    }
}

/// 既定解決を掛けて 1 回選ぶ。戻り値は (adapter, provider, 記録)。
fn pick(
    d: &mut Dispatcher,
    active: bool,
    hint: &WorkerHint,
    full: &mut std::collections::HashSet<ProviderId>,
) -> (
    String,
    String,
    Option<crate::dispatcher::coding_default::CodingDefaultOutcome>,
    task_core::model_routing::ProviderSelection,
) {
    let (picked, selection, outcome) = d.select_provider_with_default(
        active,
        hint,
        Instant::now(),
        TaskId::new(),
        full,
        None,
        false,
        &std::collections::HashSet::new(),
    );
    let (adapter, provider, _) = picked.expect("a provider is picked");
    (adapter, provider, outcome, selection)
}

fn store() -> Arc<dyn TaskStore> {
    Arc::new(SqliteStore::open_in_memory().unwrap())
}

#[test]
fn coding_harness_default_claude_row_prefers_claude_code() {
    // opencode go が Claude を出す pi 行が先にあっても、Claude の既定は claude-code。
    let mut d = family_dispatcher(
        store(),
        &[
            row(
                "go-claude-pi",
                "pi",
                "opencode-go/claude-sonnet",
                LlmSourceRef::OpencodeGo,
                Some("claude"),
            ),
            row("cc", "claude-code", "", LlmSourceRef::ClaudeOauth, None),
        ],
    );
    let (adapter, provider, outcome, _) = pick(
        &mut d,
        true,
        &hint(Tier::Standard, None),
        &mut Default::default(),
    );
    assert_eq!((adapter.as_str(), provider.as_str()), ("claude-code", "cc"));
    let outcome = outcome.unwrap();
    assert_eq!(outcome.family.family, ModelFamily::Claude);
    assert_eq!(outcome.family.basis, FamilyBasis::LlmSource);
    assert_eq!(outcome.choice, AdapterChoice::Preferred);
}

#[test]
fn coding_harness_default_gpt_openai_prefers_pi() {
    let mut d = family_dispatcher(
        store(),
        &[
            row("codex", "codex", "gpt-5.5", LlmSourceRef::CodexOauth, None),
            row(
                "openai-pi",
                "pi",
                "gpt-5.5",
                LlmSourceRef::OpenaiCompatible("openai".into()),
                Some("gpt"),
            ),
        ],
    );
    let (adapter, provider, outcome, _) = pick(
        &mut d,
        true,
        &hint(Tier::Standard, None),
        &mut Default::default(),
    );
    assert_eq!((adapter.as_str(), provider.as_str()), ("pi", "openai-pi"));
    let outcome = outcome.unwrap();
    assert_eq!(outcome.family.family, ModelFamily::Gpt);
    assert_eq!(outcome.choice, AdapterChoice::Preferred);
}

#[test]
fn coding_harness_default_opencode_go_prefers_pi() {
    let mut d = family_dispatcher(
        store(),
        &[
            row(
                "go-acp",
                "acp",
                "opencode-go/glm-5",
                LlmSourceRef::OpencodeGo,
                Some("glm"),
            ),
            row(
                "go-pi",
                "pi",
                "opencode-go/glm-5",
                LlmSourceRef::OpencodeGo,
                Some("glm"),
            ),
        ],
    );
    let (adapter, provider, outcome, _) = pick(
        &mut d,
        true,
        &hint(Tier::Standard, None),
        &mut Default::default(),
    );
    assert_eq!((adapter.as_str(), provider.as_str()), ("pi", "go-pi"));
    let outcome = outcome.unwrap();
    assert_eq!(outcome.family.family, ModelFamily::Other);
    assert_eq!(outcome.family.basis, FamilyBasis::ModelProfile);
}

#[test]
fn coding_harness_default_deepseek_prefers_pi() {
    let deepseek = || LlmSourceRef::OpenaiCompatible("deepseek".into());
    let mut d = family_dispatcher(
        store(),
        &[
            row("ds-acp", "acp", "deepseek-v4", deepseek(), Some("deepseek")),
            row("ds-pi", "pi", "deepseek-v4", deepseek(), Some("deepseek")),
        ],
    );
    let (adapter, provider, outcome, _) = pick(
        &mut d,
        true,
        &hint(Tier::Standard, None),
        &mut Default::default(),
    );
    assert_eq!((adapter.as_str(), provider.as_str()), ("pi", "ds-pi"));
    assert!(!outcome.unwrap().family.family.is_claude());
}

#[test]
fn coding_harness_default_unknown_provider_prefers_pi() {
    // source も catalog も無い新しい provider（将来のモデル）は自動で非 Claude → pi。
    let mut d = family_dispatcher(
        store(),
        &[
            row(
                "new-acp",
                "acp",
                "brand-new-model",
                LlmSourceRef::Unknown,
                None,
            ),
            row(
                "new-pi",
                "pi",
                "brand-new-model",
                LlmSourceRef::Unknown,
                None,
            ),
        ],
    );
    let (adapter, provider, outcome, _) = pick(
        &mut d,
        true,
        &hint(Tier::Standard, None),
        &mut Default::default(),
    );
    assert_eq!((adapter.as_str(), provider.as_str()), ("pi", "new-pi"));
    let outcome = outcome.unwrap();
    assert_eq!(outcome.family.family, ModelFamily::Unknown);
    assert_eq!(outcome.family.basis, FamilyBasis::Unknown);
}

#[test]
fn coding_harness_default_falls_back_when_pi_unavailable() {
    let rows = [
        row(
            "go-acp",
            "acp",
            "opencode-go/glm-5",
            LlmSourceRef::OpencodeGo,
            None,
        ),
        row(
            "go-pi",
            "pi",
            "opencode-go/glm-5",
            LlmSourceRef::OpencodeGo,
            None,
        ),
    ];
    let mut d = family_dispatcher(store(), &rows);
    // この tick で pi の行は埋まっている → 絞る前の候補（acp）へ倒れる。
    let mut full: std::collections::HashSet<ProviderId> = ["go-pi".to_string()].into();
    let task_id = TaskId::new();
    let (picked, _, outcome) = d.select_provider_with_default(
        true,
        &hint(Tier::Standard, None),
        Instant::now(),
        task_id,
        &mut full,
        None,
        false,
        &std::collections::HashSet::new(),
    );
    let (adapter, provider, _) = picked.unwrap();
    assert_eq!((adapter.as_str(), provider.as_str()), ("acp", "go-acp"));
    assert!(matches!(
        outcome.unwrap().choice,
        AdapterChoice::Fallback { .. }
    ));
    // preferred の全滅で「選べない」記録を残さない（fallback で選べている）。
    assert!(!d.unroutable.contains(&task_id));

    // pi の行が cooldown（429 等）でも同じ。retry / cooldown の仕組みは変わらない。
    let mut d = family_dispatcher(store(), &rows);
    d.policy.report(
        "go-pi".to_string(),
        &ProviderOutcome::Throttled {
            retry_after: Duration::from_secs(600),
        },
    );
    let (adapter, _, outcome, _) = pick(
        &mut d,
        true,
        &hint(Tier::Standard, None),
        &mut Default::default(),
    );
    assert_eq!(adapter, "acp");
    assert!(matches!(
        outcome.unwrap().choice,
        AdapterChoice::Fallback { .. }
    ));

    // 既定ハーネスの行がそもそも無い（codex だけ）なら今どおり codex が走る。
    let mut d = family_dispatcher(
        store(),
        &[row(
            "codex",
            "codex",
            "gpt-5.5",
            LlmSourceRef::CodexOauth,
            None,
        )],
    );
    let (adapter, _, outcome, _) = pick(
        &mut d,
        true,
        &hint(Tier::Standard, None),
        &mut Default::default(),
    );
    assert_eq!(adapter, "codex");
    assert!(matches!(
        outcome.unwrap().choice,
        AdapterChoice::Fallback { .. }
    ));
}

#[test]
fn coding_harness_default_provider_order_policy_unchanged() {
    // 既定解決を掛けない（provider_order・明示 adapter・CoS・planner）なら従来どおり設定順。
    let mut d = family_dispatcher(
        store(),
        &[
            row("go-acp", "acp", "glm-5", LlmSourceRef::OpencodeGo, None),
            row("go-pi", "pi", "glm-5", LlmSourceRef::OpencodeGo, None),
        ],
    );
    let (adapter, provider, outcome, _) = pick(
        &mut d,
        false,
        &hint(Tier::Standard, None),
        &mut Default::default(),
    );
    assert_eq!((adapter.as_str(), provider.as_str()), ("acp", "go-acp"));
    assert!(outcome.is_none());
}

#[test]
fn coding_harness_default_cheap_local_qwen_pi_first() {
    // cheap lane のローカル優先（ADR-0132 付記 L2）は preferred の中で効く: Qwen の acp 行と pi 行が
    // 両方ローカルなら pi 行を選び、理由は従来どおり LocalPreferred。
    let qwen = || LlmSourceRef::OpenaiCompatible("qwen".into());
    let mut d = family_dispatcher(
        store(),
        &[
            row("cc", "claude-code", "", LlmSourceRef::ClaudeOauth, None),
            row("qwen-acp", "acp", "qwen3", qwen(), Some("qwen")),
            row("qwen-pi", "pi", "qwen3", qwen(), Some("qwen")),
        ],
    );
    let url = "http://qwen.invalid/v1";
    let local = |id: &str| crate::dispatcher::LocalProviderSpec {
        provider: id.into(),
        health: vec![crate::dispatcher::LocalHealthTarget {
            base_url: url.into(),
            bearer_token: None,
        }],
    };
    d.set_local_providers(vec![local("qwen-acp"), local("qwen-pi")]);
    d.set_local_provider_probe(Arc::new(|_, _| Reachability::Ok));
    let (adapter, provider, outcome, selection) = pick(
        &mut d,
        true,
        &hint(Tier::Cheap, None),
        &mut Default::default(),
    );
    assert_eq!((adapter.as_str(), provider.as_str()), ("pi", "qwen-pi"));
    assert_eq!(selection.reason, ProviderSelectionReason::LocalPreferred);
    assert_eq!(outcome.unwrap().family.family, ModelFamily::Qwen);
}

#[test]
fn coding_harness_default_pi_adapter_id_matches_worker() {
    assert_eq!(
        task_core::CodingHarness::Pi.adapter_id(),
        task_worker::PiAdapter::ID
    );
    assert_eq!(
        task_core::CodingHarness::ClaudeCode.adapter_id(),
        task_worker::ClaudeCodeAdapter::ID
    );
}

fn coding_task(ws: &std::path::Path, adapter: Option<&str>) -> Task {
    let mut task = new_task(
        ws,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        2,
    );
    task.genre = Some("coding".into());
    task.worker_hint = hint(Tier::Standard, adapter);
    task.routing = Some(task_core::TaskRouting {
        tier_source: task_core::TierSource::Human,
        ..Default::default()
    });
    task
}

fn go_rows() -> [Row; 2] {
    [
        row(
            "go-acp",
            "acp",
            "opencode-go/glm-5",
            LlmSourceRef::OpencodeGo,
            Some("glm"),
        ),
        row(
            "go-pi",
            "pi",
            "opencode-go/glm-5",
            LlmSourceRef::OpencodeGo,
            Some("glm"),
        ),
    ]
}

/// dispatch から routing audit まで: 実際に走った adapter（pi）と既定解決の記録が残る。
#[tokio::test]
async fn coding_harness_default_records_adapter_in_metrics() {
    let ws = tempfile::tempdir().unwrap();
    let store = store();
    let task = coding_task(ws.path(), None);
    store.insert(&task).unwrap();
    let mut d = family_dispatcher(store.clone(), &go_rows());
    d.tick().unwrap();
    let events = store.events_for(task.id).unwrap();
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, Event::WorkerStarted { adapter, .. } if adapter == "pi")),
        "{events:?}"
    );
    let record = routing_record(&events).expect("routing_decided");
    assert_eq!(record.resolution.adapter, "pi");
    let coding = record.resolution.coding_default.expect("coding_default");
    assert_eq!(coding.adapter_choice, AdapterChoice::Preferred);
    assert_eq!(coding.family, ModelFamily::Other);
    let audit = task_ops::routing_audit::task_routing_audit(store.as_ref(), task.id).unwrap();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].adapter.as_deref(), Some("pi"));
    assert_eq!(audit[0].harness.as_deref(), Some("coding"));
    assert_eq!(
        audit[0].coding_default.as_ref().map(|c| &c.adapter_choice),
        Some(&AdapterChoice::Preferred)
    );
}

/// 明示 adapter は既定解決より強い（acp を明示すれば pi 行があっても acp）。記録は Explicit。
#[tokio::test]
async fn coding_harness_default_explicit_adapter_wins() {
    let ws = tempfile::tempdir().unwrap();
    let store = store();
    let task = coding_task(ws.path(), Some("acp"));
    store.insert(&task).unwrap();
    let mut d = family_dispatcher(store.clone(), &go_rows());
    d.tick().unwrap();
    let events = store.events_for(task.id).unwrap();
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, Event::WorkerStarted { adapter, .. } if adapter == "acp")),
        "{events:?}"
    );
    let record = routing_record(&events).expect("routing_decided");
    let coding = record.resolution.coding_default.expect("coding_default");
    assert_eq!(coding.adapter_choice, AdapterChoice::Explicit);
}

/// `adapter_policy` を書いていないハーネスの task は従来どおり（設定順の acp）、記録も出さない。
#[tokio::test]
async fn coding_harness_default_other_harness_unchanged() {
    let ws = tempfile::tempdir().unwrap();
    let store = store();
    let mut task = coding_task(ws.path(), None);
    task.genre = Some("writing".into());
    store.insert(&task).unwrap();
    let mut d = family_dispatcher(store.clone(), &go_rows());
    d.tick().unwrap();
    let events = store.events_for(task.id).unwrap();
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, Event::WorkerStarted { adapter, .. } if adapter == "acp")),
        "{events:?}"
    );
    let record = routing_record(&events).expect("routing_decided");
    assert!(record.resolution.coding_default.is_none());
}
