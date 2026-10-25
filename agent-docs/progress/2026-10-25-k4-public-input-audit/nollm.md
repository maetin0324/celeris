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
  sha256 `fa0934224cb3bed29b1e0d2ed1c8f6cb087822b6666bbfc751e959401d887250`（旧 bundle の lock は `abdf0b61…`）。
  K4 の inputs は `main-v3-n2-43706-public2.tgz`/`0671a855…`、private `46ea074b…`、grade-case `7751a857…`。bundle には書いていない（stdout を記録へ出しただけ）。
- 旧資産: 旧 tgz（`128057cf4016535941963fc4142ba424c05c65889905ca2bbcc190f6161077a1`）と旧資産一覧 `old-assets-after.json` の 63 file、計 64 file の sha256 が実行前後で同じで固定値と一致（`old_assets.unchanged: true`）。

## 変えたもの
- `crates/campaign/tests/preflight_input_archive/real.rs`: `real_archive_offline_audit` が各 case の argv・exit・期待 exit を全部記録してから一致を確かめる形にした。
  数値誤り・失敗 job の W2 を有効・欠測を分母から落とす負例の grade を足し、preflight の拒否で transport 呼び出し数と out の有無を残す。
  出力は `audit.json`。fixture 全体の写し（1 GB 超）はやめ、manifest・case・grade-case だけ残す。
- `crates/campaign/scripts/nollm-k4-v2.sh`（新規）: build、lock、上の試験、旧資産の前後照合をして `summary.json` を書く。

commit: 「campaign: K4 v2 の無 LLM 通しを script にし、独立負例と lock を記録する」。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `sh crates/campaign/scripts/nollm-k4-v2.sh <public2.tgz> …/artifacts/k4-nollm-v2` → exit 0、15 case 全 ok、old assets unchanged True（64 files）、pin match True。
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` の結果は下の「検査」に書く。

## 未解決事項
- 偽 Host・模擬 executor の通しで、実 launcher・daemon・cap-env・release 接続は pending のまま（測定開始の証拠ではない）。
- main-4 bundle の lock.json・pins は旧 v1 のまま。新 expected_lock を bundle に固定するかは close WU・P0-2 の判断。

## 提案
- P0-2 は bundle を作り直すとき、本記録の lock sha256 と再計算した lock を照合する。
