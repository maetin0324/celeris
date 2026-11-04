# 課題 T2: 項目の見出しを digest と card の両方で切る

対象: `cos_digest` 領域（CoS の受信箱の一次対応の結果を chat に出す digest）。

## 症状
受信箱の項目の `summary`（見出し）に長い文が入ると、digest の本文の `### <見出し>（<種類>）` の行と、chat の card の
`title` の両方が長くなり、画面が崩れる。

## 求める挙動（これだけを変える）
- 項目の `summary` が 80 文字（Unicode の code point。UTF-16 の長さではない）を超えたら、本文の `### ` の見出しと
  card の `title` の両方を、先頭の 80 文字の後ろに `…`（U+2026 を 1 文字）を足したものにする。80 文字以下は今のまま。
- `summary` が空白だけのときの見出し（`<source_kind>:<source_key>`）は変えない。他の行・他の欄は変えない。
- 既存の入力（旧挙動の fixture）の出力は 1 byte も変えない。

## 経路
- TS の経路: `control-ts/src/` の中だけを変える（`control-ts/src/domains/cos_digest.ts` の `digestMessage`・`title`）。
- 旧 Rust の経路: `crates/task-core/src/`・`crates/task-dispatch/src/` の中だけを変える
  （`crates/task-core/src/chat/digest.rs` の `cos_digest_message`、`crates/task-core/src/chat/triage.rs` の `CosTriageItemReport::title`）。
