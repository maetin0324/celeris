//! dry-run: 実 LLM を呼ばず、受入判定可否と token・資源の見積りを出す。
//! 実測が無いので値はすべて `estimated`。仮定は出力の `assumptions` に明示する。
use serde::Serialize;
use serde_json::{Value, json};

use routing_eval::manifest::Manifest as InputManifest;

use crate::manifest::{CampaignManifest, ConfigId};

/// 仮定（実測で置き換える前の初期値。値は根拠のない仮置きで、予算判定の目安にだけ使う）。
const CHARS_PER_TOKEN: u64 = 4;
const OUTPUT_TOKENS_PER_TRIAL: u64 = 20_000;
const CONTEXT_OVERHEAD_TOKENS: u64 = 30_000;
const JEV_TOKENS_PER_DECISION: u64 = 1_500;
const WALL_SECS_PER_TRIAL: u64 = 600;
const DISK_BYTES_PER_TRIAL: u64 = 2 << 30;

#[derive(Serialize)]
struct ConfigEstimate {
    config: ConfigId,
    trials: u64,
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
    label: &'static str,
}

/// 問題ごとの有効計画（`planned_configs`）で数える。runner・checker と同じ計画。
fn problems_for(m: &CampaignManifest, c: ConfigId) -> Vec<&String> {
    m.problems
        .iter()
        .filter(|p| m.planned_configs(p).contains(&c))
        .collect()
}

fn trials_for(m: &CampaignManifest, c: ConfigId) -> u64 {
    problems_for(m, c).len() as u64
}

pub fn estimate(m: &CampaignManifest, input: &InputManifest) -> Result<Value, String> {
    m.check_against_input(input)?;
    let prompt_chars = |id: &str| {
        input
            .problems
            .iter()
            .find(|p| p.id == id)
            .map_or(0, |p| p.prompt.chars().count() as u64)
    };
    let attempts = u64::from(m.repeats);
    let mut per_config = Vec::new();
    for c in m.active_arms() {
        let ids = problems_for(m, c);
        let base_in: u64 = ids
            .iter()
            .map(|id| prompt_chars(id) / CHARS_PER_TOKEN + CONTEXT_OVERHEAD_TOKENS)
            .sum();
        let trials = trials_for(m, c);
        let jev = if c == ConfigId::C {
            trials * JEV_TOKENS_PER_DECISION
        } else {
            0
        };
        let input_tokens = (base_in + jev) * attempts;
        let output_tokens = trials * OUTPUT_TOKENS_PER_TRIAL * attempts;
        per_config.push(ConfigEstimate {
            config: c,
            trials,
            input_tokens,
            output_tokens,
            total_tokens: input_tokens + output_tokens,
            label: "estimated",
        });
    }
    let total_tokens: u64 = per_config.iter().map(|c| c.total_tokens).sum();
    let total_trials: u64 = per_config.iter().map(|c| c.trials).sum::<u64>() * attempts;
    let parallel = u64::from(m.budget.max_parallel);
    let wall =
        total_trials.div_ceil(parallel) * WALL_SECS_PER_TRIAL.min(m.termination.trial_timeout_secs);
    // 試行ごとに workspace を分離するので disk は並列数ぶんだけ同時に占有する（後始末前提）。
    let disk = parallel.min(total_trials) * DISK_BYTES_PER_TRIAL;
    let mut reasons = Vec::new();
    if total_tokens > m.budget.max_tokens {
        reasons.push(format!(
            "estimated tokens {total_tokens} exceed budget {}",
            m.budget.max_tokens
        ));
    }
    if wall > m.budget.max_wall_secs {
        reasons.push(format!(
            "estimated wall {wall}s exceeds budget {}s",
            m.budget.max_wall_secs
        ));
    }
    if disk > m.budget.max_disk_bytes {
        reasons.push(format!(
            "estimated disk {disk}B exceeds budget {}B",
            m.budget.max_disk_bytes
        ));
    }
    Ok(json!({
        "schema": "celeris.campaign-dryrun/1",
        "campaign_id": m.campaign_id,
        "campaign_sha256": m.compute_sha256()?,
        "split": m.split,
        "llm_calls_made": 0,
        "acceptance": {
            "manifest_valid": true,
            "input_hash_and_split_checked": true,
            "within_budget": reasons.is_empty(),
            "reasons": reasons,
            "label": "estimated",
        },
        "tokens": {"per_config": per_config, "total": total_tokens, "label": "estimated"},
        "resources": {
            "total_trials": total_trials,
            "wall_secs": wall,
            "disk_bytes": disk,
            "parallel": parallel.min(total_trials),
            "label": "estimated",
        },
        "assumptions": {
            "chars_per_token": CHARS_PER_TOKEN,
            "output_tokens_per_trial": OUTPUT_TOKENS_PER_TRIAL,
            "context_overhead_tokens_per_trial": CONTEXT_OVERHEAD_TOKENS,
            "jev_tokens_per_decision": JEV_TOKENS_PER_DECISION,
            "wall_secs_per_trial": WALL_SECS_PER_TRIAL,
            "disk_bytes_per_trial": DISK_BYTES_PER_TRIAL,
            "note": "仮置き。実測（RunMetrics）が無い欠測値で、実性能・実費用ではない。",
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Split, fixtures::*};

    #[test]
    fn estimate_is_labeled_and_checks_budget() {
        let i = input(4, "dev");
        let m = campaign(&i, Split::Dev, 4);
        let v = estimate(&m, &i).unwrap();
        assert_eq!(v["llm_calls_made"], 0);
        assert_eq!(v["tokens"]["label"], "estimated");
        assert_eq!(v["resources"]["total_trials"], 4 + 4 + 2);
        assert_eq!(v["acceptance"]["within_budget"], true);
        let mut tight = m.clone();
        tight.budget.max_tokens = 1000;
        let v = estimate(&tight, &i).unwrap();
        assert_eq!(v["acceptance"]["within_budget"], false);
    }

    #[test]
    fn estimate_rejects_split_mixing() {
        let i = input(4, "dev");
        assert!(estimate(&campaign(&i, Split::Final, 4), &i).is_err());
    }
}
