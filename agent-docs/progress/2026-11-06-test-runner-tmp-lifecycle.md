---
title: signal 終了を再現し、試験 runner の一時領域の寿命管理を設計する
tasks: [01M6S9KKWT7EM7FP5CCG1Q8DH0]
status: done
updated: 2026-11-06
---
# signal 終了を再現し、試験 runner の一時領域の寿命管理を設計する

完了日: 2026-11-06（設計 unit。実装は後続の unit）

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
- 旧い残骸（`/tmp/celeris-test-affected.*`・`/tmp/celeris-test-parallel.*`）は人が消す。手順は ADR の D6 にある。agent は消していない。

## 提案
- daemon の決定的 check の timeout を、SIGKILL だけでなく SIGTERM → grace → SIGKILL の 2 段にする（`crates/task-worker/src/workspace.rs` の `exec` と `GroupKillGuard`）。
  runner の trap が後片付けできる機会が増える。別の task で判断する。
- 実装の unit は、ADR の D1〜D5 を共通 helper にまとめ、両 runner を切り替える。
  その際に `test-runner-tmp-repro.sh` の期待表を新しい挙動へ書き換える。新しい挙動では、SIGKILL の後の次回起動で回収されること、子の使用中は消さないことを確かめる。
