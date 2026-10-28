---
title: 流れ3 案件横断の停滞から統括CoSの対応と人への判断依頼
tasks: [01M60AKVZEB61PRBKX5AX44H3A]
status: done
updated: 2026-10-28
---
# 流れ3 案件横断の停滞から統括CoSの対応と人への判断依頼

完了日: 2026-10-28

## 変更

- `phase 1: 案件横断の停滞から統括CoSと人の判断を照合する試験を追加` — `actor_trace_flow3` に2試験を追加。共有harnessは意味を変えず利用し、rootのtests/supportは変更しない。
- 2案件のtaskの最終eventを固定UTCで記録し、`stagnation::gather/detect` に時刻を注入する。閾値直前・片方の到達・両方の到達と、受信箱の論理キー・revisionを確認する。
- 統括CoSの偽HTTPに項目の根拠を渡す。認可内ならqueueの決定要求へ回答し、認可外なら停滞の札をエスカレーションして、具体的な決定要求を人が回答する。対応していない別案件の札は未解決のまま残す。
- decision log export・eval requests・human-ops reportを実際に生成する。主体・operation・run・policy・根拠revision・通知本文・決定要求の参照と、統括CoSの共有消費を照合する。消費を案件へ推測で割り当てない。
- 停滞の札と具体的な決定要求は別の論理要求である。人の回答は後者の人待ちを閉じ、元の停滞の札を人が直接回答したと偽らない。人の作業時間と人待ちは別に扱う。
- 本物のLLM・外部ネットワーク・sleep・負荷試験は使わない。製品コード・設定・migrationの変更なし。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）

- 条件0: `cargo test -p celeris --test actor_trace_flow3` → exit 0、2 passed、0 ignored。根拠付きの統括CoS対応と人への判断依頼・回答を検証。
- 条件1: 同試験で固定UTCの最終eventと注入した時計を照合。閾値直前は0件、最初の到達時は1件、両案件の到達時は2件。sleepなし。
- 条件2: `bash scripts/dev/test-affected.sh --list` → exit 0、celerisとcelerisctlを選択。
- 条件2: `bash scripts/dev/test-affected.sh` → exit 0、809 passed・0 skipped。celerisのdoc testは0件、exit 0。流れ3も通常のintegration test targetとして実行。
- 条件2: `cargo clippy --workspace -- -D warnings` → exit 0。
- 条件2: `cargo fmt --all -- --check` → exit 0、差分なし。
- `git diff --check`・`sh scripts/dev/progress-index.sh --check`・`sh "$CELERIS_WU_SCOPE_PATHS"` → exit 0。変更は流れ3の試験と本進捗だけ。

## 未解決

なし。HTTP認証・実LLM harnessの起動は共有ADRの試験境界外。停滞の札は案件に新しい出来事が起きるまで残り、具体的な決定要求への回答だけでは解消したと扱わない。

## 提案

なし。

## 本番への移行

不要。
