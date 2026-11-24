---
tasks: [01M87M20BAB5C21EVARQ7BWCCN]
wu: cos-route
status: done
completed: 2026-11-24
---
# CoS の代替経路が同じ source の別の account に移れない不具合を直す（WorkUnit cos-route）

## 結論

- adr-cos の調査（[task の進捗ファイル](../2026-11-24-planner-supply-failure-cooldown.md)）どおり不具合なので、ADR
  [2026-11-24-planner-supply-failure-cooldown](../../adr/2026-11-24-planner-supply-failure-cooldown.md) D5 の方針でコードを直した。
- CoS の run が供給側の失敗（認証の失敗・枠切れ・throttle・起動の失敗）で終わると、失敗した `(source, account)` を worker の run と同じ
  `Dispatcher::record_account_failure`（pool の account）か `record_source_failure`（pool の無い source・spawn の失敗）で cooldown にし、
  同じ chat run を `cos.route_candidates` の全体から選び直す。admission が cooldown 中の account を外すので、候補が 1 件でも同じ source の
  別の account（本番では `claude_max_lab` の次に `claude_max_personal`）で起動する。
- 候補を使い切ったときは `NoEligible`（"no declared route candidate for harness claude_code is eligible"）で閉じる。
  事象の "no route_defaults candidates are declared for harness claude_code" はこの経路では出なくなった。
- 認証の失敗の account は AuthFailed の cooldown（`AUTH_FAILED_HOLD_SECS`）になり、`notify_account_auth_failed` の bad_news が 1 件出て、
  `GET /api/v1/llm/accounts` の一覧では `cooldown.reason = auth_failed`・`excluded_reason = cooldown`・`logged_in = false`（再ログインが必要）になる。
  これは worker と同じ owner を呼ぶことで得た挙動で、新しい経路は足していない。

## 実装前 brief

1. 根本原因: `cos_chat/launch.rs` が供給側の失敗で `RouteRetry { next_route: route_index + 1 }` を積み、`route_select.rs` の
   `select_cos_route_from` が `cos.route_candidates` を `skip(next_route)` した。候補 1 件では 2 回目の列が空になり `NoDeclaredDefault` で閉じた。
   加えて CoS の run は失敗した account を cooldown にしなかった。症状は chat run の reason と「利用可能な経路がなくなりました」の通知に出た。
2. 責務 owner: `cos_chat/routes.rs`（`retry_routes`・`launch_routes`）と `route_select.rs` の CoS の選択。cooldown の owner は既存の
   `route_failure.rs` の `record_account_failure`・`record_source_failure`（変えずに呼ぶ）。
3. 再利用: `route_failure::provider_failure_reason`・`provider_failure_outcome`（分類）、`record_account_failure`・`record_source_failure`（cooldown）、
   `routes::run_elapsed`（実行期限）、試験の `pool_dispatcher`・`accounts_fixture`・`cos_chat_launch` の fixture。
4. 選択肢: (a) 番号を残し、同じ source の account を番号で列挙する → 状態が 2 つになり cooldown も付かないので退けた。
   (b) ADR D5 の案: 失敗した route を cooldown にして全体から選び直す → 採用（worker と同じ規則、旧い番号の仕組みを消せる）。
5. 新しい状態・分岐: `RouteRetry` に失敗した route（試行番号・source・account）と分類（`Option<SupplyFailure>`、1 回だけ取り出す）を載せた。
   `ChatFinish` に `supply` 欄（adapter の `Err` の分類）を足した。どちらも既存の in-memory の再試行の列と終端の値の中で、新しい表・flag・fallback は無い。
6. 撤去: `routes::supply_failure`（文字列で分類し直す関数）、`RouteRetry.next_route`、`select_cos_route_from` の `skip(start)`（`select_cos_route` に置換）、
   `SlotWait.first_route`（常に 0 だった）、`retry_routes` の通らない `next_route == 0` の分岐と「利用可能な経路がなくなりました」の文面
   （「別の経路で再試行します」に揃えた）、`retry_routes` の実行期限の計算の重複（`run_elapsed` を使う）。
7. 不変条件と試験: 下の「試験」。

## 挙動の変化（review で見てほしい点）

- 起動の失敗（`AdapterError::Spawn`）も ADR D1 のとおり供給側の失敗として扱う。従来は文字列の分類に当たらず即座に failed だった。
  今は source を cooldown にして選び直し、ほかに経路が無ければ次の tick で `NoEligible` で閉じる（launched_at は残り、費用不明の扱いも変わらない）。
  既存の試験 `cos_launch_wire::a_spawn_failure_keeps_launched_at` と `cos_launch_cross::spawn_failure_after_the_notice_and_usage_null_stop_the_budget` は、
  1 tick 進めてから同じ不変条件（launched_at が残る・prelaunch でない・USD 不明で予算が止まる）を確かめる形に変えた（弱めていない）。
- `Terminal::Error` など `Ok(RunOutcome)` の終わり方は、文言に "429" などが含まれても供給側にしない（ADR D1）。従来の文字列の分類はこれを再試行していた。

## commit（件名）

- cos route: 供給側の失敗の account を cooldown にして候補全体から選び直す（番号での読み飛ばしを消す）
- cos route: 新しい型を alias にし cooldown を Dispatcher の method から外す（repo-health の size の悪化を戻す）
- progress: cos-route の WorkUnit の結果

## 試験

足した試験 `crates/task-dispatch/src/dispatcher/tests/cos_route_failover.rs`（偽の adapter・固定の時計・task の join。sleep と負荷なし）。
既存の試験に CoS の代替経路を通るものは無く、この不具合を検出できなかった。

- `cos_route_auth_failure_cools_the_account_and_retries_on_another_account`（ADR D7 の 6）: 候補 1 件・account `a`・`b`、`a` は
  "Failed to authenticate: this login reached its time limit …" の `AuthFailed`。同じ chat run が `b` で起動して completed、
  `a` は auth_failed の cooldown（until ≥ 固定時刻 + `AUTH_FAILED_HOLD_SECS`）・`logged_in = false`、bad_news 1 件、
  「別の経路で再試行します」の通知があり「CoS route unavailable」は無い。
- `cos_route_every_account_expired_closes_with_no_eligible_route`（ADR D7 の 7）: `a`・`b` とも期限切れ → run は `NoEligible` の理由で failed、
  "no route_defaults candidates are declared" は reason にも通知にも出ない。両方が auth_failed の cooldown、bad_news は account ごとに 1 件（計 2 件）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `cargo nextest run -p task-dispatch -E 'test(cos_route_) | test(cos_chat) | test(cos_slot_wait) | test(cos_launch) | test(sink) | test(cos_capacity) | test(cos_live)'`
  → 266 passed。
- `bash scripts/dev/test-affected.sh` → exit 0（対象 crate: celeris celeris-campaign celerisctl llm-proxy task-dispatch。4007 passed・11 skipped。
  repo-health は verdict=pass、new=0・worsened=0）。
  最初の提出前の実行では repo-health の `size.responsibility-growth` が worsened=3（Dispatcher の method 数 374→375、dispatcher module の論理行
  32202→32245、`launch_claimed` 345→360 行）で落ちた。2 つ目の commit で 374・32202・345 に戻した（`repo_health.py --rules size.responsibility-growth` で確認）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 変えた path は `crates/task-dispatch/src/dispatcher/` の cos_chat・route_select・試験と、この進捗ファイルだけ。
  planner-supply が触る `planner_flow.rs`・`worker_finish.rs`・`route_failure.rs`・`design_gate.rs` は変えていない。

## 未解決

- 本番の chat run 01M87BQVH0SWEA0875FRGZG14T と同じ状況（`claude_max_lab` の期限切れ）の実機の再現はしていない（偽の adapter の試験だけ）。
- cooldown の振り分け（pool の account か source か、spawn は source）は `worker_finish.rs` と同じ規則を `cos_chat/routes.rs` の `cool_down` にも書いた。
  `review_verdict.rs` も spawn の区別を除いて同じ形なので、3 か所の重複になる。

## 提案

- integrate-build の後（planner-supply が `route_failure.rs` 周りを変え終えた後）に、`worker_finish.rs`・`review_verdict.rs`・`cos_chat/routes.rs` の
  「pool の account なら `record_account_failure`、それ以外か spawn なら `record_source_failure`」を `route_failure.rs` の 1 つの関数にまとめる。
  この WorkUnit では planner-supply と同じ file を避けるため、まとめていない。
