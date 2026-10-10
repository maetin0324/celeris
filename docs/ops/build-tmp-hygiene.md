---
tasks: [01M4B4J92KBR73EQA5S7FWB21G, 01M4HWZMWSTH3HYPCRRPE6PXB0]
---
# ビルド成果物と /tmp の衛生 — 運用手順（人が実行する）

- 対象: [ADR 2026-10-07-build-tmp-hygiene](../../agent-docs/adr/2026-10-07-build-tmp-hygiene.md)
  （共有 cargo target の定期掃除 D1、run ごとの TMPDIR D2、試験の後片付け D3、ディスク使用率の監視 D4）。
- 実行者: 人。本番 host の操作（`~/.config/celeris` の編集、daemon の再起動・昇格、cron の有効化、`/tmp` の掃除）は
  この手順どおり人が行う。このリポジトリの run からは実行していない。
- cron の一般手順は [cron-jobs.md](cron-jobs.md)、release の手順は [selfdeploy.md](selfdeploy.md)。

## 0. 前提

- この変更を含む release を `release.sh <ref>` → `verify.sh <sha12>` → `promote.sh <sha12>` で昇格済みであること
  （`GET /health` の `schema_version` が 58 以上。migration 0058 = `disk_watch_state`）。
- 以降の例の `CTL` は昇格した release の `celerisctl`:

```bash
export CTL="$HOME/.local/celeris/releases/<sha12>/bin/celerisctl"
export CELERIS_CONFIG="$HOME/.config/celeris/config.toml"
```

## 1. 掃除で消える量を確認する（dry-run）

`celerisctl target sweep` は既定が `--dry-run`（木も DB も変えない。LLM も呼ばない）。

```bash
$CTL target sweep --json | head -c 4000                 # 既定の roots
$CTL target sweep --root /var/tmp/agent-platform-build --root ~/.local/celeris/build-cache/cargo
```

注意: `celerisctl target sweep` は config を読まず、`--root` 省略時は既定の 2 root、規則は既定値（7 日・120 GiB）で計画する。
config で値や roots を変えた場合、dry-run は同じ root を `--root` で渡して近似する（cron の executor は daemon が読んだ config を使う）。

見るところ（JSON）: root ごとの `before_bytes` / `after_bytes` / `deleted_bytes` / `by_reason`（`age`・`cap`・`stale_target`）、
`skipped`（`build_in_progress` = cargo が `.cargo-lock` を持っている profile は 1 つも消さない）、
`over_cap_unresolved`（`true` なら build 中の profile のせいで上限の 80% まで下がらなかった）。
意図しない path が `deleted` に入っていないこと、`roots` の外へ出る path が無いことを確かめる。

## 2. config を足す

`~/.config/celeris/config.toml` に追記する（値は既定値。既定のままでよければ **何も書かなくても** 動く。変えたい項目だけ書く）:

```toml
[maintenance.target_sweep]
# roots を省略すると <[workspace].build_cache_dir>/cargo と /var/tmp/agent-platform-build の 2 つ。
# scratch pool の lease（release-build）の target は入れない。release.sh が前回 build の marker
# (.celeris-release-build-start) より古い test binary を刈り、共有を数えない大きさ（btrfs du の
# Exclusive + Set shared、非 btrfs は inode ごと st_blocks）が SD_RELEASE_TARGET_MAX_BYTES
# （既定 64 GiB）を越えたら target を作り直す。詳細は [local-disk-growth.md](local-disk-growth.md#3-release-build-target-の初回刈り込み)。
# roots = ["/home/<user>/.local/celeris/build-cache/cargo", "/var/tmp/agent-platform-build"]
max_age_days = 7                     # 最後に使ってからこの日数を過ぎた項目を消す
max_bytes_per_root = 128849018880    # 120 GiB。超えたら古い順に target_ratio まで下げる
target_ratio = 0.8
stale_target_days = 14               # どの profile も使われていない target dir ごと消す
scratch_targets = true              # scratch の終端 task（WU 含む）を root に追加
workspace_target_after_hours = 6    # workspace 内の cargo target の猶予（0 は無効）

# ディスク監視（省略すると下の 3 つ。止めるなら [maintenance] に disk_watch = []）
[[maintenance.disk_watch]]
path = "/"
warn_pct = 80        # 以上で通知（種類 disk）
critical_pct = 95    # 以上で受信箱（disk_full）

[[maintenance.disk_watch]]
path = "/local"
warn_pct = 80
critical_pct = 95

[[maintenance.disk_watch]]
path = "/tmp"
warn_pct = 80
critical_pct = 95
```

`0 < warn_pct < critical_pct <= 100`、path は絶対 path・重複不可（違反すると起動時に config エラー）。
config は起動時と reload でしか読まれないので、反映は `systemctl --user restart celeris@<sha12>`
（または admin の reload）。監視は反映した daemon の tick で 60 秒ごとに測り始める。

## 3. target 掃除の cron job を有効にする

`[[cron.seed]] target-sweep`（`enabled = false`）は **`cron_jobs` 表が空のときだけ** 入る。すでに job がある本番では
明示的に作る（cron-jobs.md §2 と同じ理由）。`action = "target_sweep"` の task は worker に渡らず、dispatcher の
決定的な executor が実行する。まず `dry_run` で 1 回回し、記録を見てから `apply` に替えるのが安全:

```bash
export PROJECT=<agent-platform 案件の ULID>
$CTL cron --config "$CELERIS_CONFIG" create \
  --name target-sweep --schedule "15 4 * * *" --timezone Asia/Tokyo \
  --overlap skip --catch-up latest --enabled false \
  --template '{"title":"共有 cargo target の掃除: {date}","action":"target_sweep","mode":"dry_run","project":"'"$PROJECT"'","objective":"共有 cargo target の定期掃除（ADR 2026-10-07-build-tmp-hygiene D1.4）。決定的な保守 executor が実行する（LLM なし）。"}'

$CTL cron --config "$CELERIS_CONFIG" run target-sweep        # 1 回だけ手動発火（dry_run）
# 記録（§4）を確かめてから apply に替えて有効化:
$CTL cron --config "$CELERIS_CONFIG" update target-sweep --template '{…"mode":"apply"…}'   # 上の template の mode だけ変える
$CTL cron --config "$CELERIS_CONFIG" resume target-sweep
```

止めるときは `$CTL cron --config "$CELERIS_CONFIG" pause target-sweep`。

## 4. 有効化後の確認

```bash
$CTL cron --config "$CELERIS_CONFIG" list                    # target-sweep が enabled、next_run が翌 04:15
$CTL cron --config "$CELERIS_CONFIG" history target-sweep    # 発火ごとの task id・結果
$CTL show <history の task id>                               # event を見る
du -sh /var/tmp/agent-platform-build ~/.local/celeris/build-cache/cargo   # 実測
```

- 記録は task の event **`target_sweep_ran`**: `mode`、root ごとの `before_bytes` / `after_bytes` / `deleted_bytes` /
  `deleted_items` / `by_reason`、`skipped`（先頭 50 件と総数）、`over_cap_unresolved`、`duration_ms`。
  task が `done` なら成功、走査・削除の失敗があれば `failed`。
- 通知フィードに `cron_run`（job の終端）が 1 件。`over_cap_unresolved = true` のときだけ別に `disk`
  （group_key `disk:target_sweep`）が出る。
- 掃除中に `build_in_progress` が続く profile は消えない。常に出るなら、その root で長い cargo が回り続けている。
- 前回の削除途中の `.deleting-*` は次の回が片付ける。
- 掃除の後、最初の build は消えた crate の分だけ遅い（想定内）。

ディスク監視の確認: 通知（web の通知一覧、種類「ディスク」）と受信箱（`disk_full`、行き先は `/daemon`）。
`disk_full` は host の操作が要る項目で、GUI からは答えられない（`409 native_action_required`）。使用率が
しきい値より 5 ポイント下がると解除される。

## 5. /tmp の既存残骸の一回限りの掃除

今後の run・試験は `/tmp` を使わなくなるが、**既にある残骸は自動では消えない**。まず中身を見て、使用中のものを避ける。

```bash
df -h /tmp /local /
du -xsh /tmp 2>/dev/null
ls -ldt /tmp/* /tmp/.[!.]* 2>/dev/null | head -30           # 新しい順
# 1 日以上前の候補（表示だけ）
find /tmp -mindepth 1 -maxdepth 1 -mtime +1 \
  \( -name 'celeris-browser-unit-*' -o -name 'celeris-browser-dispatch-unit-*' -o -name 'celeris-web-e2e-*' -o -name '.tmp*' \
     -o -name 'rw' -o -name 'ra[0-9]*' -o -name 'rm[0-9]*' \) -print | head -100
```

内容を確認して問題なければ消す（実行中の run・試験の dir を消さないよう `-mtime +1` を外さない。読み取り専用 dir は先に権限を戻す）:

```bash
find /tmp -mindepth 1 -maxdepth 1 -mtime +1 \
  \( -name 'celeris-browser-unit-*' -o -name 'celeris-browser-dispatch-unit-*' -o -name 'celeris-web-e2e-*' -o -name '.tmp*' \) \
  -exec chmod -R u+w {} + -exec rm -rf {} +
# worker が作ったリポジトリの写し（/tmp/rw・ra4・rm6 など）は名前が不定。ls で確認した path を個別に消す:
#   rm -rf /tmp/<確認した dir>
df -h /tmp
```

`systemd-tmpfiles` 等による `/tmp` 全体の時間基準の掃除は host 管理で、Celeris は持たない（ADR §4）。

## 6. 開発側で使うもの

- `bash scripts/dev/test-parallel.sh` は `TMPDIR` を自分の dir に閉じ込め、終了時（読み取り専用の残骸も含め）消す。
  残った entry 数は `tmp_leftovers`（0 でなければ警告。gate は落とさない）。
- 1 crate の試験が一時 dir を残さないか: `sh scripts/dev/check-test-tmp-leftovers.sh -p <crate>`（残れば表示して exit 1）。
- web e2e の一時 dir は `web/e2e/support/tmp-dir.ts` の `makeTmpDir` で作る（終了で消える）。
- run の TMPDIR は `<workspace>/runs/<run_id>/tmp/`（run 終了で消える。ログと `result.json` は残る）。remote の run は範囲外。

## 7. 元に戻す

- 掃除を止める: `cron pause target-sweep`。config の `[maintenance.target_sweep]` を消せば既定値に戻る。
- 監視を止める: `[maintenance]` に `disk_watch = []` を書いて再起動。
- 掃除で消えたものは cargo が次の build で作り直す（データの損失は無い）。

## 8. `/local` 容量の 3 経路（repo target・release-build・DB backup）

ADR [2026-10-10-local-disk-growth-paths](../../agent-docs/adr/2026-10-10-local-disk-growth-paths.md) で
塞いだ 3 経路の自動対策と、既存残骸の人による掃除は [local-disk-growth.md](local-disk-growth.md) が
手順書（本番 host で人が実行）。

- **repo 直下 target の GC**: daemon の常設 tick（600 秒間隔）が、木全体が終端（done/failed/cancelled）で
  最後の終端から `workspace_target_after_hours`（既定 6 時間）経過した repo 直下 `target/` を削除する。
  `running_run`・`active_descendant`・`grace` の保護理由がなく `.cargo-lock` を取得できるものだけが
  候補。log の `repo target gc: kept` / `repo target gc: removing the target of a finished task tree` /
  `repo target gc: removed` と task の `WorkspacePruned` event で確認。既存の残骸の削除は
  [local-disk-growth.md §2](local-disk-growth.md#2-既存-repo-直下-target例-01mf3v643)。
- **release-build target の刈り込み**: `release.sh` は build 前に marker `.celeris-release-build-start`
  より古い workspace crate の test binary と `.d` を削除し（依存の `.rlib`・build 出力は残す）、
  共有を数えない大きさ（`btrfs filesystem du` の Exclusive + Set shared、非 btrfs host は inode ごとに
  `st_blocks`）が `SD_RELEASE_TARGET_MAX_BYTES`（既定 64 GiB）を越えたら target を作り直す
  （`SD_RELEASE_TARGET_SEED` があればコピー）。初回・既存の target は
  [local-disk-growth.md §3](local-disk-growth.md#3-release-build-target-の初回刈り込み)。
- **DB backup の保持**: promote 前 backup は 10 本（`CELERIS_PROMOTE_BACKUP_KEEP`）、rollback 前は 3 本
  （`CELERIS_ROLLBACK_BACKUP_KEEP`）までで、`promote.sh` の成功末尾が `prune-backups.sh` で刈る。
  定期 backup は daemon の `backup_daily_keep`（7）・`backup_weekly_keep`（4）・
  `backup_max_total_bytes`（64 GiB）で刈る。どちらの削除も、消す候補がある型の最新 backup を
  read-only で `PRAGMA integrity_check` し `ok` でなければ 1 本も消さない。既存の残骸の削除は
  [local-disk-growth.md §4](local-disk-growth.md#4-既存-promoterollback-backup-の刈り込み)。

## 2026-10-09: workspace target 対策の運用

通常 worker・WorkUnit・子 task に加え、reviewer も対象 task の `CARGO_TARGET_DIR` を使う。scratch 有効時は `<scratch>/targets/task-<id>[/wu-<id>]/target`。既存の `shared_build_cache = false` と remote/container の除外は維持する。

配送後、人が §3 の cron を `dry_run` で手動発火する。CLI 単独の `target sweep` は DB を見ないため、scratch の task 状態判定と workspace 掃除を含まない。保守 task の `TargetSweepRan` で scratch root と workspace root、`build_in_progress`・`active_or_unknown_owner` を確認してから、§3 の `apply` と `resume` を行う。

workspace の対象は終端（done/cancelled/failed）から既定 6 時間経過した task の cargo target。非終端、running run のある task、Cargo lock 中、symlink は残す。ソース・成果物は保持する。scratch sweep も非終端・不明・外部 owner を残すため、上限まで回収できない場合がある。

warn の通知には大きい target 上位 5 件の path と実使用量を載せる。critical では新しい coding run とそのレビュー・検査を保留し、使用率が critical 閾値から 5 ポイント下がった測定で再開する。実行中 run は中断しない。`min_free_disk_mb` による従来の停止も有効。cron の掃除は critical 中も実行できる（ただし `min_free_disk_mb` による全体停止中は既存の制約どおり）。
