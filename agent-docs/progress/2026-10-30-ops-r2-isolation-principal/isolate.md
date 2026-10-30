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
[leak-runtime](leak-runtime.md)・[runtime-probe](runtime-probe.md)・[sandbox-script](sandbox-script.md)・[preflight-gate](preflight-gate.md)・
[l1-owner](l1-owner.md)・[leak-owner](leak-owner.md) にある。

## 実装範囲

- L1（評価器の別 uid）: `celeris-eval` uid と `EVAL_ROOT` は人が作る（運用文書 §4）。preflight は人の uid から `EVAL_ROOT` の下の path を開き、開けたら止める。
  開けなくても、lstat で `EVAL_ROOT` から path までの持ち主が run の uid でも 0 でもなく run の uid から書けないこと、祖先を run の uid が持たず書けないこと
  （run の uid が chmod・rename できないこと）を確かめ、満たさなければ `deny_readable:L1:<path>:<理由>` で止める（`crates/campaign/src/l1_owner.rs`）。
- L2（runtime の mount 許可表）: A は `Sandbox::runtime_args`（A の実起動と同じ関数）、B の測定 daemon と operator session は `scripts/campaign/ops-r2-sandbox.sh`。
- 確認: `celeris-campaign probe-read` が主体の包み方の中で deny path を開き、mountinfo の source を許可表と照合する。
- 止める: 正式試行の preflight が主体（A・B・operator・L1）ごとに probe を走らせ、`deny_readable` が 1 件でも残れば有料実行・task 作成を 0 件で止める（`--deny-list` 等が欠けても止める）。
- 漏洩検査: `check-ops-r1-leak.py --runtime-probe` が probe の結果で可読判定を置き換える。`same_uid` を外すのは、runtime と L1 の両方で拒否され、かつ `owned_by_run_uid`（path と祖先のうち run の uid が持つもの）が空の path だけ。
- 同じ uid の chmod やプロンプトの禁止は隔離に数えていない。人の uid が持つ読める file は L1 の probe で hit になる（試験 `preflight_principals_probe_with_their_real_wrapping`）。
  人の uid が持つ dir と file を chmod 000 にすると probe は拒否になるが、持ち主の判定が `owned_by_run_uid` で止める
  （試験 `same_uid_chmod_000_eval_root_stops_l1_and_an_injected_other_uid_passes`・`cli_preflight_stops_on_a_same_uid_chmod_000_eval_root`）。
  漏洩検査も同じ fixture で `same_uid` を残す（`test_same_uid_chmod_000_keeps_same_uid_even_if_both_probes_deny`）。

## 変更（commit 件名）

- `leak-runtime: check-ops-r1-leak.py に --runtime-probe（probe で deny_readable を判定、runtime と L1 の拒否で same_uid を外す）`
- `phase 1: add runtime read probe and shared A sandbox arguments`（runtime-probe）
- `phase 1: add ops-r2 sandbox mount allowlist and trial HOME`（sandbox-script）
- `base-record: leak-runtime の進捗記録（--runtime-probe の判定規則、統合 v1 の失敗と v2 の検査差し替え）`
- `preflight-gate: 正式試行の preflight が主体ごとに runtime probe を走らせ deny_readable で止める`
- `preflight-gate: 進捗に v2 範囲 check の不合格と v3 での再検査を記録`
- close-docs: `docs/ops/ops-r2-isolation.md`（新規）とこの進捗
- `leak-owner: 漏洩検査は owned_by_run_uid の残る path の same_uid を外さない`・`leak-owner: 進捗に検査結果を書く`
- `l1-owner: preflight の L1 が持ち主の uid と書き換えの可否を lstat で確かめ、同じ uid の chmod 000 で止める`
- `repair-fix-1: launcher 試験の daemon port を SO_REUSEPORT の socket で押さえ、試験 daemon も SO_REUSEPORT で bind する`（fix 段の統合後の検査の修復）
- docs-align: ADR の付記（2026-10-30）、運用文書 §1・§2.3・§2.4・§4・§6、この進捗

## 差し戻しの経緯

- 計画 v3 までの final review（criterion 3）は不合格だった。L1 は `Wrap::Host` の process から `File::open` と `read_dir` の成否だけで判定しており、
  人の uid が持つ `EVAL_ROOT` の下の dir と file を chmod 000 にすると deny_readable 0 件で preflight が通った。漏洩検査も、runtime と L1 の probe が両方拒否なら
  持ち主を見ずに `same_uid` を外していた。前の版のこの進捗の「L1 の probe は同じ uid では必ず hit になる」は、chmod 000 の場合に成り立たなかった。
- 計画 v4 で fix 段（l1-owner・leak-owner）と record 段（docs-align）を足した。L2（A の bwrap と `ops-r2-sandbox.sh` の mount 許可表）は namespace による拒否で、review は問題なしとした。
- 運用文書の `EVAL_ROOT` の例 `/local/celeris/eval` は、祖先 `/local`・`/local/celeris` を人の uid（1001）が持つので新しい L1 で止まる。例を `/var/lib/celeris-eval` に改めた（ADR 付記）。

## 証拠

各 leaf の証拠（各 leaf の提出時の HEAD で実行。基準は $CELERIS_WU_BASE）:

| leaf | コマンド | 結果 |
|---|---|---|
| leak-runtime | `python3 scripts/campaign/test_check_ops_r1_leak.py` | exit 0、34 tests OK |
| runtime-probe | `cargo nextest run -p celeris-campaign --test runtime_probe` | exit 0、5 passed（実 bwrap の 1 件は opt-in 無しで skip） |
| runtime-probe | `bash scripts/dev/test-affected.sh` | exit 0、2,862 passed・9 skipped |
| sandbox-script | `python3 scripts/campaign/test_ops_r2_sandbox.py` / `test_ops_r1_measure_config.py` | exit 0、5 tests / 10 tests |
| preflight-gate | `cargo nextest run -p celeris-campaign --test preflight` | 30 passed（deny_readable で止まる新規 3 件を含む） |
| preflight-gate | `CELERIS_USERNS_TESTS=require cargo nextest run -p celeris-campaign --test runtime_probe` | 6 passed（実 bwrap と実 `ops-r2-sandbox.sh` の中で許可表の外を読めない。L1 は同じ uid の読める file なので hit） |
| leak-owner | `python3 -I scripts/campaign/test_check_ops_r1_leak.py` | exit 0、35 tests OK（同じ uid の chmod 000 で `same_uid` が残る試験を含む） |
| leak-owner | `bash scripts/dev/test-affected.sh` / clippy / fmt / `sh scripts/dev/progress-index.sh --check` | exit 0 / exit 0 / exit 0 / exit 0 |
| l1-owner | `cargo test -p celeris-campaign --lib l1_owner` | exit 0、6 passed |
| l1-owner | `cargo test -p celeris-campaign --test preflight --test runtime_probe --test launcher --test runner_checker` | exit 0（preflight 32 passed、runtime_probe 6 passed、launcher 15 passed・1 ignored、runner_checker 16 passed） |
| l1-owner | `bash scripts/dev/test-affected.sh` / clippy / fmt | exit 0、2,874 passed・9 skipped / exit 0 / exit 0 |
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
- 実施済み（l1-owner）: 同じ uid の chmod 000 の実 fixture で、実 binary の preflight が実 uid・実 lstat で exit 1（L1 の missing 1 件、有料実行 0 件）になる。
  実 binary が L1 を通る経路は、host にある他 uid 持ちの dir（中の path は存在しない）を `EVAL_ROOT` にして確かめた。
- 未実施: 実 uid `celeris-eval` が持つ `EVAL_ROOT` の中に実在する private で L1 が通る e2e。試験は別の uid を作れないので、人が `celeris-eval` を準備した後に
  運用文書 §4.7 の手順で人が確かめる。それまで正式試行の preflight は L1 で止まる。

## 人が準備する OS 権限・設定

運用文書 §4 にコマンドと確認方法を書いた。

1. `celeris-eval` の uid・group の作成
2. `EVAL_ROOT`（0700）・drop box（1733）・outbox（0755）の作成（root が持ち人の uid が書けない祖先の下。例 `/var/lib/celeris-eval`。`/local/celeris` の下は不可）
3. 採点 wrapper・binary の配置（`celeris-eval` 持ち、人は書けない）
4. 評価器の起動方法の選択（狭い sudo 規則か `sudo -iu`）
5. 正式 case の private を `EVAL_ROOT/private/final/<case>/` へ置く
6. 測定専用 Claude account dir（B 用・operator 用）の login
7. 人の uid から `EVAL_ROOT` の private を開けないことの確認

本番設定・本番 daemon・本番 DB はこの task で変えていない。

## 未解決

- 他 uid で L1 が通る e2e（実 uid `celeris-eval` が持つ `EVAL_ROOT` の実在の private）は、人の celeris-eval 準備の後（運用文書 §4.7）。
- 共通 fixture の `foreign_eval_root()` は host の他 uid 持ちの dir を探す。sandbox の外の host（release gate）でどの dir が選ばれるかは未確認（見つからなければ理由付きで panic）。
- `cargo clippy -p celeris-campaign --tests` は既存の `tests/runtime_probe.rs:18`（`cloned_ref_to_slice_refs`）で落ちる（規定の `--workspace` の clippy は通る）。
- K2 の private.json は git の履歴と既存の clone に残る。L2 で塞いだうえでそのまま使うか、作り直すかは ADR の「人に決めてもらう点」1 で人が決める。
- 採点 wrapper（drop box → 採点 → 漏洩検査 → outbox）はまだ無い。今の採点は人の uid で `--grader-root` を読む。
- bwrap の中から sudo が効かないこと（no_new_privs）は実機で未確認。
- release dir に repo の source（fixtures）が入っていないことは未確認。
- deny list は glob を展開しない。承認票の `/local/celeris/data/workspaces/*/repos/*` のような項目は実在の path に展開して書く。

## 提案

- 採点 wrapper を `scripts/campaign/` に置く unit を、人の準備（uid と dir）の後に足す。L1 の評価器経路はそれまで手順だけで閉じていない。
- 運用文書 §4.7 の L1 確認と preflight の deny_readable 0 件を、K2 の開始前チェック（ADR の close 段の `ops-r2-k2-preflight.md`）の必須項目にする。

## docs-align の検査（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）

- `sh scripts/dev/check-doc-links.sh` → ok。`sh scripts/dev/check-adr-numbers.sh` → ok（350 files）。`sh scripts/dev/progress-index.sh --check` → ok
- `python3 scripts/dev/check-human-text.py` → 0 findings
- `bash scripts/dev/test-affected.sh` → exit 0（試験対象の変更なし。docs のみ）
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0
- `sh "$CELERIS_WU_SCOPE_PATHS"` → ADR・`docs/ops/ops-r2-isolation.md`・この進捗の 3 file だけ。コードは変えていない
