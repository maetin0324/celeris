---
title: 配送と sweep の競合による古い未統合通知を防ぐ
tasks: [01M60JC1N9V08GHGDSEHP27TRC]
status: done
updated: 2026-10-28
---
# 配送と sweep の競合による古い未統合通知を防ぐ

完了日: 2026-10-28

## 変更

commit: `phase 1: 配送と競合した sweep の古い未統合通知を防ぐ`

- sweep は祖先判定前の delivery と integration の snapshot に束縛して保存する。
- SQLite の IMMEDIATE transaction 内で一致を検査し、配送中の unintegrated は保存しない。
- integrated への更新は配送中も許し、既存の受信箱項目を自動で解消する。
- 人の hold / discard と他の統合結果を古い判定で上書きしない。
- [ADR](../adr/2026-10-28-stranded-sweep-delivery-race.md) に判断を記録した。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）

- `cargo test -p task-ops --lib stranded::sweep_race_tests` → exit 0、6 passed。
  hook で配送開始・完了を祖先判定の前後へ挟む。実 Git merge を使い、通知の元になる
  integration 行、IntegrationDetected の不在、受信箱項目の消去と追記 event を検査する。
- `bash scripts/dev/test-affected.sh` → exit 0。Rust 6,654 passed（21 skipped）、web 型検査合格、
  vitest 685 passed、node 105 passed、対象 doc test 3 passed（1 ignored）。
- `cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/check-doc-links.sh` / `sh scripts/dev/check-adr-numbers.sh` /
  `sh scripts/dev/progress-index.sh --check` / `git diff --check` → exit 0。

## 未解決

なし。本番への反映・本番 DB の変更は行っていない。
web の offline install は jsesc のキャッシュ不足で失敗したため、通常の frozen-lockfile install で補充した。
試験の外部ネットワーク接続は行っていない。

## 本番への移行

この task 自体には config・DB migration は不要。既存の unintegrated は更新後の定期 sweep（300 秒周期）で祖先を確認し、
統合済みに更新する。CoS の知らせは既存の受信箱同期で解消する。

ただし、この task を含む release は main に先に入った cos-routine-delegated-answers の config 変更を含む。
昇格の前に人が本番 config を直す（下の「配送の修復」）。

## 配送の修復（2026-10-28、attempt 2）

release 38288347af34 の prepare は gate を全て通った（fmt・test-parallel・clippy・build・web、各 exit 0）が、
verify の check 1（start-and-migrate）で staging の celeris が config の読み込みで止まった:

```
invalid config: [cos.triage] human_required: unknown reason "fundamental_change"
(known: objective_change, beyond_frame, external_publish, destructive, secret_or_security,
 standing_permission, explicit_human; fundamental_change, resource_overrun and security were
 replaced by objective_change, beyond_frame and secret_or_security)
```

- 原因はこの task の変更ではない。main に先に入った `phase 1: 承認済みの枠の内側の定型の判断を CoS が根拠つきで代答する`
  （[progress](2026-10-28-cos-routine-delegated-answers.md)、ADR cos-routine-delegated-answers）が `[cos.triage] human_required` の
  旧名を読み込みエラーにした。本番 `~/.config/celeris/config.toml` は旧名
  `fundamental_change`・`resource_overrun`・`security` と `policy_version = "4"` のまま（最終更新 2026-10-28 03:58Z）。
  その変更より前の release 830a92f82af2 は verify ok、以後の release は人が config を直すまで verify で落ちる。
- 互換の読み込みは足さない（`agent-docs/adr/2026-10-11-breaking-changes-by-default.md`）。コードは変えていない。
  この task の commit は main（`phase 1: 日次の知識候補抽出で推測した案件 slug の候補を元 task の案件か experience へ直す` の親）に
  取り込み済みで、branch は main へ fast-forward した。
- 人が行う手順は cos-routine-delegated-answers の進捗の「本番への移行」1〜3 のとおり（config の file 名を `config.toml` に直した）。
  要点: `[cos.triage]` の `human_required` を
  `["objective_change", "external_publish", "destructive", "beyond_frame", "secret_or_security", "standing_permission", "explicit_human"]`
  にし、`policy_version` を `"6"` にする。その後 `celerisctl skills import config/skills --name cos-inbox-triage --root <kb_root>` で
  本番 KB の skill を版 6 にし、release を昇格して `GET /api/v1/health` の `cos_policy_version` が `6` であることを確かめる。

## 提案

なし。
