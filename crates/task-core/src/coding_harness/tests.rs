use super::*;
use crate::accounts::AccountAdapter;
use crate::model_family::{FamilyBasis, ModelFamily, derive_family};
use crate::model_router::profiles::{Capabilities, ContextLimits, ModelProfile, Support};
use crate::provider_source::LlmSourceRef;

fn model(family: &str) -> ModelProfile {
    ModelProfile {
        id: format!("{family}-model"),
        revision: "r1".into(),
        family: family.into(),
        capabilities: Capabilities {
            tools: Support::Supported,
            structured_output: Support::Unknown,
            vision: Support::Unknown,
            streaming: Support::Supported,
            reasoning_efforts: vec![],
        },
        context_limits: ContextLimits {
            input: None,
            output: None,
            total: None,
        },
        quality: vec![],
        pricing: None,
        provenance: "test".into(),
    }
}

/// 行の材料から既定 adapter を決める（dispatch が行う合成と同じ）。
fn default_for(
    explicit: Option<&str>,
    source: &LlmSourceRef,
    pool: Option<AccountAdapter>,
    model: Option<&ModelProfile>,
) -> String {
    let family = derive_family(source, pool, model).family;
    coding_harness_default_adapter(explicit, family).to_owned()
}

#[test]
fn coding_harness_default_claude_uses_claude_code() {
    // claude_oauth の source、claude-code の pool、catalog の family のどれで決まっても Claude Code。
    assert_eq!(
        default_for(None, &LlmSourceRef::ClaudeOauth, None, None),
        "claude-code"
    );
    assert_eq!(
        default_for(
            None,
            &LlmSourceRef::Unknown,
            Some(AccountAdapter::ClaudeCode),
            None
        ),
        "claude-code"
    );
    let claude = model("Claude");
    assert_eq!(
        default_for(None, &LlmSourceRef::Celeris, None, Some(&claude)),
        "claude-code"
    );
    assert_eq!(
        CodingHarness::ClaudeCode.adapter_id(),
        AccountAdapter::ClaudeCode.as_str()
    );
}

#[test]
fn coding_harness_default_gpt_openai_uses_pi() {
    assert_eq!(
        default_for(None, &LlmSourceRef::CodexOauth, None, None),
        "pi"
    );
    assert_eq!(
        default_for(None, &LlmSourceRef::None, Some(AccountAdapter::Codex), None),
        "pi"
    );
    let gpt = model("gpt");
    assert_eq!(
        default_for(
            None,
            &LlmSourceRef::OpenaiCompatible("openai".into()),
            None,
            Some(&gpt)
        ),
        "pi"
    );
    let d = derive_family(&LlmSourceRef::CodexOauth, None, None);
    assert_eq!(d.family, ModelFamily::Gpt);
    assert_eq!(d.basis, FamilyBasis::LlmSource);
}

#[test]
fn coding_harness_default_opencode_go_uses_pi() {
    // opencode go の pool は model 次第。catalog に family が無くても、非 Claude の family でも Pi。
    let go = Some(AccountAdapter::OpencodeGo);
    assert_eq!(default_for(None, &LlmSourceRef::OpencodeGo, go, None), "pi");
    let kimi = model("kimi");
    assert_eq!(
        default_for(None, &LlmSourceRef::OpencodeGo, go, Some(&kimi)),
        "pi"
    );
    let qwen = model("qwen");
    assert_eq!(
        default_for(None, &LlmSourceRef::OpencodeGo, go, Some(&qwen)),
        "pi"
    );
    // opencode go が Claude 系 model を出す行は catalog の family で Claude Code に戻る。
    let claude = model("claude");
    let d = derive_family(&LlmSourceRef::OpencodeGo, go, Some(&claude));
    assert_eq!(d.family, ModelFamily::Claude);
    assert_eq!(d.basis, FamilyBasis::ModelProfile);
}

#[test]
fn coding_harness_default_deepseek_uses_pi() {
    let deepseek = model("deepseek");
    let d = derive_family(
        &LlmSourceRef::OpenaiCompatible("deepseek".into()),
        None,
        Some(&deepseek),
    );
    assert_eq!(d.family, ModelFamily::Other);
    assert!(!d.family.is_claude());
    assert_eq!(
        coding_harness_default_adapter(None, d.family),
        CodingHarness::PI_ADAPTER_ID
    );
    assert_eq!(
        default_for(None, &LlmSourceRef::OpencodeGo, None, Some(&deepseek)),
        "pi"
    );
}

#[test]
fn coding_harness_default_unknown_provider_uses_pi() {
    // 材料が何も無い行・llm-proxy（lane 次第）・未知の新しい family はすべて非 Claude。
    let d = derive_family(&LlmSourceRef::Unknown, None, None);
    assert_eq!(d.family, ModelFamily::Unknown);
    assert_eq!(d.basis, FamilyBasis::Unknown);
    assert_eq!(coding_harness_default_adapter(None, d.family), "pi");
    assert_eq!(default_for(None, &LlmSourceRef::Celeris, None, None), "pi");
    // relay の識別子や model id に Claude と書かれていても family には使わない。
    let mut unclassified = model("");
    unclassified.id = "claude-model".into();
    assert_eq!(
        default_for(
            None,
            &LlmSourceRef::OpenaiCompatible("claude".into()),
            None,
            Some(&unclassified)
        ),
        "pi"
    );
    let empty = model("  ");
    assert_eq!(
        default_for(None, &LlmSourceRef::None, None, Some(&empty)),
        "pi"
    );
    let future = model("some-future-family");
    assert_eq!(
        default_for(
            None,
            &LlmSourceRef::OpenaiCompatible("new-vendor".into()),
            None,
            Some(&future)
        ),
        "pi"
    );
}

#[test]
fn coding_harness_default_respects_explicit_adapter() {
    // 明示 adapter は family に関わらずそのまま返る。
    assert_eq!(
        default_for(Some("codex"), &LlmSourceRef::ClaudeOauth, None, None),
        "codex"
    );
    assert_eq!(
        default_for(Some("claude-code"), &LlmSourceRef::CodexOauth, None, None),
        "claude-code"
    );
    assert_eq!(
        coding_harness_default_adapter(Some("acp"), ModelFamily::Unknown),
        "acp"
    );
    // 指定の妥当性は config 層で検証する。ここでは加工せず尊重する。
    assert_eq!(
        coding_harness_default_adapter(Some(" "), ModelFamily::Claude),
        " "
    );
    assert_eq!(
        coding_harness_default_adapter(Some(""), ModelFamily::Gpt),
        ""
    );
}

#[test]
fn coding_harness_default_parse_is_case_insensitive() {
    assert_eq!(ModelFamily::parse("CLAUDE"), ModelFamily::Claude);
    assert_eq!(ModelFamily::parse(" Gpt "), ModelFamily::Gpt);
    assert_eq!(ModelFamily::parse("QwEn"), ModelFamily::Qwen);
    assert_eq!(ModelFamily::parse("deepseek"), ModelFamily::Other);
    assert_eq!(ModelFamily::parse(""), ModelFamily::Unknown);
    // 名前の一部一致では Claude にしない（直書きの contains を避ける）。
    assert_eq!(ModelFamily::parse("not-claude"), ModelFamily::Other);
}

#[test]
fn coding_harness_default_source_wins_over_pool_and_profile() {
    let claude = model("claude");
    let d = derive_family(
        &LlmSourceRef::CodexOauth,
        Some(AccountAdapter::ClaudeCode),
        Some(&claude),
    );
    assert_eq!(d.family, ModelFamily::Gpt);
    assert_eq!(d.basis, FamilyBasis::LlmSource);
    let d = derive_family(
        &LlmSourceRef::OpencodeGo,
        Some(AccountAdapter::Codex),
        Some(&claude),
    );
    assert_eq!(d.basis, FamilyBasis::AccountPool);
    assert_eq!(d.family, ModelFamily::Gpt);
}

#[test]
fn coding_harness_default_prefers_matches_family_harness() {
    assert!(prefers("claude-code", ModelFamily::Claude));
    assert!(!prefers("pi", ModelFamily::Claude));
    assert!(prefers("pi", ModelFamily::Gpt));
    assert!(prefers("pi", ModelFamily::Unknown));
    assert!(!prefers("codex", ModelFamily::Gpt));
    assert!(!prefers("acp", ModelFamily::Qwen));
    assert_eq!(AdapterPolicy::default(), AdapterPolicy::ProviderOrder);
}

#[test]
fn coding_harness_default_serde_names() {
    assert_eq!(
        serde_json::to_string(&CodingHarness::ClaudeCode)
            .ok()
            .as_deref(),
        Some("\"claude-code\"")
    );
    assert_eq!(
        serde_json::to_string(&AdapterPolicy::ModelFamily)
            .ok()
            .as_deref(),
        Some("\"model_family\"")
    );
    assert_eq!(
        serde_json::to_string(&ModelFamily::Unknown).ok().as_deref(),
        Some("\"unknown\"")
    );
}
