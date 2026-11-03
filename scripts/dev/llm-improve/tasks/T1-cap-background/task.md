# 課題 T1: 人に回した項目の「背景」を digest で切る

対象: `cos_digest` 領域（CoS の受信箱の一次対応の結果を chat に出す digest）。

## 症状
人に回した項目（item の state が `escalated`・`fallback`）の判断の材料（`decision.background`）に、長い log や
stack trace がそのまま入ることがある。digest の本文の `- 背景: …` の行がそれを全部写すので、人が読む digest が埋まる。

## 求める挙動（これだけを変える）
- `- 背景: ` の行に出す背景が 300 文字（Unicode の code point。UTF-16 の長さではない）を超えたら、先頭の 300 文字の後ろに
  `…`（U+2026 を 1 文字）を足したものにする。300 文字以下は今のまま。
- cards・他の行・他の欄は変えない。背景を出さない状態（`answered` など）の出力も変えない。
- 既存の入力（旧挙動の fixture）の出力は 1 byte も変えない。

## 経路
- TS の経路: `control-ts/src/` の中だけを変える（`control-ts/src/domains/cos_digest.ts` の `digestMessage` が文面を作る）。
- 旧 Rust の経路: `crates/task-core/src/`・`crates/task-dispatch/src/` の中だけを変える
  （`crates/task-core/src/chat/digest.rs` の `cos_digest_message` が文面を作る）。
