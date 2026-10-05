---
title: CoS チャットホーム ADR と受信箱一次対応の設計契約
tasks: [01M46VVAD0ZAVZ9C4Q0KJM9ESV]
status: done
updated: 2026-10-05
---
# CoS チャットホーム — adr WorkUnit

対象は設計文書のみ。実装完了ではない。design 段の人の確認は Fable が行う。

## 完了内容

[ADR](../../adr/2026-10-05-cos-chat-home.md) の D1〜D6 に SQLite の表/列/index と migration 0050、REST/SSE の endpoint・JSON、thread ごとの session、config、全道具の境界、添付と task/KB 引渡し、UX、旧 Console/actions/gui の移行を記録した。

人の追加要望を D3/D6 に反映した。新しい待ちは決定的に CoS の継続 session を起動し、CoS が代答または Discord で人へ依頼する。人に回す基準、actor=cos と理由、代答カードと取消/差し戻し、Discord は送信のみ・web で回答、CoS 不在時の直接退避を確定した。現行 ADR-0133 の inbox_new/reminder/digest も置き換え対象に含めた。

旧 ADR-0033/0048/0054 と通知の 0037/0050/0133、並列度の 0089 には本文を残したまま付記した。architecture-map に設計契約の索引行を追加した。

## 調査の根拠

- 既存 messages と node_sessions、Console API、notify::schedule_routes、provider/config の実装を確認した。
- 全 1,331 refs（heads/remotes）と登録 worktree の migration を走査し最大 0049、未使用の 0050 を選択。現 HEAD は 8dc4c5e48b08415372d747a0d5f91fcab5a5b9d4。完了前にも 1,333 refs・159 worktree で再走査し、0050 が空いていることを確認した。store-api の SQL 追加前にも再走査する。
- ADR の API/JSON 例は後段実装を縛る契約であり、現行 API と混同しない。

## 検証

全て exit 0。

| コマンド | 結果 |
|---|---|
| `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | ok |
| `sh scripts/dev/check-doc-links.sh` | ok |
| `sh scripts/dev/check-adr-numbers.sh` | ok、141 files |
| `sh scripts/dev/check-adr-numbers.sh --refs` | ok |
| `sh scripts/dev/progress-index.sh --check` | ok |
| `python3 scripts/dev/check-architecture-map.py` | 235 パスの実在を確認 |
| `git diff --check` | 空白エラーなし |

追加の構造確認で D1〜D6、REST/SSE 表、通知の 3 経路を確認。旧 ADR 7 本は base の内容が byte 単位で前方一致し、本文を変更せず付記だけであることを確認した。変更範囲は CELERIS_WU_BASE との diff と untracked を合わせ、指定 ADR・旧 ADR 付記・architecture-map・この進捗だけである。

## 後続段の必須確認

ADR D6 の受け入れ条件に偽 harness・偽 webhook の 3 経路（人不要→CoS 代答、人必要→Discord、不在→退避）と通知入口の一本化試験を追加した。これらの実装/実行は cos-run が担当し、close-out でも確認する。web-chat は代答の取消/差し戻しを含む UI と Playwright、live-check は試験 DB の実機 1 回、close-out は全体 Rust/web 検査を担当する。本 WorkUnit ではアプリコードを変更していないため Rust/web の試験は実行しない。
