# ADR-0089: CoS の対話 run は max_concurrency とプールの concurrency の外で走る

- Date: 2026-09-29
- Status: Accepted
- 関連: ADR-0012（並列度・プロバイダの選択）、ADR-0024 / ADR-0025（アカウントプール、`max_runs_per_account`）、
  ADR-0033 D4 / ADR-0046 D6（CoS = 秘書ノードとの対話）、ADR-0048 D3（`POST /console/instruct`）、ADR-0054（CoS の継続セッション）、
  ADR-0074 D1.3（並列 WU）
- Phase: R6-5

## 文脈

本番は `max_concurrency = 6`（全体）、claude-pool `concurrency = 4`、codex-pool `concurrency = 2`、
`[accounts] max_runs_per_account = 2`。compound な task の並列の葉（WU・子 task）が全ての枠を埋めると、Console から CoS に話しかけた
一言（CoS の対話 run）が葉の後ろで `ready` のまま待つ。人が状況を尋ねたい・止めたいときほど枠は埋まっており、返事が来ない。

人の指示（2026-09-29 23:0xZ）: 「CoS のチャット task が unit が全て埋まっているときに動かないのは不便なので、CoS に割り当てられるものだけ
max concurrency の制限から特別に除外する」。

CoS の対話 run は ADR-0054 のとおり継続セッション 1 本で、`task_ops::conversation` が CoS の未終了の対話を `depends_on` で直列化している
（通常は同時に 1 本）。短く、返事を書くだけで作業はしない（ADR-0033 D4 の preamble）。

## 決定

### 規則 1: CoS run の判定は 1 か所

`task_dispatch::capacity::is_cos_run(task, org)`:

```text
task_core::is_conversation(task)            // Task.conversation がある（人の発言から作った対話用タスク）
  && !task_core::is_milestone_review(task)  // 途中目標レビューの対話（milestone_id あり、裏方）は除く
  && task.assignee が org の OrgKind::Secretary のノード（= CoS、ADR-0046 D6）
```

`POST /console/instruct` の既定の宛先・`POST /org/cos/messages`・MCP の CoS への発言が作るタスクがこれに当たる。
既存の `cos_conversation_session`（ADR-0054 Phase 67c の sticky 判定）と同じ材料（対話の印 + 担当が秘書）で、裏方の途中目標レビューだけを
外している。判定は決定的（ストアの `org_list` を引くだけ。LLM なし）で、対話でないタスクでは組織を引かない。

### 規則 2: CoS run は枠の外で走るが、アカウントは消費する

- **`max_concurrency` に数えない**。`in_flight ≥ max_concurrency` でも起こす。
- **アカウントプールのプロバイダ（`account_pool = true`）の `concurrency` に数えず、超えてよい**。プールでないプロバイダ（API キーの行・ACP 等）
  には例外を作らない（これまでどおり CoS も含めた合計で `concurrency` と比べる）。
- **アカウントは消費する**（`account_in_use` に数える）。CoS run のアカウントは、選べる候補のうち**走っている run の最も少ない**もの
  （同数ならスコアの高い方、次に id 昇順。`accounts::select_account_least_loaded`）。除外判定は ADR-0024 D3 の `evaluate` のままで、
  `max_runs_per_account` だけを CoS に限り **+1** する（CoS は (max+1) 本目として載れる。`capacity::account_run_limit`）。
  cooldown・未ログイン・枯渇したアカウントには載らない。
- ADR-0054 の sticky（継続セッションの (adapter, account) に留まる）はそのまま先に試す。留まれるかの判定も +1 の上限とプールの例外で行う。
- この tick の「満杯のプロバイダ」集合（非 CoS の判定）を CoS は共有しない（CoS の選択が葉の判定を汚さず、葉の満杯が CoS を止めない）。

### 規則 3: 例外が非 CoS を飢えさせない

- 全体の枠の会計（`workers_in_flight`）とプロバイダの `in_use` は **CoS run を数えない**。葉は CoS が走っていても `max_concurrency` と
  プールの `concurrency` いっぱいまで起きる。
- 逆に葉は CoS の例外に乗らない（`max_concurrency` を超えて起きることはない）。
- 観測: `GET /providers`（とデーモンのスナップショット `providers[]`）に **`in_use_cos`** を足し、CoS run を `in_use` とは別に出す
  （GUI・運用が「枠の外で走っている CoS」を見分けられる）。`in_use` は CoS を含まない。

### 規則 4: 絶対の上限 `[execution] max_cos_runs`

- `[execution] max_cos_runs`（既定 **2**、範囲 0..=8）。走っている CoS run がこの数に達したら、次の CoS run は待つ（`ready` のまま）。
  CoS の対話は通常 1 本に直列化されているので、2 は「直列化をすり抜けた 1 本」までの余裕。
- `0` は例外の無効化（CoS run も通常の run と同じく `max_concurrency` / `concurrency` / `max_runs_per_account` で待つ）。運用の切り戻し口。
- したがって最悪でも同時の run は `max_concurrency + max_cos_runs`、1 アカウントあたり `max_runs_per_account + 1`。

## 実装

- `crates/task-dispatch/src/capacity.rs`（新設・純粋関数）: `is_cos_run`、`RunLoad`（`admits(cos)` / `any_slot`）、`provider_full`、
  `account_run_limit`、`DEFAULT_MAX_COS_RUNS`。
- `crates/task-dispatch/src/accounts.rs`: `select_account_least_loaded`。
- `crates/task-dispatch/src/dispatcher.rs`（局所）: `RunEntry.cos`、`workers_in_flight` / `provider_in_use` は CoS を除く、`cos_in_flight` /
  `provider_in_use_cos` / `run_load` / `is_cos_task` / `provider_full`、`dispatch_ready` は非 CoS の枠が無くても CoS の枠があれば対話用タスクだけを
  走査、`dispatch_one` の入口で `run_load().admits(cos)`、`select_provider_for(…, cos)`（`select_provider` は `cos = false` の薄い包み）、
  `pick_account` / `account_usable` / `matching_provider_for_adapter` / `sticky_provider` に `cos`。
- `task_ops::daemon::ProviderLive.in_use_cos`（`#[serde(default)]`）、`task_api::types::ProviderView.in_use_cos`。
- `celeris::config::ExecutionTomlConfig.max_cos_runs` → `task_dispatch::ExecutionConfig.max_cos_runs`。

## 帰結

- Console の一言は葉で枠が埋まっていても次の tick で走る（アカウントが全て cooldown・枯渇・max+1 のときだけ待つ）。
- アカウントの同時 run は CoS の分だけ一時的に `max_runs_per_account + 1` になりうる（レート制限の観点では CoS の run は短い対話 1 本）。
- 部署ノードとの対話・途中目標レビュー・CoS に割り当てた通常の仕事は例外に乗らない（必要になれば規則 1 を広げる別の決定にする）。
- `max_concurrency` の意味は「CoS の対話以外の run の上限」に変わる。

## 付記: CoS チャットと受信箱一次対応への置き換え（2026-10-05）

[ADR 2026-10-05-cos-chat-home](2026-10-05-cos-chat-home.md) D2/D6 により、規則 1 の対象に内部で生成する CoS chat/受信箱 triage run を加える。
全体 1 本の直列化はスレッドごとの直列化となる。規則 2〜4 の会計、max_cos_runs、pool 例外と account 上限 +1 は維持する。
max_cos_runs=0 は従来どおり通常枠に戻すだけで、cos.enabled=false とは異なる。quota は迂回せず、一次対応できない待ちは新 ADR の退避通知へ進む。
通常 task や非 CoS worker をこの例外に含めない。切替は新 ADR の後続実装で行う。
