---
title: 流れ4 配備とrollbackの行為者・判断参照の照合
tasks: [01M60AKVZEB61PRBKX5AX44H3A]
status: done
updated: 2026-10-28
---
# 流れ4 配備とrollbackの行為者・判断参照の照合

完了日: 2026-10-28

## 変更

- `phase 1: 配備とrollbackの監査を判断記録まで照合する試験を追加` — `actor_trace_flow4.rs` に追加。既存 deployment_audit 試験と同じ shell fixture の host command と隔離 state を再利用し、実際の promote.sh と rollback.sh を実行する。
- 同じ隔離stateでpromoteが書いたpreviousリンクを使い、promote・rollbackを各1回成功させる。別stateでstart失敗のpromoteを1回流す。認証付きGET /releasesのdeployment_auditでrelease、from/to、operation、actor、理由、policy、decision/task参照、verify、status/exit_codeを照合する。失敗では成功summaryが存在しないことも確認する。
- 一時DBの人の判断を固定UTCで保存し、decision log exportとeval requestsのtask・判断者human:webを監査参照と突き合わせる。外部operator実行を人の判断へ吸収せず、DBに操作・LLM消費を推測で足さない。
- 現行D2の語彙はexternal_operatorであり、旧SD_ACTOR=operatorは拒否対象。external_operator:flow4を使う。
- Python時計は子プロセスのsitecustomize、shell時計は偽dateで固定する。credentialdの待ちは既存のSD_CREDENTIALD_SETTLE=0を使い、sleep到達は偽commandの失敗で検出する。本番host・本物のsystemctl・LLM・外部networkを使わない。製品コードと共有helperは変更しない。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）

- `bash scripts/dev/test-affected.sh --list` → exit 0。celeris・celerisctlが対象。
- `bash scripts/dev/test-affected.sh` → exit 0。808 passed・0 skipped、doc test 0件・exit 0。流れ4も通常のintegration testとして通過。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0、差分なし。
- `git diff --check` → exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → exit 0。変更は本WUの試験と進捗の2ファイルのみ。

## 未解決

なし。

## 提案

なし。
