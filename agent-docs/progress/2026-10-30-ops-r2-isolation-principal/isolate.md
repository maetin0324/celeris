---
title: 評価器を別の信頼境界に置く入力隔離と、deny_readable で止める preflight（unit isolate）
tasks: [01M66JYQ249SZHNXK0AEVTZNEC]
status: done
updated: 2026-10-30
---
# 評価器を別の信頼境界に置く入力隔離と、deny_readable で止める preflight（unit isolate）

完了日: 2026-10-30

ADR は `agent-docs/adr/2026-10-30-ops-r2-isolation-principal.md`、人の決定は iso-boundary（評価器専用の別 uid を人が作る）。
運用文書は [ops-r2 の入力隔離](../../../docs/ops/ops-r2-isolation.md)。各 leaf の詳細は同じ dir の
[leak-runtime](leak-runtime.md)・[runtime-probe](runtime-probe.md)・[sandbox-script](sandbox-script.md)・[preflight-gate](preflight-gate.md) にある。

## 実装範囲

- L1（評価器の別 uid）: コードは変えていない。`celeris-eval` uid と `EVAL_ROOT` は人が作る（運用文書 §4）。preflight は人の uid から `EVAL_ROOT` の下の path を開き、開けたら止める。
- L2（runtime の mount 許可表）: A は `Sandbox::runtime_args`（A の実起動と同じ関数）、B の測定 daemon と operator session は `scripts/campaign/ops-r2-sandbox.sh`。
- 確認: `celeris-campaign probe-read` が主体の包み方の中で deny path を開き、mountinfo の source を許可表と照合する。
- 止める: 正式試行の preflight が主体（A・B・operator・L1）ごとに probe を走らせ、`deny_readable` が 1 件でも残れば有料実行・task 作成を 0 件で止める（`--deny-list` 等が欠けても止める）。
- 漏洩検査: `check-ops-r1-leak.py --runtime-probe` が probe の結果で可読判定を置き換え、runtime と L1 の両方で拒否された path だけ `same_uid` を外す。
- 同じ uid の chmod やプロンプトの禁止は隔離に数えていない。L1 の probe は同じ uid では必ず hit になり、試験 `preflight_principals_probe_with_their_real_wrapping` がそれを確かめる。

## 変更（commit 件名）

- `leak-runtime: check-ops-r1-leak.py に --runtime-probe（probe で deny_readable を判定、runtime と L1 の拒否で same_uid を外す）`
- `phase 1: add runtime read probe and shared A sandbox arguments`（runtime-probe）
- `phase 1: add ops-r2 sandbox mount allowlist and trial HOME`（sandbox-script）
- `base-record: leak-runtime の進捗記録（--runtime-probe の判定規則、統合 v1 の失敗と v2 の検査差し替え）`
- `preflight-gate: 正式試行の preflight が主体ごとに runtime probe を走らせ deny_readable で止める`
- `preflight-gate: 進捗に v2 範囲 check の不合格と v3 での再検査を記録`
- close-docs: `docs/ops/ops-r2-isolation.md`（新規）とこの進捗

## 証拠

各 leaf の証拠（各 leaf の提出時の HEAD で実行。基準は $CELERIS_WU_BASE）:

| leaf | コマンド | 結果 |
|---|---|---|
| leak-runtime | `python3 scripts/campaign/test_check_ops_r1_leak.py` | exit 0、34 tests OK |
| runtime-probe | `cargo nextest run -p celeris-campaign --test runtime_probe` | exit 0、5 passed（実 bwrap の 1 件は opt-in 無しで skip） |
| runtime-probe | `bash scripts/dev/test-affected.sh` | exit 0、2,862 passed・9 skipped |
| sandbox-script | `python3 scripts/campaign/test_ops_r2_sandbox.py` / `test_ops_r1_measure_config.py` | exit 0、5 tests / 10 tests |
| preflight-gate | `cargo nextest run -p celeris-campaign --test preflight` | 30 passed（deny_readable で止まる新規 3 件を含む） |
| preflight-gate | `CELERIS_USERNS_TESTS=require cargo nextest run -p celeris-campaign --test runtime_probe` | 6 passed（実 bwrap と実 `ops-r2-sandbox.sh` の中で許可表の外を読めない。L1 は同じ uid なので hit） |
| preflight-gate | `bash scripts/dev/test-affected.sh` / clippy / fmt | exit 0、2,866 passed・9 skipped / exit 0 / exit 0 |

close-docs（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）:

- `python3 scripts/campaign/test_check_ops_r1_leak.py` → exit 0、34 tests OK。`python3 scripts/campaign/test_ops_r2_sandbox.py` → exit 0、5 tests OK
- `sh scripts/dev/check-doc-links.sh` → ok。`sh scripts/dev/check-adr-numbers.sh` → ok（350 files）。`sh scripts/dev/progress-index.sh --check` → ok
- `bash scripts/dev/test-affected.sh` → exit 0（試験対象の変更なし。docs のみ）
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0
- `sh "$CELERIS_WU_SCOPE_PATHS"` → この 2 file だけ（範囲内）

## 実 runtime の拒否確認

- 実施済み: preflight-gate の run の host（bwrap が使える）で、一時 dir の fixture に対して A（実 bwrap）・B と operator（実 `ops-r2-sandbox.sh`）から許可表の外の file を読めず、親を一覧できず、許可外の mount source が 0 件であることを確かめた。
- 未実施: 本番の deny list（`EVAL_ROOT` の private、repo clone の grader-private、`~/.claude` 等）と、測定専用 account・release に対する確認。
- 未実施: 実 uid `celeris-eval` での L1 の確認。人の準備（運用文書 §4）の後に、§4.7 の手順で人が確かめる。それまで正式試行の preflight は L1 で止まる。

## 人が準備する OS 権限・設定

運用文書 §4 にコマンドと確認方法を書いた。

1. `celeris-eval` の uid・group の作成
2. `EVAL_ROOT`（0700）・drop box（1733）・outbox（0755）の作成
3. 採点 wrapper・binary の配置（`celeris-eval` 持ち、人は書けない）
4. 評価器の起動方法の選択（狭い sudo 規則か `sudo -iu`）
5. 正式 case の private を `EVAL_ROOT/private/final/<case>/` へ置く
6. 測定専用 Claude account dir（B 用・operator 用）の login
7. 人の uid から `EVAL_ROOT` の private を開けないことの確認

本番設定・本番 daemon・本番 DB はこの task で変えていない。

## 未解決

- 実 uid での L1 の確認は、人の準備の後（運用文書 §4.7）。
- K2 の private.json は git の履歴と既存の clone に残る。L2 で塞いだうえでそのまま使うか、作り直すかは ADR の「人に決めてもらう点」1 で人が決める。
- 採点 wrapper（drop box → 採点 → 漏洩検査 → outbox）はまだ無い。今の採点は人の uid で `--grader-root` を読む。
- bwrap の中から sudo が効かないこと（no_new_privs）は実機で未確認。
- release dir に repo の source（fixtures）が入っていないことは未確認。
- deny list は glob を展開しない。承認票の `/local/celeris/data/workspaces/*/repos/*` のような項目は実在の path に展開して書く。

## 提案

- 採点 wrapper を `scripts/campaign/` に置く unit を、人の準備（uid と dir）の後に足す。L1 の評価器経路はそれまで手順だけで閉じていない。
- 運用文書 §4.7 の L1 確認と preflight の deny_readable 0 件を、K2 の開始前チェック（ADR の close 段の `ops-r2-k2-preflight.md`）の必須項目にする。
