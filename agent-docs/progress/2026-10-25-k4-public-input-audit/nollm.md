---
title: K4 公開入力 v2 の無 LLM 通し
tasks: [01M5SRJMA9CFNG2C9KCCS5VS4B]
status: done
updated: 2026-10-25
---
# K4 公開入力 v2 の無 LLM 通し

完了日: 2026-10-25

## 要約
- 旧記録（K4 再監査 task の artifacts `mlsys-main-4/nollm/summary.json`）の手順を、新 archive
  `main-v3-n2-43706-public2.tgz`（sha256 `0671a85528b4a29a8f6fb5176986c178afb2174cb22e56b003c33ed5f2da3a1e`、case の pin と一致）と
  v2 の case で再実行した。手順は `sh crates/campaign/scripts/nollm-k4-v2.sh <新tgz> <出力dir>` で再実行できる。
- 記録: `/local/celeris/data/workspaces/01M5SRJMA9CFNG2C9KCCS5VS4B/artifacts/k4-nollm-v2/summary.json`（`cases[]` に name・argv・exit・expect_exit・matches）。
  全 15 case が期待 exit と一致（`all_match: true`）。LLM・HPC・本番 daemon は使っていない。preflight は既存試験と同じ偽 Host の library 呼び出し。

| 種別 | case | exit（期待） | 拒否理由（負例） |
|---|---|---|---|
| lock | `lock` | 0（0） | — |
| 正例 | `stage-positive`・`preflight-positive`・`grade-positive`（32 セル/30 有効）・`grade-out`・`check`・`export` | 0（0） | — |
| 独立負例 | `grade-all-valid-negative` | 1（1） | benchfs W2 の 2 セル `marked valid but the raw shows an invalid cell` |
| 独立負例 | `grade-invalid-w2-as-valid-negative`（失敗 job の W2 だけ有効、32/32） | 1（1） | 同上 |
| 独立負例 | `grade-numeric-error-negative`（有効セル 1 つの write_MiB_s を 2% 誤り） | 1（1） | `chfs/rep1/w1a: write_MiB_s … (tolerance 0.001)` |
| 独立負例 | `grade-missing-dropped-from-denominator-negative`（30/30） | 1（1） | `not in the table (missing cells must stay in the denominator)` |
| 事前拒否 | `stage-missing-negative`・`stage-sha-negative` | 1（1） | archive 不在・pin と sha256 不一致。不一致時に grade-case の隣へ何も置かない |
| 事前拒否 | `preflight-missing-negative`・`preflight-sha-negative` | 1（1） | `graders` の不足だけ。run_gated は拒否、transport 呼び出し 0、out 未作成 |

- lock（新 expected_lock）: main-4 の `judged-scope-v2/pinned/main/final.json` を repo の v2 case・grader・grade-case で lock した。
  sha256 `fa0934224cb3bed29b1e0d2ed1c8f6cb087822b6666bbfc751e959401d887250`（旧 bundle の lock は `abdf0b6147ad830551200218206545a113591d585f4e59bb8498359815d2ae7a`）。
  K4 の inputs は `main-v3-n2-43706-public2.tgz`/`0671a85528b4a29a8f6fb5176986c178afb2174cb22e56b003c33ed5f2da3a1e`、private `46ea074be15202c4b3d2fca143563ce9c06822607f51e656469da6ad5f6871da`、grade-case `7751a8577dd09d3f8cd5729179bfbc819aaf87249bb46f16e3ecb751ce48ca54`。bundle には書いていない（stdout を記録へ出しただけ）。
- 旧資産: 旧 tgz（`128057cf4016535941963fc4142ba424c05c65889905ca2bbcc190f6161077a1`）と旧資産一覧 `old-assets-after.json` の 63 file、計 64 file の sha256 が実行前後で同じで固定値と一致（`old_assets.unchanged: true`）。

## 変えたもの
- `crates/campaign/scripts/nollm-k4-v2.sh`（新規）: build、lock、既存の opt-in 試験 `real_archive_offline_audit`（変更なし。出力は `<出力dir>/audit/`）、
  独立負例 3 本の作成と CLI の grade、旧資産の前後照合をして `summary.json` を書く。
  負例は試験の正例報告（`audit/positive-artifact`）から 1 つずつ別の誤りだけを入れて作り、正例と同じ grade argv の `--artifact` だけ差し替える。
  試験が残す fixture の写し（1 GB 超）は 1 MB を超える file を消し、manifest・case・grade-case だけ残す。
- 前回の attempt は試験 `crates/campaign/tests/preflight_input_archive/real.rs` も変えたが、WU の範囲（script と進捗だけ）の外だったので基点の版へ戻した。

commits: 「campaign: K4 v2 の無 LLM 通しを script にし、独立負例と lock を記録する」、「campaign: K4 v2 無 LLM 通しの負例を script へ移し、試験を基点の版へ戻す」。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `sh crates/campaign/scripts/nollm-k4-v2.sh <public2.tgz> …/artifacts/k4-nollm-v2` → exit 0、15 case 全 ok（`all_match: true`）、old assets unchanged True（64 files）、pin match True。
  負例の拒否理由は各 `grade-*-negative.stderr` の verdict fail の reasons で確かめた（表のとおり）。
- WU の範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"` から script と進捗を除いた残り）→ 空。
- `bash scripts/dev/test-affected.sh` → exit 0、2645 tests run: 2645 passed、7 skipped（crates: celeris celeris-campaign celerisctl llm-proxy task-dispatch）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。`sh scripts/dev/progress-index.sh --check` → ok。

- 再実行（retry）: 初回 run の後の check で `real_archive_offline_audit` が `preflight_setup::copy` の EACCES で落ちた（前回残した `audit/fixture/repo/.git/objects` が読み取り専用で上書き copy できない）。
  script が試験の前に `<出力dir>/audit` を消すよう直し（commit「campaign: K4 v2 無 LLM 通しは前回の audit を消してから試験する」）、
  同じ出力 dir へ check と同じコマンドを 2 回続けて流して両方 exit 0（15 case 全 ok、旧 tgz sha256 OK）。

## 未解決事項
- 偽 Host・模擬 executor の通しで、実 launcher・daemon・cap-env・release 接続は pending のまま（測定開始の証拠ではない）。
- main-4 bundle の lock.json・pins は旧 v1 のまま。新 expected_lock を bundle に固定するかは close WU・P0-2 の判断。

## 提案
- P0-2 は bundle を作り直すとき、本記録の lock sha256 と再計算した lock を照合する。
