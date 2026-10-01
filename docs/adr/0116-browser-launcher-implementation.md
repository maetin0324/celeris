# ADR-0116: browser launcher の実装方針（IPC protocol・userns 生成・設定切替）

---
tasks: [01M3WW2RBB9QW9NPN862TZEK9P]
---

- 日付: 2026-10-01
- 状態: 採用（実装方針。実装・host 準備・実 process 試験は後続の作業単位）
- 関連: [ADR-0115](0115-browser-ptrace-owner-ns-launcher.md)（本 ADR はその実装判断）、[ADR-0105](0105-browser-p4a-same-uid-bwrap-runtime.md)、[ADR-0108](0108-browser-p4a-relay-supervisor-launch-restore.md)、[ADR-0113](0113-browser-p3c-control-gate-action-server.md)

## 背景

ADR-0115 は「専用 host user `celeris-browser`（host UID/GID `B`）の process が user namespace を作り、daemon（UID 1001）はその namespace の owner にも map にも現れない」と決めたが、コードの置き場所・IPC の形・userns の作り方・設定の切替は未決だった。本 ADR はそれを決める。DESIGN.md の原則（ディスパッチャ・ストアに LLM を入れない、`unwrap()` 禁止）は変えない。

## D1. 置き場所

- library: `crates/task-worker/src/browser_launcher.rs` と子 module `browser_launcher/{protocol,server,client,registry,userns}.rs`。
  - `protocol`: 要求・応答の型と frame の encode/decode（D2）。
  - `server`: launcher 側の accept loop・peer 検査・要求の振分け。
  - `client`: daemon 側の接続と要求（`browser.rs` の isolated 経路から呼ぶ）。
  - `registry`: session の登録・記録・回収（D6）。
  - `userns`: 2 map の user namespace 生成（D3）。
- binary: `crates/task-worker/src/bin/celeris-browser-launcher.rs`。既存の `celeris-browser-sandboxd` / `celeris-browser-egress` と同じ crate に置き、同じ release 成果物に入れる。
- 再利用: launcher 側で既存の `browser_runtime`（`RuntimeSpec`・`bwrap_args`・`IsolatedRuntime`・`collect_facts`・`write_record` / `reap_recorded`・`same_process_alive`）と `browser_supervisor`（`Supervisor::start`・`attach_controller`・`stop`・`reap_on_start`）をそのまま使う。CDP controller（`CdpController`）と action 実行（`browser_action` の固定 verb 対応）も launcher の process 内で動かす。新しい launch 経路のために browser 起動の論理を複製しない。
- daemon 所有の経路（same-uid / subuid wrapper）は削らない。`runtime = "daemon"` が既定のまま従来と同じ code path を通る。

## D2. Unix socket の固定 protocol

- 枠: **4 byte big-endian の長さ前置き + UTF-8 JSON 1 個**。改行区切りより境界が明確で、payload に改行を含む値（観測の text）を escape の正しさに依存せず扱えるため採る。長さが上限を超える frame は読まずに接続を切る。
- JSON は `#[serde(deny_unknown_fields)]` の `tag = "type"` enum。未知の type・未知の field は `error{code:"bad_request"}` で拒否し、接続を閉じる。
- 要求（daemon → launcher）:
  - `start_session{task_id, run_id, lease_id, policy}`: `policy` は既存の browser policy の非機密部分（許可 domain の列・許可 action の列・期限）だけ。launcher が session id と instance id を採番する。
  - `action{session_id, lease_id, verb, args}`: `verb` は `browser_action::allowed` と同じ固定集合（`open` / `click` / `snapshot` / `extract` / `screenshot` / `download` / `scroll` / `close`）。`args` は verb ごとの固定型（URL・selector・座標など）で、launcher 側で再度 policy 検査する。
  - `observe{session_id, lease_id}`: 状態と isolation の事実を返す。
  - `stop{session_id, lease_id}`。
- 応答（launcher → daemon）:
  - `started{session_id, instance_id, receipt}`、`action_result{receipt, observation}`、`observed{state, facts}`、`stopped{receipt}`、`error{code}`。
  - `state` は固定 enum（`starting` / `running` / `stopping` / `stopped` / `failed`）。`error.code` は固定の短い列挙（`bad_request` / `unauthorized` / `lease_mismatch` / `limit` / `timeout` / `launch_failed` / `isolation_failed`）で、自由文の診断を返さない。
  - `receipt` は session id・instance id・verb・結果種別・時刻・`verify_isolation` の結果を含む固定の構造体。`facts` は `RuntimeFacts` のうち非機密の項目（host uid / gid、uid_map / gid_map、namespace owner uid、CapEff、NoNewPrivs、listen 数）に限る。
  - `observation` は既存 action の非機密の出力（snapshot text・extract text・screenshot の artifact 参照）だけ。cookie・credential・CDP payload・profile の中身は型として表現できないようにする。
- 受けないもの: 任意 argv、bind mount、環境変数、UID/GID map、FD（`SCM_RIGHTS` は送らず、受信しても閉じて `bad_request`）、実行 path、bwrap / sandboxd / Chrome の path。これらは launcher の固定設定（root 所有のファイル）からだけ決まる。
- 上限（launcher の固定設定。既定値）: frame 64 KiB、同時接続 4、同時 session 2、1 接続あたり未応答要求 1、要求の読取り期限 5 秒、`start_session` 期限 60 秒、`action` 期限 120 秒、idle 接続 300 秒。超過は `error{code:"limit"}` か `timeout`。
- peer 検査: accept 直後に `SO_PEERCRED` を読み、uid が launcher 設定の `allowed_uids`（本番は daemon の 1 個）に無ければ応答せずに閉じる。`SO_PEERCRED` は同 UID の worker と daemon を区別しないため、**全ての session 操作で `lease_id` を必須**とし、launcher は `start_session` で受けた lease と一致し、期限内のときだけ処理する。接続を開いた peer（pid・starttime）と session を結びつけ、別接続からの同 session 操作は lease が一致しても拒否する。socket に届けることを機密 state の受領権限とみなさない。

## D3. userns 生成（2 map）

- launcher（host UID/GID `B`）が `userns::create` で子 process を `fork` し、子は `unshare(CLONE_NEWUSER)` して親に pipe で合図し、map 完了まで待つ。親は:
  1. `newgidmap <pid> 0 B 1 1000 S 1` に先立って子の `setgroups` を `deny` にする（`newgidmap` が行わない構成では `/proc/<pid>/setgroups` に `deny` を書く）。
  2. `newuidmap <pid> 0 B 1 1000 S 1` と `newgidmap` を実行する（`S` は `/etc/subuid` / `/etc/subgid` の `celeris-browser` 範囲の先頭を launcher が読み、固定値にしない）。
  3. 子の `/proc/<pid>/uid_map`・`gid_map`・`setgroups` を読み戻し、期待値と一致しなければ子を殺して `launch_failed`。どの段の失敗も起動を止め、map なし・単一 map での継続や daemon 経路への切替えはしない。
- 子の userns を `/proc/<pid>/ns/user` で開いた FD を bwrap に `--userns <fd>` で渡し、bwrap は `--unshare-user` を使わず `--uid 1000 --gid 1000` で内側 1000（= host `S`）に下げて Chrome を起動する。`bwrap_args` は `RuntimeSpec` に `userns: UsernsMode`（`Unshare` = 現行・既定 / `Fd(RawFd)`）を足して分岐し、`Unshare` の出力は現行と完全に一致させる（既存試験で不変を確認）。
- 起動後、`collect_facts` に `NS_GET_OWNER_UID`（`ioctl(ns_fd, NS_GET_OWNER_UID)`）で Chrome の user namespace owner uid を加え、launcher は owner が `B` であること・daemon UID が uid_map / gid_map に現れないことを `verify_isolation` と併せて検査し、満たさなければ session を `isolation_failed` で止める。

## D4. FD と state の所有

- CDP pipe（fd 3/4）、egress 中継の制御 FD（sandboxd の FD 6 の request channel と egress relay）、profile / session dir、`CdpController` は launcher の process だけが持つ。daemon には D2 の receipt・状態・非機密の観測だけを返し、FD を渡さない。
- launcher の状態 dir・socket・helper の executable を Chrome の mount に bind しない（`RuntimeSpec.ro_dirs` に入れない検査を server 側に置く）。
- log・error には cookie、credential、CDP payload を書かない。error は D2 の固定 code だけ。

## D5. 設定

- `celeris` の `[browser]`（`BrowserRuntimeConfig`）に 2 項目を足す。
  - `runtime = "daemon" | "launcher"`（既定 `"daemon"`）。
  - `launcher_socket = "<path>"`（`runtime = "launcher"` のとき必須。無ければ設定読込みで error）。
- `runtime = "launcher"` で socket に接続できない・応答が不正・peer が想定外のとき、browser run は `isolated_runtime_unavailable` で失敗させる（**fail closed**）。daemon 所有経路への fallback はしない。
- launcher 側の設定（`allowed_uids`、bwrap / sandboxd / egress / Chrome の path、上限値、状態 dir）は root 所有の別ファイルで、daemon の設定からは変えられない。具体の配置は host 準備の手順書で定める。

## D6. session 記録と回収

- `registry` は session ごとに `{session_id, instance_id, pid, pgid, starttime, peer_pid, peer_starttime, lease_id, lease_deadline}` を `celeris-browser` だけが書ける状態 dir に記録する（`write_record` 形式を拡張）。instance id は launcher 起動ごとの乱数。
- 回収の契機: (a) daemon 接続の切断、(b) lease 期限切れ（launcher 内の timer）、(c) `stop` 要求、(d) launcher の SIGTERM / SIGINT。いずれも CDP と channel を閉じ、`Supervisor::stop` で process group 全体を止めて回収する。
- launcher 起動時は `reap_on_start` で記録を読み、`same_process_alive(pid, starttime)` が真のものだけを殺す（PID 再利用先は殺さない）。記録の instance id が現 instance のものは触らない。
- launcher が落ちたら systemd が process group / cgroup ごと止め、daemon 側は接続断で session を失敗にする（browser を残して継続しない）。

## D7. 試験方針

- 単体（環境不要・常に走る）: protocol の encode/decode と上限・未知 field の拒否、peer uid 拒否、lease 不一致の拒否、registry の記録と starttime 照合、`bwrap_args` の `Unshare` 出力不変と `Fd` の `--userns` 出力。
- 実 process（host 準備が要る）: launcher 経由の session で (1) Chrome の userns owner（`NS_GET_OWNER_UID`）が daemon UID でない、(2) daemon UID の別 process からの `PTRACE_ATTACH` / `strace -p` と `/proc/<pid>/environ`・`mem` の読取りが拒否される、(3) `verify_isolation` が `Ok`、を確かめる。正の対照（同じ手順が root または `celeris-browser` からは通ること、あるいは daemon 所有経路では attach が成功すること）を併記して試験自体が機能していることを示す。
- skip: `celeris-browser` user・socket・subuid 範囲のいずれかが無ければ理由を表示して skip する。環境変数 `CELERIS_LAUNCHER_TESTS=require` のときは skip を失敗に変える（実証の run と CI の区別に使う）。
- 既存の daemon 所有経路の試験（`browser_runtime_isolated.rs` など）は変更せずに通ることを確認する。

## 機密能力

この ADR とその実装は機密能力 `CredentialInjection` / `IdentityRestore` を**解放しない**。launcher 経路の admission でもこれらは従来どおり拒否のままとし、解放は ptrace 拒否の実証・A13・controller の秘密非露出・全 lifecycle の回収を揃えた後の別の判断（別 task）で行う。

## 検討した代替案

- **改行区切り JSON**: 実装は簡単だが、境界の決定を JSON の escape に依存し、上限超過を読み切る前に検知しにくい。長さ前置きを採る。
- **launcher を別 crate にする**: 既存の runtime / supervisor を `pub` で共有するだけで足り、crate を分けると依存と release 成果物が増える。task-worker 内の module + bin とする。
- **bwrap の `--unshare-user` に map を任せる**: bwrap は単一 map しか張らず `0→B, 1000→S` を表現できない（ADR-0115）。外で作った userns を `--userns` で渡す。
- **launcher 未稼働時に daemon 経路へ fallback**: 利用者が選んだ隔離を黙って弱めるため棄却（ADR-0115 移行手順 1）。
