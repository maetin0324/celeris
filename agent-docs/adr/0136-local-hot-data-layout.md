# ADR-0136: サービスの hot データの正本を /local に置く

---
tasks: [01M3Z08A0T81ZQ60XVR62XJMPD]
---

- 日付: 2026-10-02
- 状態: Accepted（人の 2026-10-02 19:20 UTC の方針。実装と移行は後続工程）
- 関連: [ADR-0129](0129-host-sccache-reflink-targets.md)、home の NFS 移行手順（`docs/ops/home-nfs-migration-2026-09-25.md`、ADR-0128 の整理で削除済み）

## 決定と棚卸し

`/local` はサービスの hot データの**正本**であり、消してよい cache 用の置き場ではない。Proxmox の LVM-thin 上の 300G btrfs（`compress=zstd:1`）を container に bind mount したものを使う。設定と account 情報は `/home/rmaeda` の NFS に残す。main checkout、KB、backups も home に残す。以下の容量は人が 2026-10-02 19:00 UTC に測った概数であり、同じファイルを含む親子の数字は足さない。

| 現在の場所 | 容量 | 分類 | 移行後の正本・処置 |
| --- | ---: | --- | --- |
| `~/.local/celeris/releases`（`current`・`previous`、`.build`、`.pnpm-prod-cache`、`.cargo-target` を含む） | 2.0G | hot | `/local/celeris/state/releases` と同階層の `current`・`previous`。release の `bin`・`gui`・scripts、GUI の依存、build worktree、pnpm cache は同じ btrfs に置く。停止中の web 配布物 `web/app` はコピー・再構築対象から外す |
| `~/.local/celeris/tools` | 5.2G | hot | `/local/celeris/state/tools`。venv・実行ファイルを含み、設定中の絶対 path も合わせる |
| `~/.local/celeris/staging` | 80M | hot | `/local/celeris/state/staging` |
| `~/.local/celeris/logs` | 109K | hot | `/local/celeris/state/logs`。selfdeploy の一時 log も hot。監査が必要なものは別途保存する |
| `~/.local/celeris/backups` | 2.5G | cold | home に履歴を残す。サービスが作る新規 backup は一度 `/local/celeris/state/backups` に置き、人の別手順で home へ保管する |
| `~/.local/celeris/claude-accounts` | 73K | config | home に残す。資格情報の mode と ACL を維持し、`[accounts].claude_dir` は現行 path |
| `~/.local/celeris/codex-accounts` | 421M | config | home に残す。資格情報の mode と ACL を維持し、`[accounts].codex_dir` は現行 path |
| `~/.local/celeris/credentiald` | 未測定 | hot | vault・audit は `/local/celeris/state/credentiald`（0700）。鍵は `~/.config/celeris/credentiald/keys` に残す。鍵と vault の両方が必要なので両方のバックアップを考慮する |
| `~/.local/celeris/build-cache-nfs` | 7.7G | 削除 | コピーしない。参照がないことを確認後、人が削除 |
| `~/.local/celeris/cache/sccache-l2` | 92G | 削除 | コピーしない。ADR-0129 の host sccache 切替後、旧 unit 停止と参照なしを確認して人が削除 |
| `~/.local/share/celeris/knowledge` | 24M | cold | home に残す。`[knowledge].root` は現行 path。KB の `_inbox` 書き込みだけは残る |
| `~/.config/celeris`（config・token・password・secrets・credentiald keys） | 未測定 | config | home に残す。unit の設定ファイル参照も home |
| `~/workspace/agent-platform`（main checkout） | 未測定 | cold | home に残す。release の一時 worktree は `/local` |
| `/var/lib/celeris/celeris.sqlite3` と `-wal`・`-shm` | 242M + WAL 35M | hot | `/local/celeris/data/db/celeris.sqlite3` と同じディレクトリの WAL・SHM |
| `/var/lib/celeris/workspaces` | 126G | hot | `/local/celeris/data/workspaces`。終端 task の GC 後、残す worktree・run・成果物を移す |
| `/var/lib/celeris/scratch` | 172G | hot | `/local/celeris/data/scratch`。lease・target を残す必要があるものだけ移す。seed・reflink・sccache の設計は ADR-0129 が正 |
| `/var/lib/celeris/build-cache` | 実測なし | hot | `/local/celeris/data/build-cache`。既存の再生成可能物は GC 可能 |
| `/var/lib/celeris/web` | 409M | hot | `/local/celeris/data/web`。サービス側の永続データ。リリース中の `web/app` 配布物とは別 |
| `/var/lib/celeris/memory` | 664K | hot | `/local/celeris/data/memory` |
| `/var/lib/celeris/release-build` | 692M | hot | `/local/celeris/data/release-build`。scratch lease・target の扱いは ADR-0129 に従う |
| `/var/lib/celeris` の上記以外の実在項目 | 移行前に測定 | hot | 原則 `/local/celeris/data/<同名>`。参照元を棚卸ししてから切り替える |

現在値の単純合計は `/var/lib/celeris` の既知項目で約 299.4G、home から移す既知 hot 項目で約 7.3G、合計約 306.7G である。300G にそのまま全コピーできない。btrfs の metadata、移行中の差分、WAL、reflink 共有 extent も余裕が要る。移行台本は **移す集合を確定した時点の `/local` の物理空きで判定**し、切替直前に 30GiB 以上の空きを残す。目安としてコピー対象を 240G 以下まで減らす（現在の論理使用量から少なくとも約 67G の削減が必要）。`du` の合計は reflink の共有 extent を二重に数えるため最終判定に使わない。LVM-thin pool の空きも別に確認する。

## path の契約（既定は現状互換）

移行後の設定値は次のとおり。既存の TOML key は改名せず、未設定時の既定は現行値のままにする。`/local` が mount されていない host で既定だけが突然失敗することを避けるためである。path は絶対 path を指定し、異なる tree に移す項目を一つの `CELERIS_STATE_DIR` へ無理に束ねない。

| 利用者 | key / 環境変数 | 移行後 | 未設定時 |
| --- | --- | --- | --- |
| daemon・CLI | `[db].path`（旧 `db =` も読める） | `/local/celeris/data/db/celeris.sqlite3` | `~/.local/celeris/celeris.sqlite3` |
| daemon | `workspace_root` | `/local/celeris/data/workspaces` | `~/.local/celeris/workspaces` |
| daemon | `[workspace].build_cache_dir` | `/local/celeris/data/build-cache` | `~/.local/celeris/build-cache` |
| daemon | `[scratch].dir` | `/local/celeris/data/scratch` | `[workspace].build_cache_dir` の親の `scratch` |
| daemon | `[containers].build_dir` | `/local/celeris/data/containers` | `~/.local/celeris/containers` |
| daemon | `[memory].dir` | `/local/celeris/data/memory` | 未設定なら memory 無効、節内の `dir` 省略なら `~/.local/celeris/memory` |
| daemon | `[selfdeploy].releases_dir` | `/local/celeris/state/releases` | `~/.local/celeris/releases` |
| daemon | `[db].backup_dir` | `/local/celeris/state/backups`（新規 backup の staging） | 未設定なら定期 backup 無効 |
| daemon | `[knowledge].root`、`[accounts].claude_dir`・`codex_dir` | それぞれ現行の home path | それぞれ既存の規則を維持 |
| selfdeploy 全台本 | `CELERIS_STATE_DIR` | `/local/celeris/state` | `$HOME/.local/celeris` |
| selfdeploy 全台本 | `CELERIS_BACKUPS_DIR`（新設、`SD_BACKUPS` の入力） | `/local/celeris/state/backups` | `$CELERIS_STATE_DIR/backups` |
| selfdeploy 全台本 | `CELERIS_LOGS_DIR`（新設、`SD_LOGS` の入力） | `/local/celeris/state/logs` | `$SD_BACKUPS`（旧台本の log 置き場と互換） |
| credentiald `init`・`serve` | `CELERIS_CREDENTIALD_DATA_DIR`（新設） | `/local/celeris/state/credentiald` | `$HOME/.local/celeris/credentiald` |
| unit と install script | `CELERIS_STATE_DIR` を含む `~/.config/celeris/paths.env`（新設、絶対 path のみ） | `/local/celeris/state` | file 不在なら従来の `%h/.local/celeris` |

`[selfdeploy].releases_dir` と `$CELERIS_STATE_DIR/releases` は同じ path でなければ起動・release を拒否する。`CELERIS_BACKUPS_DIR` は `lib.sh` の `SD_BACKUPS`、`CELERIS_LOGS_DIR` は `SD_LOGS` を上書きする。現行の promote/rollback/relocate-db 台本が `SD_BACKUPS` に置く log は `SD_LOGS` に分離する。`SD_CURRENT`・`SD_PREVIOUS` は `$CELERIS_STATE_DIR` 直下、`.build/tree`・`.pnpm-prod-cache`・`.cargo-target` は `$SD_RELEASES` 直下を保つ。`.cargo-target` は legacy fallback であり、主経路の release target は ADR-0129 の scratch lease。`SD_REPO` は main checkout の home path のまま。`docs_maintenance.rs` の state root も同じ `CELERIS_STATE_DIR` を受け取り、未設定時は従来値とする。`web-follow.sh` の home 固定 fallback も同じ path 契約へ寄せる。home の backup archive への転送は daemon と release の成功判定から切り離し、人が別途実行・検証する。未転送の新規 backup が残れば移行完了扱いにしない。

`paths.env` は秘密を入れず、unit ごとに `EnvironmentFile=-%h/.config/celeris/paths.env` を読む。systemd の `WorkingDirectory=` と `ExecStart=` は `EnvironmentFile` の変数展開を一律には使えないため、`install-units.sh` が `CELERIS_STATE_DIR` の値を読んで unit テンプレートの絶対 path を生成・設置する。未設定時は既存の home path を生成する。`celeris@`、`celeris-gui@`、`celeris-credentiald@`、`celeris-web@` の release 参照と作業ディレクトリを同じ値で生成する。`celeris-sccache`・`celeris-scratch-cache` は ADR-0129 で撤去するので移行後は生成・起動しない。`celeris-browser-launcher` は別の system unit・別 UID の `/var/lib/celeris-browser` であり対象外。`celeris-web@` は既存の停止状態を維持し、web 配布物を移行の成功条件に入れない。

worker の `db_guard` は設定済み DB の実体 path を保護し続け、`[selfdeploy].releases_dir` の**実体 path**も読み取り専用にする。home の固定 `~/.local/celeris/releases` だけを守る現行実装では `/local` の新しい releases を守れない。設定済み path の canonicalize と存在確認を行い、失敗時は保護なしで run を起こさない。host の `~/.config/celeris` と `~/.config/systemd` の保護は残し、worker の本番操作禁止文言にも `/local` 側の値を反映する。DB の親ディレクトリに他の可変データを同居させない。

## btrfs 上の SQLite

DB と WAL・SHM は同じ `/local/celeris/data/db` に置く。**コピー前の空ディレクトリ**に `chattr +C /local/celeris/data/db` を人が適用し、`lsattr -d` で `C` を確認してから DB を作る。既存ファイルに後から `+C` を付けても既存 extent が非 CoW へ変わるわけではない。`+C` は新規ファイルへ継承されるため、起動後に作り直される `-wal`・`-shm` にも効く。`compress=zstd:1` mount 上でもこのディレクトリの DB family は圧縮対象から外す。DB family を reflink・snapshot の共有 extent にしてから更新すると CoW が再発し得るので、コピー時は通常の実体コピーとし、DB は seed/reflink 対象にしない。DB ディレクトリだけを NOCOW にし、workspaces・scratch の btrfs reflink は維持する。

SQLite は現行コードどおり WAL mode、定期 `PRAGMA wal_checkpoint(PASSIVE)`（既定 30 秒）を使う。WAL mode は DB と同じ host 上の `-wal`・`-shm` を要し、NFS 上での利用を避ける理由になる。移行の停止後、旧 DB で `PRAGMA wal_checkpoint(TRUNCATE)` と `PRAGMA integrity_check` を確認する。コピー・検証では DB 単体の生コピーをしない。SQLite backup API / `VACUUM INTO` を使うか、全接続停止・checkpoint 完了後に DB family を扱う。新 DB の `integrity_check` が `ok` と出るまで切替えない。`PASSIVE` は reader がいると WAL を全部縮められないので、停止時の検査を省かない。新規 backup は `[db].backup_dir` の `/local` に作り、人が検証済みの世代を home の archive に複製する。DB 正本の btrfs snapshot だけに依存しない。

根拠: [chattr(1) の `C` 属性](https://man7.org/linux/man-pages/man1/chattr.1.html)（空ファイル・空ディレクトリに先に設定）、[Btrfs の NOCOW と compression](https://btrfs.readthedocs.io/en/latest/btrfs-man5.html)、[SQLite WAL](https://www.sqlite.org/wal.html)（共有メモリと checkpoint）、[SQLite Online Backup API](https://www.sqlite.org/backup.html)。このホストでの性能・故障耐性は移行後の実測と backup の復元試験で確認する。

## 人が実行する移行の順序

後続の `scripts/selfdeploy/migrate-to-local.sh` は `--dry-run` を持ち、presync → gc → stop → delta → switch → start → verify → rollback の各段をログに残す。実際の systemd 操作、config 編集、DB 書込み、旧データ削除は人が実行する。台本は dry-run では read-only の確認とコピー計画のみを行う。

1. **presync**: `findmnt -M /local` で `/local` 自身が mount point であること、source・fstype=btrfs・mount ID・書込み可・uid 1001 を確認する。rootfs 上に同名ディレクトリがあるだけなら中止する。LVM-thin の空きと `btrfs filesystem usage /local` も人が確認する。新しい空 DB ディレクトリへ先に `chattr +C`。hot の初回コピーは稼働中なので DB の生コピーを避け、旧データは保持する。`releases/*/web/app` と不要な cache は除外する。
2. **gc**: running/reviewing と lease を列挙し、終端 task の workspace・target だけを `celerisctl workspace prune --dry-run` と scratch の GC 計画に沿って人が整理する。terminal の未レビュー差分・成果物は保護し、対象 ID を記録してから消す。ADR-0129 の seed/reflink と sccache の所有・GC を重複実装しない。GC 後にコピー対象・`/local` 物理空き・LVM-thin 空きを再測定し、切替後 30GiB の余裕がなければ止める。
3. **stop**: in-flight が 0 と確認してから、人が daemon・GUI・credentiald と release/verify の処理を停止する。旧 sccache unit は ADR-0129 の切替手順に従う。DB checkpoint を取り、旧 DB を検査する。
4. **delta**: 停止中に差分を再コピーし、DB は上記の一貫した方法で作る。symlink の `current`・`previous` は target が新 tree に存在することを検証する。旧 source は消さない。
5. **switch**: 人が `~/.config/celeris/config.toml` の key と `paths.env` を同時に更新し、新 unit を設置・reload する。`/local` がない場合は unit の起動前検査で失敗させ、rootfs に自動作成しない。新旧の DB/設定を混ぜて daemon を並行起動しない。
6. **start / verify**: 人が新 instance を起動し、`GET /api/v1/health`、認証付き config の DB path、release SHA、workspace と scratch の新規作成先、credentiald、DB `integrity_check`、backup の新世代、journal、空き容量を確認する。GUI の依存 symlink が `/local` 内で解決されることも確認する。web は起動判定に含めない。
7. **rollback**: 新 instance を止め、旧 config/unit と旧 source を戻して旧 instance を起動する。ただし新 DB に切替後の書込みが発生した場合、旧 DB をそのまま再開すると書込みが失われる。人が新 DB の一貫した backup を旧側に復元するか、書込みを失うことを明示的に選ぶまで自動復旧しない。旧 source と config の控えは検証完了後も保持し、削除は別の人の作業とする。

旧 `build-cache-nfs` と `cache/sccache-l2` の削除は参照元を止めた後、人が path・サイズ・mount を再確認して実行する。`relocate-db.sh` は DB だけを動かす旧台本なので、この一括移行の起点には使わない。旧 home-NFS 手順の「releases もローカルへ寄せると速い」「DB backup は home」の記述は、この ADR の配置で具体化する。
