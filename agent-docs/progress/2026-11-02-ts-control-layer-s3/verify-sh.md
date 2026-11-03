---
title: S3 verify-sh — ts-verify.sh の分類と runner の cache key
tasks: [01M6EVP95S9VM3DBMZFRD8FS35]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S3 verify-sh — ts-verify.sh の分類と runner の cache key

完了日: 2026-11-02（task 01M6EVP95S9VM3DBMZFRD8FS35、unit verify-sh。親 task は 01M6DCMB75G5F0DSE58ZNZMBAK）

## 結論
- [ADR 2026-11-02-ts-fast-verify](../../adr/2026-11-02-ts-fast-verify.md) の (1)(2)(4) を実装した。
  - `scripts/dev/ts-verify.sh`: `--list`（分類・file ごとの区分と理由・単体試験の選択・走らせる段）、引数なしで 10 段の実行、
    `--record <file>`（段ごとの秒と exit、`cargo_invocations`・`cargo_calls`・`cargo_compile_invocations`・`link_count`、選んだ単体試験）、
    `--no-build`（runner の cache が無ければ exit 3）。
  - `scripts/dev/script-runner-cache.sh`: `key`・`inputs`（key の元の 5 入力）・`path`・`ensure`。
  - 試験 `scripts/dev/tests/ts-verify-classify.sh`（45 件）と `scripts/dev/tests/script-runner-cache-key.sh`（30 件）。どちらも `bash <file>` で走る。
- runner の実体（unit runner）はまだ無い。ts-verify は ADR (3) の CLI（`domains`・`run --suite safety|golden`・`activate`、
  stdout の JSON の `ok`・`complete`）を前提に呼ぶ。試験は偽の runner で通した。
- release gate・`release.sh`・`test-affected.sh`・`control-ts/` は変えていない。

## 実装の決め事（ADR に書いていない細部）
- import グラフの helper（ADR の例 `scripts/dev/ts-verify-graph.mjs`）は別 file にせず、`ts-verify.sh` が `$TMPDIR` の使い捨て dir に書く
  heredoc にした。unit の範囲 check が `scripts/dev/ts-verify.sh` だけを許すため。中身は ADR (1)-1 のとおり
  （`control-ts/node_modules` の esbuild を `createRequire` で読み、`src/main.ts` と各 `*.test.ts` の metafile の inputs を出す。
  式の `import(...)`・どの入口からも読まれない・試験から読まれない・計算の失敗のときは全部）。
- 分類の追加の規則: rename は旧い側を削除、新しい側を追加として扱う（試験 file の rename は host-api）。`control-ts/` の下で
  `src/**/*.ts` 以外の path（`src/` の .ts でない file を含む）はすべて host-api。
- runner の `domains` の出力は `{"domains":[{"domain":…,"fixture":"<domain>/<file>.jsonl"}]}`（または同じ要素の配列）を読む。
  `domain` の代わりに `name` も受ける。runner unit がこの形と違う形を選んだら、wire unit で ts-verify の `do_domains` を合わせる。
- cache の `path` は `meta.json` の runner と golden の sha256 を file と照合し、当たれば `used_at` を更新する。`ensure` は
  `<root>/<key>.lock` を `flock` し、`<root>/.tmp-<pid>/` に作ってから `mv` する。最近使った 4 個より古い key の dir を消す。
- 試験の中の文書の path は doc-links の検査に実在の path と見なされないよう変数で組んだ（`test-affected-list.sh` と同じ書き方）。

## 証拠（提出時の HEAD で実行）
- `bash scripts/dev/tests/ts-verify-classify.sh` → exit 0、`45 passed, 0 failed`。
- `bash scripts/dev/tests/script-runner-cache-key.sh` → exit 0、`30 passed, 0 failed`。
- `bash scripts/dev/ts-verify.sh --list` → exit 0、`class: rust`（この unit の変更は scripts/dev と進捗なので rust）。
- 手元の確認（試験ではない）: WIP commit を基準に `control-ts/src/domains/route_priority.ts` へ 1 行足し、
  `--list` → `class: ts-only`、`unit: import グラフで選んだ 3 file`（本物の esbuild）。同じ状態で、手で作った cache の entry（偽の runner と
  本物の fixture の写しの golden）を使い `--no-build --record` → exit 0、10 段すべて exit 0、本物の tsc・bundle・vitest・
  `node --test scripts/*.test.mjs`・`compare.mjs` 2 領域（19,683 件・2,026 件一致）、total 3.9 秒、`link_count` 0。
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` の結果は報告に書く。

## 未解決
- **cargo の起動 0 回の目標との衝突**: 上の手元の確認で `cargo_invocations` が 5 だった。5 回とも `cargo metadata --format-version 1 --no-deps` で、
  unit 段の `node --test control-ts/scripts/*.test.mjs` のうち `test-affected.test.mjs` が `scripts/dev/test-affected.sh --list` を走らせるため
  （compile・link は無い。`cargo_compile_invocations` 0、`link_count` 0）。ADR (2)(7) は「cargo の起動 0 回」とし、unit 段で
  `control-ts/scripts/*.test.mjs` を常に全部走らせるとしているので、両方は同時に満たせない。wire unit か bench unit で次のどれかを決める必要がある:
  (a) 目標の判定を `cargo_compile_invocations` 0 と `link_count` 0 にする（ADR に付記）、
  (b) `test-affected.test.mjs` を ts-only の unit 段から外す（`scripts/dev/test-affected.sh` の試験で、ts-only では変わらない）。
- runner の実体での通し（`ensure` の本物の cargo build・golden の作り直し）は未確認。wire unit の範囲。

## 提案
- 上の未解決の (a) を推す。読み取りだけの `cargo metadata` は compile・link を起こさず、試験を外す (b) より検証を弱めない。
