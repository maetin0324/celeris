---
title: 同時実行数をアカウントごとに一本化し、プロバイダの concurrency を deprecate する（暗黙アカウントを含む）
tasks: [01M4D7RVKXY9A967ZXDADWVHQJ]
status: done
updated: 2026-10-08
completed: 2026-10-08
---
# 同時実行数をアカウントごとに一本化（close-out）

ADR `agent-docs/adr/2026-10-08-account-concurrency-limits.md`（状態: Accepted）。人の決定 `nonpool-limit` =
(ii) implicit-account（非サブスクリプションの LLM source 行に source ごとに暗黙アカウント 1 件を作り、
同時実行数はその暗黙アカウントに持たせる）。葉ごとの進捗は `2026-10-08-account-concurrency-limits/<key>.md`
（adr・limits・implicit・api・docs・gui・web）。

## 各 unit の結果

| unit | 結果 |
| --- | --- |
| adr | 完了。ADR 本体と ADR-0012/0017/0024/0089/0132 の付記（上書き関係は双方向）。commit `d2ced79b` |
| limits | 完了。`[[providers]].concurrency` は `Option<usize>`（書かれていれば警告、dispatch では使わない）。migration 0062 の `account_limits` 表（SQLite 正本。再起動・reload を跨ぐ）。実アカウントは保存値 → `max_runs_per_account`、CoS は実効上限 +1 |
| implicit | 完了。`capacity::provider_full` を削除。`implicit/<行 id>` の暗黙アカウントで非プール行を止める（既定は旧 `concurrency` を引き継ぎ、CoS の +1 なし）。行の削除・pool 化で保存値は prune される |
| api | 完了。`PATCH /api/v1/accounts/{adapter}/{id}`（`max_runs` 1..=32 / null で解除、400/404/409、`check`/`login` の id も可）。`GET /accounts` に `max_runs`・`effective_max_runs`・`max_runs_source`・`implicit`・`provider`。providers の `concurrency` は deprecated 欄（受け付けても保存しない、警告） |
| docs | 完了。`docs/guides/providers.md`・`docs/ops/provider-llm-source-migration.md`（移行手順と戻し方）・`docs/guides/knowledge.md`・`docs/ops/opencode-go-and-model-catalog.md`・`docs/ops/selfdeploy.md`・`docs/api/v1/gui-api.md`・schema・生成型を更新 |
| gui | 完了。旧 gui のプロバイダ画面から concurrency の入力・送信・表示を除去（最小変更。置き換え予定のため） |
| web | 完了。アカウント画面で各アカウント（暗黙アカウントを含む）の上限を編集（保存・既定に戻す・検証・API エラー表示）。プロバイダ画面から concurrency 入力と `in_use / concurrency` 表示を外しアカウント画面への導線 |
| verify | 完了（このファイル）。統合後の全試験と文書検査を流し、本番移行の人の手順は `artifacts/production-migration.md`（task の成果物） |

## 試験の結果（統合後。2026-10-08、run 01M4E2RS1XM2C8V2HJ2W9J5X3D）

試験の一時ファイルは短い `TMPDIR`（`/var/tmp/celeris-verify`、unshare の bind mount）で回す。
run の長い `TMPDIR` では credentiald の UNIX socket パスが `SUN_LEN` を超えて既存の browser 試験が落ちるため（環境要因、ADR 以前から）。

| コマンド | exit | 要点 |
| --- | --- | --- |
| `bash scripts/dev/test-parallel.sh` | 0 | nextest 4832 passed / 0 failed / 13 skipped、doc-test 10 crate。`CELERIS_TEST_SUMMARY` `{"passed":4832,"failed":0,"ignored":14,"nextest_exit":0,"doctest_exit":0,"tmp_leftovers":0}` |
| `cargo clippy --workspace -- -D warnings` | 0 | |
| `corepack pnpm@12.6.0 -C web typecheck`（tsc -b） | 0 | |
| `corepack pnpm@12.6.0 -C web test` | 0 | vitest 85 files / 642 tests 通過、node --test 78 通過 |
| `corepack pnpm@12.6.0 -C web` の e2e（`WEB_E2E_SCOPE=functional` playwright test e2e/admin/ops-config.spec.ts e2e/parity/ops.spec.ts e2e/work/narrow-r6.spec.ts e2e/admin/models.spec.ts） | 0 | 38 passed（accounts の上限保存・検証・既定解除・API エラー・暗黙アカウント、providers の concurrency 無し） |
| `corepack pnpm@12.6.0 -C web` の a11y/states e2e（`WEB_E2E_SCOPE=all playwright test e2e/a11y e2e/states --grep 'accounts|providers'`） | 0 | 6 passed（axe /accounts・/providers を含む） |
| `corepack pnpm@11.27.0 -C gui typecheck`（react-router typegen && tsc -b） | 0 | |
| `corepack pnpm@11.27.0 -C gui test` | 0 | 96 files / 1342 tests 全部通過 |
| `sh scripts/dev/check-doc-links.sh`（全体） | 0 | `sink_tests.rs:544 docs/SKILL.md` は試験の入力値（skill 配送の fixture 文字列）なので `FOREIGN_DOCS` に `crates/task-dispatch/src/dispatcher/cos_chat/sink_tests.rs` を追加した（コメント付き）。self-test も exit 0 |
| `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | 0 | |
| `sh scripts/dev/check-adr-numbers.sh` | 0 | ok (167 files) |
| `sh scripts/dev/progress-index.sh --check` | 0 | 本体と葉 6 件とも front matter 合格。索引はこの script が都度生成する（commit しない） |

本番 config・`~/.config/celeris`・systemd・daemon・本番 DB は変更していない。

## 本番への移行（人が実行）

`docs/ops/provider-llm-source-migration.md` の節「プロバイダの concurrency を外し、同時実行数をアカウントで管理する」
（控え → 引き継ぎの確認 → アカウント画面 / PATCH で上限を保存 → config・providers.d から concurrency を外し検査 →
`POST /reload`（または再起動）→ 確認）と戻し方を、本リリースの daemon 差し替えの手順（`docs/ops/selfdeploy.md`）と
合わせた形でまとめたものが task の成果物 `artifacts/production-migration.md`。
要点: **config から `concurrency` を消す前に、暗黙アカウントの `max_runs` を保存する**（旧値 1 だった行は消すと既定 2 に上がる）。
保存値は DB にあるので reload・再起動・リリース入れ替えを跨いで残る。

## 残課題（今回は触っていない）

- **llm-proxy の `max_concurrent_per_account` との統合**: ADR §(f) のとおり別勘定（proxy の上流 HTTP 要求数 vs dispatcher の run 数）。
  同じ subscription に両方が効く構成の合算は未実施。
- **旧 gui の置き換え**: 旧 gui は入力欄除去の最小変更に留めている（置き換え予定）。
- **pi-opencode-go 経路の model id 確認**: WU adr の 3 回とも pi-opencode-go（model `opencode-go/grok-4.6`）が
  「Model "grok-4.6" not found for provider "opencode-go"」と 400 `ModelProtocolUnsupported` で即時失敗した。
  この経路の model id / protocol の確認が再使用前に必要（harness 管理側。この task では LLM CLI・API を起動しないため未実施）。
- `docs/ops/cos-chat.md:87` と `scripts/dev/cos-chat-live.sh` の試験用 provider `concurrency = 4` は今は効かない（警告が出るだけ）。
  台本の変更は今回の範囲外（docs 葉の申し送り）。外すなら別の小さな task で。

## 提案

- `[[clusters]].concurrency`（ADR-0018）・`[execution] max_concurrency`・`max_cos_runs` は今回のまま維持する。
- 同じ LLM source を指す複数行（本番の `opencode-qwen` と `pi-qwen`）は行ごとに上限を持つ（旧 `concurrency` と同じ）。
  source 単位で束ねる場合は別の決定にする（ADR §(e)「既知の限界」）。
