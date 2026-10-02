---
title: docs 再構成 — 検査の台本（doc-tools）
tasks: [01M3YBGM64RYPEY9NZANF79A0M]
status: done
updated: 2026-10-02
---
# docs 再構成 — 検査の台本（doc-tools）

ADR-0128 D4・D5・D7 の台本 3 本を `scripts/dev/` に足した。いずれも POSIX sh（dash で確認）。

## 足したもの

- `scripts/dev/check-doc-links.sh` — 引数なし / `<path>...` / `--self-test`。
  fixture は `scripts/dev/testdata/doc-links/{good,bad}/`（`bad/expected.txt` が期待出力）。
- `scripts/dev/check-adr-numbers.sh` — 引数なし / `--refs`。許可リストは D5 の 6 本。
- `scripts/dev/progress-index.sh` — 引数なし（索引） / `--check`。

## 仕様の細部（D7 に書かれていない部分の決め方）

- Markdown リンクは fenced code と inline code の中を見ない。`/` 始まりはリポジトリ root から解決する。
- 素のパスは `docs/`・`agent-docs/` の直前が英数字・`/`・`.`・`-`・`_` のときは数えない（`gui/docs/…`、URL の途中）。
  `../docs/…`（`include_str!`）はファイルの位置から、それ以外は root・ファイルの位置・先頭ディレクトリ（`gui/` など）のどれかで実在すればよい。
- 雛形の除外に D7 の `NNNN`・`<…>`・`*`・`{…}`・`$` のほか `YYYY`・`xxx`・`...` を足した（`docs/adr/0074-...md`、`docs/research/xxx.md`）。
- `docs/adr/0008`・`docs/adr/0040-` のように番号だけで呼ぶものは `番号-*` が 1 本あれば実在とみなす。
- 台本内の変数 `FOREIGN_DOCS` に、`docs/…` が別の文書リポジトリや試験の作業場所を指すファイル
  （celeris の文書機能 `task-api/src/docs.rs`・`task-ops/src/docs*` とその試験、gui/web の試験 fixture など）を列挙し、素のパスを見ない。
- 移行期間の対象外は変数 `MIGRATION_EXCLUDE`（`agent-docs/PROGRESS.md`、`agent-docs/progress/phase-F.md`）。
- `check-adr-numbers.sh --refs` は、同じファイル名は merge で共有された同じ ADR とみなし、名前が違って番号（日付形式なら slug）が同じものを出す。
- `progress-index.sh` の索引は front matter のあるファイルだけを出す（旧 `phase-*.md` は出さない）。`--check` は D4 どおり `YYYY-MM-DD-*.md` だけ。

## 今の tree（base 21abef99、移動前）での実行結果

| コマンド | exit | 要点 |
|---|---|---|
| `sh scripts/dev/check-doc-links.sh --self-test` | 0 | good → 0・出力なし、bad → 1・期待の 6 行 |
| `sh scripts/dev/check-doc-links.sh` | 1 | 壊れた参照 6 件（下表）。記録のみ、直すのは後続の葉 |
| `sh scripts/dev/check-adr-numbers.sh` | 0 | 113 本（docs/adr 108、docs/gui/adr 2、docs/web/adr 3）。重複は許可リストの 0078 のみ |
| `sh scripts/dev/check-adr-numbers.sh --refs` | 0 | この branch で足した ADR は 0128 のみ、他の branch と重ならない |
| `sh scripts/dev/progress-index.sh --check` | 0 | 違反なし（このファイルは WU 名なので対象外） |
| `sh -n`（3 本） | 0 | dash -n も 0 |

壊れた参照（移動前の tree）:

- `docs/gui/bootstrap/CLAUDE.md:22: docs/celeris-requests.md` — move-docs で削除される文書
- `gui/app/components/task-detail/FailureBanner.tsx:18: docs/gui/help`
- `gui/docs/celeris-api-v1.md:3: ../celeris-api-v1.md`
- `gui/scripts/check-run-log.mjs:7: docs/gui/run-log`（23 行目も同じ。出力先ディレクトリの既定値）
- `web/features/runs/run-log.ts:4: docs/adr/0013-run-log-conversation-view.md`

違反検出の確認（一時ファイルで実施し、消した）: `docs/adr/0129-x.md` → 0128 超え、`docs/adr/Bad.md` → 形式違反、
`0050-dup.md` → 番号重複、日付形式の slug 重複 → 違反、`--refs` で `0124-zzz.md` → 他 branch の 0124 2 本と重なる、で exit 1。
`progress-index.sh --check` は front matter 無し・status 不正・tasks 不正・updated 不正をそれぞれ検出した。

## 未解決

- 上の壊れた参照 6 件は refs-repo / cleanup-api の範囲。
- `FOREIGN_DOCS` のファイルにある `include_str!` などの本物の参照は見ない。そこへ docs のパスを足すときは注意。
