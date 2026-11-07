---
title: Claude 予約（CosReservation）を route_select に配線する（WU cos-reserve）
tasks: [01M6SQJF9YD3WPTD8NSB191PZW]
status: done
completed: 2026-11-07
---
# Claude 予約の配線（WU cos-reserve）

完了日: 2026-11-07。正本 ADR: [2026-11-06-cos-run-launch-accounting-and-drain](../../../../adr/2026-11-06-cos-run-launch-accounting-and-drain.md) D2-3。新しい設計判断は無く、ADR は足していない。
`worker_reserve_five_hour` の意味と既定（0.90）は変えていない。それまでこの値は config と `CosChatLaunchConfig` の欄にあるだけで、判定には使われていなかった（別経路の予約は無かった）。

## 変更（commit は件名で列挙）
- `phase 1: CosReservation を route_select に配線し、実経路の試験を足す`
- `task-core/llm_route.rs`: `QuotaSnapshot.cos_reserved`（予約に入っている `(source, account)`）を足し、`admit_source` が該当 account を `ExclusionReason::CosReservation` で除外する（`is_slot` は偽のまま）。
- `task-dispatch/accounts.rs`: `five_hour_utilization`（5 時間枠の使用率。観測なし・窓が過ぎていれば 0）。
- `task-dispatch/dispatcher/route_select.rs`（`route_state`）: CoS が有効で、worker の route（`cos == false`）のとき、Claude の account の 5 時間枠使用率が `worker_reserve_five_hour` 以上なら `cos_reserved` に入れる。CoS の route には効かない。
- 試験専用の hook `Dispatcher.test_cos_reserved_sources`（`#[cfg(test)]`）: CoS の経路でも予約を再現するための注入口。
- driver・judge・drain のコードに差分は無い。

## 試験（先に書き、実装前に走らせた）
- `route_select::slot_wait::cos_reservation_is_traced_and_never_a_slot_wait`: (a) RouteTrace に CosReservation (b) `slot_only_wait` 偽 (c) 全候補予約でも待たない (d) 枠理由と予約理由が混在しても偽。account の並び順に依らない。
- `cos_slot_wait::cos_reservation_closes_with_route_unavailable_without_waiting`: 実経路（`tick_cos_chat_launch`）で、予約された CoS run が Failed・`launched_at` なし・`prelaunch_reason = route_unavailable`、Waiting の通知 0、次 tick で新しい run なし。
- `route_select::worker_is_excluded_from_the_cos_five_hour_reserve_but_cos_is_not`: 実 account の 95% は worker で `cos_reserved`、60% は入らない、CoS には空（この試験は配線と同時に追加し、修正前の不合格は取っていない）。

### 実装前の不合格
`cargo test -p task-dispatch --lib cos_reservation`（`QuotaSnapshot.cos_reserved` の欄と試験の注入口だけ足し、`admit_source` は未変更）:
```
test ...::cos_reservation_is_traced_and_never_a_slot_wait ... FAILED   (a) reasons が AccountConcurrency のみ
test ...::cos_reservation_closes_with_route_unavailable_without_waiting ... FAILED   reserved: RoutePick {... source p1 ...}（予約が効かず選ばれた）
test result: FAILED. 0 passed; 2 failed; exit code 101
```

### 修正後の合格
`cargo test -p task-dispatch --lib cos_reservation`: `2 passed; 0 failed`、exit 0。`cargo test -p task-dispatch --lib reserve`: `12 passed`。

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash scripts/dev/test-affected.sh`: exit 0、`7825 tests run: 7825 passed (10 slow), 22 skipped`。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。

## 未解決
- 本番の実 Claude account で 5 時間枠の使用率が実際に予約へ入る様子は未確認（LLM・本番操作は禁止）。観測値は `AccountBook` の既存の usage を使う。
- worker が予約で除外されたとき、枠待ちにはせず従来の route 失敗で扱う（ADR D2-3 のとおり）。

## 提案
- 予約で worker が止まる頻度を `routing_decided` 以外の指標で見たいときは、`CosReservation` の件数を route 失敗の集計に出す。
