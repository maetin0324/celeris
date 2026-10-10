---
tasks: [01M4J36GBBT2BDJD24ZGMHJF68]
---
# `/local` 容量の回復手順

この手順は ADR [2026-10-10-local-disk-growth-paths](../../agent-docs/adr/2026-10-10-local-disk-growth-paths.md) の実装を含む release の配送後に、人が本番 host で実行する。daemon の切替・削除・設定変更は人が判断して行う。最初に [selfdeploy の手順](selfdeploy.md)で release、verify、promote を完了し、対象 release の健全性を確認する。

## 1. 配送後の daemon 差し替え

```sh
scripts/selfdeploy/release.sh <ref>
scripts/selfdeploy/verify.sh <sha12>
~/.local/celeris/releases/<sha12>/scripts/promote.sh <sha12>
systemctl --user status 'celeris@<sha12>.service'
curl -fsS -H "Authorization: Bearer $(cat ~/.config/celeris/api.token)" http://127.0.0.1:7710/api/v1/health
```

`verify.sh` が成功し、health の schema と release が意図したものか確認する。切替後は [handoff の確認手順](single-active-handoff.md)で active instance と draining 旧 instance を確認する。異常時は追加の prune を止め、通常の rollback 判断に移る。

## 2. 既存 repo 直下 `target/`（例: `01M4F3V643`）

repo target の自動回収は daemon の常設 tick（約10分間隔）で動き、`[maintenance.target_sweep] workspace_target_after_hours`（既定6時間）を使う。候補の保護理由は daemon log の `repo target gc: kept`、移動・回収は `repo target gc: removing...` / `repo target gc: removed` と task の `WorkspacePruned` event で確認する。`celerisctl target sweep --json` は `--root`（省略時は共有 build-cache root）を走査する別機能で、repo target GC の確認・削除には使わない。自動 GC の対象履歴を確認できない旧 target は、利用者が分からなければ手で消さない。手作業で判断する場合は、task の親子関係と終端状態、木の最後の終端から6時間以上経過、実行中 run が無いこと、各 profile の `.cargo-lock` が非 lock 中であることをすべて確認する。
repo target の自動回収は daemon の常設 tick（600 秒間隔）で動き、`[maintenance.target_sweep] workspace_target_after_hours`（既定6時間）を使う。`running_run`、`active_descendant`、`grace` の保護理由がなく、cargo lock を取得できるものだけが候補になる。木全体の最後の終端 task を基準にするため、子 task が親 checkout を使っている間は親の target も残る。候補の保護理由は daemon log の `repo target gc: kept`、移動・回収は `repo target gc: removing the target of a finished task tree` / `repo target gc: removed` と task の `WorkspacePruned` event で確認する。`celerisctl target sweep --json` は `--root`（省略時は共有 build-cache root）を走査する別機能で、repo target GC の確認・削除には使わない。自動 GC の対象履歴を確認できない旧 target は、利用者が分からなければ手で消さない。手作業で判断する場合は、task の親子関係と終端状態、木の最後の終端から6時間以上経過、実行中 run が無いこと、各 profile の `.cargo-lock` が非 lock 中であることをすべて確認する。

```sh
T="$HOME/.local/celeris/workspaces/<task-id>/repos/<repo>/target"
find "$T" -name .cargo-lock -type f -print
# lock を保持中の cargo がないことを個別に確認する。削除直前にも再確認する。
# 所有 task と子孫 task / run の状態、最終終端時刻を daemon API/UI で確認して記録する。
# 条件を確認できた後に限り、対象の絶対 path を再確認してから削除する。
printf 'target=%s\n' "$T"
readlink -f "$T"
# rm -rf -- "$T"  # 人が上記の確認を完了した対象だけに実行する
```

`target` が存在するだけでは削除根拠にならない。親 checkout の所有 task と参照する子孫を特定できない場合は、対象を残して運用担当へ調査を依頼する。

## 3. release-build target の初回刈り込み

新しい `release.sh` は `release-build` lease を得た後、test binary prune と上限確認をしてから gate を実行する。既定上限は `SD_RELEASE_TARGET_MAX_BYTES=68719476736`（64 GiB）。超過時は target を再作成し、存在する `SD_RELEASE_TARGET_SEED` から内容をコピーする。初回 release は同じ処理を実行するので、別途 target を直接消さない。

```sh
scripts/selfdeploy/release.sh <ref> 2>&1 | tee "$HOME/.local/celeris/logs/release-first-prune.log"
rg 'release prune:|scratch:' "$HOME/.local/celeris/logs/release-first-prune.log"
```

release が成功し gate が全段成功したこと、log の lease size / prune 結果を確認する。新しい marker `.celeris-release-build-start` より古い workspace crate test binary と対になる `.d` が削除候補で、依存 `.rlib`/`.rmeta` と build-script 出力は保持される。gate が失敗したら log を保存し、原因確認前に target を手で消さない。

## 4. 既存 promote/rollback backup の刈り込み

まず最新の対象 backup を read-only で検査する。ファイル名の時刻順は `YYYYMMDD-HHMMSS` 順。

```sh
B="$HOME/.local/celeris/backups"
ls -1t "$B"/*-pre-*.sqlite3
sqlite3 "file:$B/<最新の YYYYMMDD-HHMMSS-pre-<sha12>.sqlite3>?mode=ro" 'PRAGMA integrity_check;'
sqlite3 "file:$B/<最新の YYYYMMDD-HHMMSS-pre-rollback.sqlite3>?mode=ro" 'PRAGMA integrity_check;'
```

該当種類が存在する場合、各最新 backup の出力が厳密に `ok` であることを確認する。次に対象ファイルを一覧し、既定保持数（promote 10、rollback 3）を超える古い候補を目視する。

```sh
find "$B" -maxdepth 1 -type f \( -name '*-pre-????????????.sqlite3' -o -name '*-pre-rollback.sqlite3' \) -printf '%f\n' | sort -r
sh scripts/selfdeploy/prune-backups.sh "$B"
find "$B" -maxdepth 1 -type f \( -name '*-pre-????????????.sqlite3' -o -name '*-pre-rollback.sqlite3' \) -printf '%f\n' | sort -r
```

**未対応:** `prune-backups.sh` に dry-run option はない。上の一覧確認は削除予定の予行表示ではなく、実行前の対象確認である。人が候補名・保持数・最新 backup の integrity 結果を確認してから実行する。実行後に保持数と最新ファイルを再確認する。最新の検査が失敗、または候補が想定外なら script を実行しない。定期 backup の retention は daemon の `backup_daily_keep`（7）、`backup_weekly_keep`（4）、`backup_max_total_bytes`（64 GiB）で管理される。`backup_keep`（48）は時間単位の互換保持数。

## 5. dangling `.cargo-target` symlink

新しい `browser-ledger.sh` は lease を解決し、成功時は symlink を現 lease に更新する。scratch が使えず symlink が dangling の場合は unlink して fallback directory を使う。scratch が使えずリンクが生きている場合は stale lease の可能性があるため拒否する。

```sh
L="$HOME/.local/celeris/releases/.cargo-target"
ls -ld "$L"
readlink "$L"
test -e "$L" && echo 'target exists' || echo 'dangling or absent'
```

生きているリンクを手で付け替えたり、その参照先を削除しない。dangling link の修理は次回の `release.sh` または `browser-ledger.sh <sha12>` で lease の解決後に行わせる。事前にリンクが存在していた場合は log の `updated cargo target link to current lease`、dangling で scratch lease が使えない場合は `removed dangling cargo target link` を確認する。scratch が利用できず安全な fallback がない場合は ledger/release が失敗するので、運用担当が scratch 状態を調査する。

## 6. 効果の確認

同じ filesystem の前後差を記録する。`df` と `stat -f`（statvfs）は mount 全体の free blocks/bytes を確認する。lease 個別量は scratch lease manifest の `size_bytes`、または daemon target-sweep report の `before_bytes` / `after_bytes` / `deleted_bytes` を使う。これらは実装が記録する値で、手順書に存在しない CLI オプションを追加しない。

```sh
df -B1 /local
stat -f -c 'path=%n blocks=%b free=%f available=%a block_size=%S' /local
# scratch lease の owner と size_bytes を確認（JSON 出力）
~/.local/celeris/current/bin/celerisctl scratch status --config ~/.config/celeris/config.toml --json
```

削除直後と次の定期観測で値を比較し、mount 全体の変化と対象 lease の帰属量を分けて記録する。**`du` は使わない**。reflink や圧縮を使う filesystem では共有・圧縮分を重複して数え、実際に解放された容量を表さない。
