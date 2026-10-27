# ADR 2026-10-27: 案件単位の停滞を決定的に検出して受信箱に出す

## 背景
止まりの検出は task 単位だけだった。生存確認（ADR-0079 D10、`tree_units.rs` の `check_tree_liveness`）は計画を持つ
`ready` の task だけを見て、「止まり始めた時刻」を dispatcher のメモリ（`stall_watch`）に持つ。案件として進んでいない・
人の操作待ちが長い・verify 済みの release が昇格されていない、を出すものは無かった。棚卸し（2026-10-27）:

- P0-2 の計画の承認待ちが約 3 時間 16 分放置（10-25 14:51→18:08）。
- verify 済み release の放置が約 1 時間 44 分（10-26 ca6f52cb）と 10-14。
- 01M5HDSN3A は 16 時間 run が起きず、待ちの理由も空だった。

## 決定
- D1（4 つの受信箱の種類）: `InboxKind` に `project_stalled`・`human_wait_aging`・`release_unpromoted`・
  `task_unscheduled` を足す。判定は `task_ops::stagnation` の純関数（LLM なし。`now` を引数に取るので時計を差し替えて試験する）。
  - `project_stalled`: `active` の案件で、終端でも draft でもない task が 1 件以上あり、案件の task の最後の event
    （`events.ts` の最大）から `project_idle_secs` 以上経った。
  - `human_wait_aging`: 受信箱の「人の操作待ち」の項目（`decision`・`plan_gate`・`phase_gate`・`authorization`・
    `browser_wait`・`question`・`acceptance_check`・`draft_accept`・`project_plan`・`credential_wait`・`cluster_login`・
    `integration_request`）のうち `age_secs >= human_wait_secs` のものを案件ごとに 1 件に束ねる。
  - `release_unpromoted`: `[selfdeploy] releases_dir` で、`verify.json` の `ok = true`・`promoted.json` が無い・
    最新の昇格済み release より `built_at` が新しい release がある。最も古い `verify.json.at` から
    `release_unpromoted_secs` 以上経った。
  - `task_unscheduled`: `ready` の task で、run が走っておらず（dispatcher の in-flight に無い）、待ちの理由
    （`task_ops::wait::task_waits`。DB・計画・人の gate・dispatcher のメモリ）が空で、最後の event から
    `task_unscheduled_secs` 以上経った。生存確認の `stalled`（ADR 2026-10-23 D5）に載っている task は除く（同じ止まりを 2 件にしない）。
- D2（論理キーと revision）: 項目の id は論理キー（`project_stalled-<案件 id>`・`human_wait_aging-<案件 id | none>`・
  `release_unpromoted-selfdeploy`・`task_unscheduled-<task id>`）。`created_at` は状態が始まった時刻（最後の event・
  最も古い待ち・最も古い verify）で、これが revision（CoS triage の `source_revision`）になる。同じ状態を何度検出しても
  id と `created_at` は変わらないので項目は増えない。状態が変われば（古い待ちが答えられた等）同じ id のまま
  `created_at` が変わる。解消すれば判定から外れ、受信箱から消える（受信箱は派生の一覧。ADR-0133）。
- D3（どこで判定するか）: `project_stalled`・`release_unpromoted`・`task_unscheduled` は dispatcher が
  `scan_interval_secs`（既定 60 秒）ごとに store と releases_dir を読んで判定し、`DaemonSnapshot.stagnation` に載せる
  （`StagnationFinding`）。始まりの時刻は DB の events と release の file から毎回求めるので、メモリに状態を持たず、
  再起動しても同じ結果になる。`human_wait_aging` は受信箱を組むとき（`human_inbox::from_inbox`）に、その時点の
  項目の `age_secs` から判定する（閾値は `DaemonSnapshot.stagnation_policy`）。snapshot が無い呼び出しでは 4 種とも出ない。
- D4（閾値は config）: `[maintenance.stagnation]`（`project_idle_secs` 既定 21600・`human_wait_secs` 既定 7200・
  `release_unpromoted_secs` 既定 3600・`task_unscheduled_secs` 既定 1800・`scan_interval_secs` 既定 60）。各閾値は `0` で
  その種類を止める。`[maintenance]` は動的な欄（次の tick から）なので、`apply_config` が dispatcher に渡し直す。
- D5（答え方）: 4 種とも受信箱では答えない（native なし。答えると 409 `native_action_required`）。選択肢は人の手
  （案件・task を開く、release を昇格する）を示すだけ。
- 生存確認（`stalled`）は残す。計画の中の詰まり（`plan_stalled` 等の名指しの理由）を `StallDetected` と障害通知で
  出すもので、`task_unscheduled`（待ちの理由が空）とは対象が重ならない。

## 結果
- 棚卸しの 3 例は、それぞれ `human_wait_aging`（計画の承認待ち）・`release_unpromoted`・`task_unscheduled` として
  閾値を過ぎた時点で 1 件ずつ受信箱に出て、解消すれば消える。
- 試験: `task_ops::stagnation` の単体試験（時計を差し替えて閾値の前後・再検出・解消）、`human_inbox` の
  `human_wait_aging` の試験、dispatcher の事実の収集の試験。
