# promote・rollback で credentiald を切り替える

日付: 2026-10-27 / 状態: 採用

## 背景
本番の drop-in（`celeris@.service.d/50-credentiald.conf` の `Wants=`）は、新しい daemon と一緒に同じ release の
`celeris-credentiald@<sha12>` を起こす。旧 broker は旧 daemon が draining の間 socket を持ち続けるため、新は
permission_denied（`crates/celeris-credentiald/src/ipc.rs`）で失敗を繰り返し、operator が毎回手で旧を止めて新を起こしていた。

## 決定
- `lib.sh` の `sd_switch_credentiald <新> <旧>`: 旧が active なら止め、新の失敗状態を `reset-failed` で消し、新を起こし、
  `SD_CREDENTIALD_SETTLE`（既定 3 秒）後も active なのを確かめる。新の unit が入っていない host では何もしない。
- 切り替えは昇格・rollback を記録し終えた後に行う（`lib.sh` の `sd_credentiald_after_switch`）。
  - promote.sh: live・stop-start・引き継ぎの再開（`RESUME_HANDOFF`）の共通の末尾、`promoted.json` を書いた後。
    `promoted.json` の後の非 0 終了では EXIT trap は `promote_failed.json` を書かない（昇格は済んでいる）。
  - rollback.sh（`--restore-db`）: `previous`・`current` の付け替えと `promoting.json` の片付けの後。それより前に
    落ちると `current` が旧のまま印が消え、戻した daemon が次の再起動で未昇格（exit 4）として止まるため。
    schema が読める rollback は promote.sh に渡るので promote.sh の経路になる。
- 失敗しても daemon の昇格・rollback は取り消さない（daemon は健全）。release の `credentiald_switch_failed.json`
  （`failed_at`・`from`・`error`）に残し、手で行う手順を log に出して exit 3 で終わる。次に切り替えが成功したら消す。
- 手動前提だった `docs/ops/browser-credential-login-v5.md` の手順は、失敗時の fallback に改めた。

## 試験
`scripts/selfdeploy/tests/credentiald_switch.sh`。`sd_switch_credentiald` 単体（順序・旧が非 active・unit 無し・起動後に
非 active・停止失敗）と、偽の systemctl・curl・sqlite3・ss で promote.sh（live・stop-start の失敗・引き継ぎの再開）と
rollback.sh --restore-db（失敗・成功）を最後まで走らせ、切り替えが link の付け替えの後に走ること、失敗時の exit 3・
link・`promoted.json`・`promote_failed.json`・`promoting.json`・`credentiald_switch_failed.json` を確かめる。
