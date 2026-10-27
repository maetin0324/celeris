---
title: CoS 消費を thread・案件・種類付きで export
updated: 2026-10-27
status: done
tasks: [01M5XXF7Q7VE9G6Q3S83CA9ZBE]
---
# CoS 消費を thread・案件・種類付きで export

完了日: 2026-10-27

## 変更

- `phase 1: CoS 消費の識別情報と種類別小計を export する`
- CoS usage に thread_id・thread_kind・project_id・model・session を既存 DB から結合する。model は run 記録を優先し、session は run が参照した node_sessions（引退済みも含む）から取る。
- shared_overhead_by_kind で統括・案件・legacy・不明を分離。campaign 未帰属の共有行も campaign_id=null のまま export する。試行・arm への配分はしない。複数 export の結合では dedup_key で重複除去する。
- usage の無い終端 run も欠測として記録。観測 0 と区別する。進行中 run は投影しない。
- store 試験は既存行の context・session 交代・欠測・観測 0・小計・冪等性を確認。export 試験は複数試行への非配分・重複排除・別 campaign の除外を確認。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）

- 条件 0・1: `bash scripts/dev/test-affected.sh` → exit 0。追加 2 試験を含む Rust 6,494 passed / 21 skipped、対象 doc test 成功。web tsc 成功、Vitest 682 passed（98 files）、Node 98 passed。
- 条件 2: `cargo clippy --workspace -- -D warnings` → exit 0。
- 条件 2: `cargo fmt --all -- --check` → exit 0（差分なし）。
- `git diff --check` → exit 0。
- 初回 clippy の type_complexity は CoS 取得行を名前付き struct に整理して解消。その後に必須検査を再実行した。
- コミット後の検査 1 回で既存の `negative_paths_non_target_identity_and_canary_through_real_daemon` が task 投入時の `Connection reset by peer` で失敗（6,493 passed / 1 failed）。`cargo nextest run -p celeris-campaign --test ablation_application -E 'test(negative_paths_non_target_identity_and_canary_through_real_daemon)'` は exit 0（1 passed）。その後に上記の必須検査を再実行した。失敗ログは run 成果物の test-affected-connection-reset.log に保存。
- 本物の browser/userns 試験・web e2e・全 workspace 試験の直接実行は行わず、release gate に委ねる。

## 本番への移行

config・DB schema の変更なし。既存行の context は読み出し時に結合する。
未記録の usage 欠測 run は通常の CoS turn 投影で追加される。

## 未解決

CoS 変更についてはなし。既存試験の一時的な接続断は単独実行で再現せず、原因未特定（範囲外のコードは未変更）。既存の campaign の活動窓による一意帰属は維持し、案件が判明しても試行への直接帰属とは扱わない。

## 提案

なし。
