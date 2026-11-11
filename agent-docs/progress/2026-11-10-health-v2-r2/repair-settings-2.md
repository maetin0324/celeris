---
title: 健全性v2 R2 unit repair-settings-2 — settings 段の統合の health（dispatcher module の行の増加）の修正
tasks: [01M6TYVJT0Z1CBGNK8VTNK8ZJH]
status: done
updated: 2026-11-11
completed: 2026-11-11
---
# 健全性v2 R2 unit repair-settings-2 — settings 段の統合の health の修正

task 01M6TYVJT0Z1CBGNK8VTNK8ZJH の repair WorkUnit repair-settings-2 の記録。基準は `$CELERIS_WU_BASE`（段 settings の基点は統合 commit『integrate wu/browser-port (phase impl)』）。

## 指摘

- `health:size.responsibility-growth`（段 settings の統合の指摘）。module `crates/task-dispatch/src/dispatcher` の本体の行（空行・注釈・use・mod 宣言を除く）が段の基点 35116 → 統合後の HEAD 35117。上限 8000 を超えた module は 1 行でも増えると落ちる。
- 増えたのは commit『phase 1: fold WorkerAdapter with_* into configure(AdapterSettings)』（cos_chat/launch.rs +11、route_launch.rs +7、worker_task.rs -16）。launcher-login-fix は無関係。
- repair-settings-1 は直す file（task-dispatch と task-worker/src/adapter.rs）が範囲外として plan_issue で返した。operator が comment（2026-11-11T02:16Z）で直し方と範囲（R2 の write_set の 5 file と adapter.rs）を示した。

## 直し方（owner の側で重複を消す）

「`AdapterSettings::env_only(..)` → `configure(&settings).resolve(adapter)` → `unsupported.contains(&AdapterSetting::Env)`」の同じ形が 5 か所にあった。判定を `crates/task-worker/src/adapter.rs` に 1 つ置き、5 か所をそれを呼ぶ形にした。

- `Configured::resolve_env(base) -> Result<Arc<dyn WorkerAdapter>, EnvNotAccepted>`: `Env` が unsupported にあれば `EnvNotAccepted`、無ければ `resolve` と同じ adapter。`Env` 以外の受けなかった項目（route の model）は見ない。
- `configure_env(adapter, env)`: `AdapterSettings::env_only(env)` を 1 回の `configure` で適用し、`resolve_env` で判定する。`lib.rs` から `EnvNotAccepted`・`configure_env` を公開した。

各所の失敗時の扱いは変えていない。

| 呼び出し側 | 使う関数 | 受けなかったときの扱い（従来どおり） |
|---|---|---|
| `dispatcher/eval_trials.rs`（eval の CARGO_TARGET_DIR） | `configure_env` | trial を失敗にして作業ツリーと target を片付け、`Err` |
| `dispatcher/housekeeping.rs` `with_review_cargo_env` | `configure_env` | 先頭 repo が Rust なら warn、元の adapter と `None` |
| `dispatcher/route_launch.rs` `route_adapter`（model + account env） | `configure(&settings).resolve_env(base)` | fail closed で `AccountUnavailable` |
| `dispatcher/cos_chat/launch.rs` `adapter_with_run_env` | `configure_env` | run credential を revoke して `Err` |
| `review.rs` `with_run_tmpdir` | `configure_env` | warn して元の adapter と `None`（with_run_tmpdir の中だけを変えた） |

受けなかったとき従来は `resolve` の返す adapter（何も適用しないので base の clone）を返していた。今は base の adapter をそのまま返すので、返る adapter は同じ。旧い書き方（`env_only` → `resolve` → `contains(&Env)`）は 5 か所とも消した。`env_only` は `configure_env` の中と task-worker の試験（`browser_tests.rs`）が使う。

## module の行の前後

`repo_health.py --repo . --base <段の基点> --head HEAD --profile celeris --policy-version 1 --rules size.responsibility-growth`（提出時の HEAD で実行）:

| | dispatcher module の本体の行 |
|---|---|
| 段の基点（統合『integrate wu/browser-port (phase impl)』） | 35116 |
| 修正前の HEAD（統合『integrate wu/launcher-login-fix (phase settings)』） | 35117（fail） |
| 修正後の HEAD | 35103（pass、exit 0、worsened=0） |

## 試験

`adapter.rs` の `settings_tests` に `adapter_settings_configure_env_reports_whether_env_was_taken` を足した。守る不変条件: env を受ける adapter は env を重ねた複製を返し（run の env に末尾で載る）、受けない adapter は `EnvNotAccepted` を返す（呼び出し側の fail closed・警告の根拠）。空の env は元の adapter のまま。`resolve_env` は `Env` 以外の受けなかった項目を失敗にしない。既存の試験は呼び出し側ごと（`route_launch`・`eval_trial_dispatch`・`run_tmpdir` など）に結果を見ていたが、判定そのものを 1 か所で見る試験は無かった。browser-specialist の試験の run の組み立ては同じ module の helper `run_once` に移して共用した（試験の中身は同じ）。

## 証拠（提出時の HEAD で実行）

- `repo_health.py … --rules size.responsibility-growth`: exit 0（上の表）。
- `cargo fmt --all -- --check`: exit 0。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `bash scripts/dev/test-affected.sh`: 下の「影響試験」に結果を書く。

## 未解決事項

- この WU の範囲外差分の検査（objective の正規表現）は launcher-login-fix の write_set（browser_launcher・credentiald など）を写したもので、operator が認めた 5 file と `crates/task-worker/src/adapter.rs`・`lib.rs` に合わない。この検査は exit 1（out of scope）になる。operator の comment の範囲で直したので、file は変えていない。`lib.rs` の変更は adapter.rs の新しい関数を公開する `pub use` の欄だけ。

## 提案

- repair WU の範囲外差分の検査は、段の統合の health の指摘なら段の write_set（または operator が認めた範囲）から作る。

## 影響試験

- `bash scripts/dev/test-affected.sh`（提出時の HEAD で実行。対象は celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker）: 前段の repo-health は verdict=pass。nextest は 5869 件中 5866 件合格、22 件 skip。残る 3 件（`dispatcher::tests::project_wake::soak::project_controller_soak_72h_{enabled,shadow,off}`）は、run の壁時計に収めるため外から付けた `timeout 780` の SIGTERM で約 82 秒の時点で打ち切られた。試験自身の失敗ではないが、合格も未確認。exit は 124（timeout）。この 3 件は adapter の env の経路を通らない。再実行は次の検査（統合の check）に任せる。
