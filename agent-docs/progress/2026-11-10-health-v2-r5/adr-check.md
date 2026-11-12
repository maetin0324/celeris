---
title: R5 adr-check — ADR の束の表と tests/<束>/main.rs を照合する台本
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: done
updated: 2026-11-12
---
# R5 adr-check — ADR の束の表と tests/<束>/main.rs を照合する台本

完了日: 2026-11-12

結論: 照合の台本 `adr-check.py`（この dir）を自己試験つきで書いた。今の ADR
[2026-11-10-test-target-bundles.md](../../adr/2026-11-10-test-target-bundles.md) に対しては exit 1 で、食い違いは 5 件
（下の「今の ADR で出た食い違い」）。ADR・`classify.py`・`inventory.json`・コードは変えていない。ADR を直すのは unit adr-tables。

## 呼び方
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/adr-check.py <ADR の path> [--root <repo の根>]`（`--root` の既定は cwd）
- `python3 agent-docs/progress/2026-11-10-health-v2-r5/adr-check.py --self-test`
- exit: 0 = 一致、1 = 食い違い（1 件ずつ「食い違い: …」の行で理由を出す）、2 = 引数の誤り・読めない入力（ADR が読めない、`--root` の下に `crates/<c>/tests` が無い）。

## 照合の規則
1. 束: task-api・celeris・celerisctl・task-worker の `crates/<c>/tests/<束>/main.rs`。main.rs の最上位の `mod x;` のうち、
   `#[path = …]` が付かず `tests/<束>/x.rs` が在るものを target、`#[path]` 付きの mod（と `x.rs` の無い mod）を支援 module とする。
   comment・文字列・inline の `mod x { … }` の中は数えない。celeris の `browser_eval_surface_suite` の `fake_http` は target、
   `cos_delivery_suite` の `fake_http` は支援 module になる。
2. 表: 見出しに `crate`・『束』で始まる列・`件`・`target` を持つ Markdown の表だけを読む（今の ADR では段 bundle と段 final の 2 表。
   「動かさない target」・baseline・試験の表は読まない）。
   - 束の欄の最初の backtick の名前が束（段 final の `cos_delivery_suite`（R3 の競合が解ければ `cos_delivery_suite_*` へ）は `cos_delivery_suite`）。
   - backtick を除いた束の欄が `keep` で始まる行は keep の行。backtick の名前も keep も無い行は食い違いにする。
   - target の欄は（…）の注記を入れ子ごと除き、backtick を除いて『・』で分ける。
3. exit 0 は次をすべて満たす時だけ:
   - tree の各束が表のちょうど 1 行に載る（0 行は「表に無い束」、2 行以上は「N 行に載る」）。
   - その行の件数が束の target の mod の数と、target の集合が束の target の mod の集合と一致する。
   - 表が実在しない束（`crates/<c>/tests/<束>/main.rs` の無い束）を挙げない。表の crate は上の 4 crate のどれか。
   - keep の行の target が `crates/<c>/tests/<t>.rs` として在り、行の件数が target の数と一致する。
   - `- 状態:` の行が在り、『採用（設計）』を含まない。
4. `--self-test` は実 ADR に依らない。TMPDIR（未設定なら台本の dir）に一時 dir を作り、4 crate の小さな fixture の tree と一致する ADR を書いて
   子 process で台本を走らせる。fixture は上の 2 つの `fake_http` の形、`#[path]` と mod が同じ行の形、`#[cfg]` 付きの target、
   comment の中の mod、`x.rs` の無い dir の mod、inline の mod、keep の行、読まない表に載る架空の束を含む。
   一致の写しで exit 0、壊した 7 種の写しで exit 1（出力に壊した箇所の理由が出ることも確かめる）、読めない ADR で exit 2 を確かめ、
   終わりに一時 dir を消す。

## 今の ADR で出た食い違い（提出時の HEAD で実行）
`python3 agent-docs/progress/2026-11-10-health-v2-r5/adr-check.py agent-docs/adr/2026-11-10-test-target-bundles.md` → exit 1、食い違い 5 件:

| # | ADR の行 | 食い違い | tree の事実 |
|---|---|---|---|
| 1 | 8 行（状態） | 状態の行が『採用（設計）』のまま | 段 bundle・段 final の束は実装済み |
| 2 | 110 行（段 final） | task-worker の束の欄「1 件だけの組（束ねない）」に束の名前も keep も無い | `browser_action_recovery`・`browser_restore_deliver` は `crates/task-worker/tests/<t>.rs` に在る（worker-singleton・adjust-worker の後の keep） |
| 3 | 109 行（段 final） | task-worker `unmapped_suite` は実在しない束 | `crates/task-worker/tests/unmapped_suite/main.rs` は無い。`reap_finished_children`・`ssh_cluster_manual` は `tests/<t>.rs` に在る（adjust-worker で keep） |
| 4 | 92 行（段 bundle） | task-api `unmapped_suite_a` の件 8 が tree の target 7 件と違う | main.rs の target は approvals・cluster_settings_admin・concurrency・console・console_instruct・conversation・files |
| 5 | 92 行（段 bundle） | task-api `unmapped_suite_a` の表だけにある target: docs | `docs` は `crates/task-api/tests/docs.rs` に在る（bundle-api-b で keep。`check-doc-links.sh` の `FOREIGN_DOCS` が path で挙げる） |

tree の束は 17 個（task-api 10・celeris 4・celerisctl 2・task-worker 1）。`unmapped_suite_a` を除く 16 束は、表の 1 行に載り件数と target が一致した。

確認のため、上の 5 件だけを直した ADR の写し（TMPDIR に置き、確認の後に消した。repo の ADR は変えていない）に台本を走らせ、exit 0
「一致（4 crate、束 17 個、束の target 85 件）」を得た。直した写しでは 2・3 の行の束の欄を `keep（…）` で始めた。
adr-tables が段 final の表を直すときも、keep の行は束の欄を `keep` で始める必要がある。

## 実装前 brief
1. 根本原因: ADR の束の表は人が手で書き、実装の leaf（bundle-api-b・adjust-worker・worker-singleton）が束を変えても表は追従していなかった。
   表と tree の食い違いを決定的に見つける検査が無かった。
2. 責務 owner: R5 の進捗の dir の検査台本（`classify.py`・`cases.py` と同じ置き場）。ADR の本文は unit adr-tables。
3. 既存の再利用先: `classify.py --verify-leaf` は `inventory.json` と tree を照合するが、ADR の表は読まない。ADR の表の読み方は新しく書いた。
4. 選択肢: (a) `classify.py` に ADR の照合を足す → ADR の表は inventory と別の正本ではなく、tree を直に見る方が「ADR が実装に合う」を確かめられるので採らない。
   (b) 独立の台本で tree の main.rs と ADR を照合する → 採用。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: 無し。
7. 不変条件: ADR の束の表が tree の束（4 crate の `tests/<束>/main.rs`）と 1 対 1 で、件数・target が一致し、keep の target が元の path に在る。
   `--self-test` の 7 種の壊した写しが、それぞれの検出を確かめる（既存の検査は ADR の表を読まないので、これらを検出できない）。

## 変更
- 件名「phase 1: adr-check: ADR の束の表と tests/<束>/main.rs を照合する台本を書く」の commit —
  `adr-check.py` とこの進捗を足した。ADR・`classify.py`・`inventory.json`・コードは変えていない。

## 証拠（提出時の HEAD で実行）
- 条件 0: `python3 agent-docs/progress/2026-11-10-health-v2-r5/adr-check.py --self-test` → exit 0、「self-test: 9/9 ok」
  （一致 0、件数・target・状態・実在しない束・表に無い束・2 行に載る束・keep の file 欠けの 7 種がそれぞれ exit 1、読めない ADR が exit 2）。
  `env -u TMPDIR` で走らせても exit 0（一時 dir は台本の dir の下に作り、終わりに消えた）。
- 条件 1: `python3 agent-docs/progress/2026-11-10-health-v2-r5/adr-check.py agent-docs/adr/2026-11-10-test-target-bundles.md` → exit 1、食い違い 5 件（上の表）。
- 引数の誤り: ADR の path 無し → exit 2（argparse）、`--root /nonexistent` → exit 2「/nonexistent/crates/task-api/tests が無い」。
- 条件 3: `git diff --name-only "$CELERIS_WU_BASE"` と `sh "$CELERIS_WU_SCOPE_PATHS"` は `adr-check.py`・`adr-check.md` だけ。
- `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check` → 3 つとも exit 0（「ok」「ok (427 files)」「ok」）。
- `git diff --name-only $(git merge-base HEAD main) | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → exit 0「write-set R5: ok」。`$CELERIS_WU_BASE` との差でも exit 0。
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"` → exit 0（verdict=pass、new=0、worsened=0。source-size は doc_only で not_applicable）。
- Rust と web は変えていないので、`test-affected.sh`・clippy・fmt は走らせていない。

## 未解決事項
- 今の ADR は exit 1（上の 5 件）。直すのは unit adr-tables で、統合（integrate-sync）で exit 0 を確かめる。

## 提案
- adr-tables は段 final の task-worker の 2 行を keep の行（束の欄を `keep` で始める）にし、`unmapped_suite` の行を消す。
  段 bundle の `unmapped_suite_a` は 7 件にして `docs` を外す。状態の行から『採用（設計）』を消す。
