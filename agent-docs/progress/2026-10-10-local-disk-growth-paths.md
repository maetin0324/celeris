---
title: "/local disk growth paths: ADR 作成"
tasks: [01M4HX54MZ1P6ZYZEB6ABHD6BV, 01M4J36GBBT2BDJD24ZGMHJF68]
status: done
updated: 2026-10-10
---

# `/local` disk growth paths: ADR 作成

## 完了

- 2026-10-10 に `agent-docs/adr/2026-10-10-local-disk-growth-paths.md` を作成し、repo target、release-build lease、DB backup、dangling `.cargo-target` の原因と D1〜D5 を記録した。
- inv-targets / inv-release-db の findings を読み、記載されたコード位置と本番読み取り調査を根拠として統合した。`01M4D7RVKX` の残存理由は証拠不足のため断定せず、6時間猶予・木の checkout 追跡漏れ・cron apply 状態を候補として明記した。
- コード変更なし。`crates/` と `scripts/` に差分はない。

## 証拠

| 確認 | コマンド | 結果 |
|---|---|---|
| 調査 findings の存在 | `test -s /local/celeris/data/workspaces/01M4HX54MZ1P6ZYZEB6ABHD6BV/wu/inv-targets/artifacts/findings.md && test -s /local/celeris/data/workspaces/01M4HX54MZ1P6ZYZEB6ABHD6BV/wu/inv-release-db/artifacts/findings.md` | exit 0 |
| ADR・担当葉・進捗 | `test -s agent-docs/adr/2026-10-10-local-disk-growth-paths.md && grep -q worker-target-env agent-docs/adr/2026-10-10-local-disk-growth-paths.md && grep -q repo-target-gc agent-docs/adr/2026-10-10-local-disk-growth-paths.md && grep -q release-prune agent-docs/adr/2026-10-10-local-disk-growth-paths.md && grep -q backup-retention agent-docs/adr/2026-10-10-local-disk-growth-paths.md && test -s agent-docs/progress/2026-10-10-local-disk-growth-paths.md` | exit 0 |
| 文書リンク | `sh scripts/dev/check-doc-links.sh` | `check-doc-links: ok`、exit 0 |
| progress index | `sh scripts/dev/progress-index.sh --check` | `progress-index --check: ok`、exit 0 |
| ADR 番号 | `sh scripts/dev/check-adr-numbers.sh` | `check-adr-numbers: ok (178 files)`、exit 0 |
| 製品コード差分なし | `git diff --quiet "$(git merge-base HEAD main)" -- crates/ scripts/` | exit 0（差分なし） |

## 未解決

- `01M4D7RVKX` の実際の sweep event / live DB 状態がなく、残存原因の個別確定はできない。
- D4 の提案既定値（promote 10本、rollback 3本、合計64 GiB）は実装時に設定値として導入する。実装前の本番 backup 削除はしない。
- 本番適用は人による手順が必要。lease prune による clean rebuild と backup retention の本番実行は未実施。

## 提案

後続の `worker-target-env`、`repo-target-gc`、`release-prune`、`backup-retention` を ADR の表に記した担当範囲と試験 prefix で実装する。適用前に dry-run と復元点の integrity check を確認し、release-build lease の再作成時間を運用枠に含める。

## 実装の記録（backup-retention）

- promote 側: [promote-prune.md](2026-10-10-local-disk-growth-paths/promote-prune.md)
- 定期側: [periodic-retention.md](2026-10-10-local-disk-growth-paths/periodic-retention.md)
- 試験の既存 `backup_once` の 48 時間規則への合わせ: [fix-backup-once-test.md](2026-10-10-local-disk-growth-paths/fix-backup-once-test.md)
- ADR の実装付記（D4 節）: [ADR](../adr/2026-10-10-local-disk-growth-paths.md)

## close-out（統合後 HEAD の全体検査、2026-10-10）

ADR 2026-10-10-local-disk-growth-paths の 3 経路（repo 直下 target、release-build の古い test binary、DB backup の無制限保持）と dangling `.cargo-target` を塞ぐ実装が統合された。検査は HEAD `622096b4`（ops-runbook 込み）で行い、全て exit 0。製品コードは本 close-out で変えていない。

### 葉の成果

| 葉 | 成果 | 記録 |
|---|---|---|
| worker-target-env（D1） | 作業ツリーの run・check に scratch の `CARGO_TARGET_DIR` を渡す | [記録](2026-10-10-local-disk-growth-paths/worker-target-env.md) |
| repo-target-gc（D2） | 終端 task の repo 直下 target を猶予後に候補化・削除（running・生存子孫は保護） | [記録](2026-10-10-local-disk-growth-paths/repo-target-gc.md) |
| release-prune（D3） | release-build の前回 test binary を刈り、上限超過時は target を作り直す | [記録](2026-10-10-local-disk-growth-paths/release-prune.md) |
| ledger-target | browser-ledger の target 解決を lease 経由に統一、dangling link を除去 | [記録](2026-10-10-local-disk-growth-paths/ledger-target.md) |
| backup-retention（D4・D5） | promote/rollback 前 backup の刈り込み、定期 backup の保持規則（48 時間・日次・週次・総量） | [promote](2026-10-10-local-disk-growth-paths/promote-prune.md)、[定期](2026-10-10-local-disk-growth-paths/periodic-retention.md)、[試験修正](2026-10-10-local-disk-growth-paths/fix-backup-once-test.md)、[ADR 付記](2026-10-10-local-disk-growth-paths/adr-note.md) |
| ops-runbook | 人が本番 host で行う手順と確認方法 | [docs/ops/local-disk-growth.md](../../docs/ops/local-disk-growth.md) |
| verify-record（本 WU） | ADR 状態を「実装済み」へ、本進捗を close | この文書、[ADR](../adr/2026-10-10-local-disk-growth-paths.md) |

### 証拠

| 確認 | コマンド | 結果 |
|---|---|---|
| 全体試験 | `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` | exit 0。nextest passed 5041 / failed 0 / ignored 14、doctest exit 0。nextest 293 秒。ログ: `artifacts/test-parallel.log` |
| 試験の一時領域 | 同上の末尾 | `tests left 3 entries in TMPDIR (removed on exit)` の警告のみ（exit に影響なし）。daemon e2e の時間切れは無し（disk_watch の確認は不要。実行時の `/local` は 68% 使用） |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0（`Finished dev profile`）。ログ: `artifacts/clippy.log` |
| fmt | `cargo fmt --all -- --check` | exit 0。ログ: `artifacts/fmt.log` |
| 文書リンク | `sh scripts/dev/check-doc-links.sh` | exit 0、`check-doc-links: ok` |
| ADR 番号 | `sh scripts/dev/check-adr-numbers.sh` | exit 0、`check-adr-numbers: ok (178 files)` |
| progress 索引 | `sh scripts/dev/progress-index.sh --check` | exit 0、`progress-index --check: ok` |
| ADR 状態 | `grep -n '状態' agent-docs/adr/2026-10-10-local-disk-growth-paths.md` | `実装済み（2026-10-10）` |

sandbox 既知失敗（browser・launcher・credentiald・CDP 系）は今回 0 件で、分類の対象は無かった。

### 未解決事項

- 本番未適用。配送後の daemon 差し替え、既存 repo 直下 target（例: `01M4F3V643`）の削除、release-build の初回刈り込み、既存 backup の刈り込み、dangling `.cargo-target` の修理は人が [docs/ops/local-disk-growth.md](../../docs/ops/local-disk-growth.md) の手順で行う。本進捗は本番での効果を確認していない。
- `01M4D7RVKX` の残存理由は、sweep event と本番 DB の状態がないため個別に確定できていない（前段の記録どおり）。
- test-parallel が TMPDIR に 3 件の残骸を残す警告（exit 0）。どの試験が残すかは未調査。
- D4 の既定値（promote 10 本、rollback 3 本、定期 daily 7・weekly 4・総量 64 GiB）は実装の既定。本番の設定に載せるかは人が決める。

### 提案

- 配送後、docs/ops/local-disk-growth.md の 1〜6 を順に行い、効果を `df` と scratch lease の size で記録する。
- test-parallel が残す TMPDIR の 3 件を特定し、試験側で後始末する（別 WU）。


## 全体試験の再確認（2026-10-10）

final review の `5041 passed / 2 failed` は再実行で再現せず、元報告でも試験名が不明なため失敗名は「再現せず」とした。原因は特定できず、branch 起因を示す証拠はなく、コード修正はしていない。`/local` は 68% 使用で disk_watch の 95% 条件未満。`TMPDIR=/tmp bash scripts/dev/test-parallel.sh` は1回目・最終2回目とも exit 0、nextest 5043 passed / 0 failed / 14 ignored、doctest exit 0。詳細とログの場所は[再確認記録](2026-10-10-local-disk-growth-paths/reverify-full.md)を参照。fmt と clippy も exit 0。

## 運用文書の修正（docs-fix、2026-10-10）

final review の差し戻し（build-tmp-hygiene.md の未更新と local-disk-growth.md §2 の段落重複）を修正した。

- `docs/ops/build-tmp-hygiene.md` に §8「`/local` 容量の 3 経路」を追加し、repo 直下 target の GC（600 秒 tick、保護理由 `running_run`・`active_descendant`・`grace`）、release-build の刈り込み（test binary prune と `SD_RELEASE_TARGET_MAX_BYTES` 超過時の作り直し、大きさ判定は共有を数えない方式）、DB backup の保持（promote 10・rollback 3、定期の 48 時間・日次・週次・総量、削除前の `integrity_check`）を短く書いた。詳細は `docs/ops/local-disk-growth.md` §2〜§4 への相対リンクで指す。§2 の「release-build の target は入れない」注記も新しい刈り込みに合わせた。
- `docs/ops/local-disk-growth.md` §2 の重複していた「repo target の自動回収は daemon の常設 tick…」段落（約10分間隔版と 600 秒間隔版）を 1 段落に統合した（600 秒間隔・保護理由・cargo lock・木の最後の終端基準を残す）。

証拠コマンドと結果は[docs-fix の記録](2026-10-10-local-disk-growth-paths/docs-fix.md)を参照（文書検査 3 本は全て exit 0、`crates/` と `scripts/` は未変更）。

## v4: 全 workspace target の刈り込みと scratch の測り方（reprune、2026-10-10）

final review の差し戻し（release prune が integration test の binary を刈らない）を直した。

- **prune-workspace**: `sd_release_prune_stale_test_binaries` の候補を `cargo metadata` の全 target 名（test・bin・lib・example・bench）と package 名の和にした。`.d` が registry・git checkout を参照する依存 crate は残す。試験 `release_prune_stale_test_binaries.sh` で integration test・crates 外 member・依存との同名・marker より新しい binary を確かめた（exit 0）。記録は[prune-workspace](2026-10-10-local-disk-growth-paths/prune-workspace.md)。
- **本番 target の dry run（読み取りだけ、削除 0）**: 2026-10-10 08:40:45Z 開始の release-build target で、候補 307 file・25.2 GiB（allocated）。内訳は今回の build 済み 124 個・34.5 GiB（残す、build 途中の値）、古い package 名 8 個・3.1 GiB（旧規則でも刈れた）、古い integration test 等 67 個・15.4 GiB（新規則で初めて刈れる）。依存との名前衝突は 0。
- **sizing-adr**: ADR 2026-10-10-local-disk-growth-paths.md の付記「scratch の上限と測り方」で、seed_reflink の有効化条件（`/local` は btrfs、scratch・作業場所・`$TMPDIR` が同じ fs、`cp --reflink=auto` と FIEMAP で共有 extent を確認。`--reflink=always` は worker の seccomp 下で EPERM）、`targets_max_gb` の既定 160・`total_max_gb` 200、共有を重ねない測り方（FIEMAP の physical extent を pool 全体で重複除去）を決めた。記録は[sizing-adr](2026-10-10-local-disk-growth-paths/sizing-adr.md)。
- **sizing-impl**: 既定値を 160 / 200 GiB にし、`scratch::gc::measure_tree_shared` で共有 extent を 1 回だけ数えて GC と watermark の `SizeCache` に入れた。FIEMAP 失敗時は `st_blocks` に戻す。試験 `scratch_shared_` 3 件と scratch の試験が通った。記録は[sizing-impl](2026-10-10-local-disk-growth-paths/sizing-impl.md)。

### 全体検査（reclose、HEAD 033b776f）

- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh`: exit 0、nextest 5047 passed / 0 failed / 14 ignored、失敗名 0 件。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- 文書検査 3 本（check-doc-links・check-adr-numbers・check-doc-layout）: 全て exit 0。

証拠の詳細は[reclose](2026-10-10-local-disk-growth-paths/reclose.md)。

### 人の本番手順（配送後）

- 配送後の daemon 差し替えと release の刈り込み確認: [docs/ops/local-disk-growth.md](../../docs/ops/local-disk-growth.md) §1・§3。
- `[scratch] seed_reflink` の有効化（起動 log での probe 確認と戻し方）: 同 §7。
- `[scratch] targets_max_gb` の暫定 150 を消して既定 160 に戻す手順と確認（pressure の記録）: 同 §8。
- 既存の repo 直下 target・backup 刈り込み: 同 §2・§4。

## v5: 本番相当の削減量の試験と全体検査（reclose-2、2026-10-10）

人のコメント（v5）への対応。final review の基準 2 の差し戻し（削減量を本番相当で示す）に対する記録。

- **prune-scale（本番相当の削減量）**: fixture は 2026-10-10 09:36Z 採取の inventory（199 行）で、marker より古い workspace executable は 11 個・0.095 GiB。縮尺試験の reclaimed は 0.113 GiB（production-equivalent）。registry 依存と新しい binary は残る。**08:40Z の 381 個・108 GiB 相当は、この inventory に無いため再現していない。** integration test 由来の割合は inventory の列からは出せない（前回の dry-run では 67 個・15.4 GiB が新規則で初めて刈れる分）。8 release 蓄積の非増加は未実装。記録は [prune-scale](2026-10-10-local-disk-growth-paths/prune-scale.md)・[reclose-2](2026-10-10-local-disk-growth-paths/reclose-2.md)。
- **(2) `seed_reflink`**: 既定 `false` のまま。有効化の条件（btrfs で `probe_pool_share` が成功、測り方 S3 の release、`seed_refresh_hold`）と手順・戻し方は ADR 2026-10-10-local-disk-growth-paths.md の付記「scratch の上限と測り方」S1 と [docs/ops/local-disk-growth.md](../../docs/ops/local-disk-growth.md) §7。
- **(3) `targets_max_gb`**: 恒久の既定は 160（`total_max_gb` 200）。測り方は FIEMAP の physical extent を pool 全体で 1 回だけ数える（S3）。根拠と暫定 150 を戻す手順は同付記 S2 と docs/ops §8。既定値はコードで確認済み（`crates/celeris/src/config/scratch.rs`）。
- **全体検査（HEAD 706151b3）**: `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` exit 0・5060 passed / 0 failed / 14 ignored。`cargo clippy --workspace -- -D warnings` exit 0。`cargo fmt --all -- --check` exit 0。文書検査 3 本（check-doc-links・check-adr-numbers・check-doc-layout）exit 0。証拠は [reclose-2](2026-10-10-local-disk-growth-paths/reclose-2.md)。

未解決: 本番 108 GiB 相当の削減量の試験（08:40Z の inventory か、人が host で採取した読み取り専用 inventory が要る）と、8 release 蓄積の非増加の試験。

## v6: 8 release 積み上げの試験と本番換算（reclose-3、2026-10-10）

final review（v5 後）の基準 2 の差し戻しへの対応。prune-accum（縮尺 1/1024 の 8 release 積み上げ）の結果と、全体検査（HEAD 5e0dafd9）。記録は [reclose-3](2026-10-10-local-disk-growth-paths/reclose-3.md)。

- **8 release 積み上げ（prune-accum、RESULT 行）**: `RESULT no_prune_bins=597 no_prune_gib=162.945 catchup_reclaimed_gib=149.367 prune_max_gib=67.902 prune_final_gib=27.156 prune_bound_gib=67.902 nonincreasing=1 limit_recreate_without_prune=1 limit_recreate_with_prune=0 deps_kept=1`
  - 刈らない系列（A）は 8 release で 597 binary・162.945 GiB まで増える。毎回刈る系列（B）の最大は 67.902 GiB（理論上限以下）、最終は 27.156 GiB で、release 2〜8 はほぼ一定（27.15〜27.17 GiB）。**非増加**（`nonincreasing=1`）。
  - 上限超過で target を作り直したのは刈らない系列だけ（`limit_recreate_without_prune=1`、刈る系列は `limit_recreate_with_prune=0`）。依存 crate の rlib/.d は保持（`deps_kept=1`）。
- **本番換算の扱い**: 上の数字は縮尺 1/1024 の fixture での比。本番の 108 GiB 相当（08:40Z 時点）は、repo の inventory（199 行、marker 以前の workspace binary 11 個・0.095 GiB）に無いため、再現していない。「刈らない累積が 8 release で約 163 GiB、刈ると約 27 GiB に収まる」は縮尺の比であり、本番の実数ではない。
- **全体検査（HEAD 5e0dafd9）**: `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` exit 0・nextest 5060 passed / 0 failed / 14 ignored・`test-parallel: ok`（失敗名 0 件）。`cargo clippy --workspace -- -D warnings` exit 0。`cargo fmt --all -- --check` exit 0。`release_prune_production_scale.sh` exit 0。

未解決（本 WU では解けない）: 本番 108 GiB 相当の再現（本番 `release-build/target/debug/deps` の読み取り専用採取が要る。人が host で行う）と、本番の候補が少ない理由（既に消えたか、判定が漏らしているか）の確認。

## v7: 本番 release-build の監査・試験の本番 path 隔離・close-out（reclose-4、2026-10-10）

v6 final review の基準 2 の差し戻し（本番の刈り込み候補が 67 個・15.4 GiB（08:40Z）から 11 個・0.095 GiB（09:36Z）に減った理由）への対応。仕様は `artifacts/reclose4-spec.md`（工程の外）。監査の記録は [prod-touch-audit](2026-10-10-local-disk-growth-paths/prod-touch-audit.md)、試験隔離は [prune-path-guard](2026-10-10-local-disk-growth-paths/prune-path-guard.md)、ADR は [付記 2026-10-10: 試験の本番 path 隔離](../adr/2026-10-10-local-disk-growth-paths.md)。

### 基準 0: 試験結果（HEAD は本 close-out の作業ツリー、製品コードは変更なし）

| 検査 | command | 結果 |
|---|---|---|
| 全体試験 | `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` | exit 0。`CELERIS_TEST_SUMMARY {"passed": 5060, "failed": 0, "ignored": 14, "nextest_exit": 0, "doctest_exit": 0, "tmp_leftovers": 5}`、`test-parallel: ok`。失敗名は `test-parallel: failed:` 行 0 件。nextest 226.7 秒。ログ: `artifacts/test-parallel.log` |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0（`Finished dev profile`）。ログ: `artifacts/clippy.log` |
| fmt | `cargo fmt --all -- --check` | exit 0。ログ: `artifacts/fmt.log` |
| prune 試験の本番 path 隔離 | `bash scripts/selfdeploy/tests/prune_tests_stay_in_tmp.sh` | exit 0、`prune_tests_stay_in_tmp: ok`。ログ: `artifacts/prune_tests_stay_in_tmp.log` |

試験の一時領域の警告は `tests left 5 entries in TMPDIR (removed on exit)`（exit に影響なし。前回の 3 件から増えた。どの試験が残すかは未調査）。

### 基準 1: 監査の結果（本番 release-build を刈ったか）

prod-touch-audit.md の行頭の 2 行をそのまま写す:

AUDIT runs_scanned=56 prod_path_commands=27 nondry_prune_on_prod=0 removed_files=0 removed_gib=0

CAUSE rebuilt_in_place 08:40Z の target/.rustc_info.json 更新・deps 件数増加と 08:59Z built_at の release 5b60d8b7bd80 が同じ期間にあり、09:36Z inventory は 67→11 の残存候補を示す。56 個の run 記録に本番非 dry prune/release.sh 実行はないため、再 build による mtime 更新が最も整合する。ただし全 56 個の再 build 前後 inode を結び付ける記録がなく、56 個が再生成された個別証明はない。

- **67→11 の理由**: この task 群の記録に本番の削除は無く、削除で消えた候補は 0 件・0 GiB。08:40Z 以降の release-build の再 build（cargo が同名 hash の file を作り直すと mtime が新しくなり候補から外れる）で候補が減ったと判定した。56 個それぞれの再生成は個別には立証していない（原票の inode/mtime 保存が無い）。
- **本番 path への prune の有無**: 監査の範囲で本番 path に対する非 dry の prune・rm・find -delete・release.sh は 0 件（件数 0・合計 0 GiB）。本番 target に prune 関数を走らせたのは 08:42:42Z の `SD_RELEASE_PRUNE_DRY_RUN=1` の 1 回だけで、unlink は実行されていない。
- **08:40Z の build の出所**: 起動した run・人は特定できていない（指定 8 workspace の記録に `release.sh` 起動者が無い）。現存 release `5b60d8b7bd80`（built_at 08:59:22Z）と時間的に整合する。
- **本 close-out の run**: 本番 path に書き込む command は無い（試験は一時 dir、prune 試験は `SD_PRUNE_ALLOWED_ROOT` で隔離）。ただし `test-parallel` 全体が本番 path に触れなかったことは、個別の試験の guard 試験で固定しているものの、全体の前後 inode 比較では直接観測していない。

### 基準 1 続き: 8 release 積み上げの RESULT（prune-accum）

prune-accum.md の行を写す:

RESULT no_prune_bins=597 no_prune_gib=162.945 catchup_reclaimed_gib=149.367 prune_max_gib=67.902 prune_final_gib=27.156 prune_bound_gib=67.902 nonincreasing=1 limit_recreate_without_prune=1 limit_recreate_with_prune=0 deps_kept=1

（縮尺 1/1024 の fixture。本番の実数ではない。前回 reclose-3 の記録のとおり。）

### 基準 2: 試験の本番 path 隔離（guard）

- `scripts/selfdeploy/lib.sh` の prune・backup 削除に `SD_PRUNE_ALLOWED_ROOT` の guard（line 787 付近）を入れ、未設定の本番の挙動は変えていない。
- `prune_tests_stay_in_tmp.sh` は静的に tests/*.sh の本番 path literal を、動的に一時 dir の外の囮に対する拒否と残存を確かめ、exit 0。

### 未解決事項

- 本番 108 GiB 相当の再現（08:40Z 時点の本番 `release-build/target/debug/deps` の読み取り専用採取が要る。人が host で行う）。
- 67 個の候補がどの file で再生成されたかの個別証明（inode/mtime の原票）。
- 08:40Z の build の起動者の特定（人が host の shell 履歴・ログで確かめる）。
- test-parallel の TMPDIR 残骸（5 件）の出所。

### 提案

- 本番の候補が減った理由を確定するなら、人が host で `ls -l --time-style=full-iso` の読み取り採取を 1 回行い、inventory と突き合わせる（書き込みなし）。
