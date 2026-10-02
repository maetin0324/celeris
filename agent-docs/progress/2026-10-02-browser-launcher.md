---
title: browser: ADR-0115 権限分離 launcher の実装と実 process 実証
tasks: [01M3VFQK2ZSPJ89VDA1F4FMA2G]
status: done
updated: 2026-10-02
---
# browser: ADR-0115 権限分離 launcher の実装と実 process 実証

> 旧 `docs/PROGRESS.md` の節を ADR-0128 D6 に従い land-verify（task 01M3YBGM64RYPEY9NZANF79A0M）がここへ移した。本文は元のまま（リンクだけ新配置へ直した）。

## browser: ADR-0115 権限分離 launcher

run `01M3X8SRB3X08AXW8WK5PY7P9N` で launcher 実装・設定・host unit/手順書を統合後に検査。`cargo fmt --all -- --check` と `cargo clippy --workspace -- -D warnings` は exit 0。workspace test は sandbox の user namespace probe が `EPERM` となり、ADR-0095 DB guard を使う `instance_handoff` 5 件が失敗して exit 101（`CELERIS_ISOLATION_TESTS=skip` を付けても同じ。skip は browser isolation 試験だけに適用）。launcher ptrace 試験は `celeris-browser` user と `celeris-browser-launcher.socket` が存在しないため `SKIPPED (not passed)`。host 管理者に [browser-launcher-host-setup.md](../../docs/ops/browser-launcher-host-setup.md) の準備を依頼し、準備後に実 process 証跡を追加する。機密能力は未解放。本節の詳細は [phase-browser-4](phase-browser-4.md)。

- 2026-10-02: phase3 control flaky 再実行は ADR-0095 の user namespace 拒否で `cargo test --workspace` と `api_scenarios` が失敗、clippy は pass。詳細は [phase-R.md](phase-R.md)。

## browser: ptrace 境界分離 launcher 実装・実 process 実証完了

完了日 2026-10-02（task 01M3VFQK2ZSPJ89VDA1F4FMA2G、WorkUnit `real-evidence` / `close-out`）。

- 成果: [ADR-0115](../adr/0115-browser-ptrace-owner-ns-launcher.md) に沿って `celeris-browser-launcher` を実装し、人が host 準備手順（[browser-launcher-host-setup.md](../../docs/ops/browser-launcher-host-setup.md)）どおりに `celeris-browser` user・subuid/subgid・systemd socket/service を整えた環境で、launcher 経由の実 Chrome について daemon UID からの ptrace・`/proc` 読取り拒否を実証した。既存の daemon 所有 runtime（same-uid / subuid wrapper）経路は維持し、launcher 経由は設定（`[browser] runtime` / `launcher_socket`）で選択する。
- 証拠（host の人の実行、詳細は [phase-browser-4.md](phase-browser-4.md) の該当 run 記録）:
  - `CELERIS_LAUNCHER_TESTS=require cargo test -p task-worker --test browser_launcher_ptrace -- --nocapture` → **exit 0（1 passed、0 failed、0.15 s）**。
  - 正の対照: 同 UID 子プロセスへの `PTRACE_ATTACH=0`／`PTRACE_DETACH=0`（検査手段自体が機能することの確認）。
  - launcher 観測・Chrome: `owner=Some(296608)`（Chrome userns owner、subuid）、`NS_GET_OWNER_UID(launcher)=Some(296608)`、daemon UID からの同 NS open は `errno=Some(13)`（EACCES）。
  - `verify_isolation=Ok (launcher isolation_ok=true, CapEff=0000000000000000 NoNewPrivs=true)`。
  - daemon UID 1001 の別プロセスからの攻撃: `PTRACE_ATTACH Chrome pid: errno=Some(1)`（EPERM）、`strace -p` は `exit=Some(1)`／`Operation not permitted`、`/proc/<pid>/environ`・`/proc/<pid>/mem` ともに `errno=Some(13)`（EACCES）。
  - check runner（db_guard namespace、`/proc/self/uid_map`=`1001 1001 1`）からの再実証でも同じ構成で **exit 0（1 passed、0.14 s）**。namespace 越しに subuid が `4294967295` と写る分だけ試験側の map 判定を修正済み（launcher 側の不具合ではない）。
  - 既存の daemon 所有 runtime 経路: `cargo test -p task-worker --test browser_runtime_isolated` → **exit 0（4 passed、1 ignored）**、壊れていない。
  - `cargo test --workspace && cargo clippy --workspace -- -D warnings` → exit 0（close-out run、下記参照）。
- 手順書・証跡リンク: host 準備は [docs/ops/browser-launcher-host-setup.md](../../docs/ops/browser-launcher-host-setup.md)、試行錯誤と各 run の journal・eprintln 全文は [agent-docs/progress/phase-browser-4.md](phase-browser-4.md)（ADR-0115 launcher 節以降）。
- 未解決事項:
  - この host は LXC 内のため、試験 runner（UID 1001）の親 namespace map は初期 namespace の `0 0 4294967295` ではなく `0 100000 1001 / 1001 1001 1 / 1002 101002 64534 / 65536 165536 262144`。検証は db_guard namespace の外・この map のもとで行った。
  - Chrome stderr の `category=other-startup-error`（journal に 4 行）の中身（無害な起動時警告か）は未確認。
- 機密能力（`CredentialInjection`・`IdentityRestore`）はこの task では解放していない。解放は後続 task `01M3VFQZ2TX3W0KTDQHKCAVJR6` / `01M3WV4BFJ71J9ZWJ020MP2Z4K` で判断する。

#### 統合検査 flaky の単独再実行（tick_prunes）

- 対象: `dispatcher::tests::cleanup_and_disk::tick_prunes_the_oldest_terminal_workspace_and_records_an_event`。各回の直前に `/proc/loadavg` を読み、`cargo test -p task-dispatch --lib tick_prunes_the_oldest_terminal_workspace_and_records_an_event` を個別に foreground 実行した。
- 1回目: loadavg `24.22 24.05 24.86 28/1583 3`、exit 0、`1 passed; 0 failed`（498 filtered out）。
- 2回目: loadavg `24.33 25.07 25.22 3/1496 3`、exit 0、`1 passed; 0 failed`（498 filtered out）。
- 3回目: loadavg `21.29 24.39 25.00 2/1427 3`、exit 0、`1 passed; 0 failed`（498 filtered out）。
- 先行する `cargo test --workspace` は load 25 前後で同じ試験が失敗し、`498 passed; 1 failed` だった。人は環境（host 高負荷）起因の tick 依存 flaky と判断した。今回の3回はすべて pass。指示どおりコードは変更せず、修正は別 task `01M3Y4AV5Z` が担当する。

### 最新 main（7f3482a3）取り込み — 2026-10-02（work unit `land-main`）

`main` の `7f3482a3` を merge で取り込んだ（rebase なし）。衝突は 2 ファイル。
- `crates/task-worker/tests/browser_shared_cdp.rs`: main 側 5eb666f6（R7-12）の `ISOLATION`・`PREFLIGHT_TIMEOUT`・`probe.err` の excepthook・WebSocket frame の読み切り（`recv_exact`）を土台にした。そこへこのブランチの 97eb5194（接続から CDP 応答までを 1 試行とし、期限 55 秒の中で再試行する）を載せた。`find_browser`・`preflight`・`run_bounded` と、launcher 用の `userns: UsernsMode::Unshare` は自動 merge でそのまま残っている。
- `docs/PROGRESS.md`: main の R7-10・ADR-0117・repair 許可範囲の節を先に、この節を含む launcher 節を後に置き、どちらも残した。
- 証拠: `cargo build -p task-worker --bins` exit 0。`cargo fmt --all -- --check` exit 0。`cargo clippy --workspace -- -D warnings` exit 0。`cargo test --workspace` exit 0（3261 passed / 0 failed / 12 ignored）。`CELERIS_ISOLATION_TESTS=require cargo test -p task-worker --test browser_shared_cdp` を 3 回実行し、3 回とも exit 0（2 passed）。flaky は出なかった。launcher の実 host 試験は再実行していない（人の側で済み）。

### main（764a737d）取り込み — 2026-10-02（work unit `land-main2`）

`main` の `764a737d` を merge で取り込んだ（rebase なし）。衝突は 2 ファイル。
- `crates/task-worker/src/browser_runtime.rs`: main 側 c11ffd35 の `RuntimeError::InitNotReady` と、`IsolatedRuntime::launch` で `--info-fd` の後に pid ns init の starttime を記録して `/proc/<pid>/wchan` が `do_wait` になるまで待ち、未完了なら本人確認のうえ init を SIGKILL する処理を残した。このブランチ側の `RelayNotReady(String)`・`NoChildPid(failed_stderr)` の診断、launcher 経路（`UsernsMode::Fd`、`/tmp/celeris-session` の bind、relay 診断、Chrome lifecycle 診断）もそのまま残した。
- launcher 経路での init 待ち: launcher は bwrap の親として host の pid ns にいるので `--info-fd` の pid は host pid。init の userns は launcher（euid celeris-browser）が owner の userns の子孫なので、launcher は init の `/proc/<pid>/stat`・`wchan` を読め、SIGKILL も送れる。このため launcher 経路の扱いは変えていない（理由をコードのコメントにも書いた）。unit（`deploy/systemd/celeris-browser-launcher.service`）に `ProtectProc`・`PrivatePIDs` は無く、他 UID の `/proc` は見える。
- `tests/browser_runtime_isolated.rs` の main 側変更は自動 merge で残り、launcher 用の `userns: UsernsMode::Unshare` も残っている。
- `docs/PROGRESS.md`: main の verify-land2・land-final・reland-main・rerun-flaky などの節を先に、launcher 節を後に置き、どちらも残した。
- 証拠: `cargo build -p task-worker --bins` exit 0。`cargo fmt --all -- --check` exit 0。`cargo clippy --workspace -- -D warnings` exit 0。`cargo test --workspace` exit 0（3279 passed / 0 failed / 12 ignored）。`cargo test -p task-worker --test browser_runtime_isolated` exit 0（5 passed、1 ignored）。flaky は出なかった。
- **launcher の binary に効く変更あり**: `browser_runtime.rs` の launch（init 待ち）は launcher・sandboxd・egress の binary に入る。host の binary は 86ce1a88 のビルドのままなので、反映には人による入れ替えが要る。
- main 取り込み後の launcher binary の実 host 再試験は未実施（任意で人が require 試験を再実行）。手順は `CELERIS_LAUNCHER_TESTS=require cargo test -p task-worker --test browser_launcher_ptrace -- --nocapture`（binary の入れ替えは `sudo /usr/local/sbin/celeris-browser-launcher-update`）。

### land-main3: 最新 main の統合 — 2026-10-02

main `0d438ec19d9a` を merge し、`docs/PROGRESS.md` の両側の節を保持した。main の ADR-0122 完了・ui-ux 外部 skill の結合試験・planner 指針・最終検査記録に加え、browser launcher の実 process 証跡、tick_prunes の単独再実行、過去の land-main/land-main2 記録も残した。

- main 由来の launcher 関連差分を確認: `crates/task-worker/src/browser_runtime.rs` は main 側の init 待ち変更を含み、launcher/sandboxd/egress の起動経路に効く。この変更は既に land-main2 の記録に記載済みで、host の binary 入れ替えと require 試験の再実行が必要。
- `git merge-base --is-ancestor 0d438ec19d9a HEAD` → exit 0。`git merge-tree --write-tree main HEAD` → exit 0（tree `d7c0705a6e14f6dc89fbd842b679f4078c084bd5`）。main の ADR-0122 / ui-ux 記録と launcher 節は両方保持。
- 最終検査: `cargo fmt --all -- --check` → exit 0。`cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo test --workspace` → exit 101。`instance_handoff` 8件中3 passed / 5 failed。`cargo test -p celeris --test instance_handoff` 単独再実行も exit 101、同じ5件を再現。3件は ADR-0095 worker db guard の user namespace 作成が `Operation not permitted` で失敗。残り2件（新旧 daemon の dispatch/standby 引継ぎ）も同じ環境で失敗した。検査は pass 扱いにしない。
- launcher binary に効く main 差分は `crates/task-worker/src/browser_runtime.rs` の init 起動待ち処理である。既存の記録どおり host の binary 入れ替えと require 試験の再実行が必要。

### pick-chrome: 並走 session での launcher Chrome 特定 — 2026-10-02

`browser_launcher_ptrace.rs` の Chrome 特定が並走 session で曖昧になって落ちていた件を、試験 file だけで直した。launcher は daemon から読める `/proc` に session の印を出さないため、launcher 子孫の新しい Chrome 候補を全部検査して 1 件以上を要求し、自分の session の停止で検査済みの session root が消えることを確かめる。選択は純粋な関数に分け、単体試験 `chrome_pick_*` 4 件を足した。詳細は `agent-docs/progress/phase-browser-4.md`『並走 session での Chrome 特定』。

- `cargo test -p task-worker --test browser_launcher_ptrace -- --nocapture` → exit 0（5 passed、実 launcher 試験も実行）。
- `cargo clippy -p task-worker --all-targets -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `crates/task-worker/src/` は不変（host の binary 入れ替え不要）。
