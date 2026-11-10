---
title: 健全性v2 R5 scaffold-fix — commit 済みの雛形 mapping/*.json の確認と map leaf の手順の修正
tasks: [01M7388T31FEX7W469M2TJS236]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 R5 scaffold-fix — commit 済みの雛形 mapping/*.json の確認と map leaf の手順の修正

完了日: 2026-11-10

WorkUnit `scaffold-fix`。tool 段の統合が exit 2 で落ちた後始末をした。基準は `$CELERIS_WU_BASE`。`contracts.py`・`scanner.md`・crates/ は変えていない。

## 結論
- commit 済みの雛形 5 file（`mapping/api-a.json`・`api-b.json`・`api-c.json`・`celeris-src.json`・`cli.json`）は、HEAD で作り直した雛形と試験 id・組（id・kind・members）が全部同じだった。作り直した fragment は無い。
- 違いは `scan.commit`（雛形を作った commit）だけだった。`scan` のほかの欄（crates の tree・crate ごとの tree・ADR の tree・capability-map と architecture-map と検査器の blob）と、`files`・`groups` の全欄は同じだった。
- [`fill-guide.md`](fill-guide.md) の 1 節の手順 1 を、--scaffold を走らせずに commit 済みの `mapping/<name>.json` を埋める手順に直した。

## 原因
- WorkUnit `scanner` の計画の check（v1）は `contracts.py --scaffold <name>` を `--out` なしで実行した。台本は既定の出力先 `mapping/<name>.json` に雛形を書いた。
- celeris は WorkUnit の終わりに作業ツリーに残った変更を commit する。5 つの雛形は commit `wu/scanner: contracts.py: 試験関数の走査・setup の hash・同値 scenario の検出・fragment と --check` に入った（この commit は 5 file だけを足している）。
- tool 段の統合で同じ check を流し直すと、`--scaffold` は既にある file を上書きしないので exit 2 で止まった。
- scanner の check は v2 で `--out` に一時 dir を渡す形に替わっている。この WorkUnit では check を変えていない。

## 確かめた結果（提出時の HEAD で実行）
手順: 一時 dir（`$TMPDIR` の下。repo の外）に `python3 agent-docs/progress/2026-11-10-health-v2-r5/contracts.py --scaffold <name> --out <一時 dir>/<name>.json` で 5 つの雛形を作り（5 回とも exit 0）、commit 済みの file と python で比べた。試験 id は `<path>::<files.<path>.tests の要素>`、組は `(id, kind, members)` の集合で比べた。

| fragment | file | 試験 | 組 | 試験 id | 組 | 埋めた欄 |
|---|---|---|---|---|---|---|
| `api-a` | 50 | 320 | 37 | 同じ | 同じ | 0 |
| `api-b` | 44 | 320 | 8 | 同じ | 同じ | 0 |
| `api-c` | 51 | 215 | 3 | 同じ | 同じ | 0 |
| `celeris-src` | 60 | 537 | 9 | 同じ | 同じ | 0 |
| `cli` | 104 | 472 | 30 | 同じ | 同じ | 0 |

- 数は commit 済みの file と作り直した file で同じ。試験の和 1864、組の和 87（duplicate_setup 81・equivalent_scenario 6）は [`scanner.md`](scanner.md) の数と一致する。
- 「埋めた欄」は capability・contract・`fns`・`adopt` のどれかが入った file と組の数。5 file とも 0 で、雛形のまま。
- 比較の script は exit 0（5 fragment とも試験 id と組が同じ）。
- 埋めていない雛形の `--check-fragment` は 5 file とも exit 1。不備の数（api-a 714・api-b 656・api-c 436・celeris-src 1092・cli 1004）は、どれも「試験の数 × 2（capability・契約が無い）＋ 組の数 × 2（adopt・reason が無い）」と一致する。試験 id や組の食い違いの不備は 0 件。
- `>` で受けた場合の確認（一時 dir に api-c の雛形を写して `--dir` で指した）: `--scaffold api-c --dir <一時 dir> > <一時 dir>/mapping/api-c.json` は exit 2（「既にある（上書きしない）」）で、file は 23445 byte から 0 byte になった。file を消してから `--scaffold api-c --dir <一時 dir>` を走らせると exit 0、stdout は 0 byte で、`scan` を除いて commit 済みの file と同じ雛形ができた。

## fill-guide.md の変更
1 節の手順 1「雛形を作る」を次の内容に替えた。
- 雛形は commit 済みなので、map leaf は --scaffold を走らせない。commit 済みの `mapping/<name>.json` をそのまま埋める。`scan` 欄は変えない（`--check-fragment` はこの欄の tree で走査し直す）。
- `--scaffold <name> > mapping/<name>.json` のように `>` で受けない。shell が台本より先に file を空にするので雛形が壊れる（台本は exit 2 で止まり、空の file が残る）。`--scaffold` は file を自分で書き、stdout には何も出さない。
- 作り直すときは `mapping/<name>.json` を消してから `--scaffold <name>` を走らせる。作り直すと埋めた欄も空に戻る。
- 見比べるだけなら `--out <一時 dir>/<name>.json` で repo の外に書く。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）
- 上の比較と `>` の確認。
- `bash scripts/dev/test-affected.sh` → exit 0（`試験対象の変更なし（docs のみ等）`）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/progress-index.sh --check` → exit 0。
- `git diff --name-only $(git merge-base HEAD main) | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → `write-set R5: ok`。
- 範囲: `git diff --name-only $CELERIS_WU_BASE` の中で、この WorkUnit が変えた file は `fill-guide.md` と `scaffold-fix.md` だけ。`contracts.py`・`scanner.md`・crates/ に差分なし。

## commit
- `R5 scaffold-fix: map leaf は commit 済みの雛形を埋める手順に直し、雛形の確認を進捗に書く`

## 未解決事項
- 無し。

## 提案
- celeris が WorkUnit の終わりに残った変更を commit するので、WorkUnit の check が repo の中に file を書くと、その file が成果物として commit される。check の書き手向けの手引き（計画の手順）に「check は repo の外（`$TMPDIR` か `--out`）に書く」を足す案がある。この task の write set の外なので、ここでは変えない。
