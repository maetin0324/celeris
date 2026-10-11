---
title: "アカウントごとの同時実行数（task 01M4D7RVKX / 子 01M4EVE9BV）を main に統合する"
tasks: [01M4D7RVKXY9A967ZXDADWVHQJ, 01M4EVE9BVH163074JJNV683Z4]
status: done
updated: 2026-10-11
---

# アカウントの同時実行数の統合（2026-10-11）

- 統合したもの: task workspace の branch `celeris/01M4D7RVKXY9A967ZXDADWVHQJ`（HEAD 073a56c4、merge-base 14cddd28 の上に 9 commit）を
  branch `ops/account-concurrency-merge`（main 0f58d1bd から）へ `--no-ff` で merge。決定は
  [ADR 2026-10-08-account-concurrency-limits](../adr/2026-10-08-account-concurrency-limits.md)、元の進捗は
  [2026-10-08-account-concurrency-limits.md](2026-10-08-account-concurrency-limits.md)。
- 子 task 01M4EVE9BV の workspace: `5c721599`（dispatch: share implicit account capacity by LLM source）と `3abc6838`（docs）は親 branch の
  祖先ではないが、同じ内容（暗黙アカウントを LLM source 単位にする・旧 concurrency を既定値から外す）を親 branch の `ca56a55c`・`64d419ff`・
  `09c10512` が実装し、レビューを通ったのは親 branch。よって親 branch だけを統合した。子 workspace の `knowledge curation 2026-10-09` 4 commit は
  試験の一時 dir（`t/.tmp*/`）を commit したごみなので取り込まない。

## 衝突と解き方

- `crates/task-core/src/store/migrations.rs`: branch の migration `0062_account_limits.sql` は main の 0062〜0065 と番号が衝突。
  **`0066_account_limits.sql` に振り直し**、`SCHEMA_VERSION = 66`。
- schema 版数の試験（`chat/tests.rs`・`cluster_job/tests.rs`・`cron/store_tests.rs`・`delivery.rs`・`store/routing_log_tests.rs`・`store/tests.rs`）: 66 に揃え、
  `delivery.rs` の版の列に 66 を足した。
- `crates/celeris/src/daemon/admin.rs`（reload）: main の `reload_cos_chat_launch` と branch の `set_provider_llm_sources`・`prune_implicit_account_limits` を両方残した。
- `crates/task-api/src/handlers/accounts.rs`: main は `create_account` を同期の `create_account_op`（CoS の `account.create` と共有）に分けていた。
  `AccountView` の新しい欄（`max_runs` など）は `create_account_op` に入れ、古い上限の掃除（`clear_stale_limit`、async）は呼び出し側で行う:
  HTTP の handler は作成後に `await`、CoS の `account.create` は監査 transaction の**後**に、その要求が実際にディレクトリを作ったとき（`state = applied`）だけ行う
  （transaction の中で store に触ると store を待って詰まる。`cos_ops_admin_config` が 600 秒で時間切れになったのはこれ）。
- `crates/task-dispatch/src/dispatcher/tests/cos_chat_launch.rs`: branch の試験と main の `mod source_fallback` を両方残した。

## main 側の新しいコードの手直し（旧 provider concurrency の利用）

- dispatch の新しい経路（model routing の `provider_select`・`routing_shadow`・`routing_enforce`、CoS の source fallback、escalation 候補、
  browser fallback）を確認した。上限に使っていたのは `provider_select` の `concurrency_limit` と CoS launch の満杯判定で、どちらも branch の
  merge で暗黙アカウントの上限（`implicit_account_limit` / `implicit_account_full`）になっている。`routing_enforce` は `in_use` を観測値として
  渡すだけ。`dispatch_run.rs` の browser fallback の doc comment にあった「non-zero concurrency」を直した（コードは concurrency を見ていない）。
- main で増えた試験の手直し: `cos_source_fallback.rs`（`ProviderSpec.concurrency` は `Option`。`max_runs_per_account = 0` の試験は acp 行の
  暗黙アカウント `implicit/p3` に 1 を保存）、`celeris daemon::cos_reload_tests`（保留したままの run が 2 本を超えるので両 source の暗黙アカウントに 8 を保存）、
  `task-api tests/cos_ops_admin_config.rs`（`PATCH /providers` の `concurrency` は保存されないことを確かめる形に）、branch の
  `cos_chat_pool_uses_account_override_plus_one_and_ignores_provider_zero`（main の source fallback 以降、pinned アカウントが満杯なら入力は queue に残らず
  経路が使えないと報告されるので、「3 本目はそのアカウントで走らない」を確かめる形に）。
- CoS の操作（ADR 2026-10-09-cos-operations-all-mutations: 全変更を ALLOWED か EXCLUDED に分類する）: 新しい `PATCH /accounts/{adapter}/{id}`
  （と `check`・`login` の静的な路）を `account.limit_set`（外部効果）として ALLOWED に加えた。PATCH の本体を `set_account_limit_effect` に分けて
  HTTP と CoS で共有。ADR の領域表・付記、`config/skills/cos-operator/operations.md`、`docs/api/v1/api.md` の CoS 表に行を足した。
- web: `providers-screen.tsx` の状態の表から「同時実行」列を外した（branch はアカウント画面への link だけの列を残していた）。form に入力欄は無く、
  `providers-form.ts` は `concurrency` を送らない。アカウント画面は上限（1〜32）を表示・編集する（branch のまま）。旧 gui の `providers.tsx` には欄が無い（型の deprecated 欄だけ）。
- 範囲外として触っていないもの: llm-proxy の `max_concurrent_per_account`、`ResourceGroupEntry.concurrency_limit`、`[execution] max_concurrency` / `max_cos_runs`、
  `[[clusters]].concurrency`。API 応答の `concurrency`（deprecated、表示のみ）は ADR §(a) のとおり残る。

## 証拠

すべて `CARGO_TARGET_DIR=/local/celeris/data/scratch/merge-acct-conc-target`（ローカル disk）で、commit 直前の tree に対して実行。

- `bash scripts/dev/test-parallel.sh` → exit 0。nextest 5128 run / 5128 passed / 13 skipped、summary `passed 5131, failed 0, ignored 14`（doc-test 込み）。
  手直し前の 1 回目は 5 failed（上の main 側の試験 4 本と CoS registry 1 本）、途中で `cos_ops_admin_config` の 600 秒時間切れ（transaction 内の store 待ち）を直した。
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0（merge 直後は `providers.rs` の needless_borrow 1 件。直した）。
- `cargo fmt --all -- --check` → exit 0。
- web（`pnpm install --frozen-lockfile --offline` の後）: `pnpm run typecheck` exit 0、`pnpm run lint` exit 0（既存の warning 4）、
  `pnpm test` exit 0（vitest 92 files / 676 tests passed、node --test 93 passed）、`check:boundaries`・`check:parity` exit 0、
  `pnpm run build` の後 `check:secrets` exit 0。
- `sh scripts/dev/check-doc-links.sh` → exit 0。
- 残りの provider concurrency の利用の検査: `grep -rn -E 'concurrency_limit|provider_full|legacy_concurrency|in_use_cos|\.concurrency\b' crates/task-dispatch/src crates/task-api/src crates/celeris/src`
  → `provider_full`・`legacy_concurrency` は 0 件。残りは表示用の deprecated 欄（`ProviderView`/`ProviderConfigView`/`ProviderLive`）、観測値の `in_use_cos`、
  範囲外の model routing の資源グループ・クラスタ、`provider_select` の `concurrency_limit`（暗黙アカウントの上限から作る）だけ。

## 本番の移行（人の手順。ADR §既定上限・docs/ops/provider-llm-source-migration.md）

本番の config（`~/.config/celeris/config.toml`、読むだけ）では Qwen の 2 行 `opencode-qwen`・`pi-qwen` が両方
`llm_source = "openai_compatible:qwen"`・`concurrency = 1`。新しいリリースでは `concurrency` は無視され、2 行は 1 つの暗黙アカウント
`implicit/openai_compatible:qwen` を共有し、保存値が無ければ既定の 2 本まで走る。GPU 1 枚なので、配備の後に:

```
PATCH /api/v1/accounts/implicit/openai_compatible:qwen
{"max_runs": 1}
```

（管理 token が要る。アカウント画面の暗黙アカウント `openai_compatible:qwen` の「同時実行数の上限」に 1 を保存しても同じ）。
その後 config から `concurrency = 1` を外してよい（残しても警告が出るだけ）。

同じく `concurrency = 1` を書いた `ldr-qwen`（`model = "celeris/cheap"`）・`langmem-main`（`[knowledge.langmem] model = "celeris/cheap"`）・
`paperqa-qwen`（model は `[adapters.paperqa]` 側。proxy のモデルなら同じ）は llm_source を書いておらず `celeris`（LLM source proxy）に
導出されるので、暗黙アカウント `implicit/celeris` を共有し、既定では合わせて 2 本まで走る。旧来どおり 1 本に保つなら
`PATCH /api/v1/accounts/implicit/celeris` に `{"max_runs": 1}`（任意。proxy は Qwen が落ちていれば Claude/GPT に倒すので、Qwen の GPU の保護は上の 1 件が主）。

## 未解決・提案

- CoS の `account.limit_set` は registry・到達性の試験で覆うが、daemon 付きの往復の試験は無い（HTTP の PATCH の試験は branch の
  `api_account_limits_tests` が持つ）。
- `gui/docs/celeris-api-v1.md` は main の時点で既に `docs/api/v1/api.md` と食い違っており、今回も追随させていない。
