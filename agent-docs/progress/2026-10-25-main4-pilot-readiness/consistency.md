---
title: pin binary の無 LLM out の4源照合（consistency）
tasks: [01M5WJTMZE0YWRMZE900NDPYRG]
status: blocked
updated: 2026-10-26
---
# pin binary の無 LLM out の4源照合（consistency）

照合完了日: 2026-10-26。全試験合格の条件は下記の既存 fixture 問題で未達。

## 変更
- `consistency: check-out-consistency.py に final-accounting の照合を足し、共有 human-op を identity で判定する` — final-accounting の arm 別 n・ran・not_run・token 欠測、cell、final_cutoff、共有 usage 行を生記録と比較。共有 human-op は identity により判定。
- `phase 1: restore consistency scope and record pinned recheck` — 前回の範囲外 docs/ops 3件を開始時の内容に戻した。この葉では運用文書を変更しない。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
- 前回不合格の scope コマンドを同じ cwd で再実行 → exit 0、範囲外 path なし。
- `python3 scripts/campaign/test_check_out_consistency.py` → exit 0、15 tests OK。
- `python3 scripts/campaign/test_check_main_pins.py` → 別途 result.json に結果を記録。
- `bash scripts/dev/test-affected.sh` → exit 0、health 通過、Rust 試験対象変更なし。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- RP の consistency-work/cmd.txt の argv を読み、--json-out のみ今回の artifacts/consistency.rechecked.json に替えて再実行 → exit 0。checker 引数は launcher と同じ。consistent=true、mismatches=[]、sources は checker/export/human_op_report/final_accounting の4種。
- pin binary の sha256 を repin.json と直接比較 → 一致。別 binary は build・実行しない。
- R2 と RP/nollm 配下9,018ファイルのsha256一覧を照合前後で比較 → 差分0。既存 RP/consistency.json も consistent=true、exit=0、pin一致。
- 共有 human-op は start/end 2行で setup 1件、report に1度。final-accounting の共有 usage は生記録と同じ0行（LLM消費なし）、totalへ加算しない。
- 有料 LLM・pilot・主測定は実行していない。

## 未解決
`python3 -m unittest discover -s scripts/campaign -p 'test_*.py'` → exit 1、65 tests 中64成功・1失敗。
既存 test_sub_pins.SubPins.test_baseline_has_only_handover_gaps が、実 artifacts 内 launcher.env の CELERIS_BIN が指す削除済み release の不在で失敗。
この葉の変更と無関係だが「試験が全部通る」は未達。test_sub_pins.py は許可範囲外、本番 release の復元も禁止のため変更しない。

## 提案
sub-pins 試験の外部 release 依存を除く別葉を追加し、fixture と検査の意味を保って再試験する。今回の scope 制約を広げずに行う。

## 本番への移行
なし。
