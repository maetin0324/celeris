# 試行の開始時に実際に動いた版を記録し、宣言と違えば拒否する（main-4 契約 U4・U11）

- 日付: 2026-10-27
- 状態: 採用（実装済み: `crates/campaign/src/versions.rs`・`manifest.rs`（`contract`）・`runner.rs`・`checker.rs`・
  `celeris_exec.rs`・`harness_exec.rs`、`task_core::pricing::PRICE_TABLE_VERSION`、`GET /health` の 2 欄）
- tasks: [01M5XXF7QAXTD8E7MM6DF25JRG]
- 関連: [2026-10-18-measure-env-launcher](2026-10-18-measure-env-launcher.md)（env-check・worker_programs）、
  [2026-10-21-preflight-env-lock-check](2026-10-21-preflight-env-lock-check.md)（env-lock の版固定）、
  `docs/ops/mlsys-main-4-ab-contract.md` §10 U4・U11

## 背景

`TrialRecord` の `harness_version`・`model`・`model_version`・`policy_version` は manifest の宣言の写しで、観測値ではない。
A は `A_PROGRAM`・`A_VERSION`、B の run は測定 daemon の `[adapters.claude_code] command` で別々に固定され、
両腕が同じ Claude Code の実体・版で動いたことを試行ごとに照合する検査が無かった（U4）。preflight・env-lock は測定開始前に
1 回固定するだけで、測定中に binary や daemon が差し替わっても試行は走る。B の費用推定に使う単価表にも版の識別子が無く（U11）、
違う単価表で数えた試行を見分けられなかった。

## 決定

- D1（契約 revision）: campaign manifest に省略可の `contract = {revision, pinned_versions}` を足す。`revision` は既知の名前だけ
  （現在は `main-4-ops-r1`。CoS・外部 operator を含む通常運用の評価条件の改訂と同じ名前）。`contract` を宣言した manifest では、
  有効 arm の全部が `pinned_versions.<arm>` を持つ: `claude_code_version`（`--version` の先頭語）、`claude_code_sha256`（64 hex）、
  B/C は `pricing_version`（必須）と `cos_policy_version`（CoS を使う構成だけ）。A に daemon の欄を書くと拒否する。
  B/C の release の宣言は従来どおり `configs[].harness_version`。`contract` が無い manifest は直列化も hash も挙動も変わらない
  （固定済み bundle `final-freeze-2`・pilot の条件は変えない。新しい欄は新しい revision の契約だけで必須）。
- D2（開始時の観測と拒否）: runner は契約 revision の試行を、routing・投入の前に `Executor::observe_versions` で観測する。
  B/C（`CelerisExecutor`）は測定 daemon の `GET /api/v1/health` の `release`・`pricing_version`・`cos_policy_version` と、
  `--worker-program claude=<path>` の実行ファイルの sha256・`--version`。A（`HarnessExecutor`）は起動する Claude Code の
  実行ファイルの sha256・`--version`（daemon の欄は `None`）。観測は `TrialRecord.observed_versions` に残し、宣言の写しの 4 欄は
  そのまま。宣言との不一致・観測できない欄があれば、その試行は task を作らず `executor_error`（本文 `version_mismatch: …`）にする
  （課金しない。routing の投入前拒否と同じ扱い）。観測値は投入前の台帳行（`plan_with`）に fsync し、再開（投入済み・待ち）の試行は
  観測し直さず開始時の値で記録する。
- D3（単価表の版）: `task_core::pricing::PRICE_TABLE_VERSION`（現在 `2026-09-approx-1`）を置き、`PRICE_TABLE` を変えたら必ず
  変える。daemon は `GET /health` の `pricing_version` で公開する。
- D4（CoS の方針の版）: daemon は `GET /health` の `cos_policy_version` に `[cos.triage] policy_version` を出す（CoS が無効なら
  `null`）。外部 operator の方針の版は daemon から観測できないので、この決定では照合しない（human-ops の `policy_ref` に残る）。
- D5（事後検査）: checker は契約 revision の実行した試行（`executor_error` 以外）に `observed_versions` が無い、または宣言と
  食い違えば違反にする。

## 採らなかった案

- 宣言の写し（`harness_version` 等）を観測値で上書きする: 既存の checker・export・bundle の意味が変わる。写しは残し、観測は別欄にした。
- daemon に Claude Code の版・sha256 を報告させる: daemon の起動時の値しか出せず、試行の開始時の実体を示さない。campaign が
  env-lock と同じ `pin_program` で同じ path を読む方が、env-lock の固定とも食い違わない。
- 不一致でも記録だけして走らせる: 違う版の試行が分母に入り、§9 の「比較可能でない値」を後から除く作業になる。
