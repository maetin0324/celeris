---
title: backend 統合結果の検証と親 task への契約（verify-backend）
tasks: [01M6TA6JKSR1N132CSY7C4XYP0]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# verify-backend

完了日: 2026-11-07。正本 ADR: [2026-11-06-cos-run-launch-accounting-and-drain](../../../../adr/2026-11-06-cos-run-launch-accounting-and-drain.md)。
検証専用 task。コードと ADR は変えていない。新しい設計判断は無く、ADR は足していない。基準は `git merge-base HEAD main`。
LLM・外部ネットワーク・userns 実試験・本番操作・全 suite・web e2e・release/verify/promote は実行していない。

## 結論
backend 統合結果（前 2 段）は、ADR D1・D2・D4 と D5 の API 部分について、起動側（永続化・通知・回収・枠待ち）と集計側（予算の和・cost_state）の両方が接続済みで、再実行した試験はすべて合格した。不備は見つからなかった。
未実装は D3（drain と共有の期限）と D5-3・D5-4（driver の facts・judge）で、親 task の作業として残る。

## 受け入れ条件ごとの証拠（提出時の HEAD で実行）
- 条件 0（必須 3 検査）
  - `bash scripts/dev/test-affected.sh`: exit 0。nextest は 7825 件を対象に実行して失敗 0（22 件は skip 設定）、doctest ok、web typecheck と unit（vitest 104 ファイル・711 件）通過、ts-verify 通過。
  - `cargo clippy --workspace -- -D warnings`: exit 0。
  - `cargo fmt --all -- --check`: exit 0。
- 条件 1（web）
  - `corepack pnpm@12.6.0 -C web typecheck && corepack pnpm@12.6.0 -C web test && node web/scripts/gen-types.mjs --check`: exit 0（生成型は `docs/api/v1/api-v1.schema.json` と一致）。
- 条件 2（文書）
  - `sh scripts/dev/check-doc-links.sh`、`sh scripts/dev/check-adr-numbers.sh`、`sh scripts/dev/progress-index.sh --check`: すべて exit 0。
- 条件 3（親 Objective の条件ごと。回帰試験を個別に再実行した結果）

| 条件 | 再実行したコマンド | 結果 | 修正前の不合格（各 unit の進捗に記録。assertion の内容） |
|---|---|---|---|
| 永続記録（launched_at・prelaunch_reason・attempt 欠測） | `cargo test -p task-core --lib launch_boundary_regression` | 15 passed | exit 101。`missing column launched_at`、`no such column: launch_tracked` |
| fail closed 会計（起動前だけ除外、旧行・起動後 null・境界不確かは unknown） | `cargo test -p task-core --lib project_guard_usage` / `cargo test -p task-ops --lib project_budget` | 4 passed / 11 passed | exit 101。`prelaunch run must not be summed: [ProjectRunUsage { model: Some("m1"), usage: None }]` |
| 壊れた usage_json で一覧が落ちない（cost_state=unknown） | `cargo test -p task-core --lib run_list_survives_unreadable` | 1 passed | exit 101。`one unreadable usage row must not fail the list: Invalid(...)` |
| 起動通知（通知失敗なら spawn しない、全 adapter） | `cargo test -p task-worker --lib launch_notice` | 4 passed | exit 101、4 件とも FAILED（phase 1 の commit 時点） |
| 起動側の接続（launched_at を spawn 直前に書く、回収 3 分岐、人の stop、triage claim） | `cargo test -p task-dispatch --lib cos_launch_wire` | 12 passed | exit 101、10 件中 10 件失敗（例: `launched_at` が None、stub の `spawned.marker` が作られる） |
| 構造化枠待ち（実 account は +1、暗黙 account は +1 なし、cooldown と予約は待たない） | `cargo test -p task-dispatch --lib slot_wait`（route_select 側）/ `cargo test -p task-dispatch --lib cos_slot_wait` | 14 passed / 5 passed | exit 101、8 件中 4 件失敗（`assertion failed: slot_only_wait(..)`、`implicit_account_gets_no_extra_cos_slot` left 3 right 2 など）。cos_slot_wait は 4 件中 3 件失敗 |
| 上限保持（`max_cos_runs`、枠待ちの期限で `slot_wait_expired`、同じ run が再開） | `cos_slot_wait` の `slot_wait_expires_at_the_run_deadline_and_is_not_extended` ほか、`cos_launch_cross` の `max_cos_runs_makes_the_next_input_wait_without_a_failed_run` | 上の 5 件・8 件に含まれ合格 | 同上 |
| worker/reviewer と CoS の重なり、解放後 usage | `cargo test -p task-dispatch --lib cos_launch_cross` | 8 passed | 本番コードの不備は無く、試験のみ追加された unit。解放後に同じ run が起動し usage が DB と予算に載ることを検証 |
| API・schema（`cost_state`・`launched_at`・`prelaunch_reason`・`not_launched_runs`） | `cargo test -p task-api --test project_controller budget_today_counts` / `cargo test -p task-api --test chat_api chat_api_launch_boundary` | 1 passed / 1 passed | exit 101。`budget_today` に `not_launched_runs` が無く `left: Null  right: 0` |

  - 修正前の不合格は、各 unit の進捗ファイル（同じ directory の durable-contract・usage-query・notify・launch-and-wait・slot-wait・api-contract・budget）に記録された assertion を読んで確認した。いずれも compile error や環境エラーではなく、目的の assertion（欄の欠落、起動前 run が和に残る、marker が作られる、枠判定が偽、など）による失敗だった。この task では修正前の状態を再現して再実行してはいない（未実施）。
  - 起動側と集計側の接続: `ChatRunSink::before_spawn`（`task-dispatch/.../cos_chat/sink.rs`）が `chat_run_mark_launched` を呼び、`routes.rs` の `fail_routes` が起動前の run を `prelaunch_reason` 付きで閉じ、`task-core/src/store/project_guard.rs` の和と `task-ops` の `project_budget` が同じ条件で除外数を数える。`cos_launch_cross` が通知 → dispatch → 予算の和を実経路で通す。
  - 旧経路の削除: route 選択の文字列判定は無く、枠待ちは `slot_only_wait`（構造化理由）で判定する。`supply_failure`（`routes.rs`、`launch.rs`）は起動後の失敗の分類として残り、ADR D2 の枠待ち判定とは別の用途。usage 無しで run を閉じる旧経路は `fail_routes` から外れている。`chat_run_finish` の残る呼び出しは、人の stop・起動済み run の終端・枠待ちの stop で、起動前の run の費用不明化には使われない。
  - 生成型: `docs/api/v1/api-v1.schema.json` に `cost_state`・`launched_at`・`prelaunch_reason` があり、`gen-types.mjs --check` が exit 0。
- 条件 4: 記録は本 directory 配下。未実装範囲は下の「親への引き継ぎ」に明示した。

## ADR の範囲の区別
| ADR | 状態 |
|---|---|
| D1 起動境界・除外条件・fail closed・既存行を埋めない | backend 実装済み・試験済み |
| D2 枠待ち（実 account・暗黙 account・`max_cos_runs`・Claude 予約・期限） | backend 実装済み・試験済み |
| D4 failover の attempt 会計、再起動の回収 2 分岐、stop | backend 実装済み・試験済み |
| D5-1 chat run view、D5-2 `budget_today.not_launched_runs` | 実装済み・schema・生成型・`api.md` 更新済み |
| D3 drain と共有の期限 `deadline_at` | **未実装（親の driver）** |
| D5-3 facts の `launch`・`prelaunch_reason`・`slot_wait_secs`・`drain` | **未実装（親の driver）** |
| D5-4 judge（`pilot_gate.py`）の `cos_runs`・`cos_not_launched`・`cos_drain_timeout` | **未実装（親の judge）** |
| D0 旧 ADR 2026-11-05 への訂正の付記 | この task では確認していない。親で要否を判断する |

## 親への引き継ぎ（契約）
- API 欄: chat run view は `launched_at`・`prelaunch_reason`・`cost_state`（`known`・`unknown`・`not_launched`）を出す。値が無い `launched_at`・`prelaunch_reason` は null ではなく**欄ごと省略**される。driver は「省略 = null」と読むこと。案件 controller の `budget_today` は `usd`・`usd_unknown_runs`・`not_launched_runs` を持つ。`launched_at` を返さない旧 daemon の記録は `unrecorded` として起動済み扱いにする（ADR D5-3）。
- cost_state の判定: `not_launched` は `finished_at` があり、`launched_at` が無く、`prelaunch_reason` が閉じた 6 値のどれかのときだけ。旧行（両欄 NULL）、`prelaunch_reason` 無し・未知の値、起動後の usage 無し・破損、attempt の一部欠測は `unknown`（案件の予算を止める）。既知 usage は `known`。
- 期限: backend は CoS run の `max_wall_secs`（`started_at` からの残り）で枠待ちを `slot_wait_expired` で閉じる。試行全体の `deadline_at` の算出、`[cos] max_wall_secs` の絞り込み、runner が渡す承認の開始時刻は親の driver の作業。
- 待機記録: 枠待ちは新しい run・入力を作らず、`chat_run_status` の Waiting（文面「CoS の枠の空き待ち」）で保持し、解放後に同じ run が起動する。driver の facts に `slot_wait_secs` を書くのは親。drain（最後の操作の後、SIGTERM の前に案件の全 chat run が終わるのを DB の読み取りで待つ）と `cos_drain_timeout` も親。
- 旧行の保守的解釈: migration は既存行を埋めない。旧 raw と旧記録は起動済みとして読み、1 回目の G7 の raw の判定（verdict unknown・g7 fail・`cos_usage_null=3`）を変えないこと。
- 本番への移行: 必要になるのは新 binary の release と DB migration の適用で、通常の release 手順に含まれる。config の欄は消していないので、本番 config の書き換えは不要（人が実行する手順は無く、この task では何も実行していない）。

## 未解決
- `launched_at`・`prelaunch_reason` を null で返すか欄ごと省略するか（上記）。常に null を返すなら `task-core/src/chat/mod.rs` の `skip_serializing_if` を外し、schema と web 型の再生成が要る。
- 起動前の reason の割り当て: 設定・credential・attachment・workspace の失敗は閉じた 6 値に合うものが無く `route_unavailable` に寄せている。別の値（`launch_unavailable` など）が要るなら ADR の列挙の変更が要る。
- acp・aider の専用 stub 試験は無い（notify unit からの引き継ぎ）。

## 提案
- 親は D3・D5-3・D5-4 を実装する前に、上の「API 欄」の省略規則を facts の読み取りへ反映する。
- 実 LLM による G7 の再実行は、親の driver と judge の接続後に、人が承認した手順で行う（この task では実施しない）。

## commit（件名）
- `phase 1: backend 統合結果の検証と親への契約を記録する`
