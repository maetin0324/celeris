---
title: campaign launcher 試験 a_swapped_daemon_binary の flaky を直す（fix-launcher-flaky）
tasks: [01M6SVVGDHQT5C4T2B1QSCAY1Y]
status: done
updated: 2026-11-07
---
# campaign launcher 試験 a_swapped_daemon_binary の flaky を直す（fix-launcher-flaky）

完了日: 2026-11-07

## 原因（コード読みで特定。高負荷での再現は未実施）
`scripts/campaign/measure-launcher.sh` の `cmd_start` は `nohup "$CELERIS_BIN" ... &` の直後に pid と starttime を記録し、health 待ちの中で `owned_alive` を呼ぶ。
`owned_alive` は cmdline に `--config $DAEMON_CONFIG` を含むことを要求する。fork 直後の子は nohup が exec するまで launcher 自身の cmdline のままなので、
全体並走の負荷で exec が遅れると照合に落ち、「daemon exited before becoming healthy」と判定して `cleanup_started`（stop_owned）に進む。
stop_owned の `owned_identity` も同じ理由で落ち、「pid N is not the daemon this launcher started (starttime/cmdline/image differ)」が出る。
試験の待ち方ではなく、本体の起動直後の照合窓が原因。

## 変更
- `scripts/campaign/measure-launcher.sh`: 起動直後に、exec の完了（cmdline が daemon のものになる）か子の終了（zombie 含む）を出来事として待つ。時計や固定 sleep の延長はしていない（0.01 秒の poll のみ。終了条件は出来事）。
- 指示は変更を crates/campaign と進捗に限っていたが、原因が script 側にあるため script を直した。試験側（crates/campaign）は変更なし。`scripts/dev` の runner と task-worker・task-dispatch は触っていない。

## 証拠
- `cargo test -p celeris-campaign --test launcher`: 15 passed, 0 failed, 1 ignored（exit 0）。
- 検査（test-affected・clippy・fmt）は提出時の HEAD で実行した結果を報告に書く。

## 未解決
- 高負荷下での再現は未確認（CPU を焼く台本は使わない規則）。窓は構造上存在するため修正したが、実際の失敗がこの窓かは次に落ちたとき log で確認が要る。
