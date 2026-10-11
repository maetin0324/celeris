---
title: PATCH /api/v1/accounts/{adapter}/{id}、GET の実効上限と暗黙アカウント、providers の concurrency を deprecated に
tasks: [01M4D7RVKXY9A967ZXDADWVHQJ]
status: done
updated: 2026-10-08
---
# api: PATCH /accounts/{adapter}/{id}、GET の実効上限と暗黙アカウント、providers の concurrency を deprecated に

2026-10-08、WorkUnit `api`（run 01M4DDJ713KHN0YNHPDP5HDSGY と 01M4DEH8PAPS67F24XFQGMCA4X）。ADR 2026-10-08-account-concurrency-limits §(a)(c)(d)(e)。

## 実装

- `task_ops::daemon::DaemonSnapshot.implicit_accounts`（`ImplicitAccountLive {provider, in_use, legacy_concurrency?}`）。ディスパッチャが
  非プール行ごとに設定順で出す（`implicit_accounts_snapshot`）。`ProviderLive.concurrency` は schema で `deprecated: true`。
- `AccountView` に `max_runs`・`effective_max_runs`・`max_runs_source`・`implicit`・`provider` を追加。`dir` は `Option`（暗黙アカウントは `null`）。
  `GET /accounts` は実アカウント（adapter → id）の後ろに暗黙アカウントを並べる。上限の保存値は DB（`account_limits`）から毎回読み、
  旧 `concurrency` はスナップショット、既定は `max_runs_per_account`（celeris は `[accounts]` が無ければ組み込みの 2 を渡す）。
  PATCH の直後から GET に反映される。暗黙アカウントの `stats` はその行のプロバイダ集計。
- `PATCH /api/v1/accounts/{adapter}/{id}`（管理系・active のみ）: 本文 `{"max_runs": 1..=32 | null}` を手で検査（欠落・未知の欄・範囲外・小数・文字列は 400）。
  adapter は `claude-code`/`codex`/`opencode-go`/`implicit`、id の規則違反は 400。実アダプタの根が無い・管理経路が無いと 409。
  celeris 側 `AdminRequest::AccountLimitSet` → `accounts_admin::set_account_limit`（実: ディレクトリ、暗黙: `implicit_account_ids()` で存在確認、無ければ 404）→
  `Dispatcher::set_account_limit`（DB に書いてから表を更新。次の tick から効く。走っている run は止めない）。
  `/accounts/{adapter}/check`・`/login` にも PATCH を登録し、id が `check`/`login` のアカウントも編集できる。
- `POST /accounts` は同じ `(adapter, id)` の古い保存値を消す（celeris 経由。経路が無ければ DB を直接）。
- providers: `ProviderConfigFile.concurrency` は `Option`（`skip_serializing_if`）。`POST`/`PATCH` の本文の `concurrency` は型検査（0 は 400）と warn だけで保存しない。
  人が書いた旧値は PATCH の往復で残る。`ProviderView`/`ProviderConfigView` の `concurrency` は残し、schema で deprecated。
- `DELETE /providers/{id}` は暗黙アカウントの保存値を即時には消さない（行は reload まで dispatcher に残るので、その間は保存値が効く方が一貫する）。
  次の reload の prune（WU `implicit`）で消える。gui-api.md に明記した。
- 文書: `docs/api/v1/gui-api.md`（改訂欄、エンドポイント表 113・216、§3.19・3.24・3.25、§3.29〜3.35 に上限と暗黙アカウント、新設 §3.29a、スナップショットの欄）。
- 生成物: `UPDATE_SCHEMA=1 cargo test -p task-api` で `docs/api/v1/api-v1.schema.json`、`corepack pnpm@12.6.0 -C web gen:types` で `web/api/generated/{schema.json,types.ts}`。
  新しい必須欄に合わせて `web/features/ops/accounts-screen.test.tsx` の fixture だけ直した（画面の変更は WU `web`）。

## 試験

| 受け入れ条件 | 試験 |
| --- | --- |
| 0: PATCH の正常・検証エラー・404・null 解除 | `crates/celeris/src/daemon/api_account_limits_tests.rs::patch_account_limits_validate_persist_and_survive_reload_and_restart`（本物の API・`handle_admin_request`・`Dispatcher`。400 は 0/33/-1/1.5/"3"/{}/未知欄/配列/不明 adapter/不正 id、404 は実の欠落・暗黙の欠落・プール行、409、401、`check`/`login` の id）、`crates/task-api/tests/accounts_admin.rs::implicit_accounts_are_listed_and_patchable_without_the_accounts_section`、`standby.rs`（PATCH も 503） |
| 1: 暗黙アカウントが GET に出て PATCH で編集できる | 上の 2 つ（`[accounts]` 無しでも編集可、`legacy_concurrency` → `account` → null で戻る、`at_capacity`） |
| 2: reload 後も残る | 上の celeris 試験（`POST /reload` 後の GET、再起動＝同じ DB で `build_dispatcher` し直しても残る）。providers の concurrency が dispatch に効かないこと: `POST /providers {"concurrency":5}` の行が reload 後に既定 2（`implicit_account_limit`）、`provider_concurrency_is_accepted_but_not_saved` |
| 3: schema drift・生成型 | `cargo test -p task-api --lib schema`（drift 検査）通過、`gen:types` 済み、`pnpm typecheck` と `vitest run features/ops`（55 件）通過 |

- `TMPDIR=<workspace>/test-tmp cargo test --no-fail-fast -p task-api -p celeris -p task-ops -p task-dispatch -- --test-threads=4`: 2511 passed、5 failed。
  失敗は `task-api` の `browser_e2e`（4）と `browser_waits`（1）だけで、workspace の長い TMPDIR だと credentiald の UNIX socket のパスが SUN_LEN（108）を超えて
  「timed out waiting for fixture readiness」になる環境要因。`TMPDIR=/tmp` では 4 + 6 件とも通る（小さな tempdir のみ、自動削除）。
- `cargo test -p e2e --test provider_admin_scenarios --test account_pool_scenarios --test api_scenarios --test codex_account_pool_scenarios --test multi_account_scenarios`: 全通過
  （`provider_admin_scenarios` の PATCH concurrency の期待値を「保存しない（1）」に更新）。`task-api/tests/providers_admin.rs` の 2 件も同様に更新。
- `cargo clippy --workspace --all-targets -- -D warnings`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check`・`git diff --check`: exit 0。

本番 config・systemd・daemon・本番 DB は変更していない。

## 後続への引き継ぎ

- `web`: `AccountView` の新しい欄で上限を表示・編集する。`PATCH /accounts/{adapter}/{id}` の本文は `{max_runs: number | null}`（型は生成型に無いので画面側で書く）。
  暗黙アカウントは `implicit: true`・`dir: null`、ログイン・確認・削除のボタンは出さない。
- `docs`: 本番 config から `concurrency` を消す前に暗黙アカウントの `max_runs` を PATCH で保存する手順（§3.29a）。

## 再実行（run 01M4DFW5BT049PS2G0DKTZ1CKJ）

- check `gen:types && git diff --exit-code -- web/api/generated` の不合格は、生成物が最新だが index に載っていなかったため（`git diff` は作業木と index を比べる）。
  `web/api/generated` と `docs/api/v1/api-v1.schema.json` を `git add` した。再生成後の `git diff --exit-code` は 0。
- `cargo test -p task-api -p celeris`（短い TMPDIR）: 0、`cargo clippy -p task-api -p celeris --all-targets -- -D warnings`: 0。
- scope check の不合格（`crates/task-ops/src/daemon.rs`・`crates/task-ops/src/inbox/tests.rs`・`tests/e2e/tests/provider_admin_scenarios.rs`・
  `web/features/ops/accounts-screen.test.tsx`）は計画の範囲の取り方の問題として申告した（plan_issue）。どれもこの WU の変更に必然的に伴う:
  `DaemonSnapshot`（暗黙アカウントの観測値 `implicit_accounts`、`ProviderLive.concurrency` の deprecated 印）は task-ops にあり、
  e2e は「PATCH /providers の concurrency を保存しない」挙動の期待値、web の fixture は `AccountView` の新しい必須欄に追随したもの。

## 範囲チェックの再確認（run 01M4DGN0MXF2T60CB9J49D03FM）

- 指定された cwd と command をそのまま実行し、exit 1、`out of scope: tests/e2e/tests/provider_admin_scenarios.rs` を再現した。
- check の許可パターンは `e2e/` だが、workspace の Cargo.toml と実ディレクトリは `tests/e2e/`。この fixture の変更は deprecated concurrency の PATCH 応答を 5 ではなく 1 とする、ADR §(a) に必要な追随である。
- 計画の範囲チェックに `tests/e2e/`（最小なら `tests/e2e/tests/provider_admin_scenarios\.rs$`）を追加する必要がある。チェック自体はこの run では変更していない。差分を取り消して試験を旧仕様に戻すこともしていない。
- 再検証: `cargo test -p task-api --test accounts_admin --test providers_admin`（36 件）、`cargo test -p celeris patch_account_limits_validate_persist_and_survive_reload_and_restart --lib`（1 件）、`cargo test -p e2e --test provider_admin_scenarios`（3 件）、`cargo test -p task-api --lib schema`（3 件）、`corepack pnpm@12.6.0 -C web gen:types && git diff --exit-code -- web/api/generated`: すべて exit 0。
- artifacts/result.json に `yield.plan_issue` を記録し、done は返さず replan に引き継ぐ。本番への操作は行っていない。

## 再開（run 01M4DSKZ11J5P09AJBT30VWM6K）

- 計画の指示どおり `tests/e2e/tests/provider_admin_scenarios.rs` の期待値変更（deprecated concurrency の PATCH 応答は 1）は残した。コードの追加変更は無し。
- 再検証: `cargo test -p task-api -p celeris`: 0（celeris lib 412 件、task-api 全件）。`browser_e2e`・`browser_doctor` は run の TMPDIR が長く
  Unix socket の SUN_LEN を超えるので短い TMPDIR で実行して通過。`db_maintenance::spawned_tasks_run_on_their_interval_and_stop_cleanly` は
  1 回だけ時刻依存で落ち、再実行 2 回で通過（この WU で触れていない file）。
- `cargo clippy -p task-api -p celeris --all-targets -- -D warnings`: 0。`gen:types && git diff --exit-code -- web/api/generated`: 0。
  `cargo test -p task-api --lib schema`: 0。`cargo test -p e2e --test provider_admin_scenarios`: 0。
