---
title: 非プール行の暗黙アカウントを作り、同時実行数をそこで止める
tasks: [01M4D7RVKXY9A967ZXDADWVHQJ]
status: done
updated: 2026-10-08
---
# implicit: 非プール行の暗黙アカウント

2026-10-08。WorkUnit `implicit` の実装（ADR 2026-10-08-account-concurrency-limits §(e)、人の決定 `nonpool-limit` = (ii) implicit-account）。

## 実装

- `ProviderSpec.concurrency` を `Option<usize>`（書かれていたか）にした。`ProviderPolicy::concurrency_limit` は
  `legacy_concurrency() -> Option<usize>` に置き換え、行 id を設定順に返す `provider_ids()` を足した。旧値は暗黙アカウントの既定を作るためだけに読み、何とも直接比べない。
- `capacity::provider_full` を削除した。代わりに `IMPLICIT_ACCOUNT_ADAPTER = "implicit"`、`MaxRunsSource`（`account` / `legacy_concurrency` / `default`）、
  `effective_account_limit(stored, legacy, default)`、`implicit_account_full(in_use_total, limit)` を足した。
- Dispatcher:
  - `implicit_account_ids()`: pool に入っていない行を設定順に並べたもの。config の読み込み・`POST /reload` で policy を差し替えれば、その時点で導き直される。
  - `implicit_account_limit(provider)`: 保存値 `("implicit", <id>)`、無ければ旧 `concurrency`（書かれていれば）、それも無ければ `max_runs_per_account`（`[accounts]` が無ければ組み込みの 2、`DEFAULT_MAX_RUNS_PER_ACCOUNT`）。保存先は `limits` で作った `account_limits` 表と同じ。
  - `implicit_account_full(provider, extra)`: 行の全 run（非 CoS・CoS・レビュー）を数える。CoS の +1 は無い。表を読めないときは満杯として扱う（実アカウントの扱いと同じ）。
  - `prune_implicit_account_limits()`: 対応する非プール行が無い `implicit/*` の保存値を消し、`info` を出す。起動時（`build_dispatcher`）と `reload_providers` の後に呼ぶ。行の削除・pool への変更・`DELETE /providers/{id}` の後の reload がこれに当たる。
- `provider_full` を使っていた経路は全部置き換えた: `provider_select.rs`（ランキング・ローカル優先 L2(c)/L3・sticky）、`routing_shadow.rs`、`cos_chat/launch.rs`（in-flight の chat 起動も数える）、`dispatch_run.rs`（browser fallback の `disabled` は「pool 行か実効上限 0」）。
  routing のモデル候補の `concurrency_limit` は暗黙アカウントの実効上限（pool 行は `None`）。
- 旧 `concurrency = 0`（`limits` unit で警告付きで読む扱いにした）は「run を載せない」の意味を保ち、実効上限 0 になる（黙って既定の 2 へ上げない）。保存値の範囲は 1..=32 のまま。
- config の warn の文言を「暗黙アカウントの既定 max_runs としてだけ使う（implicit/<id>）」にした。LLM source の config に上限の欄は足していない。

## 試験

| 受け入れ条件 | 試験 |
| --- | --- |
| 0: concurrency=1 の非プール行が 1 本で止まる | `non_pool_row_is_limited_by_its_implicit_account`、`implicit_account_counts_cos_runs_without_an_extra_slot`（CoS も +1 なし）、`implicit_account_limit_follows_legacy_concurrency_and_survives_reload`（実 config から） |
| 1: 上書き 2 で 2 本・null で既定・reload 後も残る | 同上 `non_pool_row_...`（2 本 → 解除で既定 1）、`implicit_account_overrides_survive_reload_and_are_pruned_with_their_row`（reload・再起動で残る、行の削除・pool への変更で消える、実アカウント `claude-code/p1` と衝突しない）、celeris の `implicit_account_limit_follows_...`（config の行を消して reload すると消え、戻しても引き継がない） |
| 既定 | `implicit_account_without_legacy_concurrency_uses_the_default`、`capacity::tests::effective_limit_prefers_stored_then_legacy_then_default` |
| 2: provider_full が判定から外れた | `capacity::provider_full` を削除した（grep で 0 件） |
| 3: 試験・clippy | 下記 |

- `TMPDIR=<workspace>/test-tmp cargo test -p task-dispatch -p celeris -- --test-threads=4`: exit 0（task-dispatch lib 878、celeris lib 411、統合試験を含む）。
- `cargo clippy --workspace --all-targets -- -D warnings`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check`・`git diff --check`: exit 0。

## 後続 unit への引き継ぎ

- `api`: `GET /accounts` には `implicit_account_ids()`・`implicit_account_limit()`（実効値と `MaxRunsSource::as_str()`）・`implicit_account_in_use()`・`account_max_runs("implicit", id)` を使えば並べられる。PATCH は `set_account_limit("implicit", id, …)` を使う。存在の確認は `implicit_account_ids()` に含まれるかで行う。
  `DELETE /providers/{id}` は今は file を消すだけで、保存値は次の reload の prune で消える。即時に消すかどうかは api unit で決める。
- 本番 config・systemd・daemon・本番 DB は変更していない。
