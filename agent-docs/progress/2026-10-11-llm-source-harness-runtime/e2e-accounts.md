---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: done
title: e2e account fixtures を source×account に移行
updated: 2026-10-11
---

# e2e-accounts

通常 run・delegation・phase7 の fake-worker fixture を新しい source/harness/route 宣言へ更新した。provider admin の lifecycle・cooldown 試験は後続の provider API 削除対象のため削除した。API cooldown scenario は兄弟 unit `e2e-api` の担当として変更していない。

## headroom 選択の調査

`account_selection_follows_headroom_and_survives_restart_and_throttle_only_cools_the_account` が 2 回目に期待した `b` ではなく `a` を選ぶ原因は、旧前提の試験ではなく dispatcher の route account 順序だった。source の宣言順（a,b）をそのまま admission に渡しており、既に観測済みの usage score と `(source_id, account_id)` ごとの in-use を考慮していなかった。`route_state` で候補を AccountBook score の降順に並べ、同点を account ID で決定するよう直した。Claude account pool と Codex account pool の headroom 試験を含む focused check で再現を解消した。

## 試験の移行

- `second_account_runs_the_overflow_when_the_first_is_at_capacity`: source `claude_oauth` の accounts `a,b` に各 1 run の上限を設定し、2 task が同時に別 account を獲得することを `llm_route_selected` で確認する。adapter の provider ごとの `ACCOUNT` 注入・tier ごとの provider 固定は除去した。
- `throttled_account_falls_back_to_the_next_account`: source account `a` が throttle を返し、retry attempt を消費せず次の選択が `b` になることを event で確認する。
- `task_without_a_matching_provider_does_not_block_until_idle`: route candidate がない fixture にして task が Ready のまま `--until-idle` が終了することを確認する。
- Codex account pool の source accounts を `accounts=[]` にし、login root の scan で `a,b` を解決する。
- 現行 config loader が `[[providers]]` を必須としており、provider を完全に外すと daemon が起動できないため、account fixture と fake harness fixture に無動作 sentinel を残した。config-remove 後はこれを除去する必要がある。

## 検証

- `bash scripts/dev/test-parallel.sh -E 'package(e2e) and (test(account_selection_follows_headroom_and_survives_restart_and_throttle_only_cools_the_account) or test(second_account_runs_the_overflow_when_the_first_is_at_capacity) or test(task_without_a_matching_provider_does_not_block_until_idle) or test(throttled_account_falls_back_to_the_next_account) or test(codex_account_selection_follows_headroom_and_sets_codex_home_and_records_rate_limits))'` — exit 0、5 tests passed。
- `bash scripts/dev/test-parallel.sh -E 'package(e2e) and not binary(api_scenarios)'` — exit 0、33 e2e tests と workspace doctests が成功。
- API scenario cooldown の実行・移行は `e2e-api` が担当する。
