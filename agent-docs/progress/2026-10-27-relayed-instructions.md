---
title: 外部 operator・MCP が取り次いだ人の指示
tasks: [01M5XXF7QGZ69112THK8RE3TEB]
status: done
updated: 2026-10-27
---
# 外部 operator・MCP が取り次いだ人の指示

完了日: 2026-10-27

## 変更

- `phase 1: 取り次がれた人の指示を保存し CoS の判断根拠にする` — 既存チャットの metadata に原文の発言時刻・認証済み取り次ぎ主体・案件/task/期限を記録。operator 専用 API と MCP console_instruct を接続。
- CoS は同スレッドの有効な取り次ぎを instructed_by で参照する。適用監査に判断者 human（取り次ぎ主体つき）と実行者 cos_project/cos_general を記録する。別案件・範囲外・期限切れを拒否する。
- API/schema/worker prompt・triage skill・外部 operator 手順書を更新。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）

- `bash scripts/dev/test-affected.sh` → exit 0。Rust 6573 passed / 21 skipped、web tsc 成功・unit 682 passed・gateway 99 passed、対象 doc test 成功。
- 同 command で operator の原文・時刻・主体・範囲保存、CoS の代答と判断者/実行者分離、期限切れ・別案件・別 task・取消済みの拒否、MCP の主体、後続 run への配送を検査した。
- 初回は route 分類の網羅性だけ失敗（6572 passed / 1 failed）。専用 route を CoS の EXCLUDED に登録し、ADR/skill と揃えて再実行した。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0、差分なし。

## 本番への移行

本番変更は実行していない。配備後、人が `config/skills/cos-inbox-triage/SKILL.md` を本番 KB の同 skill に反映し、
本番 config `[cos.triage] policy_version` を `"4"` に更新する。operator は自身の A1 token で専用 route を使う。
DB migration は不要（既存 chat metadata）。既存の発言と events は保持する。

## 未解決

コード・自動検査の未解決事項なし。本番での動作は未確認。

## 提案

なし。
