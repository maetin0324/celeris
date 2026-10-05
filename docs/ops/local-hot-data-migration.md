# サービスの hot データを /local へ移す（手順書）

---
tasks: [01M3Z08A0T81ZQ60XVR62XJMPD]
---

作成: 2026-10-03。実行者: **人**（systemd・`~/.config/celeris`・`~/.local/celeris`・本番 DB の操作が要るため。ADR-0095 付記 D-d）。
正本の決定は [ADR-0136](../../agent-docs/adr/0136-local-hot-data-layout.md)（分類表・path の契約・btrfs 上の SQLite）。scratch・target・sccache の seed/reflink 配置は [ADR-0129](../../agent-docs/adr/0129-host-sccache-reflink-targets.md) が正で、この手順書では重複して決めない。
台本は `scripts/selfdeploy/migrate-to-local.sh <stage> [--dry-run]`。全段とも `--dry-run` を先に走らせ、ログ（`$HOME/.local/celeris/migrate-to-local/logs/`）と計画を見てから本実行する。

## 0. 前提確認

本実行の前に、人が次を確認する（`--dry-run` の `presync` も同じ内容を読み取り専用で確かめる）。

- `/local` が **mount point である**こと: `findmnt -M /local`（source・fstype）。`findmnt -n -o FSTYPE -M /local` が `btrfs` であること。rootfs 上にたまたま `/local` という名前のディレクトリがあるだけなら中止する（bind mount が外れている）。
- 所有者と書込み: `stat -c '%u %a' /local` が uid **1001** であり、`[ -w /local ]` が通ること。
- 空き容量: `df -B1 --output=avail /local` と、Proxmox host 側の LVM-thin pool の空き（host 管理者が別途確認）。
- DB ディレクトリの NOCOW 属性: `presync` 実行後に `lsattr -d /local/celeris/data/db` が `C` を含むこと（空ディレクトリに先に `chattr +C` する。既存ファイルに後から付けても既存 extent は変わらない — ADR-0136 §3）。

## 1. 容量見積もり（GC 前後）

2026-10-02 19:00 UTC の実測（ADR-0136 の棚卸し表）では、`/var/lib/celeris` の既知項目で約 299.4G、home から移す hot 項目（releases・tools・staging・logs）で約 7.3G、合計 **約 306.7G**。`/local` は 300G しかなく、btrfs metadata・移行中の差分・reflink 共有 extent の余裕も要るため、そのままでは収まらない。

- `migrate-to-local.sh gc --dry-run` が、終端 task の workspace・target の prune 候補（`celerisctl workspace prune --dry-run`）と scratch GC 候補（`celerisctl scratch gc --dry-run`）、および現時点でコピーする予定の合計バイト数（`planned_bytes`、reflink は二重計上）と `/local` の空きを出す。
- 目安として、コピー対象を **240G 以下** まで減らす（現在値から少なくとも約 67G の削減が必要 — ADR-0136 §1）。削減の主な対象は `/var/lib/celeris/workspaces`（126G）と `/var/lib/celeris/scratch`（172G）の終端 task 分。
- 不要と分かっている `~/.local/celeris/build-cache-nfs`（7.7G）と `~/.local/celeris/cache/sccache-l2`（92G）は**コピーしない**（台本が最初から対象に含めない）。削除は §6 を参照。
- `gc --dry-run` で候補を確認したあと `migrate-to-local.sh gc`（dry-run なし）で実際に prune・GC を回し、`presync --dry-run` を再実行して `/local` の空きとコピー予定量を測り直す。切替直前に **30GiB 以上**の空きが残る見込みになるまで `gc` を繰り返す。running/reviewing の保護と scratch lease の所有は `celerisctl` 側が行う（この台本は重複実装しない）。

## 2. 停止時間を最小にする順序

段名は `migrate-to-local.sh` の `<stage>` 引数と同一（presync → gc → stop → delta → switch → start → verify）。**停止が必要なのは stop 以降だけ**。presync と gc は稼働したまま流せるので、ここで大半のデータ（DB を除く）を先にコピーしておき、停止時間を「差分コピー + DB 切替」だけに縮める。

| 順 | 段 | 呼び出し | 稼働状態 | すること |
| --- | --- | --- | --- | --- |
| 1 | presync | `migrate-to-local.sh presync --dry-run` → 確認後 `migrate-to-local.sh presync` | 稼働中 | `/local` の検査、DB ディレクトリの `chattr +C`、hot 項目（DB family を除く）の初回 rsync コピー。容量不足なら `not enough space` で止まる → §1 の gc に戻る |
| 2 | gc | `migrate-to-local.sh gc --dry-run` → 必要なら `migrate-to-local.sh gc` | 稼働中 | 終端 task の workspace・target と scratch の回収。`presync` を取り直して空きを測り直す |
| 3 | stop | `migrate-to-local.sh stop --dry-run` → `migrate-to-local.sh stop` | **停止** | in-flight（running + reviewing）が 0、release.sh/verify.sh の lock が空いていることを確認してから `celeris@` / `celeris-gui@` / `celeris-credentiald@ <current>` を止め、旧 DB を `wal_checkpoint(TRUNCATE)` + `integrity_check` で検査する |
| 4 | delta | `migrate-to-local.sh delta --dry-run` → `migrate-to-local.sh delta` | 停止中 | 差分を `rsync --delete` で再コピーし、DB は SQLite backup API で `+C` のディレクトリへ写して `integrity_check` |
| 5 | switch | `migrate-to-local.sh switch --dry-run` → `migrate-to-local.sh switch` | 停止中 | `config.toml` の path key と `paths.env` を書き換え（`.bak-migrate-<ts>` を残す）、旧い場所を `.bak-migrate-<ts>` に退けて `/local` への symlink に替え、`install-units.sh` で unit を新しい根で再設置 |
| 6 | start | `migrate-to-local.sh start --dry-run` → `migrate-to-local.sh start` | 再開 | `/local` の mount を確認し、`systemctl --user daemon-reload` のあと stop で止めた unit を起こす |
| 7 | verify | `migrate-to-local.sh verify`（常に read-only、dry-run と同じ） | 稼働中 | §3 の確認 |

停止〜再開（3〜6）は差分コピーと DB 切替だけなので、presync・gc を先に流しておけば実データ量に対して短時間で終わる。
`switch` が途中で失敗した場合は、config・paths.env・symlink・旧 DB・unit（drop-in を含む）を控えから自動で戻し、非ゼロで終了する。切替記録も消すため、原因を直して `switch` を再実行できる。

## 3. 確認方法

`verify` 段が次を自動で確かめる（NG が 1 つでもあれば非ゼロ終了し、ログに `NG:` 行を残す）:

- `GET /api/v1/health` が 200（`MIGRATE_VERIFY_TIMEOUT` 既定 60 秒まで待つ）。
- 認証付き `GET /api/v1/config` の `db` が新しい DB path（`/local/celeris/data/db/celeris.sqlite3`）と一致。
- 新 DB の `PRAGMA integrity_check` が `ok`。
- `/local/celeris/data/db` に `C` 属性が付いている（`lsattr -d`）。
- `$NEW_STATE/current` が `$NEW_STATE/releases/...` を指す（GUI の `node_modules` も `/local` 配下に解決されること）。
- `/local` の空きが `MIGRATE_MIN_FREE_GIB`（既定 30GiB）以上。
- 旧 path を指したままの symlink が `/local` 配下に残っていないか（警告として列挙）。

これに加えて人が確認する:

- `scripts/selfdeploy/status.sh` の JSON（`current` / `previous` / `health` / `gui_health` / `daemon_instances` / `stale_instances`）。読むだけで何も変えない。
- `journalctl --user -u 'celeris@*' -u 'celeris-gui@*' -u 'celeris-credentiald@*' -n 100` でエラーが出ていないこと。
- 新規 backup（`[db].backup_dir` = `/local/celeris/state/backups`）が作られ、検証済みの世代を home の archive に人が複製したこと（`verify` は backup の home 転送まではしない。転送が済むまで移行完了とはしない — ADR-0136 §2）。
- web 配布物（`web/app`）は移行の成功判定に含めない（移行当時は release.sh の `SD_GATE_SKIP_WEB` が既定 1 — main f1904ecd。2026-10-05 に既定 0 へ戻した）。

## 4. 戻し方（rollback）

`migrate-to-local.sh rollback [--dry-run] [--restore-db-from-new | --discard-new-writes]`。

- **start していない**（`switch` までで止めた）場合: 追加オプションなしで良い。symlink・config・`paths.env`・unit を `.bak-migrate-<ts>` から復元し、旧 unit を起こす。
- **start 済み**（新 instance が書込みをしている可能性がある）場合: 次のどちらかを明示しないと rollback は何もせず止まる。
  - `--restore-db-from-new`: 新 DB を SQLite backup API で旧 DB の位置へ複製してから戻す（旧 DB の直前の内容は `celeris.sqlite3.pre-rollback-<ts>` として残る）。
  - `--discard-new-writes`: 新 DB への書込みを捨てて旧 DB のまま戻す。
- rollback は `/local` 配下にコピーした実体を削除しない（やり直しの再コピーを避けるため）。不要になれば人が別途消す。
- rollback 後も `.bak-migrate-<ts>` の控えと `migrate-to-local/logs/` は保持する。削除は §5・§6 の確認後に人が行う。

## 5. 不要になった控えの削除（移行完了後、人が実行）

移行後しばらく運用し、新しい配置に問題がないことを確認してから、人が次を消す（台本は消さない）。

- `$HOME/.local/celeris/<item>.bak-migrate-<ts>`（`switch` が退けた旧 releases・tools・staging・logs・credentiald など）。
- `/var/lib/celeris/<item>.bak-migrate-<ts>`（同様に退けた workspaces・scratch・build-cache・web・memory・release-build など）。
- 旧 DB family の控え `$HOME/.local/celeris/celeris.sqlite3.bak-migrate-<ts>`（と `-wal`・`-shm`）。
- `$HOME/.local/celeris/migrate-to-local/` の古い log（監査のため直近分は残してよい）。

## 6. 不要な build-cache-nfs と cache/sccache-l2 の削除（ADR-0129 の sccache 移管後、人が実行）

これら 2 つは最初からコピー対象に含まれない（ADR-0136 の分類は「削除」）。削除は ADR-0129 のタスク（01M3YD2Z585N1YCBZK4AH8QXR0、host の `~/.cargo/config.toml` への sccache 移管）が完了し、旧 `celeris-sccache.service` / `celeris-scratch-cache.service` が止まって参照元がなくなったことを人が確認してから行う。

1. 参照なしの確認: `systemctl --user status celeris-sccache celeris-scratch-cache`（both inactive/not-found であること）。`grep -r` 等で設定・scripts に旧 path への参照が残っていないか確認する（移行後の `config.toml` には `[scratch.sccache]` 等の節が残っていても起動時は警告のみで無視される — ADR-0129 §1）。
2. 容量とパスの再確認: `du -sh "$HOME/.local/celeris/build-cache-nfs" "$HOME/.local/celeris/cache/sccache-l2"`。
3. 削除: `rm -rf "$HOME/.local/celeris/build-cache-nfs" "$HOME/.local/celeris/cache/sccache-l2"`（`verify` 段のログにもこの手順が「人に残された作業」として出る）。

## 7. home に残るもの（移さない）

ADR-0136 の分類どおり、次は home（NFS）のまま: `~/.config/celeris`（config.toml・token・password・secrets・credentiald の鍵）、`~/.local/celeris/claude-accounts`・`codex-accounts`（資格情報、mode/ACL 維持）、`~/.local/share/celeris/knowledge`（KB）、`~/.local/celeris/backups`（履歴。新規 backup は一度 `/local` に置いてから人が home へ複製する）、`~/workspace/agent-platform`（main checkout）。

## 8. 本番での実行について

この手順は**人が本番 host 上で実際のコマンドを実行する**ことを前提に書いている。実装エージェント（Celeris の run）は systemd 操作・本番 `~/.config/celeris` や `~/.local/celeris` の書き換え・本番 DB への書込みを行わない（ADR-0095 付記 D-d）。`--dry-run` による確認の実行結果をこの run の証跡として残すことはできるが、本実行・停止・起動・削除は人が行う。
