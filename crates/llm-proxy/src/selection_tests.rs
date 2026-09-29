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
