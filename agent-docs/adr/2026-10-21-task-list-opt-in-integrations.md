# task 一覧 API: 取り込み状態を opt-in にし、N+1 と固定 sleep を除く

- 日付: 2026-10-21
- 状態: 決定（実装は後続 unit: run-wait / store-select / api-with / web-with）
- task: 01M5EDRV4B2K5NYPMF9EZ0YN9A
- 関連: ADR 2026-10-11-stranded-done-branches、ADR 2026-10-11-breaking-changes-by-default、ADR-0133（tick・API の lock count 回帰固定）

## 背景（現状の行番号は本 ADR 時点の main 641f334b）

board が呼ぶ `GET /api/v1/tasks?limit=200&order=created_desc` が本番で 3.6〜7.9 秒（web log p50 4.1 s、最大 11.8 s）。原因は 1 行ごとの取り込み状態の付加:

- `crates/task-api/src/handlers/tasks.rs:296` が一覧の全行に `attach_task_integrations`（`tasks.rs:28-86`）を呼ぶ。
  - 行ごとに `store.get(id)`、`repo_list(project_id)`、`task_integration_list(None)`（全件。`tasks.rs:47`）を実行する。
  - repo ごとに `resolve_default_branch`（`crates/task-ops/src/changes.rs:404`）を呼ぶ。`default_branch` 未設定の repo（asmr-archive・pairs-reply-assistant）は毎行 git を 2 回起こす。
- `attach_integrations`（`crates/task-ops/src/view.rs:1624`）が同じ `task_integration_list(None)` 全件を行ごとにもう一度読み、`task_id` で絞る。さらに promotion の最新 release を行ごとに解決する。
- `task_integration_list_impl`（`crates/task-core/src/store/task_integration_state.rs:95-114`）は id 一覧を取ってから 1 行ずつ `task_integration_get_impl` を呼ぶ N+1。1 行の付加で全件 N' 回 + 1 回の SELECT になり、一覧 N 行では N × (N'+1) 回。
- `crates/task-ops/src/changes.rs:80-117` の `run()` は子を `try_wait` + `sleep(10ms)` で poll する（103 行）。git 1 回ごとに最大 10 ms の遅れが載り、行数に比例して効く。
- board（`web/features/projects/board-screen.tsx`・`web/features/tasks/wait-reason.tsx`）は integrations も promotion も使わない。使うのは `web/features/tasks/task-list-view.tsx` と `overview-view.tsx` だけ。

## 決定

### D1: 一覧は既定で integrations / promotion を付けない。`?with=integrations` で明示した画面だけが付ける

- `GET /api/v1/tasks` は既定で `integrations` と `promotion` を付けず（欄は省略、または既存の `None` と同じ表現）、git も起動しない。
- クエリ `with=integrations` を足すと両方を付ける。`with` はカンマ区切りの集合として parse し、未知の値は 400（`QueryParams` の既存の厳格な検証に従う）。値は `integrations` だけ。
- 旧い常時付加は消す。`tasks.rs:296` の無条件ループ、`with` を見ない経路、deprecated 分岐・新旧切替 flag は残さない。`GET /tasks/{id}`（単体）の付加は変えない。
- web は `task-list-view.tsx` と `overview-view.tsx` の取得だけ `with=integrations` を付ける。board と `wait-reason.tsx` は付けない。API schema（`docs` の schema・`gui/`・`web/` の生成型）を再生成する。

### D2: 付加するときは要求ごと・repo ごとに 1 回

`with=integrations` の要求では、行数 N に比例する処理を持たない。

- `task_integration_list` は要求ごとに 1 回だけ呼び、`task_id → Vec<TaskRepoIntegrationState>` の map にする（または一覧の task_id 集合に対する `WHERE task_id IN (...)` の 1 回）。`tasks.rs:47` と `view.rs:1624` の行ごとの全件読みを、この map の参照に置き換える。`attach_integrations` は map（の該当 task 分）を引数に受ける形に変える。
- repo の文脈（`IntegrationRepoContext`）と `resolve_default_branch` は、要求の中で `repo_id → Option<context>` の cache を作り repo ごとに 1 回だけ解決する。解決失敗（`tracing::warn!` して context に入れない現行の挙動）も repo ごとに 1 回。`project_id → repo_list` も同様に要求ごと・project ごとに 1 回。
- promotion の最新 release（`releases_dir` の走査）は要求ごとに 1 回解決して全行で共有する。
- `task_integration_list_impl` は `task_integrations` への 1 本の `SELECT branch,head_sha,state,reason,actor,checked_at,updated_at,task_id,repo_id ... WHERE (?1 IS NULL OR state=?1) ORDER BY updated_at DESC` にする。id 一覧 → 1 行ずつ `task_integration_get_impl` の N+1 は消す（並び順・エラー表現は現行どおり）。

### D3: `changes.rs` の `run()` は固定 sleep の poll をやめ、終了を待つ

- 子の終了は、`child.wait()` を専用 thread で呼び、結果を channel（`std::sync::mpsc`）で送る形にする。呼び出し側は `recv_timeout(deadline - now)` で待つ。
- timeout の挙動は保つ: 時間切れなら `kill_group` + `child.kill()` で孫ごと殺し、wait thread の結果を回収して `timed_out = true`。パイプの読み切り thread（`read_all`）と process group の扱いは変えない。起動失敗は従来どおり `None`。
- 子の `Child` は wait thread が所有するので、kill は pid（`child.id()` を先に控える）で行う。`sleep` を使う poll は残さない。

### D4: 試験は CPU 負荷なしで、回数を決定的に数える

busy loop・stress・並走負荷の台本は使わない。数えるものは次のとおり。

- git 起動回数: `changes.rs` の `run()` の呼び出し点に試験用の計数（`#[cfg(test)]` のカウンタ、または git を PATH 上のスタブ script に差し替えて起動ごとに 1 行 append）を置き、その回数を読む。
- store 全件読み回数: `TaskStore` を包む試験用 wrapper で `task_integration_list` / `repo_list` / `get` の呼び出し回数を数える（ADR-0133 の lock count と同じ手法）。
- API の試験（`crates/task-api`）:
  1. 既定の一覧（`with` なし）は N=1 と N=20 のどちらでも git 起動 0 回、`task_integration_list` 0 回。
  2. `with=integrations` は N=1 と N=20 で `task_integration_list` が 1 回、`default_branch` 未設定の repo 1 つにつき `resolve_default_branch` の git 起動が同数（N に依存しない）、promotion の release 解決が 1 回。N=20 と N=1 の計数が等しいことを assert する。
  3. 未知の `with` 値は 400。
- store の試験（`task-core`）: `task_integration_list_impl` が行数に依らず SELECT 1 本であること（連接の `rusqlite` trace か、行を M 件入れて結果の順序・内容が従来と同じこと）。
- `run()` の試験: timeout は時計の差し替えではなく出来事待ちで決定的にする。長く眠る子（`sleep 3600`）に短い timeout（例 200 ms）を与えて `timed_out` と孫の殺しを確かめ、通常終了は終了コードと出力が返ることを確かめる。上限は保険のみで、判定は子の終了という出来事に依る。

## 却下した案

- 既定で付けたまま 1 回化だけする: board は使わない欄を払い続ける（asmr-archive 等の repo の git 起動も残る）。
- `with=integrations` を残しつつ旧い常時付加も deprecated で残す: 破壊的変更の方針（ADR 2026-10-11-breaking-changes-by-default）に反する。
- 付加を別 endpoint に分ける: 一覧と同じ行の並びに結合する web の変更が大きい。`with` なら既存の画面は 1 つの引数を足すだけで済む。

## 本番への移行

config 欄・DB schema の変更なし（SELECT の書き換えのみ）。API の既定の応答から `integrations` / `promotion` が消えるので、web と同時に release する（web は同じ repo で同期）。
