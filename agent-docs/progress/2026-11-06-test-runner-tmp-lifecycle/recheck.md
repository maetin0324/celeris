---
title: 一時領域管理の試験と文書検査の再実行（recheck）
tasks: [01M6S99ADJGBSD0CAJQDKSSMQ5]
status: done
updated: 2026-11-07
---
# 一時領域管理の試験と文書検査の再実行（recheck）

完了日: 2026-11-07（WorkUnit recheck-lifecycle。コードは変えていない）。基準は `$CELERIS_WU_BASE`。
検査は提出時の HEAD（最後の main 同期の後）で実行した。失敗は無く、直した物も無い。

## 実行コマンドと結果
- `bash scripts/dev/tests/test-runner-tmp-lifecycle.sh` — exit 0。「runner-tmp-lifecycle: ok（188 checks）」
- `bash scripts/dev/tests/test-runner-tmp-repro.sh` — exit 0。18 case が期待表と一致。一時 dir が残るのは SIGKILL の 4 case のみ。子が使用中の TMPDIR を trap が消した case は 0
- `sh scripts/dev/check-adr-numbers.sh` — exit 0。「ok (399 files)」
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` — exit 0。「ok」（引数なしでは usage を出して exit 1 になる。tsv を渡す）
- `python3 scripts/dev/check-human-text.py` — exit 0。「11 files, 244 literals, 0 findings (5 allowed)」
- `cargo fmt --all -- --check` — exit 0

## 未解決事項
なし。
