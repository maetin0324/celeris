---
title: S3 grade-gate — ts-verify --grade と、ts-only でない control-ts 変更での採点
tasks: [01M6EVP95S9VM3DBMZFRD8FS35]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S3 grade-gate — ts-verify --grade と、ts-only でない control-ts 変更での採点

完了日: 2026-11-02（task 01M6EVP95S9VM3DBMZFRD8FS35、unit grade-gate。親 task は 01M6DCMB75G5F0DSE58ZNZMBAK）

## 結論
- final review（条件 3）の抜け道を塞いだ。`control-ts/scripts/**` などの `host-api` の変更で、規則を壊し `compare.test.mjs` の試験を
  `test.skip` にした候補は、`test-affected.sh` が呼ぶ `ts-verify.sh --grade` の段 `golden`（Rust の runner の採点）で exit 1 になり、
  `test-affected.sh` も非 0 で終わる。
- ADR 2026-11-02-ts-fast-verify に付記 grade-gate（grade-gate-1〜4）を書いた。close-6 は grade-gate-3 で置き換えた。
- cargo の crate の選択・release gate・ts-only の経路は変えていない。

## 変更
- `phase s3: grade-gate: ts-verify --grade と test-affected の採点の接続`
  - `scripts/dev/ts-verify.sh`: `--grade` を足した。分類が `ts-only`・`host-api` なら候補の作業ツリーで 10 段を通す。単体試験は全部、
    段 `golden` は `compare.mjs` を使わず fixture と cache の golden の `cmp` と `script-host-runner run --suite golden --domain <d>` で決める。
    記録に `grade` を足した。`rust`・`none`・追加の禁止の扱いと引数なしの挙動は変えていない。
  - `scripts/dev/test-affected.sh`: `control-ts/` か `docs/api/script-host/` を含み ts-only でない変更で `ts-verify.sh --grade --record` を走らせ、
    非 0 なら exit 1。記録が `ts-only`・`host-api` なら 10 段すべて exit 0 を確かめる。`--list` に `control-ts: ts-verify-grade (理由: …)` の行。
  - `scripts/dev/test-ts-verify.sh`: 場合 J（2 件の確認）を足した。
  - `control-ts/scripts/test-affected.test.mjs`: 偽の `ts-verify.sh` を引数を記録し `FAKE_TSV_RC` で落とせる形にし、
    `control-ts/scripts` の変更で `--grade` が呼ばれることと失敗の伝播の試験を足した。既存の 2 件の期待に `--grade` の行と呼び出しを足した。
  - `agent-docs/adr/2026-11-02-ts-fast-verify.md`: 付記 grade-gate。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
| 受け入れ条件 | コマンド | 結果 |
|---|---|---|
| 0 付記 | `grep -n '付記 grade-gate' agent-docs/adr/2026-11-02-ts-fast-verify.md` | 1 件 |
| 1・3 `--grade` と場合 J | `bash scripts/dev/test-ts-verify.sh` | exit 0、14 passed・0 failed。J: `--list` が `ts-verify-grade` を選ぶ、引数なしは「従来の経路へ」exit 0、`--grade` は exit 1（通った段 classify〜unit、失敗の段 golden）。無害な host-api の変更（`compare.mjs` の注記）は `--grade` で 10 段 exit 0、cargo 0 回 |
| 2・3 test-affected の接続 | `cd control-ts && node --test scripts/test-affected.test.mjs` | pass 8・fail 0（新しい場合: `control-ts/scripts` の変更で `ts-verify --grade --record` を呼ぶ、偽が exit 1 なら非 0 と「ts-verify.sh --grade が失敗した」） |
| 4 既存の選択 | `bash scripts/dev/tests/test-affected-list.sh` | `test-affected-list: ok`、exit 0 |
| 4 release gate | `git diff --stat "$CELERIS_WU_BASE" -- scripts/selfdeploy .config/nextest.toml` | 差分なし |
| 完了手順 | `bash scripts/dev/test-affected.sh` | exit 0。control-ts typecheck・unit（node --test pass 31・fail 0）、`ts-verify.sh --grade`、`test-ts-verify.sh` 14 passed、nextest celeris・celeris-script・celerisctl 915 passed、doc test 通過 |
| 完了手順 | `cargo clippy --workspace -- -D warnings` / `cargo fmt --all -- --check` | exit 0 / exit 0 |

途中の失敗: 場合 J の無害な変更を最初は `host.d.ts` の改行にしたが、`--grade` の段 unit（生成型と `host.d.ts` の一致の試験）で落ちた。
これは正しい検出なので、無害な変更を `compare.mjs` の注記に替えた。

## 未解決
1. 分類が `rust` の変更（例: `crates/celeris/` と `control-ts/` の同時の変更）では `--grade` も従来の経路へ抜け、候補の control-ts は runner で採点されない
   （objective の「rust の扱いは今のまま」に従った。付記 grade-gate-4 (a)）。この unit の差分も `scripts/dev/` を含むので `rust` で、
   完了手順の `test-affected.sh` の中の `--grade` は従来の経路へ抜けた。代わりに `test-ts-verify.sh` が写しで確かめた。
2. 段 `bundle` は候補の `build-bundle.mjs` を使う（付記 grade-gate-4 (b)）。
3. `docs/architecture-map.md` の ts-verify の行は付記 grade-gate を挙げていない（この unit の範囲外として触らなかった）。

## 提案
1. `--grade` を、分類が `rust` でも `control-ts/`・`docs/api/script-host/` に変更があれば 10 段を通すようにする（未解決 1 を塞ぐ。別 task）。
2. unit reclose で S3 の進捗（close-6・未解決 1）と `docs/architecture-map.md` の行（付記 grade-gate と「`--grade` で採点する」）を直す。
