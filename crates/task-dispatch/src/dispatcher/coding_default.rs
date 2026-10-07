//! ADR 2026-10-07（coding harness の既定）: `adapter_policy = "model_family"` のハーネスで、
//! `worker_hint.adapter` の無い run の provider 行を「行の model family の既定ハーネス」
//! （Claude → `claude-code`、それ以外・未知 → `pi`）に一致するものへ絞って選ぶ。
//!
//! 絞った候補（preferred）の中の順序（sticky → cheap local → pool score → 設定順）は
//! `select_provider_excluding` のまま変えない。preferred が無い・全滅なら、絞る前の候補で従来の
//! 選択をやり直す（fallback。理由は `LaneResolution.adapter_choice` に残す）。明示 adapter・
//! CoS・planner の run は対象外。

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use task_core::{AdapterChoice, FamilyDecision, LlmSourceRef, TaskId};

use super::provider_select::{LegacyProfile, ProviderPick};
use super::*;

/// 既定解決の設定（daemon が config から作り、`Dispatcher::set_coding_harness_default` で渡す）。
#[derive(Debug, Clone, Default)]
pub struct CodingHarnessDefault {
    /// `adapter_policy = "model_family"` のハーネス id（task の `genre`）。空なら既定解決はしない。
    pub harnesses: Vec<String>,
    /// provider id → 実効の LLM source（`Config::provider_llm_source`）。family 導出の 1 番目の材料。
    pub sources: HashMap<ProviderId, LlmSourceRef>,
    /// routing catalog の model id → `ModelProfile.family`（自由文字列。読む側が `ModelFamily::parse`）。
    pub model_families: HashMap<String, String>,
}

/// 既定解決をした選択の記録（routing audit の `LaneResolution` に写す）。
#[derive(Debug, Clone)]
pub(super) struct CodingDefaultOutcome {
    pub family: FamilyDecision,
    pub choice: AdapterChoice,
}

impl Dispatcher {
    /// ADR 2026-10-07: 既定解決の設定を差し替える（起動時と設定の再読込で呼ぶ）。
    pub fn set_coding_harness_default(&mut self, default: CodingHarnessDefault) {
        self.coding_default = default;
    }

    /// 行（profile）の family。LLM source → account pool → catalog の family の順（`derive_family`）。
    pub(super) fn row_family(&self, profile: &LegacyProfile) -> FamilyDecision {
        let id = &profile.deployment.id;
        let source = self
            .coding_default
            .sources
            .get(id)
            .cloned()
            .unwrap_or(LlmSourceRef::None);
        let row_adapter = profile
            .deployment
            .adapter_constraints
            .first()
            .map(String::as_str)
            .unwrap_or_default();
        // pool の adapter は pool を使う行だけ（プール無しの claude-code 行の source は LLM source で決まる）。
        let pool = self
            .account_pool_providers
            .contains(id)
            .then(|| self.pool_adapter_of(id, row_adapter))
            .flatten();
        task_core::derive_family(&source, pool, Some(&profile.model))
    }

    /// catalog の family（無ければ空文字）。`legacy_provider_profiles` の `ModelProfile.family` に入れる。
    pub(super) fn catalog_family(&self, model_id: &str) -> String {
        self.coding_default
            .model_families
            .get(model_id)
            .cloned()
            .unwrap_or_default()
    }

    /// `select_provider_excluding` に既定解決を重ねたもの。`active = false` なら従来と同じで
    /// 記録も `None`。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn select_provider_with_default(
        &mut self,
        active: bool,
        hint: &task_core::WorkerHint,
        now: Instant,
        task_id: TaskId,
        full: &mut HashSet<ProviderId>,
        sticky_session: Option<&NodeSession>,
        cos: bool,
        excluded: &HashSet<ProviderId>,
    ) -> (
        Option<ProviderPick>,
        task_core::model_routing::ProviderSelection,
        Option<CodingDefaultOutcome>,
    ) {
        if !active {
            let (picked, selection) = self.select_provider_excluding(
                hint,
                now,
                task_id,
                full,
                sticky_session,
                cos,
                true,
                excluded,
            );
            return (picked, selection, None);
        }
        let mut families: HashMap<ProviderId, FamilyDecision> = HashMap::new();
        let mut not_preferred: HashSet<ProviderId> = HashSet::new();
        for profile in self.legacy_provider_profiles(hint) {
            let family = self.row_family(&profile);
            let row_adapter = profile
                .deployment
                .adapter_constraints
                .first()
                .cloned()
                .unwrap_or_default();
            if !task_core::coding_harness::prefers(&row_adapter, family.family) {
                not_preferred.insert(profile.deployment.id.clone());
            }
            families.insert(profile.deployment.id.clone(), family);
        }
        let outcome_for = |provider: &ProviderId, choice: AdapterChoice| CodingDefaultOutcome {
            family: families.get(provider).copied().unwrap_or(FamilyDecision {
                family: task_core::ModelFamily::Unknown,
                basis: task_core::FamilyBasis::Unknown,
            }),
            choice,
        };
        let has_preferred = families.len() > not_preferred.len();
        if has_preferred {
            let mut narrowed = excluded.clone();
            narrowed.extend(not_preferred.iter().cloned());
            // preferred が全滅したときの「選べない」記録は fallback の判定に任せる（ここでは残さない）。
            let was_unroutable = self.unroutable.contains(&task_id);
            let was_warned = self.warned_unroutable.contains(&task_id);
            let (picked, selection) = self.select_provider_excluding(
                hint,
                now,
                task_id,
                full,
                // 継続 session は CoS の run だけ（CoS は既定解決の対象外なので、ここでは常に無い）。
                sticky_session,
                cos,
                true,
                &narrowed,
            );
            if let Some(pick) = picked {
                let outcome = outcome_for(&pick.1, AdapterChoice::Preferred);
                return (Some(pick), selection, Some(outcome));
            }
            if !was_unroutable {
                self.unroutable.remove(&task_id);
            }
            if !was_warned {
                self.warned_unroutable.remove(&task_id);
            }
        }
        let reason = if has_preferred {
            "no usable row for the family default harness (cooldown, full or excluded)"
        } else {
            "no provider row matches the family default harness"
        };
        let (picked, selection) = self.select_provider_excluding(
            hint,
            now,
            task_id,
            full,
            sticky_session,
            cos,
            true,
            excluded,
        );
        let outcome = picked.as_ref().map(|pick| {
            outcome_for(
                &pick.1,
                AdapterChoice::Fallback {
                    reason: reason.to_string(),
                },
            )
        });
        (picked, selection, outcome)
    }

    /// 明示 adapter の run の記録（選んだ行の family と `Explicit`）。
    pub(super) fn explicit_coding_outcome(
        &self,
        hint: &task_core::WorkerHint,
        provider: &ProviderId,
    ) -> Option<CodingDefaultOutcome> {
        hint.adapter.as_ref()?;
        let profile = self
            .legacy_provider_profiles(hint)
            .into_iter()
            .find(|p| &p.deployment.id == provider)?;
        Some(CodingDefaultOutcome {
            family: self.row_family(&profile),
            choice: AdapterChoice::Explicit,
        })
    }
}
