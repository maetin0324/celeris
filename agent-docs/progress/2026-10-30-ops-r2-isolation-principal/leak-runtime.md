---
title: 漏洩検査 check-ops-r1-leak.py の --runtime-probe（WU leak-runtime）
tasks: [01M66JYQ249SZHNXK0AEVTZNEC]
status: done
updated: 2026-10-30
---
# 漏洩検査 check-ops-r1-leak.py の --runtime-probe（WU leak-runtime）

完了日: 2026-10-30

WU leak-runtime は進捗ファイルを残さずに終わった。この記録は WU base-record が後から書いた。
ADR は `agent-docs/adr/2026-10-30-ops-r2-isolation-principal.md`。

## 変更
- `leak-runtime: check-ops-r1-leak.py に --runtime-probe（probe で deny_readable を判定、runtime と L1 の拒否で same_uid を外す）`
  — `scripts/campaign/check-ops-r1-leak.py`・`scripts/campaign/test_check_ops_r1_leak.py`・`docs/ops/ops-r1-leak-check.md` を変えた。`crates/` は変えていない。

## --runtime-probe の判定規則
- `--runtime-probe <probe.json>`（複数可）は `celeris-campaign probe-read` の出力（schema `celeris.runtime-probe/1`）を読む。
- probe に載った `--deny-readable` の path は、host の mode bit ではなく probe の `readable`・`listable` で判定する。
  probe が無い path は従来どおり host から判定する。
- principal が `l1` か `l1:` で始まる probe は L1（評価器 uid の 0700 の dir を、人の uid の process から包まずに開いた結果）、他は runtime（bwrap で包んだ主体）の確認として扱う。
- runtime の probe と L1 の probe の両方で `readable=false` の path だけ、`same_uid` の limit を外す（`same_uid_cleared_by: runtime_probe_and_l1`）。
  片方しか無い・片方で読めた path は、従来どおり持ち主の連鎖から `same_uid` を判定する（canary が無ければ hit）。
- どれか 1 つの probe で `readable=true` の path、空でない `mounts_outside_allow`、`ok=false` の probe は `deny_readable` の hit（exit 1）。
- schema が違う・欄が欠けた・型が違う probe JSON は入力の誤り（exit 2）。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
- `python3 scripts/campaign/test_check_ops_r1_leak.py` → exit 0（Ran 34 tests, OK）
- `sh scripts/dev/check-doc-links.sh` → exit 0（check-doc-links: ok）
- `sh scripts/dev/progress-index.sh --check` → exit 0
- 段 base の統合（v1）でも同じ試験は通った（integration-checks/integrate-base の check 0: Ran 34 tests, OK）。

## 段 base の統合（v1）の失敗と v2 での検査差し替え
- 原因: 計画 v1 は leak-runtime と sandbox-script の検査に `git diff --quiet $(git merge-base HEAD main) -- crates/` を置いた。
  統合では同じ段の runtime-probe の変更（`crates/campaign` の probe-read と A の Sandbox 引数の共有）を先に取り込むので、
  統合後の HEAD では `crates/` に差分があり、この検査は exit 1 で落ちた（出力なし。check 1 と check 6 の log が空）。
  他の check（漏洩検査の試験 34 件、sandbox の試験、文書検査、build）は通っており、code の不具合ではない。
- v2 の差し替え: leak-runtime の検査は `sh scripts/dev/check-doc-links.sh` だけに、sandbox-script の検査は
  `sh -n scripts/campaign/ops-r2-sandbox.sh && sh -n scripts/campaign/ops-r1-measure-config.sh` に替えた。
  `crates/` を変えないことは各 WU の範囲 check（scope）が見る。

## 未解決
- 実 runtime の probe JSON（評価器 uid `celeris-eval` と bwrap の主体から取ったもの）での実行は未実施。試験は fixture の probe JSON だけを使う。
  評価器 uid は人が作る（決定 iso-boundary）。
- preflight が probe を集めて deny_readable で止める配線は WU preflight-gate の担当で、まだ無い。

## 提案
- 計画の WU 検査に、統合後に同じ段の他の WU の変更で必ず変わる範囲（ここでは `crates/`）の差分ゼロを置かない。範囲の制限は scope check に任せる。
