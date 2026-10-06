use super::*;
use task_dispatch::accounts::{AccountBook, AccountCooldown, AccountCooldownReason};

fn dirs(ids: &[&str]) -> Vec<AccountDir> {
    ids.iter()
        .map(|id| AccountDir {
            id: id.to_string(),
            dir: PathBuf::from(format!("/accounts/{id}")),
            logged_in: true,
        })
        .collect()
}

fn no_in_use(_: &str) -> usize {
    0
}

#[test]
fn select_from_pool_prefers_the_unused_account_and_ties_break_by_id() {
    let dirs = dirs(&["bravo", "alpha"]);
    let book = AccountBook::new_in_memory();
    let input = PoolInput {
        dirs: &dirs,
        book: &book,
        in_use: &no_in_use,
        max_concurrent_per_account: 4,
    };
    let picked = select_from_pool(SourceKind::Claude, &input, 1_000).unwrap();
    assert_eq!(picked.account_id, "alpha");
    assert_eq!(picked.source, SourceKind::Claude);
}

#[test]
fn rank_pool_orders_all_eligible_candidates_and_drops_excluded_ones() {
    let mut dirs = dirs(&["a", "b", "c"]);
    dirs[2].logged_in = false; // c: excluded
    let mut book = AccountBook::new_in_memory();
    book.set_cooldown(
        "a",
        AccountCooldown {
            until: 9_999,
            reason: AccountCooldownReason::Throttled,
        },
        0,
    ); // a: excluded (cooldown)
    let input = PoolInput {
        dirs: &dirs,
        book: &book,
        in_use: &no_in_use,
        max_concurrent_per_account: 4,
    };
    let ranked = rank_pool(SourceKind::Claude, &input, 1_000);
    assert_eq!(
        ranked.into_iter().map(|s| s.account_id).collect::<Vec<_>>(),
        vec!["b"]
    );
}

#[test]
fn select_from_pool_skips_cooldown_and_not_logged_in() {
    let mut dirs = dirs(&["a", "b"]);
    dirs[1].logged_in = false;
    let mut book = AccountBook::new_in_memory();
    book.set_cooldown(
        "a",
        AccountCooldown {
            until: 9_999,
            reason: AccountCooldownReason::Throttled,
        },
        0,
    );
    let input = PoolInput {
        dirs: &dirs,
        book: &book,
        in_use: &no_in_use,
        max_concurrent_per_account: 4,
    };
    assert_eq!(select_from_pool(SourceKind::Claude, &input, 1_000), None);
}

#[test]
fn select_across_pools_picks_the_higher_scoring_pool() {
    let claude_dirs = dirs(&["c1"]);
    let codex_dirs = dirs(&["g1"]);
    let claude_book = AccountBook::new_in_memory();
    let codex_book = AccountBook::new_in_memory();
    let claude_input = PoolInput {
        dirs: &claude_dirs,
        book: &claude_book,
        in_use: &|_| 3usize, // in_use penalty で claude を不利にする
        max_concurrent_per_account: 4,
    };
    let codex_input = PoolInput {
        dirs: &codex_dirs,
        book: &codex_book,
        in_use: &no_in_use,
        max_concurrent_per_account: 4,
    };
    let picked = select_across_pools(Some(&claude_input), Some(&codex_input), 1_000).unwrap();
    assert_eq!(picked.source, SourceKind::Gpt);
    assert_eq!(picked.account_id, "g1");
}

#[test]
fn select_across_pools_ties_prefer_claude_by_config_order() {
    let claude_dirs = dirs(&["c1"]);
    let codex_dirs = dirs(&["g1"]);
    let claude_book = AccountBook::new_in_memory();
    let codex_book = AccountBook::new_in_memory();
    let claude_input = PoolInput {
        dirs: &claude_dirs,
        book: &claude_book,
        in_use: &no_in_use,
        max_concurrent_per_account: 4,
    };
    let codex_input = PoolInput {
        dirs: &codex_dirs,
        book: &codex_book,
        in_use: &no_in_use,
        max_concurrent_per_account: 4,
    };
    let picked = select_across_pools(Some(&claude_input), Some(&codex_input), 1_000).unwrap();
    assert_eq!(picked.source, SourceKind::Claude);
}

#[test]
fn select_across_pools_falls_back_to_the_only_available_pool() {
    let codex_dirs = dirs(&["g1"]);
    let codex_book = AccountBook::new_in_memory();
    let codex_input = PoolInput {
        dirs: &codex_dirs,
        book: &codex_book,
        in_use: &no_in_use,
        max_concurrent_per_account: 4,
    };
    let picked = select_across_pools(None, Some(&codex_input), 1_000).unwrap();
    assert_eq!(picked.source, SourceKind::Gpt);
}

#[test]
fn rank_across_pools_returns_a_full_fallback_order() {
    let claude_dirs = dirs(&["c1", "c2"]);
    let codex_dirs = dirs(&["g1"]);
    let claude_book = AccountBook::new_in_memory();
    let codex_book = AccountBook::new_in_memory();
    let claude_input = PoolInput {
        dirs: &claude_dirs,
        book: &claude_book,
        in_use: &no_in_use,
        max_concurrent_per_account: 4,
    };
    let codex_input = PoolInput {
        dirs: &codex_dirs,
        book: &codex_book,
        in_use: &no_in_use,
        max_concurrent_per_account: 4,
    };
    let ranked = rank_across_pools(Some(&claude_input), Some(&codex_input), 1_000);
    let ids: Vec<_> = ranked.iter().map(|s| s.account_id.as_str()).collect();
    assert_eq!(ids, vec!["c1", "c2", "g1"]);
}

fn relay(id: &str) -> OpenAiCompatibleConfig {
    OpenAiCompatibleConfig {
        id: id.to_string(),
        base_url: format!("http://127.0.0.1:0/{id}"),
        api_key: None,
        enabled: true,
    }
}

#[test]
fn pick_relay_returns_the_first_reachable_in_config_order() {
    let sources = vec![relay("a"), relay("b")];
    let picked = pick_relay(&sources, |id| id == "b");
    assert_eq!(picked.unwrap().id, "b");
    let none = pick_relay(&sources, |_| false);
    assert!(none.is_none());
}

#[test]
fn pick_relay_skips_disabled_sources() {
    let mut sources = vec![relay("a"), relay("b")];
    sources[0].enabled = false;
    let picked = pick_relay(&sources, |_| true);
    assert_eq!(picked.unwrap().id, "b");
}

#[test]
fn rank_relays_keeps_config_order_among_reachable_sources() {
    let sources = vec![relay("a"), relay("b"), relay("c")];
    let ranked = rank_relays(&sources, |id| id != "b");
    assert_eq!(
        ranked
            .into_iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "c"]
    );
}

#[test]
fn cheap_only_qwen_tier_selection_ignores_legacy_mappings() {
    let models = HashMap::from([
        (Tier::Frontier, "legacy-frontier".to_string()),
        (Tier::Standard, "legacy-standard".to_string()),
        (Tier::Cheap, "qwen3.8-27b".to_string()),
    ]);
    assert_eq!(qwen_tier_model(&models, Tier::Frontier), None);
    assert_eq!(qwen_tier_model(&models, Tier::Standard), None);
    assert_eq!(qwen_tier_model(&models, Tier::Cheap), Some("qwen3.8-27b"));
}

#[test]
fn routing_proxy_legacy_equivalence_tiers_and_fallback() {
    use crate::config::LlmProxyConfig;
    use crate::legacy_catalog::normalize_legacy_config;
    use crate::naming::{ModelRequest, SourceScope, parse_model};

    let config: LlmProxyConfig = serde_json::from_value(serde_json::json!({
        "sources": {
            "claude_oauth": { "accounts_dir": "/unused" },
            "codex_oauth": { "accounts_dir": "/unused" },
            "openai_compatible": [
                { "id": "relay-a", "base_url": "http://localhost/a" },
                { "id": "relay-b", "base_url": "http://localhost/b" }
            ]
        },
        "models": {
            "claude": { "frontier": "claude-f", "standard": "claude-s", "cheap": "claude-c" },
            "gpt": { "frontier": "gpt-f", "standard": "gpt-s", "cheap": "gpt-c" },
            "qwen": { "frontier": "ignored", "standard": "ignored", "cheap": "qwen-c" }
        }
    }))
    .unwrap();
    let catalog = normalize_legacy_config(&config);
    assert!(
        catalog
            .deployments
            .iter()
            .filter(|d| d.source_ref.starts_with("openai-compatible:"))
            .all(|d| d.allowed_lanes == vec![Tier::Cheap])
    );

    let claude_dirs = dirs(&["c1"]);
    let codex_dirs = dirs(&["g1"]);
    let claude_book = AccountBook::new_in_memory();
    let codex_book = AccountBook::new_in_memory();
    let cases = [
        (
            "celeris/frontier",
            true,
            0usize,
            true,
            "claude-oauth",
            "c1",
            "claude-f",
        ),
        (
            "celeris/standard",
            true,
            0,
            true,
            "claude-oauth",
            "c1",
            "claude-s",
        ),
        (
            "celeris/cheap",
            true,
            0,
            true,
            "openai-compatible:relay-a",
            "",
            "qwen-c",
        ),
        (
            "celeris/cheap",
            false,
            0,
            true,
            "openai-compatible:relay-b",
            "",
            "qwen-c",
        ),
        (
            "celeris/cheap",
            false,
            0,
            false,
            "claude-oauth",
            "c1",
            "claude-c",
        ),
        (
            "celeris/cheap",
            false,
            3,
            false,
            "codex-oauth",
            "g1",
            "gpt-c",
        ),
        (
            "claude/frontier",
            true,
            0,
            true,
            "claude-oauth",
            "c1",
            "claude-f",
        ),
        ("gpt/standard", true, 0, true, "codex-oauth", "g1", "gpt-s"),
        (
            "qwen/cheap",
            false,
            0,
            true,
            "openai-compatible:relay-b",
            "",
            "qwen-c",
        ),
    ];
    for (wire_name, first_reachable, claude_in_use, second_reachable, source, account, wire) in
        cases
    {
        let ModelRequest::Tiered { scope, tier } = parse_model(wire_name).unwrap() else {
            panic!("tier name")
        };
        let claude_input = PoolInput {
            dirs: &claude_dirs,
            book: &claude_book,
            in_use: &|_| claude_in_use,
            max_concurrent_per_account: 4,
        };
        let codex_input = PoolInput {
            dirs: &codex_dirs,
            book: &codex_book,
            in_use: &no_in_use,
            max_concurrent_per_account: 4,
        };
        // The old path: relay first for cheap, then account score across pools.
        let old_relay = if tier == Tier::Cheap
            && (scope == SourceScope::Only(SourceKind::Qwen)
                || scope == SourceScope::Any && config.prefer_free)
        {
            rank_relays(&config.sources.openai_compatible, |id| {
                if id == "relay-a" {
                    first_reachable
                } else {
                    second_reachable
                }
            })
            .first()
            .map(|s| {
                (
                    format!("openai-compatible:{}", s.id),
                    String::new(),
                    config.models.qwen[&tier].clone(),
                )
            })
        } else {
            None
        };
        let old_pool = match scope {
            SourceScope::Any => rank_across_pools(Some(&claude_input), Some(&codex_input), 1000),
            SourceScope::Only(SourceKind::Claude) => {
                rank_pool(SourceKind::Claude, &claude_input, 1000)
            }
            SourceScope::Only(SourceKind::Gpt) => rank_pool(SourceKind::Gpt, &codex_input, 1000),
            SourceScope::Only(SourceKind::Qwen) => vec![],
        }
        .into_iter()
        .map(|a| {
            let (source, wire) = match a.source {
                SourceKind::Claude => ("claude-oauth", &config.models.claude[&tier]),
                SourceKind::Gpt => ("codex-oauth", &config.models.gpt[&tier]),
                SourceKind::Qwen => unreachable!(),
            };
            (source.to_string(), a.account_id, wire.clone())
        })
        .next();
        let expected = old_relay.or(old_pool).unwrap();
        assert_eq!(
            expected,
            (source.into(), account.into(), wire.into()),
            "{wire_name}"
        );

        let deployments = legacy_deployments(&catalog, scope, tier, config.prefer_free);
        let new_relay = deployments.iter().find_map(|d| {
            let id = d.source_ref.strip_prefix("openai-compatible:")?;
            let reachable = if id == "relay-a" {
                first_reachable
            } else {
                second_reachable
            };
            reachable.then(|| {
                (
                    d.source_ref.clone(),
                    String::new(),
                    d.upstream_model.clone(),
                )
            })
        });
        let new_pool = match scope {
            SourceScope::Any => rank_across_pools(Some(&claude_input), Some(&codex_input), 1000),
            SourceScope::Only(SourceKind::Claude) => {
                rank_pool(SourceKind::Claude, &claude_input, 1000)
            }
            SourceScope::Only(SourceKind::Gpt) => rank_pool(SourceKind::Gpt, &codex_input, 1000),
            SourceScope::Only(SourceKind::Qwen) => vec![],
        }
        .into_iter()
        .find_map(|a| {
            let source = if a.source == SourceKind::Claude {
                "claude-oauth"
            } else {
                "codex-oauth"
            };
            deployments
                .iter()
                .find(|d| d.source_ref == source)
                .map(|d| (source.to_string(), a.account_id, d.upstream_model.clone()))
        });
        assert_eq!(new_relay.or(new_pool), Some(expected), "{wire_name}");
    }
    assert!(
        legacy_deployments(
            &catalog,
            SourceScope::Only(SourceKind::Qwen),
            Tier::Frontier,
            true
        )
        .is_empty()
    );
    assert!(
        legacy_deployments(&catalog, SourceScope::Any, Tier::Standard, true)
            .iter()
            .all(|d| !d.source_ref.starts_with("openai-compatible:"))
    );

    // Measured quota headroom, rather than the source's position, chooses the pool.
    use task_core::{RateLimitObservation, RateWindow};
    use task_dispatch::accounts::ObservationSource;
    let mut claude_book = AccountBook::new_in_memory();
    let mut codex_book = AccountBook::new_in_memory();
    for (book, id, utilization) in [(&mut claude_book, "c1", 0.9), (&mut codex_book, "g1", 0.1)] {
        book.record_observation(
            id,
            RateLimitObservation {
                five_hour: Some(RateWindow {
                    utilization,
                    resets_at: 2_000,
                }),
                seven_day: None,
                status: Some("allowed".into()),
                resets_at: None,
                observed_at: 900,
            },
            ObservationSource::Run,
        );
    }
    let claude_input = PoolInput {
        dirs: &claude_dirs,
        book: &claude_book,
        in_use: &no_in_use,
        max_concurrent_per_account: 4,
    };
    let codex_input = PoolInput {
        dirs: &codex_dirs,
        book: &codex_book,
        in_use: &no_in_use,
        max_concurrent_per_account: 4,
    };
    let old_pick = rank_across_pools(Some(&claude_input), Some(&codex_input), 1_000)
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(
        (old_pick.source, old_pick.account_id.as_str()),
        (SourceKind::Gpt, "g1")
    );
    let deployments = legacy_deployments(&catalog, SourceScope::Any, Tier::Standard, true);
    let new_pick = deployments
        .iter()
        .find(|d| d.source_ref == "codex-oauth")
        .unwrap();
    assert_eq!(
        (&new_pick.upstream_model, old_pick.account_id.as_str()),
        (&config.models.gpt[&Tier::Standard], "g1")
    );
}
