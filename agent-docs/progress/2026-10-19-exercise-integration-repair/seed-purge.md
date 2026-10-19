---
task: 01M5ANK5TMYWWH537DPGKYSE4S
wu: seed-purge-proc
status: done
completed: 2026-10-19
---

# seed-purge: 旧方式の scratch seed を除去する人の手順

本番 host の操作なので人が実行する（worker は実行しない）。対象は旧方式（`.building-gen-*/target` で build し、workspace member の成果物と seed の checkout の絶対 path を owner へ写す）で作られた seed。症状は reflink 先での `CARGO_BIN_EXE` NotFound、seed の checkout の file を読む試験（`CARGO_MANIFEST_DIR` が `seeds/<key>/checkout/...`）。

前提: scratch root は `/local/celeris/data/scratch`、repo key は `agent-platform-f1d3fe5cc3`、旧 seed は `gen-254c885a921e-1792429644204941549`。新方式の release が本番へ昇格済みであること（手順 5 の前に確認）。

## 1. 旧 seed の特定
```
S=/local/celeris/data/scratch
ls -la $S/seeds/agent-platform-f1d3fe5cc3/ $S/seeds.broken-20261019/agent-platform-f1d3fe5cc3/ 2>&1
readlink $S/seeds/agent-platform-f1d3fe5cc3/current
```
期待: `gen-254c885a921e-1792429644204941549` が `seeds.broken-20261019/` 側にあり、`seeds/` 側の `current` から指されていない（`seeds/` に無い、または `current` が無い）。`.building-*` が残っていれば旧形式。

## 2. seeds/<key>/current の退避（まだ `seeds/` 側に旧 seed がある場合のみ）
```
systemctl --user is-active celeris@*        # seed_reflink=false 反映後なら refresh は走らない
mkdir -p $S/seeds.broken-20261019
mv $S/seeds/agent-platform-f1d3fe5cc3 $S/seeds.broken-20261019/
test ! -e $S/seeds/agent-platform-f1d3fe5cc3 && echo moved
```
期待: `moved`。退避で seed の checkout が owner から見えなくなる。

## 3. 旧 seed 由来の owner target の確認
```
grep -rlE '\.building-gen-254c885a921e|seeds/agent-platform-f1d3fe5cc3/(checkout|gen-)' \
  $S/targets/*/debug/deps $S/targets/*/debug/.fingerprint 2>/dev/null | head
```
期待: 出力なし。出た target は丸ごと消す（`rm -rf <target>`。cargo が owner の path で作り直す。DB・events は触らない）。確認は `strings <binary> | grep -c 'seeds/agent-platform-f1d3fe5cc3'` が 0。

## 4. 退避した seed の削除
手順 3 が空になり、統合 check が通ったのを確かめてから:
```
rm -rf $S/seeds.broken-20261019
```

## 5. 新 seed の再 build 確認
`~/.config/celeris` の `[scratch] seed_reflink = true` に戻し、daemon を新 release に再起動した後（housekeeping が次回 tick で refresh する）:
```
ls -la $S/seeds/agent-platform-f1d3fe5cc3/
readlink $S/seeds/agent-platform-f1d3fe5cc3/current
cat $S/seeds/agent-platform-f1d3fe5cc3/current/manifest.json
find $S/seeds -maxdepth 3 -name '.building-*'
strings $S/seeds/agent-platform-f1d3fe5cc3/current/target/debug/celerisctl | grep -c '\.building-'
```
期待: `current` が新しい `gen-<commit12>-<nanos>` を指し、その下に `target/` と `manifest.json` がある。`.building-*` は 0 件。`strings` の出力も 0（binary は最終 path `gen-*/target` を焼く）。旧 `gen-254c885a921e-…` は現れない。

## 6. owner 側の確認
新しい owner の target（seed から写されたもの）で:
```
ls $S/targets/<owner>/debug/deps | grep -c '^celerisctl-'   # workspace member の成果物は写されない: 0
```
その owner の worktree で `cargo nextest run -p celerisctl` を流し、NotFound なしで通ること。試験 binary を `strings` して `seeds/agent-platform-f1d3fe5cc3` が 0 件、owner の worktree path が含まれること。
