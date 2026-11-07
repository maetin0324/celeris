---
title: signal 終了を再現し、試験 runner の一時領域の寿命管理を設計する
tasks: [01M6S9KKWT7EM7FP5CCG1Q8DH0]
status: done
updated: 2026-11-07
---
# signal 終了を再現し、試験 runner の一時領域の寿命管理を設計する

完了日: 2026-11-07（設計 → 実装 → 統合後の検査まで完了。本番の残骸の掃除は人の手順で未実施）

## 結論
- 現行の 2 本の runner（`scripts/dev/test-affected.sh`・`scripts/dev/test-parallel.sh`）の一時 dir は、通常終了と TERM・INT・HUP では消える。
  SIGKILL（親だけ・group）では残る。親だけへの SIGKILL では、子が孤児として残った dir を使い続ける。
- 本番の確定した発生経路は、決定的 check の timeout である。`crates/task-worker/src/workspace.rs` の exec は、TERM を送らずに group と子孫へ SIGKILL を送る。
  worker run の打ち切り（`kill_now`: SIGTERM → `kill_grace` 後に SIGKILL）でも、試験が grace 内に終わらなければ残る。
- 設計は ADR [2026-11-06-test-runner-tmp-lifecycle](../adr/2026-11-06-test-runner-tmp-lifecycle.md) に書いた。
  配置は scratch LVM を優先し、`/tmp` へ fallback する。`TMPDIR` は 47 byte 以下とする。
  稼働中の保護は、flock・owner metadata（pid・starttime・sid・boot_id）・session の走査で行う。SIGKILL の残りは次回起動時に回収する。旧い残骸は人の手順で消す。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）
- `bash scripts/dev/tests/test-runner-tmp-repro.sh` → exit 0。末尾の行は
  `runner-tmp-repro: ok（18 case が期待表と一致。一時 dir が残るのは SIGKILL の 4 case。子が使用中の TMPDIR を trap が消した case: N）`（N は 0〜3）。
  所要は約 2〜3 秒。作成中に 14 回実行し、期待表を今の形にした後の 6 回と、提出前の 3 回はすべて exit 0 だった。
  走った後に、試験が作った `/tmp/celeris-test-*` と偽 cargo の process は残らない（cleanup が記録した path と pgid だけを回収する）。
- 観測の表（rc・trap の有無・孤児・子の `TMPDIR` 書き込み・残り）は ADR の「観測」節にある。
- sun_path の実測（python で bind を試した）: `/tmp`・`/local/celeris/data/scratch`・run の `TMPDIR`（93 byte）のどれでも、bind できる最長の path は 107 byte。
- host の状態（観測のみ。消していない）: 2026-11-06 21 時台（JST）に `/tmp/celeris-test-affected.*` が 27 件、`/tmp/celeris-test-parallel.*` が 15 件あり、`du -sch` で計 29G。
  `/tmp` は tmpfs 63G で、使用率は 46%。
- 必須検査:
  - `bash scripts/dev/test-affected.sh` → exit 0（`試験対象の変更なし（docs のみ等）`。check-health は通過）
  - `cargo clippy --workspace -- -D warnings` → exit 0
  - `cargo fmt --all -- --check` → exit 0
  - `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check` → exit 0

## 変更（commit は件名）
- `phase 1: 試験 runner の一時領域の signal 終了を再現する試験と寿命管理の ADR` — 再現の試験と ADR（実装より先）
- `phase 1: 一時領域の寿命管理の進捗を記録する` — この進捗ファイル

## 未解決
- claude-code の Bash 道具の子が harness の process group に入るかは、確認していない。また、run の打ち切りでどの signal が届くかも確認していない。
- test-parallel.sh の残骸 15 件が、どの経路で残ったかは特定していない。
- 親だけへの TERM・HUP で、trap が子の終わりを待たずに走る理由は推測にとどまる（bash が wait に入る前に signal が届く）。bash の source は読んでいない。
- 決定的 check の namespace から scratch LVM が見えることは、直接は確かめていない（cargo が `CARGO_TARGET_DIR` に書けることからの推測）。

## 本番への移行
人が本番 host で行う。agent は実行していない。稼働中の dir は消さない。

旧い残骸（`/tmp/celeris-test-affected.*`・`/tmp/celeris-test-parallel.*`）を自動で回収できない理由: owner metadata（pid・starttime・sid・boot_id）と flock の lock file が無く、持ち主が生きているか判定できない。新しい helper は `<root>` の直下しか走査せず、これらの旧 prefix を見ない。判定できないものは残す方に倒す設計（ADR の D3・D6）なので、人が確認して消す。

1. 候補の確認: `ls -ld /tmp/celeris-test-affected.* /tmp/celeris-test-parallel.*` と `du -sch /tmp/celeris-test-affected.* /tmp/celeris-test-parallel.*`。件数と容量を控える。
2. 経過時間と process 参照の確認: `stat -c '%y %U %n' <dir>` で更新時刻と所有者を見る。次で参照中の dir を洗い出す。
   `for d in /tmp/celeris-test-affected.* /tmp/celeris-test-parallel.*; do ls -l /proc/[0-9]*/cwd /proc/[0-9]*/fd 2>/dev/null | grep -F "$d" && echo "in use: $d"; done`
   `in use:` が出た dir、または更新から 120 分以内の dir は消さない（稼働中の試験かもしれない）。
3. 削除対象の再確認: 手順 2 を通った dir の一覧を、削除の直前にもう一度出して目で確かめる。その後、自分の UID が所有する dir だけを消す。
   `find /tmp -maxdepth 1 \( -name 'celeris-test-affected.*' -o -name 'celeris-test-parallel.*' \) -user "$(id -u)" -type d -mmin +120 -exec sh -c 'chmod -R u+w "$1"; rm -rf --one-file-system "$1"' _ {} \;`
   手順 2 で `in use:` だった dir が残っていることを `ls -d` で確かめる。
4. 削除後の容量確認: `df -h /tmp` で使用率が手順 1 の前より下がったことを確かめる。下がらなければ、`in use:` で残した dir か、別の消費元を `du -xh --max-depth=1 /tmp | sort -h | tail` で調べる。
5. 選んだ scratch 配置の確認（新しい runner を使う前）:
   - 存在: `ls -ld /local/celeris/data/scratch`（この run の環境では所有者 rmaeda、書き込み可を確認した）。
   - 権限: root は runner が初回に `/local/celeris/data/scratch/tr-<uid>` を 0700・自分の UID で作る。`stat -c '%a %U' /local/celeris/data/scratch/tr-$(id -u)` が `700 <自分>` であること。別の所有者・symlink・0700 以外なら runner はその候補を使わず `/tmp/celeris-tr-<uid>` へ fallback し、stderr に理由を出す。
   - 短さ: `TMPDIR` は `<root>/XXXXXX/t` で 45 byte（上限 47）。`CELERIS_TEST_TMP_ROOT` を設定する場合は `printf '%s' "$CELERIS_TEST_TMP_ROOT/XXXXXX/t" | wc -c` が 47 以下であること。run の `TMPDIR`（93 byte）は上限を超えるので使わない。
6. 通常の release 手順: main に取り込んだ後、既存の release 手順（`scripts/selfdeploy/release.sh` の gate、続けて promote）を人が実行する。この task では実行していない。
7. gate 後の残存確認: `ls -la /local/celeris/data/scratch/tr-$(id -u)/` と `ls -la /tmp/celeris-tr-$(id -u)/ 2>/dev/null` を見る。`.init-*`・`.trash-*` や owner の死んだ dir が残っていなければ正常。SIGKILL で残った dir があっても次の runner 起動時に回収される（下の限界を参照）。旧 prefix の dir が増え続けるなら、旧い runner（main に追従していない branch や release）が動いている。`df -h /tmp` も再確認する。

## 提案
- daemon の決定的 check の timeout を、SIGKILL だけでなく SIGTERM → grace → SIGKILL の 2 段にする（`crates/task-worker/src/workspace.rs` の `exec` と `GroupKillGuard`）。
  runner の trap が後片付けできる機会が増える。別の task で判断する。
- 実装の unit は、ADR の D1〜D5 を共通 helper にまとめ、両 runner を切り替える。
  その際に `test-runner-tmp-repro.sh` の期待表を新しい挙動へ書き換える。新しい挙動では、SIGKILL の後の次回起動で回収されること、子の使用中は消さないことを確かめる。

## 実装 unit の結論（2026-11-06、実装は scripts/dev 内だけ）
- 共通 helper `scripts/dev/lib/test-tmp-lifecycle.sh` を置き、`test-affected.sh` と `test-parallel.sh` の旧 mktemp/trap の経路を消して置き換えた。
  `crates/`・worker・release gate には差分がない（`git diff $CELERIS_WU_BASE -- crates` は空）。
- 再現試験 `tests/test-runner-tmp-repro.sh` は、helper 導入前の runner の固定した写し（`scripts/dev/testdata/runner-tmp-repro/*.pinned`）を観測する。
  runner を変えても観測は変わらない。新しい runner の期待は `tests/test-runner-tmp-lifecycle.sh` に置いた。
- `test-runner-tmp-lifecycle.sh` は偽 cargo と小さな git fixture だけで両 runner を通す。実 cargo・全体試験・負荷台本は使わない。
  通常終了・非 0 終了・INT/TERM/HUP、次回回収、稼働中 owner と子の保持、PID 再利用相当、若い dir、初期化途中、owner 不明、並行回収、symlink・別 UID、実 Unix socket の bind、test-parallel の exit code・summary・leftovers 診断を確かめる。
- 配線: `test-affected.sh` に、変更 path が helper・両 runner・両試験・固定 fixture のどれかに当たるとき lifecycle と repro を走らせる検査を足した。
  両試験は `CELERIS_RUNNER_TMP_TEST=1` を export し、fixture 内の runner の写しが同じ検査を再び呼ばない（再帰しない）。

## 証拠（提出時の HEAD で実行。最後の同期の後に再実行していない）
- `bash scripts/dev/tests/test-runner-tmp-lifecycle.sh && bash scripts/dev/tests/test-runner-tmp-repro.sh` → exit 0（repro は 18 case 一致、子が使用中の TMPDIR を trap が消した case は 0）。
- `sh scripts/dev/tests/test-affected-list.sh`・`bash scripts/dev/test-cli-binary-preparation.sh`・`bash scripts/dev/tests/test-parallel-fail-names.sh` → いずれも exit 0。
- `bash scripts/dev/test-affected.sh` → exit 0。
- `cargo fmt --all -- --check` → exit 0。Rust の変更はないので clippy は対象外（実行していない）。
- commit（件名）: `phase 4: 一時領域の回帰試験を test-affected.sh の scripts/dev 検査へ配線し再入を防ぐ`。この進捗の commit も別件名で続く。

## 実装後の未解決
- SIGKILL 直後の即時削除は約束しない。残りは次回起動時の回収で消える（条件は lifecycle 試験で固定）。
- fd を閉じて別 session へ抜けた子孫は、session の走査でも見えず、稼働中保護が効かない場合がある。限界として残す。
- 旧い残骸（`/tmp/celeris-test-affected.*`・`/tmp/celeris-test-parallel.*`）は ADR の D6 の人の手順で消す。agent は消していない。

## pin-wire-record の再実行（前回 check の不合格の原因）
- 前回の `bash scripts/dev/test-affected.sh` は exit 100 だった。落ちたのは `celeris-campaign::launcher` の
  `a_swapped_daemon_binary_is_rejected_on_every_path_and_stop_only_touches_the_launcher_pid`（daemon が healthy になる前に終了）で、
  scripts/dev の変更とは関係しない。同じ試験は変更なしの再実行で通った。負荷または環境起因の flake と判断した（原因の特定は未確認）。
- 提出時の HEAD で再実行: `bash scripts/dev/test-affected.sh` → exit 0。lifecycle 試験・repro 試験・`test-affected-list.sh`・`test-cli-binary-preparation.sh`・`test-parallel-fail-names.sh` → いずれも exit 0。
  `cargo fmt --all -- --check` → exit 0。`git diff $CELERIS_WU_BASE -- crates` は空なので clippy は対象外（実行していない）。`progress-index.sh --check` → exit 0。

## pin-wire-record の再々実行（2 回目の check 不合格）
- 2 回目の `bash scripts/dev/test-affected.sh` は exit 100 だった。落ちたのは `task-dispatch` の
  `dispatcher::tests::work_units::an_interrupted_integration_is_redone_idempotently_after_restart`（Blocked が Done にならない）。
  `crates/` に差分はなく、scripts/dev の変更とは関係しない。変更なしの再実行で通ったため、負荷起因の flake と判断した（原因の特定は未確認）。
- 提出時の HEAD で再実行: `bash scripts/dev/test-affected.sh` → exit 0。`git diff $CELERIS_WU_BASE -- crates` は空。

## pin-wire-record の再々実行（sandbox では userns 試験が走らない）
- 提出時の HEAD で `bash scripts/dev/test-affected.sh` を 2 回走らせ、どちらも exit 100 だった。落ちたのは同じ 1 件
  （`celeris-campaign::launcher a_swapped_daemon_binary_is_rejected_on_every_path_and_stop_only_touches_the_launcher_pid`、約 1 秒で失敗）で、他の 3540 件は通った。
- この試験を単体で `cargo nextest run -p celeris-campaign --test launcher a_swapped_daemon` と走らせると通る。全体の並走の中だけで落ちる。
  worker sandbox では userns を要する試験が走らない（または不安定）ためと見ており、原因の特定は未確認。
- `scripts/dev/test-affected.sh` 自体を変えると「検証の道具の変更」として celeris-script とその逆依存（campaign を含む）が選ばれる。
  このため、scripts/dev だけの変更でも `test-affected.sh` の check は sandbox でこの試験に当たる。この unit の変更とは関係しない。
- この試験は WU の check に入れない。userns を使える環境（release gate）で確かめる。

## pin-wire-record の再計画 v3（test-affected.sh を引数なしで実行しない）
- この unit では `bash scripts/dev/test-affected.sh` を引数なしで実行しない。`test-affected.sh` 自体を変える差分は検証の道具の変更として
  celeris-script の逆依存 crate を選び、worker sandbox の userns 制約で `celeris-campaign` の
  `a_swapped_daemon_binary_is_rejected_on_every_path…` が落ちる（単体では通る）。この unit の変更とは関係しない。
- 代わりに `bash scripts/dev/test-affected.sh --list` で対象を確認し、lifecycle 試験・repro 試験・`test-affected-list.sh`・`test-cli-binary-preparation.sh`・
  `test-parallel-fail-names.sh` を個別に実行した（結果は下の実行ログのとおり、提出時の HEAD で実行）。`crates/` に差分はなく、clippy・fmt は Rust 差分がないので省いた。
- 未解決: 引数なしの `bash scripts/dev/test-affected.sh` は release gate と sandbox 外の操作に回す。sandbox では userns 試験が走らない。

## verify-rerun: test-affected.sh の再実行（提出時 HEAD で実行）

- 実行: `bash scripts/dev/test-affected.sh` を foreground で 1 回。exit 0。出力の末尾は「test-affected: 試験対象の変更なし（docs のみ等）」。この WU の base（`$CELERIS_WU_BASE`）と HEAD が同じで差分が無いため、cargo の試験は 0 件で、前回 check で落ちた launcher 試験は走っていない。
- そのため `cargo nextest run -p celeris-campaign --test launcher a_swapped_daemon_binary` の単独実行は行っていない（今回は失敗が無く、対象外）。
- flaky の疑い（原因は未確認）: 前回 check で `crates/campaign/tests/launcher.rs` の `a_swapped_daemon_binary_is_rejected_on_every_path_and_stop_only_touches_the_launcher_pid` だけが落ちた。メッセージは「pid … is not the daemon this launcher started (starttime/cmdline/image differ)」で daemon が healthy になる前に終了していた。この試験は `scripts/dev` の変更とは無関係で、再現と原因の切り分けはしていない。
- 範囲外のため未変更: `crates/` 以下の Rust、worker、release gate は変更していない。この WU の変更は本進捗ファイルだけ。
- 未解決: 上記 launcher 試験の flaky 原因の調査（別 task で扱う）。最終 check は main 取り込み後の HEAD で再実行が要る。

## record-flaky: launcher 試験の flaky の記録

### 未解決
- `crates/campaign/tests/launcher.rs` の `a_swapped_daemon_binary_is_rejected_on_every_path_and_stop_only_touches_the_launcher_pid` が、`bash scripts/dev/test-affected.sh` の全体実行（引数なし、約 3541 件の並走）で落ちた。同じ試験を `cargo nextest run -p celeris-campaign --test launcher a_swapped_daemon` で単独実行すると 1 passed だった。
- 失敗時のメッセージ: `start: measure-launcher: pid … is not the daemon this launcher started (starttime/cmdline/image differ): not signalling` と `daemon exited before becoming healthy`（launcher.rs:598）。daemon が healthy になる前に終了している。
- 原因は未確認。並走の負荷、sandbox の userns 制限、試験内の時間依存のどれかは切り分けていない。この task の範囲外で、Rust は変更していない。`scripts/dev` の一時領域管理とも関係は確認していない。

### 提案
- 別 task で、この launcher 試験の flaky を調べて直す。手順の案は次のとおり。
  1. 負荷を焼かず、`SIGSTOP`/`SIGCONT` や注入した遅延で daemon 起動前後の競合を決定的に再現する（`agent-docs/guides/testing.md`）。
  2. daemon の終了理由を `daemon.log` から読み、待ちを「出来事待ち」に直す。
  3. 直したら、単独実行と全体実行の両方で確かめる。

### 検査（提出時の HEAD で実行。最後の同期の後には再実行していない）
- `bash scripts/dev/tests/test-runner-tmp-lifecycle.sh && bash scripts/dev/tests/test-runner-tmp-repro.sh`: exit 0。
- `sh scripts/dev/tests/test-affected-list.sh && bash scripts/dev/test-cli-binary-preparation.sh && bash scripts/dev/tests/test-parallel-fail-names.sh`: exit 0。
- `git diff $CELERIS_WU_BASE -- crates scripts`: 差分なし。

## verify-record: 統合後の検査と最終記録（2026-11-07）

基準: `git merge-base HEAD main`（WU では `$CELERIS_WU_BASE`）。以下は、この WU の提出時の HEAD（`$CELERIS_WU_BASE` と同じ内容。コード差分なし）で実行した。コードは追加していない。

### 受け入れ条件ごとの証拠
- 条件: 両 runner の回帰試験が通る。コマンド: `bash scripts/dev/tests/test-runner-tmp-lifecycle.sh && bash scripts/dev/tests/test-runner-tmp-repro.sh`。結果: exit 0。lifecycle は `ok（188 checks）`、repro は `ok（18 case が期待表と一致。一時 dir が残るのは SIGKILL の 4 case。子が使用中の TMPDIR を trap が消した case: 0）`。
- 条件: 既存の軽量試験が通る。コマンド: `sh scripts/dev/tests/test-affected-list.sh && bash scripts/dev/test-cli-binary-preparation.sh && bash scripts/dev/tests/test-parallel-fail-names.sh`。結果: 3 本とも exit 0。
- 条件: 必須検査が通る。コマンド: `bash scripts/dev/test-affected.sh`（exit 0。`試験対象の変更なし（docs のみ等）`、check-doc-links の self-test も ok）、`cargo clippy --workspace -- -D warnings`（exit 0）、`cargo fmt --all -- --check`（exit 0）。base と HEAD に差分が無いため test-affected の cargo 試験は 0 件で、launcher 試験は走っていない。
- 条件: 文書検査が通る。コマンド: `sh scripts/dev/check-doc-links.sh`、`sh scripts/dev/check-adr-numbers.sh`、`sh scripts/dev/progress-index.sh --check`。結果: いずれも exit 0（この進捗ファイルの更新後に再実行した結果は、最終報告に書く）。

### ADR と実装の一致（読んで確認した範囲）
`scripts/dev/lib/test-tmp-lifecycle.sh` を ADR の D1〜D5 と突き合わせた。一致した点: TMPDIR 上限 47 byte（`TTL_BUDGET`）、候補順 `CELERIS_TEST_TMP_ROOT` → `/local/celeris/data/scratch/tr-<uid>` → `/tmp/celeris-tr-<uid>`、root の 0700・自分の UID・symlink 不可の検査、`.init-XXXXXX` から公開 dir へ rename、`.sweep.lock` の flock、`.trash-XXXXXX` への rename 後の削除、1 時間超の `.init-*` だけ回収、`setsid` による別 session と signal 後 30 秒（`TTL_SIGNAL_WAIT`）の待ち、両 runner からの利用（`test-affected.sh`・`test-parallel.sh`）。ADR の状態欄は「採用（設計）」のままで、実装済みの付記を追加した。

### 原因の確定範囲
- 確定: 旧い runner の一時 dir は、通常終了と TERM・INT・HUP では消え、SIGKILL では残る。決定的 check の timeout が group へ SIGKILL を送る経路（`crates/task-worker/src/workspace.rs`）で残る。この挙動は再現試験で固定した。
- 未確定: 上の「未解決」の 4 点（Bash 道具の process group、test-parallel の残骸 15 件の経路、trap が早く走る理由、決定的 check の namespace からの scratch 可視性）は確認していない。

### 残る限界
- SIGKILL は捕捉できない。SIGKILL で止まった実行の dir は、その場では消えず、次に同じ UID で runner が起動したときの回収（起動時・終了時の走査）で消える。次の起動が無い host では残る。
- fd を閉じて別 session へ抜けた子孫は、flock と session の走査で見えず、使用中の dir を消されることがある。
- 旧 prefix の残骸は自動回収できない（理由は「本番への移行」）。
- 引数なしの `test-affected.sh` は、この WU では base と差分が無く、cargo 試験を走らせていない。先に落ちた launcher 試験の flaky は、別 WU の修正（`launcher 起動直後の exec 完了を待ってから identity 照合する`）が入っているが、この WU では再確認していない。

### commit（件名）
この task の主な commit 件名: `phase 1: 試験 runner の一時領域の signal 終了を再現する試験と寿命管理の ADR`、`phase 2: 試験 runner の一時領域の共通 helper と単体試験`、`phase 3: test-affected.sh と test-parallel.sh を一時領域の共通 helper に置き換える`、`phase 3: repro 試験が helper 導入前の runner の固定した写しを使うようにする`、`phase 4: 一時領域の回帰試験を test-affected.sh の scripts/dev 検査へ配線し再入を防ぐ`、`fix-launcher-flaky: launcher 起動直後の exec 完了を待ってから identity 照合する`。この WU の commit 件名は `verify-record: 統合後の検査結果と本番への移行手順を進捗に記録する`。

### 提案（追加）
- daemon の決定的 check の timeout を SIGTERM → grace → SIGKILL の 2 段にする案は、上の「提案」のとおり別 task で判断する（設計変更なので、ここでは実装しない）。
- release gate の後に、`tr-<uid>` 配下の残存数を見る検査を足すかは、人が移行手順の運用結果を見て決める。
