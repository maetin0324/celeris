---
title: test-affected.sh と test-parallel.sh を helper に置き換える（apply-runners）
tasks: [01M6SBM6SQKNRD9FBXM3SPN5VB]
status: done
updated: 2026-11-06
---
# test-affected.sh と test-parallel.sh を helper に置き換える（apply-runners）

完了日: 2026-11-06（WorkUnit）

## 変更
- 両 runner の旧い `mktemp -d /tmp/celeris-test-*` と EXIT trap（`cleanup_logdir` を含む）を消し、`scripts/dev/lib/test-tmp-lifecycle.sh` の
  `ttl_install_traps` / `ttl_init` / `ttl_run` / `ttl_finish` に置き換えた。新旧切替の flag・互換分岐は置いていない。
  - test-affected.sh: crate 段だけ。cargo（nextest・doc）は `ttl_run`（setsid の別 session）で起こす。`changed` file は元の場所のまま、EXIT trap で消す（`set +e` を入れ、非 0 終了でも helper の後始末まで走らせる）。
  - test-parallel.sh: log は `$TTL_DIR`、試験の一時 dir は `$TMPDIR`（`$TTL_DIR/t`）。`cargo … | tee` は `run_tee`（別 session の cargo + procsubst の tee）に替えた。tee は lock の fd を継がない。
    準備失敗 4 等の exit code、summary、`tests left N entries in TMPDIR (removed on exit)` と `tmp_leftovers` は維持。一時 dir を作れないときは exit 2。
- helper の不具合を 1 件直した（`scripts/dev/lib/test-tmp-lifecycle.sh`）: `exec {fd}<file 2>/dev/null` は stderr を**以後ずっと** /dev/null にするため、runner の診断が全部消えていた。`{ exec …; } 2>/dev/null` に直した。
- `scripts/dev/tests/test-runner-tmp-lifecycle.sh` に両 runner の case を追加した（96 → 188 check）。偽 cargo は fixture の PATH 先頭に置く（nextest list/run、metadata、test --doc。TMPDIR に dir と実 Unix socket、FIFO の ready/release）。
  - 通常終了、非 0 終了（exit 100）、INT/TERM/HUP（親へ送る。偽 cargo へ転送され、終了後に dir が消えて 128+N）— 捕捉できる signal。
  - 稼働中の別 runner の dir を保持、親のみ KILL（dir 残り・孤児の子が使う間は次の runner も残す・子終了後の次回起動で回収）、group KILL（dir 残り・次回起動で回収）— 次回回収の case。
  - runner 内の短い path（TMPDIR 47 byte 以内）で 107 byte の Unix socket を bind。
  - test-parallel の失敗: prepare 失敗（exit 1、nextest 未起動、summary prepare_exit 1）、実 bin 欠落（exit 4）、nextest 失敗（exit 100、失敗名が最終行より前）、leftovers 警告と summary `tmp_leftovers`。
  - 両 runner に旧 mktemp・旧 trap が無いこと（grep）。owner 不明・走査 root 外の非接触は helper 単体の case（tmp-helper）で固定済み。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash scripts/dev/tests/test-runner-tmp-lifecycle.sh` → exit 0（188 checks）。3 回連続で exit 0。実行後に `/tmp/ttl-t.*` は残らない。
- `sh scripts/dev/tests/test-affected-list.sh` / `bash scripts/dev/test-cli-binary-preparation.sh` / `bash scripts/dev/tests/test-parallel-fail-names.sh` / `bash scripts/dev/tests/test-parallel-userns-preflight.sh` → 全て exit 0。
- test-affected・clippy・fmt は下の「提出時の検査」を参照。

## 未解決・後続
- `scripts/dev/tests/test-runner-tmp-repro.sh` は現行 runner の写しを helper なしで fixture に置くので、この変更で期待表（旧挙動）と合わなくなる。旧 script の最小 fixture の固定と期待表の更新は pin-wire-record の担当（この unit では触っていない）。
- 軽量検査への配線は pin-wire-record。

## 提出時の検査（提出時の HEAD で実行）
- `bash scripts/dev/test-affected.sh` → exit 100。対象 crate は celeris・celeris-campaign・celeris-script・celerisctl・llm-proxy・task-dispatch。3541 件中 3540 件が通り、1 件が失敗:
  `celeris-campaign::launcher a_swapped_daemon_binary_is_rejected_on_every_path_and_stop_only_touches_the_launcher_pid`（daemon が healthy になる前に終了）。
  同じ test を単独で `cargo nextest run -p celeris-campaign --test launcher a_swapped` で走らせると exit 0（1 passed）。全体の並列実行中の負荷による失敗と見ているが、原因の特定はしていない（未確認）。
  runner の変更（scripts/dev のみ）との関係は、単独実行が通ることから薄いと見ている。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- 範囲: `scripts/dev/lib/`・`scripts/dev/test-affected.sh`・`scripts/dev/test-parallel.sh`・`scripts/dev/tests/test-runner-tmp-lifecycle.sh`・この進捗ファイルだけ。
