---
title: 健全性v2 G4 fix-sync — main で消えた TaskStore の method の呼び出しを直し、提出時の検査をやり直す
tasks: [01M6TYVJSXBZMSBC0YGAT2TTX0]
status: done
updated: 2026-11-11
---
# 健全性v2 G4 fix-sync — main で消えた TaskStore の method の呼び出しを直し、提出時の検査をやり直す

完了日: 2026-11-11

最終 review の前に celeris が main を task の branch に merge した後、task-dispatch の lib test がコンパイルできなくなった。`crates/task-dispatch/src/dispatcher/tests/health_generic.rs` の 1 か所を、main にある transaction 版の関数に替えて直した。本体のコード・`TaskStore`・検査器・policy・`crates/task-core`・`crates/campaign` は変えていない。基準は `$CELERIS_WU_BASE`（task は `git merge-base HEAD main`）。

## 症状
最終 review の test-affected が exit 101 で落ちた（`error[E0599]`、`could not compile task-dispatch (lib test)`）。決定的な check が落ちたので、最終 review の他の条件は評価されなかった。

## 原因
main の commit「store-prune: 呼び手の無い TaskStore method 4 個と B0 割り当ての未参照 store 関数 9 個を消す」が、`TaskStore` の案件の題名を書く method を消した。G4 の rs-paths が足した `health_generic.rs` の `review_task` は、flag の `project_named_celeris` の分岐でこの method を呼んでいた。G4 の branch と main を別々に試験した時点ではどちらも通り、merge の後に初めて壊れた。

## 実装前 brief
1. 根本原因: 試験の helper `review_task` が、main で消えた `TaskStore` の method を呼んでいる。症状は task-dispatch の lib test のコンパイルエラーに出る。
2. 責務 owner: 試験の helper（`health_generic.rs` の `review_task`）。method を消すという main の決定は正しく、本体側に直す所は無い。
3. 既存の再利用先: main にある `SqliteStore::project_set_text_tx` と `SqliteStore::write_tx`。`dispatcher/tests/project_achievement.rs` の `Case::set_request` が同じ形で案件の説明を書いている。
4. 選択肢: (a) `TaskStore` に method を戻す。main の決定を覆し、高 risk の path class `store`（`crates/task-core/src/store/`）を変えるので採らない。(b) `review_flow` が `SqliteStore` の handle を残し、`review_task` に渡して transaction 版の関数で書く。(b) を採った。
5. 新しい状態・分岐・fallback・port: 無し。`review_task` の引数に同じ DB の `&SqliteStore` が 1 つ増えただけ。
6. 撤去対象: 無し（消えた method の呼び出しを置き換えた）。
7. 不変条件: `health_generic_flags_do_not_change_outcome` は、案件の題名と slug を `celeris` にしても health の判定が変わらないことを確かめる。題名の書き込みが成功したこと（`assert!` が true を要求する）と slug の書き込みの確認はそのまま残した。試験の数（`health_generic_` 28 件）と `should_panic` の付いた試験（G4-R1 の 4 件）は変えていない。

## 直し
- `review_flow`: `SqliteStore::open_in_memory()` の `Arc` を `sqlite` として残し、そこから `Arc<dyn TaskStore>` を作る。
- `review_task`: 引数に `sqlite: &SqliteStore` を足し、案件の題名を `sqlite.write_tx(|tx| SqliteStore::project_set_text_tx(tx, project, Some("celeris"), None))` で書く。slug と自動統合は `TaskStore` に残っている method のまま。

## 変更（commit の件名）
- `phase 1: set the fixture project title with project_set_text_tx after the store prune`
- `phase 1: record fix-sync cause, fix and submit checks`

## 証拠（提出時の HEAD で実行）
結果は task の進捗 [2026-11-10-health-v2-g4.md](../2026-11-10-health-v2-g4.md) の「証拠」節に書いた。この unit で最初に確かめたのは次の 1 件。

- `cargo nextest run -p task-dispatch --lib -E 'test(/health_generic_/)'` → exit 0。28 tests run、28 passed（コンパイルが通った）。

## 未解決
- 無し（G4 の未解決は task の進捗の「未解決」節のまま）。
