---
title: 一時領域の共通 helper と helper 単体の試験（tmp-helper）
tasks: [01M6SBM6SQKNRD9FBXM3SPN5VB]
status: done
updated: 2026-11-06
---
# 一時領域の共通 helper と helper 単体の試験（tmp-helper）

完了日: 2026-11-06（WorkUnit。runner 本体は変えていない。適用は apply-runners が行う）

## 変更
- `phase 2: 試験 runner の一時領域の共通 helper と単体試験` — `scripts/dev/lib/test-tmp-lifecycle.sh`（ADR 2026-11-06-test-runner-tmp-lifecycle の D1〜D4）と
  `scripts/dev/tests/test-runner-tmp-lifecycle.sh`（引数なしで helper 単体を試す）を追加した。

## helper の API と ADR との差
- 公開関数: `ttl_init <runner>`・`ttl_sweep [root]`・`ttl_run <cmd…>`・`ttl_install_traps`・`ttl_finish`（先頭の説明に環境変数の注入点を書いた）。
- ADR に無い決めごと（helper の先頭にも書いた）:
  - 子の session は `ttl_run` が `<dir>/sids` に記録し、D3 の 3 は owner の sid と `sids` を走査する。runner 自身の session は子の session ではない（D4）ためである。
    回収側自身の session は走査から除く。同じ端末から起こした別の実行を、永久に保護しないためである（その実行は lock が守る）。
  - owner の boot_id が今と違うときは D3 の 3 を省く（旧 boot の process は無い）。
  - zombie は session の走査で数えない。
  - 非同期で起こした子は SIGINT を無視で継承する。INT の転送では、子が INT を無視していれば TERM を送る。
  - `ttl_finish`（EXIT）は、子の session が空で、lock を他に持つ process が無いときだけ dir を消す。
  - 待ちは `tick` fifo への `read -t`（busy loop にしない）。待ち時間は `TTL_SIGNAL_WAIT`（既定 30 秒、整数）。

## 試験の範囲（96 check）
通常終了・非 0 終了、INT/TERM/HUP（親だけ。子へ転送され、子の終了後に dir が消えて 128+N）、signal を無視する子（待ち上限 1 秒を注入。dir 残り、子の終了後の次回回収）、
親だけ KILL（孤児の子が lock を持つ間は残す）、group KILL 後の次の `ttl_init` による回収、lock の fd を閉じる子（session 走査で保護）、PID 再利用相当・boot_id 違い、
若い dir・初期化途中（1 時間境界は 3599 秒で残す）、owner 不明 5 種、/proc 不明 2 種、並行回収（`.sweep.lock` 保持中は飛ばす・6 並行で 12 件を 1 回ずつ）、
symlink・別 UID（TTL_UID 注入）・走査 root 外の非接触、root の選択（scratch→fallback、予算 38/39 byte、symlink・0755・別 UID の root は使わず作り直さない）、
短い path での実 Unix socket bind（107 byte 成功・108 byte 失敗）。SIGKILL 直後の削除は試験しない。
終了時に記録した pid が全て無いこと、試験の dir が無いことを自分で確かめ、残れば exit 1 にする。実 cargo・負荷台本は使わない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash scripts/dev/tests/test-runner-tmp-lifecycle.sh` → exit 0（`runner-tmp-lifecycle: ok（96 checks）`、約 7 秒）。作成中に 6 回実行して全て exit 0。実行後に `/tmp/ttl-t.*` と偽の子 process は残らない。
- 範囲: `scripts/dev/lib/`・`scripts/dev/tests/`・この進捗ファイルだけ。runner（`test-affected.sh`・`test-parallel.sh`）は変更なし。

## 未解決
- 試験が同時に起こす job の終了報告（`Killed`）が、まれに stderr に 1 行出る（bash の報告で、結果には影響しない）。
- 軽量検査への配線、`test-runner-tmp-repro.sh` の期待表の書き換え、runner への適用は後続の unit（apply-runners・pin-wire-record）。
