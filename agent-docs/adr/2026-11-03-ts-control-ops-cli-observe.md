---
tasks: [01M6HGFR1TR6PJ8822DHW7NNNT]
---
# ADR 2026-11-03: TS 制御層の管理 CLI・未配備の登録・tick の待ちの観測・script-guard の増分の見回り

- 状態: 採用（実装済み）。task 01M6HGFR1TR6PJ8822DHW7NNNT（段階移行 2 の P1-C）。
- 関係: [TS 制御層の境界 ADR](2026-11-02-ts-control-layer-boundary.md)（付記 S5-1・S5-3・S5-4・S5-7・S5-8）、
  [CoS の操作の registry](2026-10-09-cos-operations-all-mutations.md)（行は変えない）、
  [破壊的変更を既定にする](2026-10-11-breaking-changes-by-default.md)。
- 根拠: [S5 の進捗](../progress/2026-11-02-ts-control-layer-s5.md) の未解決 1・2・4・5・6 と提案 1〜4。

## 決定
1. `celerisctl script` に `mode … canary --canary-bps <1..9999>`、`generations list|register|retire`、`guard status|clear` を足す。
   CLI は API の要求を組むだけで、認可を足さない・緩めない。`→canary`（比率の変更を含む）・`→ts`・`guard clear` は今までどおり
   人の API token の主体だけが通る（外部 operator・MCP は 403 `script_mode_requires_human`）。CoS の run では変更系を
   `/cos/operations` に包み、`guard clear` は registry の EXCLUDED（`human_only`）で 422 `cos_operation_not_allowed` になる。
   全体の `--reason`（CoS の監査の理由）と重ならないよう、退役と解除の理由は `--retire-reason`・`--clear-reason` にする
   （mode の `--mode-reason` と同じ形）。
   CoS の run で包まずに直接送るのは読み取り（`script domains`・`generations list`・`guard status`。`GET /script/domains`・
   `GET /script/generations`）だけで、判定は `commands/script.rs` の `cos_read_only` に置く。全 subcommand は読み取りか
   `request_of`（包む変更系）のどちらか一方に入る（試験で固定）。
2. 登録（`POST /script/generations`）で配備先 `<state_dir>/script-bundles/<sha256>/` の dir が無いときは、`invalid` を記録せず
   409 `script_bundle_not_deployed` を返す（`ScriptGenerationError::NotDeployed`）。付記 S5-1 の 6（`invalid` の hash は登録し直せない）の
   対象は「配備された bundle が検証に落ちた」場合だけに狭める。dir が在って manifest・hash・selftest 等に落ちたら今までどおり `invalid`。
   未配備は bundle の不正ではないので、配備の後に同じ hash で登録し直せる。fail-closed は保つ（未配備の hash は isolate を作らず、
   表も変えない）。
3. dispatcher の `script_tick_wait_summary` を `DaemonSnapshot.script_tick_wait`（`ScriptTickWaitLive`、`GET /daemon` で読める）に載せ、
   daemon は 10 分ごとに `script_tick_wait` の log 行（`window`・`p50_us`・`p95_us`・`max_us`・`tick_budget`・`budget_us`）を出す。
   窓の中に予算切れ（`tick_budget > 0`）があれば warn にする。snapshot は観測値で DB に書かない。
4. script-guard の見回り（`sweep`）は `script_compare_log` を全件読まず、process の中の cursor（読み終えた id）から
   `SqliteStore::script_compare_log_after(after, 500)` の page で差分だけを読む。再起動で cursor を失うと最初の見回りは全件を
   1 回読む（正しさは変わらない。停止は最後に人が解いた時刻より後の記録だけで判定する）。

## 却下した案
- 未配備でも `invalid` にして、手順で「配備を先に」と書き続ける: 手順を誤るとその hash が二度と使えず、bundle を作り直すしかない。
- tick の待ちを新しい endpoint（`GET /metrics/script-tick-wait`）で出す: `GET /daemon` の snapshot に載せれば足り、endpoint の表を増やさない。
- cursor を DB に永続化する: 見回りは冪等で、再起動後の 1 回の全件読みの費用は小さい。表を増やさない。
