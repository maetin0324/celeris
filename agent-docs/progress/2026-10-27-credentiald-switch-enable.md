---
title: promote・rollback の credentiald 切り替えで自動起動の設定も切り替える
updated: 2026-10-27
status: done
tasks: [01M5YFBZDEJ4ECH35K1TNFD0E2]
---

# promote・rollback の credentiald 切り替えで自動起動の設定も切り替える

完了日: 2026-10-27

- 背景: 昇格 8c097e068bdb の後、`celeris-credentiald@<新>` は active だが disabled、`@<旧>` は inactive だが enabled で残った
  （host 再起動で旧が起きる）。operator が手で直した。
- 実装: `sd_switch_credentiald`（lib.sh）を stop 旧 → disable 旧 → reset-failed 新 → start 新 → enable 新 → active の確認にした。
  旧の disable は旧が active でなくても行う。disable・enable の失敗は切り替えの失敗（`credentiald_switch_failed.json` と exit 3、既存どおり）。
  失敗時に log へ出す手順と `docs/ops/browser-credential-login-v5.md` の手順にも disable・enable を足した。
  ADR 2026-10-27-promote-switches-credentiald 付記 A1。
- 試験: `scripts/selfdeploy/tests/credentiald_switch.sh` の偽 systemctl が enable/disable/is-enabled を状態で持つ。
  A は順序に disable・enable を含め、B は非 active の旧も disable、F は disable・enable の失敗。G（live promote）・
  I（引き継ぎの再開）・K（rollback --restore-db）の後に、新が active かつ enabled、旧が inactive かつ disabled なのを確かめる。
  変更前の lib.sh では A・B・F×2・G・I・K の 7 件が落ちることを確かめた。grep が空振りすると set -e で黙って抜けていた `line()` も直した。

## 証拠（提出時の HEAD で実行）
- `bash scripts/selfdeploy/tests/credentiald_switch.sh` → exit 0（27 ok）
- 他の promote・rollback の shell 試験 → exit 0
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` → exit 0

## 本番への移行
- 不要（次の昇格から promote.sh が enable/disable も切り替える）。今の本番は operator が手で直し済み。

## 未解決事項
- 本番での確認は次の昇格で operator が `systemctl --user is-enabled celeris-credentiald@<新> celeris-credentiald@<旧>` で行う。

## 提案
