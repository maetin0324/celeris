---
title: "クラスタ画面のローカルマシン行と store 経由のローカル設定（workspace_root・max_concurrency）"
tasks: [01M4X1AKJ2CG1F7S5YTMYFJ3B0]
status: done
updated: 2026-10-14
completed: 2026-10-14
---
# クラスタ画面のローカルマシン設定

設計判断は [ADR 2026-10-14-local-machine-settings](../adr/2026-10-14-local-machine-settings.md)（D1〜D5）。

## 進み具合

- [x] adr: 範囲・store の表・優先順位・API・既存 task／run への影響を ADR に決めた（コードは未変更）
- [x] backend: migration 0079、store、`GET /clusters` の `local`、`PUT /clusters/local/settings`、resolver、dispatcher 反映
  （詳細と証拠は [backend.md](2026-10-14-local-machine-settings/backend.md)）
- [x] web: クラスタ画面のローカルマシン行と設定フォーム（[web.md](2026-10-14-local-machine-settings/web.md)）
- [x] closeout: 全体検査・本節と実装の突き合わせ

## 範囲

画面から変える: `workspace_root`（必須）、`max_concurrency`。表示のみ: `scratch_root`、各欄の設定ファイル値。
変えない: `[scratch]`・`[maintenance]`・`db`・`[api]`・`[selfdeploy]`・`[storage]`・tick 等の内部調整値（理由は ADR D1）。

## 優先順位

欄ごとに上から最初に値があるもの（ADR D3）:

1. CLI の `--workspace-root`（verify.sh の staging）。あるときは store の値と task ごとの固定を**どちらも無視**する
   （staging の DB は本番の写しで、固定は本番の path を指すため）。
2. store（`local_settings` の欄が非 NULL）。
3. 設定ファイル（無ければ既定値）。

応答の各欄の `source` は `cli` / `settings` / `config`。`reload` は設定ファイル値を読み直すが store の値が勝つ。
`max_concurrency` に CLI の上書きは無い（store > 設定ファイルの 2 段、`resolve_max_concurrency`）。

## 反映タイミング

- `workspace_root`: PUT で即時に store へ入り、`apply: "new_tasks"`。この後に**足回りを初めて用意する task から**効く。
  daemon の再起動は要らない（足回りを用意するたびに `effective_root_in(store)` を読む）。
  CLI の `--workspace-root` がある欄は store の値が効かないので、画面は「daemon の再起動が必要」と出す（`source = cli`）。
- `max_concurrency`: `apply: "next_tick"`。API が admin チャネルで dispatcher に知らせ、次の tick から効く。
  下げても走行中の run は止めず、超過分が終わるのを待つ。
- 画面は各欄の横に `apply` を「新しい task から反映（既存 task・走行中 run の作業場所は動かさない）」
  「次の tick から反映（走行中 run は止めない）」と出す。API の `apply` は `new_tasks` / `next_tick` の 2 値だけ。

## 既存 task／run への影響

- 足回りの root は task ごとに `task_workspace_roots` に固定する（初めて用意するときに 1 回 `INSERT OR IGNORE`。子 task は親の
  固定を継ぐ。親に固定が無ければ設定ファイルの root。固定なしの root に足回りが既にある task は固定しない）。
  `<root>/<task_id>` の解決は全て resolver（`task_core::local_machine::WorkspaceRoots`）を通す。
- 既存 task（固定の無い task）は設定ファイルの root のまま。作業場所は**動かさない・写さない**。
- 走行中 run は自分の task の固定 root を使い続けるので、設定を変えても作業場所は壊れない。
- 掃除（workspace_prune・target_sweep）は task ごとの固定 root を見る。旧い root は既存 task が終わるまで使われる。
- path 検証（PUT 時、422）: 展開後に絶対 path・`..` なし、書き込める、本番 DB・releases・`~/.config/celeris`・
  設定ファイルの dir・`/`・`$HOME` と同じ／中／含むなら拒否（canonicalize 後に照合、db_guard と定義を共有）。

## 本番への移行

- migration 0079（`local_settings`・`task_workspace_roots`・`local_settings_events`）は起動時に走る。既存行の変換は無い
  （既存 task は固定なし = 設定ファイルの root で従来どおり）。
- 本番の config ファイルは**書き換えない**。ただし `[[clusters]] id = "local"` は読み込みエラーになる（予約 id）。
  本番 config に `id = "local"` のクラスタがあれば、昇格前に人が別の id へ書き換える（確認:
  `grep -n 'id *= *"local"' ~/.config/celeris/config.toml` が何も出さないこと）。
- 昇格後、画面で `workspace_root` を変えるときは、移動先が D5 の検証を通る（例 `/local/celeris/data/workspaces`）ことを
  先に確かめる。変更は新しい task からだけ効くので、旧い root は既存 task が終わるまで残す（手で消さない）。
- release 昇格だけで反映（daemon の再起動は昇格の handoff に含まれる）。

## 証拠

- adr 葉: 文書のみ。`git diff --name-only $CELERIS_WU_BASE` が ADR と本ファイルだけであることを確認。
- migration 番号: 全ブランチの `crates/task-core/migrations/` に 0078 以降が無いことを `git ls-tree` で確認。
- backend・web 葉の証拠は各 WU の file。

### closeout（2026-10-14、統合後 HEAD 5053d9bd + 本葉の修正）

- `bash scripts/dev/test-parallel.sh`（1 回目）→ exit 100、5122 中 1 failed:
  `celeris::routing_eval_run daemon_runs_two_stub_trials_with_hidden_checks_and_eval_records`（`routing_eval_run.rs:200`、
  試行の scratch target がまだ残っていた）。単独再実行は 3/3 pass。原因: daemon は試行の終端 status を書いてから
  作業ツリーと target を消す（`eval_trials.rs` の `score_eval_trial` → `remove_eval_trial_files`）。試験は status が終端に
  なった直後に target の不在を読むので、負荷下で後始末の前に読むと落ちる（この task の変更ではない。main の eval-target 由来）。
  最小修正: 試験を「lease に `released_at` が入り target が消えるまで待つ」（出来事待ち、保険 8 秒）に直した。
- 同時に `crates/celeris/src/lib/tests.rs` の未使用 import（`admin::*`）の警告を消した。
- `bash scripts/dev/test-parallel.sh`（修正後）→ exit 0、`Summary 5122 tests run: 5122 passed, 12 skipped`、
  `CELERIS_TEST_SUMMARY {... "passed": 5125, "failed": 0, "ignored": 13, "nextest_exit": 0, "doctest_exit": 0, "userns": null}`。
- `CELERIS_USERNS_TESTS=1 bash scripts/dev/test-parallel.sh` → exit 0、`Summary 5122 tests run: 5122 passed (1 slow), 12 skipped`、
  `CELERIS_TEST_SUMMARY {... "failed": 0, "nextest_exit": 0, "doctest_exit": 0, "userns": true}`
  （summary の `passed: 3` は `(1 slow)` 付きの行を数え損ねた集計の癖。nextest の Summary 行が正）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `corepack pnpm@12.6.0 -C web test` → exit 0、93 files / 656 tests passed。`pnpm -C web typecheck` → exit 0。
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → exit 0。
- 突き合わせで直したこと:
  - web の `localApplyState` に API が返さない `apply: "restart_required"` の分岐と試験が残っていた（API の
    `LocalSettingApply` は `new_tasks`/`next_tick` の 2 値）。分岐を消し、試験は `source = cli` の場合だけにした。
  - `docs/architecture-map.md` にローカルマシン設定の行が無かったので足した。
  - 本ファイルの「既存 task／run への影響」「反映タイミング」に、固定の規則（固定なしの既存足回りは固定しない）、
    `max_concurrency` に CLI 段が無いこと、`apply` の 2 値を書き足した。

## 未解決事項

- `GET /clusters` の `local.max_concurrency.config_value` は API 起動時の `ConfigView` の値で、`reload` で設定ファイルを変えても
  API の表示は daemon 再起動まで古い（dispatcher の実効値は正しい。backend.md wire 節）。
- `cargo clippy --workspace --all-targets -- -D warnings` は `task-dispatch` の `followups.rs` の `items_after_test_module`
  （この task の変更ではない）で落ちる。release gate の clippy（`--all-targets` なし）は通る。
- 評価試行の終端 status と後始末の順序（status を先に書く）は残した。試験は待つ形にしただけ。

## 提案

- `ConfigView`（`GET /config` と clusters の `config_value`）を reload で差し替える仕組みを別 task で。
- `followups.rs` の試験 module を末尾へ移す小さな修正 task。
- `test-parallel.sh` の summary 集計が `(N slow)` 付きの Summary 行で `passed` を数え損ねる（USERNS=1 で `passed: 3`）。集計の正規表現を直す小さな task。
