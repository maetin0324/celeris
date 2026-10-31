---
title: ops-r2-sandbox の socket bind と K2 preflight 手順書の修正
tasks: [01M690TDXQ4GX9A2Z0AVBKTB2D]
status: done
started: 2026-10-31
updated: 2026-10-31
completed: 2026-10-31
---

# ops-r2-sandbox の socket bind と K2 preflight 手順書の修正

## やったこと
- `scripts/campaign/ops-r2-sandbox.sh` に `--socket <path>`（複数可。`test -S` の path だけ、socket の path そのものを `--bind`、親 dir は見せない）。
- preflight の `--b-socket`・`--operator-socket`（`ScriptSandbox.sockets`）。preflight の許可表（`allow_sources`）は `--bind` の host path の mount を含むので socket も許可表に入る。
- ADR `agent-docs/adr/2026-10-31-ops-r2-preflight-fixes.md` に付記 D-socket・D-docs（手順書と道具の食い違い 5 点。すべて道具に合わせた）。
- 手順書 `docs/ops/ops-r2-k2-preflight.md`・`docs/ops/ops-r2-isolation.md` を修正。
- 試験 `crates/campaign/tests/ops_r2_socket.rs`、`crates/campaign/tests/ops_r2_probe.rs` の fixture に試験用 socket の bind を追加。

commit（件名）:
- ops-r2-sandbox: --socket で unix socket を 1 つずつ bind（preflight の --b-socket/--operator-socket）
- ops-r2 手順書: --kb-src と固定初期 KB・deny list の置き場所・--dev の allow・run uid・ssh の userns 制約
- ops_r2_probe 試験: 試験用 socket を --socket で bind し probe・preflight が mounts_outside_allow にせず deny_readable が変わらないことを確かめる

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）
- `CELERIS_USERNS_TESTS=1 cargo nextest run -p celeris-campaign --test ops_r2_probe --test ops_r2_socket`: 6 passed、skip 0 件。ops_r2_probe の fixture は試験用 socket を `--socket` で bind し、CoS run・worker run の probe で mounts_outside_allow 0 件、deny_readable は control の 1 件だけ（従来と同じ）。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/test-affected.sh`: exit 0。nextest 2912 tests run、2912 passed、9 skipped。文書検査（地図・リンク・ADR 番号・human-text・進捗 front matter）も通過。

## 本番への移行
release 後に operator が実行する（測定 daemon は起動しない）。

```sh
MUX=/run/user/1001/ssh-mux-rmaeda@130.158.241.2:22
SSHCFG_DIR=/local/celeris/data/measure-bin/ssh
sh scripts/campaign/ops-r2-sandbox.sh --state "$S" --release "$R" --account /local/celeris/measure-accounts/b \
  --ro "$SSHCFG_DIR" --socket "$MUX" -- \
  /usr/bin/ssh -F "$SSHCFG_DIR/config" -o BatchMode=yes -o ConnectTimeout=10 sirius 'echo ok; command -v qsub qstat'; echo "exit=$?"
```
期待: exit=0、`ok` と qsub・qstat の path。続けて `docs/ops/ops-r2-k2-preflight.md` §3 の preflight を `--b-ro "$SSHCFG_DIR" --b-socket "$MUX" --kb-src "$KB0" --deny-list "$DL"` 付きで、
§4 の B の probe を同じ bind 集合（`--ro "$SSHCFG_DIR" --socket "$MUX"`、`--allow` は /dev の下を含む新しい snippet）でやり直し、`missing` 0 件・probe exit 0・漏洩検査 `pass: true` を steps に残す。

## 未解決事項
- preflight 経由で socket の bind が `mounts_outside_allow` に出ないことの userns 試験（ops_r2_probe.rs の fixture に socket を足す）は未完（未確認）。

## 提案
なし。
