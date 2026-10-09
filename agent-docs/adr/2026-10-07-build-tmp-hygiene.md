# ADR 2026-10-07: ビルド成果物と /tmp の衛生規則（共有 cargo target の定期掃除・run の TMPDIR・試験の後片付け・ディスク監視）

- 日付: 2026-10-07
- 状態: 実装済み（2026-10-07。task 01M4B4J92KBR73EQA5S7FWB21G の葉 target-sweep / run-tmpdir / rust-test-tmp / web-e2e-tmp / tmp-fix / disk-watch。実装との突き合わせは末尾の付記。有効化は人の操作で、`docs/ops/build-tmp-hygiene.md`）
- 関連: ADR-0066 D1（worktree 間の cargo キャッシュ共有）、ADR-0074 F5（disk guard `min_free_disk_mb`・WU ごとの target）、
  ADR-0075（scratch pool・lease・`scratch_gc`）、ADR-0131（cron job）、ADR-0133（受信箱と通知）、ADR-0136（/local の配置）、
  ADR-0125（時間依存試験の決定化）、ADR-0001 D2 原則 1（dispatcher・store に LLM を入れない）

## 1. 文脈

2026-10-07、ホストのディスクがひっ迫した。運用セッションの調査:

- `scripts/dev/worktree-target-dir.sh` が `.cargo/config.toml` に書く共有 target
  （`/var/tmp/agent-platform-build/agent-platform`、`…/agent-platform-main`）が 205G（`debug/deps` 114G、`incremental` 43G）。
  古い commit の試験 binary が掃除されずに溜まっていた。ほかに使われなくなった target（`review-*`、`launcher-test-target`、
  `dl-target`）が約 10G。全部消して `/` は 74% → 30%。
- `/tmp`（tmpfs = メモリ）に 21G・約 3 万 4 千件。worker が作ったリポジトリの写し（`/tmp/rw`・`ra4`・`rm6` など
  1 つ 300〜560MB、計 9.3G）、試験の一時 dir（`celeris-browser-unit-*`、`celeris-web-e2e-*`、`.tmp*`）。1 日以上前のものを消して 8G。

### 1.1 今のコード（読んだもの）

| 場所 | 今の振る舞い | 足りないもの |
|---|---|---|
| `scripts/dev/worktree-target-dir.sh` | `CELERIS_BUILD_CACHE`（既定 `/var/tmp/agent-platform-build`）の下に `<worktree 名>`（本体は `<repo>-main`）の target-dir を固定 | 中身の掃除が無い。worktree が消えても target が残る |
| `crates/task-worker/src/build_cache.rs` | worker の `CARGO_TARGET_DIR=<build_cache_dir>/cargo/<repo-key>`、並列 WU は `…/<repo-key>/wu-<work_unit_id>` | `<repo-key>` 直下の共有 target の中身（deps・incremental）は誰も消さない |
| `scripts/selfdeploy/release.sh` | target は scratch pool の owner `release-build` の lease（TTL `SD_RELEASE_TARGET_TTL` 既定 48h、終了時 touch）。release 全体を `.lock-release` で保護 | リリースが 48h 以内に続く限り lease は生き続け、target の中身は増え続ける |
| `crates/task-dispatch/src/scratch_gc.rs` / `task_worker::scratch` | lease と DB の状態で owner の target を**丸ごと** `.deleting-*` へ rename → 別スレッドで削除。`min_free_disk_mb` を割ると緊急モード | target の**中の**古い項目は対象外（丸ごと消すか残すかだけ） |
| `crates/celeris/src/config/cron.rs`（ADR-0131） | `[[cron.seed]]` を空の `cron_jobs` に一度入れる。種の既定 `enabled = false`。発火は task を作る | 決定的な保守処理を LLM の worker run なしで回す口が無い |
| ADR-0074 disk guard | `min_free_disk_mb` を割ると新しい run を止める | 人に知らせない。`/tmp`・`/` は見ない |
| ADR-0133 | 受信箱 14 種・通知 9 種。種類の追加は ADR で | ディスクの種類が無い |
| `scripts/dev/test-parallel.sh` | `mktemp -d "${TMPDIR:-/tmp}/celeris-test-parallel.XXXXXX"` を `trap 'rm -rf' EXIT` で消している | nextest が子に渡す TMPDIR は `/tmp` のまま。試験が残した物は消えない |
| `crates/task-worker/src/browser_tests.rs:85` | `temp_dir()/celeris-browser-unit-<pid>` を作り、消さない | 終了時削除 |
| `web/e2e/**/*.spec.ts` | `mkdtempSync(path.join(tmpdir(), "celeris-web-e2e-…"))` を各 spec が直接呼ぶ。消さない | 共通 helper と終了時削除 |

## 2. 決定

### D1. 共有 cargo target の掃除規則

#### D1.1 対象（root の列挙）

- config `[maintenance.target_sweep]` の `roots`（path の配列）が対象。**既定は 2 つ**:
  `<dispatch.build_cache_dir>/cargo`（worker の共有 target。ADR-0066）と `/var/tmp/agent-platform-build`
  （`worktree-target-dir.sh` の `CELERIS_BUILD_CACHE` の既定）。
- **target dir** = root の直下（深さ 1）または `<root>/<repo-key>/wu-*`（深さ 2）で、中に profile dir を 1 つ以上持つ dir。
  **profile dir** = `<target>/<profile>` または `<target>/<triple>/<profile>` で、`.cargo-lock` を持つ dir（`debug`・`release`・
  `<triple>/debug` など。名前を決め打ちしない）。
- **掃除の単位（項目）** は profile dir の中の次の 4 種だけ。`<target>/doc`・`<target>/nextest`・`<target>/tmp`・profile 直下の
  最終 binary と `examples/` には触れない（小さく、消すと人の作業が壊れやすい）。

  | 種類 | 項目 | 項目の key |
  |---|---|---|
  | `deps` | `deps/<name>-<hash>*`（`.rlib`・`.rmeta`・`.d`・試験 binary 等）を `<name>-<hash>` ごとに束ねたもの | `<name>-<hash>` |
  | `fingerprint` | `.fingerprint/<name>-<hash>/` | 同上 |
  | `build` | `build/<name>-<hash>/` | 同上 |
  | `incremental` | `incremental/<name>-<hash>/` | 同上 |

  `deps`・`fingerprint`・`build` は同じ key のものを**一緒に消す**（片方だけ残すと cargo が作り直すだけで壊れはしないが、
  量の記録が読みにくくなるため）。
- **使用時刻** = 項目に属する file・dir の `max(mtime, atime)`（relatime でも 24h 粒度で atime が進む。cargo は使った
  fingerprint を書き直すので mtime でも十分近い）。
- **scratch pool の target は root にしない**。`release-build` など lease を持つ owner の target は ADR-0075 の
  `scratch_gc` が丸ごと扱う。`release-build` の中身は **lease を持つ本人が掃除する**: `release.sh` が `.lock-release` を
  持ったまま、梱包の後に `celerisctl target sweep --apply --root "$SD_CARGO_TARGET"` を同じ規則で 1 回呼ぶ（失敗は警告だけ）。
  lease が生きている `release-build` を daemon・cron 側の掃除が触ることはない。

#### D1.2 規則（既定値つき）

1. **古さ**: 使用時刻が `max_age_days`（**既定 7 日**）より古い項目を消す。
2. **上限**: 古さの段の後も root の合計（`st_blocks × 512` の実使用量）が `max_bytes_per_root`（**既定 120 GiB**）を超えたら、
   残りの項目を使用時刻の古い順（同時刻は path の辞書順）に、合計が上限の `target_ratio`（**既定 0.8 = 96 GiB**）以下に
   なるまで消す。
3. **放置された target dir 全体**: target dir のどの profile も lock が取れ、全項目の使用時刻が `stale_target_days`
   （**既定 14 日**）より古いなら、target dir ごと消す（`review-*`・`launcher-test-target` のような残骸）。
   `wu-*` は既存の WU target の掃除（ADR-0074 F5-fix）が先に扱い、こちらは取りこぼしの保険。
4. **build 中は消さない**: profile dir ごとに `<profile>/.cargo-lock` を `flock(LOCK_EX | LOCK_NB)` で取る。
   取れない profile dir は**項目を 1 つも消さず** `skip: build_in_progress` と記録する（cargo は build 中この lock を持つ）。
   取れたら、lock を持ったまま消す項目を `<profile>/.deleting-<ulid>/` へ rename し、lock を放してから別スレッドで
   `remove_dir_all`（`scratch_gc` と同じ形。lock を持つ時間は rename の間だけ。待たされた cargo は
   "Blocking waiting for file lock" で待つだけ）。前回の `.deleting-*` が残っていれば次の回で消す。
5. **上限を守れないとき**: lock の取れない profile の分で上限の 80% まで下がらなければ `over_cap_unresolved` を記録し、
   D4 の通知（`disk:target_sweep`）を出す。消さないことを優先する。
6. **symlink は辿らない**。root の外へ出る path は計画に入れない（入力の検証で落とす）。

#### D1.3 構造（純粋な計画関数と I/O 層）

- 純粋: `task_worker::target_sweep::plan(snapshot: &[TargetSnapshot], locked: &BTreeSet<PathBuf>, now: SystemTime, params: &SweepParams) -> SweepPlan`。
  `TargetSnapshot` は走査済みの項目（path・種類・key・bytes・使用時刻）、`locked` は lock が取れなかった profile dir。
  `SweepPlan` は `delete: Vec<PlannedDelete{path, bytes, reason: age|cap|stale_target}>`、
  `skip: Vec<Skip{path, reason: build_in_progress|outside_root|symlink}>`、root ごとの `before_bytes / after_bytes`、
  `over_cap_unresolved`。時計と大きさは引数で与えるので試験は決定的。
- I/O: `task_dispatch::target_sweep`（走査・`flock`・rename・削除スレッド）。`celerisctl` と daemon の両方がこれを使う。
- 既定値は `SweepParams::default()` に置き、config で上書き: `max_age_days = 7`、`max_bytes_per_root = 120 GiB`、
  `target_ratio = 0.8`、`stale_target_days = 14`、`roots = [<build_cache_dir>/cargo, /var/tmp/agent-platform-build]`。

#### D1.4 実行（cron job と手動）

- **cron**: ADR-0131 の cron job。雛形の `extra.action = "target_sweep"`（新しい欄。値は `target_sweep` / `tmp_sweep` の予約語だけ）と
  `extra.mode = dry_run | apply`。`action` を持つ雛形から発火した task は worker（LLM）に渡さず、dispatcher の
  **決定的な保守 executor**（LLM なし）が D1.3 の I/O 層を呼び、結果を task の event に書いて `done` にする
  （失敗は `failed`）。ADR-0131 D4 の「発火は決定的」・D2 の `overlap = "skip"` をそのまま使う。
  `config/celeris.example.toml` に `[[cron.seed]] name = "target-sweep"`（`enabled = false`、`schedule = "15 4 * * *"`、
  `mode = "apply"`、`overlap = "skip"`、`catch_up = "latest"`）を置く。**有効化は運用（人）が `resume` で行う**。
- **手動**: `celerisctl target sweep [--root <path>]... [--dry-run | --apply] [--json]`。既定は `--dry-run`（計画だけ表示）。
  DB に書かない（stdout に D1.5 と同じ JSON を出す）。`release.sh` の呼び出しもこれ。

#### D1.5 記録

- 新しい event `TargetSweepRan`（cron 由来の task に追記。追記専用）:
  `{mode, roots: [{root, before_bytes, after_bytes, deleted_bytes, deleted_items, by_reason: {age, cap, stale_target}}],
  skipped: [{path, reason}]（先頭 50 件と総数）, over_cap_unresolved: bool, duration_ms}`。
- 通知は ADR-0133 の既存 `cron_run`（cron 由来 task の終端）で 1 件に束ねる。`over_cap_unresolved` のときだけ D4 の
  `disk` 通知も出す。

### D2. worker run の TMPDIR

1. **場所**: ローカルの run は `<workspace_root>/<task_id>/runs/<run_id>/tmp/`（run の記録 dir の中。`/local` 上）を
   run の起動前に `0700` で作り、worker の環境に `TMPDIR`・`TMP`・`TEMP` をその path で渡す。sandbox（bwrap 等）がある
   adapter ではこの dir を書き込み可で bind する。remote の run は今回の範囲外（同じ規則を remote 側の作業場所に
   適用するのは後の ADR）。
2. **片付け**: run の終了（成功・失敗・中断・timeout・cancel・daemon の takeover による打ち切り）のすべてで、
   run の終端処理が `tmp/` を `remove_dir_all` する（`runs/<run_id>/` の他の file＝ログ・`result.json` は残す）。
   削除失敗は警告 event `RunTmpCleanupFailed{run_id, error}` を残し、次の `scratch_gc` の tick が
   `runs/*/tmp` のうち終端 run のものを拾い直す（daemon 停止中に死んだ run の取りこぼし対策）。
3. **前置き**: worker の前置き（run context）に固定文を足す: 「リポジトリの写し・pnpm store の写し・ビルド出力・
   大きな一時 file は `/tmp` に置かず、`$TMPDIR`（run 終了で消える）か作業場所の下に置く。残したい物は
   `artifacts/` に置く」。
4. **環境変数の上書き禁止**: harness が `TMPDIR` を自分で `/tmp` に戻さないことを試験で確かめる（claude-code・codex・
   acp・pi の起動引数・環境の組み立て）。

### D3. 試験の後片付け

1. **Rust**: 一時 dir は `tempfile::TempDir`（drop で削除）か、それと同じ drop で消す guard で作る。`std::env::temp_dir().join(…)`
   を `create_dir_all` して放置する書き方をやめる。`crates/task-worker/src/browser_tests.rs` の
   `celeris-browser-unit-<pid>` は試験ごとの `TempDir` を `record_dir` に渡し、試験の終わりまで保持する。
2. **web e2e**: `web/e2e/support/tmp.ts`（共通 helper）に `makeE2eTmpDir(label)` を置き、`celeris-web-e2e-<label>-` の
   `mkdtemp` と、Playwright の `test.afterAll`（worker fixture）での `rm -rf` を 1 か所にする。各 spec の
   `mkdtempSync(path.join(tmpdir(), "celeris-web-e2e-…"))` はこれに置き換える。
3. **test-parallel.sh**: 自分の一時 dir（今の `logdir`）を親として `TMPDIR="$logdir/tmp"` を nextest と doc-test に渡し、
   既存の `trap 'rm -rf "$logdir"' EXIT` で試験が残した物ごと消す。消す前に「残っていた entry 数」を
   `CELERIS_TEST_SUMMARY` の `tmp_leftovers` に出す（0 でなければ警告。gate は落とさない）。
4. **証跡を残す場所**: 残す必要がある証跡（スクリーンショット・失敗時のログ）は `CELERIS_TEST_ARTIFACTS_DIR`
   （未設定なら `<CARGO_TARGET_DIR>/test-artifacts/`、web は `web/test-results/`＝Playwright の既定）に明示して書く。
   `/tmp` に「後で見るために」残さない。
5. **確かめ方**: 試験の中で `TMPDIR` を試験専用の空 dir に向け、試験の終わりにその dir が空であることを確かめる
   （`/tmp` 全体を数えない。並走する他の試験と干渉しないため）。

### D4. ディスク使用率の監視

1. **対象と既定**: config `[[maintenance.disk_watch]]`（path としきい値の組）。既定は `/`・`/local`・`/tmp` の 3 つで、
   `warn_pct = 80`、`critical_pct = 95`。
2. **測り方**: dispatcher の tick の段 `disk_watch` が 60 秒ごとに `statvfs`（注入可能な trait `DiskProbe`）で
   使用率 `(blocks − bfree) / (blocks − bfree + bavail)` を測る。LLM なし。存在しない path は `unavailable` として 1 回だけ記録。
3. **出し先**: `warn` 以上で通知（ADR-0133 の `NoticeKind` に `disk` を足す。group_key `disk:<path>`）。
   `critical` 以上で受信箱（`InboxKind` に `disk_full` を足す。id `disk_full-<path の slug>`）。D1.5 の
   `over_cap_unresolved` は `disk:target_sweep` の通知。ADR-0133 D1.5 の「1 出来事 → ちょうど 1 経路」を守り、
   `critical` への遷移は受信箱だけに出す（通知は出さない）。
4. **重複抑止**: 状態（path ごとの `level: ok|warn|critical`・`since`・`last_pct`・`last_notified_at`）を SQLite の小さな表
   `disk_watch_state` に持つ（daemon のメモリに持たない）。通知・受信箱は**level が上がったとき**だけ作る。
   同じ level が続くあいだは 24 時間ごとに 1 回だけ再通知（束 `count` が増える）。level を下げるのは
   しきい値より 5 ポイント下回ったとき（ヒステリシス。80% 前後の揺れで通知が往復しない）。
   受信箱の `disk_full` は level が `critical` 未満に下がった時点で項目が消える（ADR-0133 D4 の自動で閉じる規則と同じ扱い）。
5. **ADR-0074 との分担**: `min_free_disk_mb` の run 停止はそのまま。D4 は人に知らせるだけで、run は止めない。

### D5. 試験の名前

- D1 の試験は `target_sweep_` で始める（例: `target_sweep_deletes_items_older_than_max_age`、
  `target_sweep_trims_oldest_until_80pct_of_cap`、`target_sweep_skips_profile_with_held_cargo_lock`、
  `target_sweep_never_touches_live_release_lease`、`target_sweep_dry_run_deletes_nothing`）。
- D2 は `run_tmpdir_`（例: `run_tmpdir_env_points_to_run_dir`、`run_tmpdir_removed_on_success_failure_and_cancel`、
  `run_tmpdir_preamble_forbids_tmp_copies`）。
- D4 は `disk_watch_`（例: `disk_watch_notifies_once_on_crossing_warn`、`disk_watch_critical_goes_to_inbox`、
  `disk_watch_hysteresis_suppresses_flapping`）。
- 試験は注入した時計・`DiskProbe`・走査済み snapshot で決定的に書く。build 中の lock は試験の中で `.cargo-lock` を
  `flock` で握って再現する（実際の cargo を走らせない）。CPU を焼く負荷・`sleep` での待ちは使わない（ADR-0125）。

## 3. 帰結

- 共有 target は既定で root あたり 120 GiB・7 日を上限に収まる。build 中の profile は消えないので、並走する cargo の
  失敗は起きない（待たされるのは rename の間だけ）。最初の 1 回の後の build は消した crate だけ作り直す。
- `/tmp` は run と試験の残骸を受けなくなる。run の写しは `/local` の run dir に入り、終了で消える。
- 新しい種類（event `TargetSweepRan`・`RunTmpCleanupFailed`、`NoticeKind::disk`、`InboxKind::disk_full`、表
  `disk_watch_state`）が増えるので、schema の再生成と web の `event-kinds.ts`・`invalidation-map.ts` の追記が要る。
- cron の雛形に `extra.action` が入り、「cron 発火 → 決定的 executor」の口ができる（ADR-0131 の付記にあたる）。
- 有効化（cron の `resume`、config の roots の調整）は人の操作として運用手順書（close-out 葉）に書く。

## 4. 範囲外

- remote の run の TMPDIR（D2.1）。
- `/tmp` 全体の時間基準の掃除（systemd-tmpfiles 等の host 設定）は host 管理であり Celeris は持たない。
  `extra.action = "tmp_sweep"` は予約だけして今回は実装しない。
- sccache・host の cargo 設定（ADR-0136 で host 管理）。

## 付記: D4 の実装（2026-10-07、葉 disk-watch）

- 表 `disk_watch_state`（migration 0058、`SCHEMA_VERSION = 58`）。列は `path`（主キー）・`level`（`ok|warn|critical|unavailable`）・
  `since`・`last_pct`・`last_notified_at`・`updated_at`。読み書きは `task_core::DiskWatchStore`（`TaskStore` の supertrait）。
  `disk_watch_apply` は状態の upsert と通知の記録（`feed::record_in_tx`）を 1 つの transaction で行う。
- 判定は `task_dispatch::disk_watch`（`next_level`・`evaluate` は純関数、`run_disk_watch` が store と `DiskProbe` を使う）。
  本物の probe は `StatvfsProbe`（path そのものの `statvfs`。祖先へは辿らず、無ければ `unavailable`）。
  しきい値は「以上」で上がる。下げるのは `< しきい値 − 5`（1 回の測定で critical → ok の 2 段も下がる）。
  `unavailable` は `ok` と同じ扱いで次の測定を評価する。
- 通知: `NoticeKind::Disk`（`"disk"`）、group_key `disk:<path>`、source_key `disk:<path>:warn:<since>:<now>`、
  target `{kind: "disk", id: <path>}`。`warn` への上昇と、`warn` が続く間の 24 時間ごとの再通知だけ。
  `critical` への上昇・level の下降は通知しない。
- 受信箱: `InboxKind::DiskFull`（`"disk_full"`、`cluster_login` の次）。`inbox::inbox` が `disk_watch_state` の
  `critical` の行を `Inbox.disk_full`（serde・schema では skip。旧来の `GET /inbox` には出さない）に載せ、
  `human_inbox` が `disk_full-<slug>`（`/` は `root`、`/local` は `local`、`/var/tmp` は `var-tmp`）を作る。
  選択肢は `free_space` の 1 つで、`POST /inbox/items/{id}/answer` は `409 native_action_required`（host の操作）。
  web は `/daemon` へ案内する。
- 書き込みを減らすため、level・通知に変化が無く `last_pct` の動きが 1 ポイント未満の測定は DB に書かない。
- 間隔の時刻（前回測った時刻）だけは dispatcher のメモリに持つ（注入時計）。config の reload で測り直すが、
  状態が DB にあるので重複しない。draining のインスタンスは測らない。
- config: `[[maintenance.disk_watch]]`（`path` 必須・絶対 path、`warn_pct` 既定 80、`critical_pct` 既定 95、
  `0 < warn < critical <= 100`、path の重複不可）。省略で既定の 3 つ、`[maintenance] disk_watch = []` で無効。
  `Config::disk_watch_entries()` を daemon の起動（bootstrap）と reload（admin）が `Dispatcher::set_disk_watch` に渡す。
- D1.5: 保守 executor の target sweep が `over_cap_unresolved` のとき `disk:target_sweep` の通知
  （source_key `disk:target_sweep:<task id>`、上限を超えたままの root と大きさを要約に書く）。

## 付記: 実装との突き合わせ（2026-10-07、葉 close-out）

D1〜D4 は実装済み。決定どおりでない所・未実装の所を先に書く。

### 決定と異なる所

- **D1.1 使用時刻**: 項目の使用時刻は file は `max(mtime, atime)`、**dir は mtime のみ**（走査の `read_dir` が atime を進めるため）。
- **D1.4 手動 CLI**: `celerisctl target sweep` は config を読まない（`--config` なし）。`--root` 省略時は既定の 2 root、
  規則は `SweepParams::default()`。config の値で計画するのは daemon の cron executor だけ（`set_target_sweep` に起動時・reload で渡す）。
- **D2.2**: 警告 event `RunTmpCleanupFailed` は**入れていない**（Event 追加は schema 再生成と web の `event-kinds.ts` 等が要るため）。
  削除失敗は `tracing::warn` のみ。取りこぼしの回収は `scratch_gc` の tick ではなく
  `task_worker::workspace_prune::prunable_paths`（終端 task の `runs/*/tmp`。`workspace_prune_after_secs` 経過後。0 だと走らない）。
- **D2.1 bind**: db_guard の namespace は workspace を覆わないので sandbox への追加 bind は不要だった。
- **D2.4**: harness が `TMPDIR` を `/tmp` に戻さないことを確かめる adapter ごとの試験は無い（grep で claude-code・codex・acp・pi の
  起動が `TMPDIR` を設定・除去しないことを確認しただけ）。
- **D3.2 web e2e**: helper は `web/e2e/support/tmp-dir.ts` の `makeTmpDir`（`tmp.ts`/`makeE2eTmpDir` ではない）。作った dir は
  process の `exit` でも消す。34 file の `mkdtempSync` を置換、各 spec の `afterAll` の `rmSync` は残した。playwright e2e 自体は走らせていない。
- **D3.3 / D3.5**: `test-parallel.sh` は `TMPDIR=$logdir/tmp`、trap は `chmod -R u+w` の後 `rm -rf`（読み取り専用の試験残骸で
  `set -e` が落ちたため。失敗は警告のみで exit 不変）。`tmp_leftovers` は標準エラーの警告と summary に出る。
  「試験の中で TMPDIR を空 dir に向けて終了時に空を確かめる」は試験側に入れず、`scripts/dev/check-test-tmp-leftovers.sh -p <crate>`
  （空 dir を TMPDIR にして `cargo test`、残りがあれば exit 1）にした。cos_chat 添付試験の 0o500 dir は
  `task_dispatch::test_support::WritableTempDir`（Drop で権限を戻して消す）で扱う。
- **D4.2**: 存在しない path は人へ通知せず、状態の行（`unavailable`）と `tracing::warn` のみ。

### 実装の位置

| 決定 | 実装 | 試験 |
|---|---|---|
| D1.2 規則の計画 | `crates/task-worker/src/target_sweep.rs`（`plan`・`SweepParams`） | `target_sweep_` 9 件（時計・大きさ注入） |
| D1.2-4 build 中は消さない・rename→削除 | `crates/task-dispatch/src/target_sweep.rs`（`.cargo-lock` の非 blocking flock、`.deleting-*`） | `target_sweep_skips_profile_with_held_cargo_lock`（実 flock）ほか 6 件 |
| D1.4 cron | `crates/task-dispatch/src/dispatcher/maintenance.rs`、`config/celeris.example.toml` の seed `target-sweep`（`enabled = false`）、`[maintenance.target_sweep]` | cron 2 件・config 5 件 |
| D1.4 手動・`release.sh` | `crates/celerisctl/src/commands/target_sweep.rs`、`scripts/selfdeploy/release.sh`（次回 release から有効） | `cli_*` 3 件 |
| D1.5 記録 | event `TargetSweepRan`（`target_sweep_ran`） | round trip 1 件 |
| D2 | `crates/task-worker/src/run_tmpdir.rs`、`dispatcher/worker_task.rs`、`preamble::run_tmpdir_note()` | `run_tmpdir_` 10 件 |
| D3 | `scripts/dev/test-parallel.sh`、`check-test-tmp-leftovers.sh`、`web/e2e/support/tmp-dir.ts`、browser 試験の `record_dir` | 各 crate で `no leftovers`、vitest 2 件 |
| D4 | 先の付記のとおり | `disk_watch_` 13 件 |

`target_sweep_` は 26 件、`disk_watch_` は 13 件（葉の進捗に所在）。

### 未解決

- `over_cap_unresolved` の通知 `disk:target_sweep` は executor が出す。`TargetSweepRan` に失敗理由欄は無い（`failed` の理由は task の記録）。
- remote run の TMPDIR、`tmp_sweep`（予約語、実装しない）は範囲外のまま。
- ADR-0133 の種類一覧（通知 9 種・受信箱 14 種）に `disk`・`disk_full` を足す追記はまだ（本 ADR の付記が唯一の記載）。
- 本番での有効化・`/tmp` の既存残骸の掃除は人の操作（`docs/ops/build-tmp-hygiene.md`）。

### 付記（h3-tmpdir、2026-10-07）: 長い TMPDIR・深い session dir での unix socket

release gate（`CELERIS_USERNS_TESTS=1`、TMPDIR が 30 文字超）で `browser_h3_injection` が `isolated_runtime_unavailable` になった原因は、`SharedCdp::start` が `<workspace>/runs/<run_id>/browser/cdp-relay.sock` を bind する際に `sun_path`（107 byte）を超えたこと（`path must be shorter than SUN_LEN`）。action socket は既に短い固定 path（ADR 2026-10-06-browser-action-socket-path）。sandbox は `/session/cdp-relay.sock` で接続するので socket は session dir に置く必要がある。そこで `browser_shared_cdp::bind_unix_listener` は、path が上限を超えるとき親 dir を開いた fd の `/proc/self/fd/<fd>/<name>` で bind し、実体を session dir に作る。別名でも収まらなければ path と長さを含む `InvalidInput` にする。daemon 経路と launcher 経路（`start_with_mode`）の両方に効き、試験側の TMPDIR を短くする回避には頼らない。試験は `tmpdir_long_path_` 4 件（userns 不要）。

## 付記: 作業場所の cargo target を止める（2026-10-09、task 01M4FDM87P9QJEBSYW5XPZFM7T）

### 文脈

2026-10-09 に `/local`（300G）が 99% に達した。主因は task の作業場所の中の `repos/<repo>/target`（1 件 36〜200G）。
同日 00:35 の本番 DB 破損も worker の cargo build（target 91G）で thin pool が尽きたのがきっかけだった。
読んだこと:

- run（`run_worker`）は ADR-0075 の scratch（`<scratch>/targets/task-<id>[/wu-<id>]/target`、lease = owner）を
  `CARGO_TARGET_DIR` に与えている。**reviewer run**（`review::run_reviewer`）は `RunRequest.cargo_target_dir: None` のまま
  adapter に env を重ねていない。final review は反証の self-execution を求めるので、reviewer が作業場所で cargo を
  走らせると `repos/<repo>/target` ができる（実装上、通常 worker と異なる経路だった）。
- `prune_one_workspace`（ADR-0066 D2）は終端 task の `target/` を消すが cargo の lock を見ない。target sweep（D1）の
  roots は `<build_cache_dir>/cargo` と `/var/tmp/agent-platform-build` だけで、作業場所も scratch も見ない。
- D4 の通知は使用率だけを書き、何が大きいかは人が `du` で探していた。D4.5 で run は止めない。

### 決定

- **A1（run の `CARGO_TARGET_DIR`）**: 置き場は ADR-0075 の scratch pool のまま（lease の単位: task 単位の run は
  owner `task-<id>`、自分の worktree の WU は `task-<id>/wu-<id>`。並列の WU は別 target なので互いに壊さない。
  子 task は自分の task id の owner）。scratch が無効なら従来の `<build_cache_dir>/cargo/<repo-key>[/wu-<id>]`。
  **reviewer run にも同じ規則の env を重ねる**（検査コマンドと同じ `check_cargo_target_env(task, None)`。owner は
  対象 task。lease は touch するだけで adopt しない）。Rust の repo（先頭 repo に `Cargo.toml`）で env を
  重ねられない adapter のときは `warn` を出す（従来は `debug`）。
- **A2（掃除 a: 置き場を sweep の root に）**: 既定の roots に `<scratch>/targets` を足す（D1.1 の「scratch pool の target は
  root にしない」を改める）。scratch の root では **owner 配置**（`<root>/<owner>/target`、`<root>/<owner>/wu-*/target`）を
  target dir として扱い、`lease.json` 等の owner dir の file には触れない。`release-build` は D1.1 のとおり lease 本人が
  掃除するので対象外。規則（古さ 7 日・上限 120 GiB・放置 14 日・build 中の profile は消さない）は D1.2 と同じ。
  scratch_gc（ADR-0075）の owner 単位の回収はそのまま残る（sweep は owner の中の古い項目を削るだけ）。
- **A3（掃除 b: 終わった task の作業場所の target）**: target sweep の executor（cron・`celerisctl` ではなく daemon の保守
  task）が、終端（`done`・`cancelled`・`failed`）になってから `workspace_target_after_hours`（既定 6 時間）経った
  task の `<workspace_root>/<task>/repos/<repo>/target` のうち、**cargo の target**（`CACHEDIR.TAG` を持つか、
  `<profile>/.cargo-lock` を持つ dir。symlink は辿らない）を消す。実行中の task（終端でない）は見ない。どれかの
  profile の `.cargo-lock` が `flock(LOCK_EX|LOCK_NB)` で取れなければ `build_in_progress` で残す。lock を持ったまま
  `<repo>/.deleting-<ulid>` へ rename し、放してから消す。記録は `TargetSweepRan.roots` に root = `<workspace_root>`
  の 1 行（`by_reason.stale_target` に件数）として載せる（event の形は変えない）。`prune_one_workspace` も同じ lock の
  確かめを通し、lock 中の target は消さない。
- **A4（監視）**: D4 の `warn` の通知（初回・24h ごとの再通知）に**大きい target の上位 5 件**（作業場所の target・scratch の
  owner・`<build_cache_dir>/cargo` の直下。`st_blocks × 512` の合計）を本文に足す。測り方は注入できる trait
  `TargetCensus`（試験は固定の一覧を返し host を見ない）。測るのは通知を作るときだけ。
  D4.5 を改める: 監視している path のどれかが **`critical` のあいだは新しい coding run（task の genre が `coding`、
  または未指定）を起こさない**（ready のまま。`min_free_disk_mb` と同じく attempts を消費しない。CoS chat・
  保守 task は止めない。coding task の新規 reviewer・検査も reviewing のまま保留する。target sweep が critical 中も走れるよう、全体の `disk_ready` は使わない）。
  解除は level が下がった測定の tick から。
- 本番の cron（`target-sweep`）の有効化は配送後に運用が `resume` で行う（D1.4 のまま）。

### A1–A4 実装時の補足

- reviewer の adapter env に加えて `RunRequest.cargo_target_dir` にも同じ値を記録する。Codex の書き込み先追加は既存の adapter env 経路を利用する。`shared_build_cache = false`、remote、container の既存の除外は維持する。
- scratch sweep は DB で終端と確認できる task owner（WU 含む）だけを対象とする。非終端・不明・外部 owner は `active_or_unknown_owner` で残す。pool lock で allocate/adopt/GC と直列化し、Cargo profile lock も保持して rename する。上限は回収可能な項目の上限であり、稼働中 target を強制削除する hard quota ではない。
- workspace は `repos/<repo>/target`、旧 `tree/target` と `wu/<key>/repos/<repo>/target` を調べる。終端でも running の runs 行があれば保護する。掃除の残骸は専用の `.deleting-cargo-target-*` のみ拾い、任意の `.deleting-*` は消さない。symlink の task/repo/target は辿らない。
- 既存の scratch GC、終端 WU キャッシュ掃除、workspace prune と cancelled worktree の片付けにも Cargo lock 保護を適用する。
- 新しい scope 設定は起動時と reload 時に適用する。DB を開かない CLI の `target sweep` は既存の共有 root のみ。scratch と workspace を含む正確な dry-run は daemon の保守 task（cron `mode = "dry_run"`）で行う。本番有効化は人の操作。

検証（2026-10-09）: 対象 nextest 64 件、全体 `test-parallel.sh` 4,900 件と doc-test、`cargo clippy --workspace -- -D warnings` が成功。詳細は [進捗](../../docs/progress/2026-10-09-workspace-cargo-targets.md)。本番有効化は未実施。
