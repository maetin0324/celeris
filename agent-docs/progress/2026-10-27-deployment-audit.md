---
title: 配備・rollback の監査記録
updated: 2026-10-27
status: done
tasks: [01M5XXF7QP63BMV6AS0WNDW01N]
---
# 配備・rollback の監査記録

完了日: 2026-10-27

## 変更

- `phase 1: 配備とrollbackの実行根拠・検証結果を監査履歴に残す` — 必須実行者・理由・方針、task/decision 参照、verify snapshot、操作ごとの終了結果を保存。履歴 API と status に公開。
- 人 / CoS の API 経路を区別し、CoS の認証済み操作への参照を引き継ぐ。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）

- `bash scripts/dev/test-affected.sh` → exit 0。web 型検査、Vitest 682 件、Node 98 件、影響 crate の nextest / doc test 合格。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `cargo test -p celeris --test deployment_audit` → exit 0、1 passed。実スクリプトを偽 host コマンドで実行し、必須情報欠落時の副作用前拒否、live / stop-start、通常 / DB 復元 rollback、失敗記録、再試行履歴、verify snapshot を検証。
- `bash scripts/selfdeploy/tests/promote_live_abort.sh` → exit 0、all ok。
- `UPDATE_SCHEMA=1 cargo test -p task-api schema --lib` → exit 0、3 passed。`node web/scripts/gen-types.mjs` で生成型を更新。
- API 履歴と環境変数伝搬、CoS の actor・理由・方針・操作参照は Rust 回帰試験で検証。
- 本番配備、release gate、全 workspace の一括試験は実行していない。

## 本番への移行

本番変更は未実行。人が初回の昇格で本変更の promote.sh を使用し、`SD_ACTOR` / `SD_REASON` / `SD_POLICY_REF` を実際の実行者・理由・承認参照で設定する。以降の CLI rollback も同じ必須値を渡す。既存の配備記録は保持する。

## 未解決

なし。既存 release の実行者の欠落は推測で埋めない。

## 提案

なし。
