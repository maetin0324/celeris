---
title: "browser A13: docs の判定欄を launcher 方式に直し、broker の別 UID 実 process 試験を launcher 経路で作る"
tasks: [01M4NV0KJTQYJQXXY4YTJM3NQN]
status: done
updated: 2026-10-11
---

# browser A13: docs の判定欄を launcher 方式に直し、broker の別 UID 実 process 試験を launcher 経路で作る

## 現在地

2026-10-11 完了（worker 側）。設計は [ADR 2026-10-11](../adr/2026-10-11-browser-a13-broker-peer-uid-real-process.md)。
host での 1 回の実行（`sudo -u celeris-browser` が要る）は人が行う（下の「本番 host での実行」）。本番 config・DB の変更は無い。

## やったこと

1. 試験 `crates/celeris-credentiald/tests/injection_peer_uid.rs` を追加。
   - `real_process_with_another_uid_is_rejected_by_dac_then_peer_uid_mismatch`: 本番と同じ `serve_attested`
     （launcher UID 付き）の実 broker を一時 runtime に起こし、(a) 正の対照（同 UID → `injection_worker_not_allowed`）、
     (b) `sudo -n -u celeris-browser` で起こした実 process（test binary を `--exact inner_peer_uid_probe` で再実行、client は
     本番と同じ `injection_ipc::call`）が本番 permission では connect `EACCES`（dac 層、journal に無い）、(c) 試験が dir 0711・
     socket 0666 に緩めると broker の `SO_PEERCRED` 照合が `peer_uid_mismatch`（broker 層、journal に `peer_uid=995` の拒否 1 行）。
   - 別 UID を用意できない環境では理由を stderr に出して skip（`SKIP: (not passed) …`）。`CELERIS_LAUNCHER_TESTS=require` では失敗。
     既定は `CELERIS_USERNS_TESTS=1` の opt-in（ADR-0126 B）。
   - `inner_peer_uid_probe`（別 UID の子の側。環境変数が無ければ no-op）、`probe_line_round_trips`、
     `same_uid_control_is_worker_not_allowed`（正の対照の単体）。
2. docs: 追跡表 `phase-browser-acceptance.md` の A13 を「合格」にし、ptrace 拒否（`launcher_chrome_denies_daemon_uid_ptrace`）と
   broker の別 UID 拒否（上の試験）を引用。P4-B-4・P4-B-6 の A13 言及、`browser-followups.md` の a13-real-process 行、
   `phase-browser-4.md` の状態行 2 箇所、`PROGRESS.md` の 2026-09-30 の記録 1 行を「01M3SPN8HE は subuid 経路のため破棄（2026-10-11）」に直した。
   ADR-0138「検証と本番反映」に試験の所在を付記。ADR-0098・phase-R.md の task 起票の記録（履歴）は残した。

## 証拠

- `TMPDIR=/tmp cargo test -p celeris-credentiald --test injection_peer_uid -- --nocapture`: exit 0、4 passed（opt-in 無しの既定 skip）。
- `TMPDIR=/tmp CELERIS_USERNS_TESTS=1 cargo test -p celeris-credentiald --test injection_peer_uid real_process -- --nocapture`: exit 0、
  `SKIP: (not passed) cannot run a process as celeris-browser: sudo -n -u celeris-browser true exit=Some(1): sudo: … The "no new privileges" flag is set …`。
  この worker sandbox は `/proc/self/uid_map` が `1001 1001 1` の 1 行で別 host UID が存在せず、`sudo`・`newuidmap` も no_new_privs で使えない。
- 全体検査（`bash scripts/dev/test-parallel.sh`・`cargo clippy --workspace -- -D warnings`）の結果は下の節。

## 本番 host での実行（人が行う）

worker sandbox では別 UID を作れないので、host で 1 回実行して証跡をこの節に貼る。本番 daemon・config・DB には触れない
（試験は一時 dir に自前の broker を起こす）。

```sh
cd ~/workspace/agent-platform   # この task の commit を含む tree
sudo -v                          # NOPASSWD でなければ先に資格を通す（試験は sudo -n を使う）
TMPDIR=/tmp CELERIS_LAUNCHER_TESTS=require \
  cargo test -p celeris-credentiald --test injection_peer_uid -- --nocapture --test-threads=1
```

期待する出力（stderr）:

```
PEER-UID control uid=1001 Reply { code: Some("injection_worker_not_allowed") }
PEER-UID dac uid=995 ConnectFailed { errno: Some(13) }
PEER-UID broker uid=995 Reply { code: Some("peer_uid_mismatch") }
PEER-UID result dac=connect_errno=13 broker=peer_uid_mismatch (peer uid 995, broker uid 1001)
test real_process_with_another_uid_is_rejected_by_dac_then_peer_uid_mismatch ... ok
```

`sudo -n -u celeris-browser` が通らない（password・sudoers）と `require` で panic する。その場合は `sudo -v` の後に再実行するか、
`CELERIS_PEER_UID_USER=<別の local user>` を与える。

- 実行記録: （未実施。実行したら日付・exit code・上の 4 行を貼る）

## 未解決事項

- host での実行は未実施（上の手順）。
- 追跡表 P4-A-7・P4-B-6・P4-C-6 の後続 task（01M3SPN8H05・01M3SPN8HP）は subuid 経路の時代の起票で、launcher 方式での扱いは別途見直しが要る（この task の範囲外）。

## 提案

- `agent-docs/ops/browser-isolated-runtime-subuid.md`（subuid の bwrap 経路の手順）は launcher 方式に置き換わっているので、
  参照している ADR-0105/0109 の付記ごと整理する task を置く。

## 全体検査

2026-10-11、この task の tree（worker sandbox、`TMPDIR=/tmp`）:

- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh`: exit 0。nextest `5172 tests run: 5172 passed, 13 skipped`、doctest 3 passed
  （summary: passed 5175 / failed 0 / ignored 14）。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt -p celeris-credentiald -- --check`: exit 0。
- `scripts/check-browser-acceptance.sh` の静的部（表の判定・引用テストの実在・file:line）: exit 0（rows 76、tests cited 146、file:line refs 28）。
  引用テスト `grant_check_and_replay_are_bound_and_scrubbed` が commit d8d946e5 で
  `browser_live_frame_authorization_is_bound_and_scrubbed` に改名されたまま表に残っていた（この task 以前の取り残し）ので、同じ変更で直した。
  cargo test 部（22 群）は sandbox では実 browser 試験が完走しないため host で回す。
- `scripts/dev/check-doc-links.sh`・`check-adr-numbers.sh`・`check-doc-layout.sh scripts/dev/docs-layout.tsv`・`check-architecture-map.py`: すべて ok。
- `scripts/dev/check-test-tmp-leftovers.sh`: 既定 TMPDIR（run の長い path）では unix socket の path 長で celeris lib の browser_doctor 試験 2 件が落ちる（既知、ADR-0126 系の fixture）。`TMPDIR=/tmp` の workspace 全体では他 crate の試験が残す dir 6 件で exit 1（この task 以前から。test-parallel の summary でも tmp_leftovers 4）。この task の試験だけ（`-p celeris-credentiald --test injection_peer_uid`）は `no leftovers`、exit 0。既存の `--test injection_ipc` も exit 0。
