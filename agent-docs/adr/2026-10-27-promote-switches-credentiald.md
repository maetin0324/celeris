# promote・rollback で credentiald を切り替える

日付: 2026-10-27 / 状態: 採用

## 背景
本番の drop-in（`celeris@.service.d/50-credentiald.conf` の `Wants=`）は、新しい daemon と一緒に同じ release の
`celeris-credentiald@<sha12>` を起こす。旧 broker は旧 daemon が draining の間 socket を持ち続けるため、新は
permission_denied（`crates/celeris-credentiald/src/ipc.rs`）で失敗を繰り返し、operator が毎回手で旧を止めて新を起こしていた。

## 決定
- `lib.sh` の `sd_switch_credentiald <新> <旧>`: 旧が active なら止め、新の失敗状態を `reset-failed` で消し、新を起こし、
  `SD_CREDENTIALD_SETTLE`（既定 3 秒）後も active なのを確かめる。新の unit が入っていない host では何もしない。
- promote.sh は live・stop-start のどちらでも、link の付け替え後に呼ぶ。rollback.sh（`--restore-db`）は web の戻しの後に呼ぶ。
  schema が読める rollback は promote.sh に渡るので同じ経路になる。
- 失敗しても daemon の昇格・rollback は取り消さない（daemon は健全）。exit 1 と手で行う手順を出して人に知らせる。
- 手動前提だった `docs/ops/browser-credential-login-v5.md` の手順は、失敗時の fallback に改めた。

## 試験
`scripts/selfdeploy/tests/credentiald_switch.sh`（偽 systemctl で順序・旧が非 active・unit 無し・起動後に非 active・停止失敗）。
