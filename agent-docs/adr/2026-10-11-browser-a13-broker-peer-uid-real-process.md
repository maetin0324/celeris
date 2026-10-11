# ADR: A13 の別 UID 実 process 試験は launcher UID の sudo で起こし、DAC 層と broker 層を分けて判定する

---
tasks: [01M4NV0KJTQYJQXXY4YTJM3NQN]
---

- Date: 2026-10-11
- Status: Accepted
- 関連: [ADR-0109](0109-browser-p4b-injection-ipc-cdp-sink.md) 表 A13、[ADR-0115](0115-browser-ptrace-owner-ns-launcher.md)、
  [ADR-0116](0116-browser-launcher-implementation.md)、[ADR-0138](0138-browser-prod-admission-confidential-release.md)「検証と本番反映」、
  [破壊的変更を是とする決定](2026-10-11-breaking-changes-by-default.md)

## Context

A13（別 host UID の実 process からの injection 要求 → `peer_uid_mismatch`）は、2026-09-30 の追跡表で
「後続 `01M3SPN8HE6A1TZ54GBZGHMZYJ`」のまま止まっていた。その branch（celeris/01M3SPN8HE）は subuid の bwrap 経路
（ADR-0105 の same-uid runtime を別 host UID で起こす前提）で攻撃試験を組んでいたが、その経路は ADR-0115/0116 の
権限分離 launcher（`[browser].launcher_uid = 995`、celeris-browser）に置き換わった。launcher 経路での ptrace 拒否は
main の `crates/task-worker/tests/browser_launcher_ptrace.rs`（`launcher_chrome_denies_daemon_uid_ptrace`）で実 process
実証済み（ADR-0138 状態行）。一方、ADR-0138 は「A13 の broker `peer_uid_mismatch` と ptrace 拒否は別の判定として記録する」
と決めているのに、launcher 経路で別 UID の実 process から broker へ接続して拒否されることを確かめる試験は無かった
（`peer_with_another_uid_is_rejected` は `PeerCred` を与える関数単位）。

broker（celeris-credentiald）の `serve_inner` / `socket` は runtime dir を 0700、`celeris-credentiald/` を 0700、
`injection.sock` を 0600 に固定する。したがって launcher UID の process は本番の permission では `connect()` の時点で
`EACCES` になり、broker の `SO_PEERCRED` 照合（`InjectionService::check_and_inject` step 1）には届かない。
`peer_uid_mismatch` は DAC（filesystem permission）が破られたときの 2 層目である。

## Decision

- D1. 旧 branch celeris/01M3SPN8HE は subuid 経路のため**破棄**し、その試験は戻さない。docs の A13 は launcher 方式の
  記録に直し、01M3SPN8HE への参照は「subuid 経路のため破棄（2026-10-11）」に直す。
- D2. 別 UID の実 process は、設定上の launcher UID（`celeris-browser`。`CELERIS_PEER_UID_USER` で別の local user に替えられる）
  で `sudo -n -u <user>` により試験 binary 自身を `--exact inner_peer_uid_probe` で再実行して作る。client は本番の
  controller と同じ `injection_ipc::call` を使う。`unshare -r` の子は host では同 UID なので別 UID の証拠にしない。
  別 UID を用意できない環境（sudo 不可・user 無し・fixture の path を他 UID が辿れない）では理由を stderr に出して
  skip し、`CELERIS_LAUNCHER_TESTS=require` では skip せず失敗する。既定は ADR-0126 B の opt-in
  （`CELERIS_USERNS_TESTS=1`）が無いと走らない（sudo の試行を CI の既定に入れない）。
- D3. 判定は 2 層に分けて記録する（試験の `PEER-UID` 行）。
  - `dac`: 本番と同じ permission で、別 UID の `connect()` が `EACCES`、broker の journal にその UID の行が無い。
  - `broker`: 試験だけが起動後に dir を 0711・socket を 0666 に緩めて DAC 層を迂回させ、broker 自身の照合が
    `peer_uid_mismatch` を返し、journal に `peer_uid=<別 UID>`・`decision_code=peer_uid_mismatch` が 1 行残る。
    設定上の launcher UID そのものが peer として来ても拒否される（launcher UID は session 証明の照合先であって、
    injection の peer として許される UID ではない）。
  - 正の対照: 同じ要求を broker の UID から送ると `injection_worker_not_allowed`（要求の形は通り、session 未登録で止まる）。
    これで別 UID の拒否を UID の差だけに帰す。
- D4. 試験は `crates/celeris-credentiald/tests/injection_peer_uid.rs` の
  `real_process_with_another_uid_is_rejected_by_dac_then_peer_uid_mismatch`。追跡表 A13 はこれと
  `launcher_chrome_denies_daemon_uid_ptrace` を引用して「合格」にする。
- D5. host での実行は人（運用セッション）が `TMPDIR=/tmp CELERIS_LAUNCHER_TESTS=require` で行い、証跡を
  `agent-docs/progress/2026-10-11-browser-a13-launcher-peer-uid.md` に残す。worker sandbox は uid_map が `1001 1001 1` の
  1 行で別 host UID が存在せず、`sudo`・`newuidmap` も no_new_privs で使えないため、sandbox では skip 理由が出る。

## Consequences

- A13 の「別 UID」は launcher 方式の UID（995）で定義される。subuid の bwrap 経路に依る試験・文書は残さない。
- broker の DAC 層（0700/0600）は本番の第一防御であり、`peer_uid_mismatch` は第二防御として試験で単独に確かめられる。
  将来 socket の permission を緩める変更（例: group 共有）をするなら、この試験の `dac` 層の期待を同じ変更で直す。
- 試験は sudo に依存するため既定では走らない。host での 1 回の実行結果が A13 の証跡になる。
