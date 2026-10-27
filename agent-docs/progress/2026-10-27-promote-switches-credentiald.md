---
title: promote・rollback で credentiald を切り替える
updated: 2026-10-27
status: done
tasks: [01M5XXF7QNDDFANAXYRND6T9VY]
---

# promote・rollback で credentiald を切り替える

完了日: 2026-10-27

- 実装: `sd_switch_credentiald`・`sd_credentiald_after_switch`（lib.sh）。promote.sh は `promoted.json` を書いた後の共通の
  末尾（live・stop-start・引き継ぎの再開）で、rollback.sh（`--restore-db`）は link の付け替えと `promoting.json` の片付けの後で呼ぶ。
  失敗は `credentiald_switch_failed.json` と exit 3（昇格・rollback は取り消さず、`promote_failed.json` も書かない）。
  ADR 2026-10-27-promote-switches-credentiald。
- 試験: `tests/credentiald_switch.sh`（単体 A〜E、promote.sh・rollback.sh を最後まで走らせる G〜K）。既存の promote 試験の偽
  systemctl は credentiald の is-active を真にした。
- attempt 2: review の指摘（rollback で link 付け替え前に落ちると daemon が再起動で exit 4、promote で失敗が
  promote_failed.json になる、引き継ぎの再開で呼ばれない、端から端の試験が無い）を直した。G〜K は前の実装で落ちることを確かめた。

## 証拠（提出時の HEAD で実行）
- `bash scripts/selfdeploy/tests/credentiald_switch.sh` → exit 0（22 ok）
- `bash scripts/selfdeploy/tests/{promote_authorization_marker,promote_live_abort,promote_handoff_settled,promote_web_follows_release,promote_backup_retention,release_notes_promote,web_follow_health_gate}.sh` → すべて exit 0
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` → exit 0

## 未解決事項
- 本番での確認は次の昇格で operator が行い、ここに残す。
- `credentiald_switch_failed.json` は `GET /releases` にはまだ写らない（log と exit 3 だけ）。

## 提案
- `GET /releases` の items に `credentiald_switch_failed` を写し、GUI に出す。
- test-affected は scripts/selfdeploy の変更で何も走らせない（「docs のみ等」）。変更した `scripts/selfdeploy/tests/*.sh` を走らせる段を足す。

## 本番への移行
config の変更は無い。次の昇格で `systemctl --user status celeris-credentiald@<新>` が active なこと、release に
`credentiald_switch_failed.json` が無いことを確かめる。
