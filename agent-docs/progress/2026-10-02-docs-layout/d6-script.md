---
title: D6 の旧進捗ファイル移行台本
tasks: [01M3YBGM64RYPEY9NZANF79A0M]
status: done
updated: 2026-10-03
---
# D6 の旧進捗ファイル移行台本

## 方式

`sh scripts/dev/progress-transition.sh <main-ref>` は、指定した ref の `docs/PROGRESS.md` 全文を `agent-docs/PROGRESS.md` に写し、その先頭に追記終了・新しい記録先・生成索引を示す案内を 1 行だけ加える。旧パスを消して両パスを index に登録するが、commit はしない。merge の衝突解消中にも使える。ref に旧ファイルがなければ何も変えず終了する。

main の旧進捗は task の分岐後に大きく増えたため、以前の短い移動先ファイルを維持すると、後から旧パスへ追記した branch の変更が rename に追従しない。main の全文を新パスの本文にすることで、後続 branch が分岐した版との対応を保つ。ADR-0128 D6 本文の更新は sync-main-3 が担当する。

## 試験結果

- `sh scripts/dev/tests/progress_transition_merge.sh` → exit 0。一時 repo で、base の旧進捗を main が大きく組み替えて追記し、task は移動と案内追加、branch X は main の旧パスへ追記した。
- task に main を merge して台本で解消すると、未解決 index は空、移動先の 2 行目以降は main の全文と一致した。main が task を fast-forward した後の branch X merge は衝突せず、その節は `agent-docs/PROGRESS.md` に入った。
- 台本を使わず task の古い移動先本文を残した場合、同じ branch X の merge は競合した。旧ファイルがない ref では worktree と index が変わらないことも確認した。
