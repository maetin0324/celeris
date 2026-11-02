---
title: S3 wire — ts-verify を本物の runner で通し、意図的破壊の試験と test-affected への接続
tasks: [01M6EVP95S9VM3DBMZFRD8FS35]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S3 wire — ts-verify を本物の runner で通し、意図的破壊の試験と test-affected への接続

完了日: 2026-11-02（task 01M6EVP95S9VM3DBMZFRD8FS35、unit wire。親 task は 01M6DCMB75G5F0DSE58ZNZMBAK）

## 結論
- `scripts/dev/ts-verify.sh` を cache の本物の `script-host-runner`（release build）で通した。safety・integration・activate の 3 段が
  runner を使う。TS だけの無害な編集で 10 段すべて exit 0、cargo の起動 0 回、`link_count` 0、計 5.1 秒（下の証拠）。
- `scripts/dev/test-ts-verify.sh`（引数なし）を足した。準備で runner を `ensure` し（cargo を使うのはここだけ）、
  `$TMPDIR` の写しで ADR (5) の 8 場合を 12 件の確認で固定した（12 passed）。
- `scripts/dev/test-affected.sh`: TS だけの変更では cargo を起動せず control-ts の検査を `ts-verify.sh --record` に委ね、
  記録の `class` が `ts-only`・`exit` が 0 であることを確かめる。Host API・検証の道具・golden の変更では `celeris-script` と
  逆依存（fixture の変更では fixture を参照する crate も）の cargo の試験を足し、runner の cache key の入力か検証の道具の変更では
  `test-ts-verify.sh` も走らせる。Rust の変更は従来どおり。
- `docs/architecture-map.md` に ts-verify の行（関連試験・不変条件）を足した。ADR に付記 wire-1〜3 を足した。
- release gate・`release.sh`・runner・daemon の経路は変えていない。

## 変更
- `phase s3: wire: ts-verify を本物の runner で通し、意図的破壊の試験と test-affected への接続`
  - `scripts/dev/ts-verify.sh`: 段 unit から `control-ts/scripts/test-affected.test.mjs` を外した（付記 wire-1。fixture の中で
    `cargo metadata` を起動するため。verify-sh の記録の未解決を解いた）。
  - `scripts/dev/test-affected.sh`: 上の委譲と crate の追加。差分は `--name-status --no-renames` で集める（rename の旧い側も見る）。
  - `scripts/dev/test-ts-verify.sh`: 新規。
  - `scripts/dev/tests/test-affected-list.sh`: 場合 (g)（TS だけの `--list` と実行で偽の cargo を 1 回も呼ばない、ts-verify の記録が
    ts-only でない・失敗のとき落ちる、host.d.ts・control-ts/scripts・package.json・schema・試験の削除・fixture・混在・道具の変更）を足し、
    (b)(e) の期待を `ts-verify: test-ts-verify` の行に合わせた。fixture の `.gitignore` に試験の stderr の file `err` を足した。
  - `control-ts/scripts/test-affected.test.mjs`: TS だけの変更は偽の ts-verify に委ねる（pnpm を呼ばない）ことを確かめる試験に直し、
    従来の control-ts の検査は `src/testing/`・`tsconfig.json` の変更で確かめる（7 tests）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
| 条件 | コマンド | 結果 |
|---|---|---|
| 0. TS だけの編集で cargo を起動せず exit 0 | `bash scripts/dev/test-ts-verify.sh` の場合 A（偽の cargo は exit 97） | exit 0、記録 `cargo_invocations` 0・`link_count` 0、10 段 exit 0、5.1 秒 |
| 1. 壊した規則を exit 1 で検出 | 同 場合 B（`frontier/costly-and-unverifiable` の lane を standard に） | ts-verify exit 1（段 unit で失敗、cargo 0 回）。同じ bundle を runner に直に渡すと exit 1、mismatch 1,458 件 |
| 道具・golden・設定は ts-only にならない | 同 場合 C・D・E・F・G | compare.mjs・fixture・package.json・bundle.config.json は host-api（F の bundle は runner が exit 3）、`@ts-nocheck`・`.only(` は classify で exit 1 |
| 2. test-affected の委譲と cargo 側 | 同 場合 H・I、`bash scripts/dev/tests/test-affected-list.sh`、`node --test control-ts/scripts/test-affected.test.mjs` | H: host.d.ts・schema で `celeris-script` を選ぶ。I: TS だけで `control-ts: ts-verify` の 1 行、cargo 0 回。`test-affected-list: ok`、7 tests pass |
| 全体 | `bash scripts/dev/test-ts-verify.sh` | `12 passed, 0 failed`（16 秒） |
| 分類と cache key の試験 | `bash scripts/dev/tests/ts-verify-classify.sh`・`bash scripts/dev/tests/script-runner-cache-key.sh` | 45 passed・30 passed |
| 分類 | `bash scripts/dev/ts-verify.sh --list` | exit 0、`class: rust`（この unit の変更は scripts/dev と control-ts/scripts） |
| 影響試験 | `bash scripts/dev/test-affected.sh` | exit 0。control-ts の型検査と試験（`node --test` 30 pass）、`test-ts-verify` 12 passed、nextest `915 tests run: 915 passed`（celeris・celeris-script・celerisctl）、doc test ok |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0 |
| fmt | `cargo fmt --all -- --check` | exit 0 |
| 地図 | `python3 scripts/dev/check-architecture-map.py`・`bash scripts/dev/check-doc-links.sh` | OK 848 件・ok |

runner の `ensure`（cache が無い状態から）は 60.6 秒（release build 23.4 秒を含む）。1 回目の `test-affected.sh` は
`could not find native static library rusty_v8` で落ちた。reflink seed の target の v8 の build output が原因で、
自分の `CARGO_TARGET_DIR` の `debug/build/v8-*` と `debug/.fingerprint/v8-*` を消して再実行し、exit 0 になった（コードの変更ではない）。

## 未解決
- 場合 B は ts-only の経路では TS の単体試験（段 unit）で先に落ちる。Rust の採点（runner）だけで落ちることは、同じ bundle を
  runner に直に渡して別に確かめた。TS の試験を同時に弱めた ts-only の編集（試験の中身を書き換える）を ts-verify に通す場合は試していない。
- runner の cache key の入力（`crates/task-core/` を含む）が変わると `test-affected.sh` が `test-ts-verify.sh` を走らせ、runner の release build
  （cache が無ければ約 60 秒）が足される。task-core の変更は多いので、bench・measure の unit で負担を見る。

## 提案
- bench unit の `policy-ts` の計測は `ts-verify.sh --record` の `cargo_invocations`（0 であること）を目標の判定にそのまま使える
  （付記 wire-1 で 0 回になった）。
