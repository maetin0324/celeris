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

### D3 付記（2026-10-02、実 host での bwrap 失敗を受けて）

実 host の strace で、bwrap 0.11 は `--userns <fd>` のとき子で `setuid(1000)` → `setgid(1000)` の順に切り替え、`setuid` で userns 内の capability を失って `setgid(1000) = -1 EPERM` になり起動できなかった。bwrap の順序は変えられないので、上の「`--userns <fd>` で渡す」を次に改める。

- bwrap を spawn する子が `pre_exec` で `setns(<userns fd>, CLONE_NEWUSER)`（owner `B` の userns に入り capability を得る）→ `setresgid(1000)` → `setresuid(1000)` の順に内側 1000（= host `S`）へ切り替える。資格の変更は `PR_SET_PDEATHSIG` を消すので、その `prctl` より前に行う。
- bwrap には `--userns` を渡さず、`Unshare` 経路と同じ `--unshare-user --uid 1000 --gid 1000` を渡す。bwrap は `S` として入れ子の userns（map `1000 1000 1`）を作るので、Chrome の userns は **owner `S`、親（launcher が作った 2 map の userns）の owner `B`** になる。daemon UID はどちらの owner でもなく、どちらの map にも現れない。
- 内側 1000 は 0700 の `session_root` を辿れないので、session dir は launcher が `O_PATH` で開いた FD を `--bind-fd 8 /session` で渡す。`Unshare` 経路の引数は従来どおり（`--bind <dir> /session`）。
- launcher の検査は、Chrome の userns の owner が `S`・`NS_GET_PARENT` の owner が `B`・launcher から見た `uid_map` / `gid_map` が `1000 S 1` の 1 行であること、に置き換える（`isolation_ok` も同じ）。2 map そのものは `userns::create` が読み戻して検査する（D3 の 3）。
- 起動失敗は段と原因（errno・bwrap の stderr）を launcher の stderr（journal）に出す。daemon に返すのは従来どおり固定の `launch_failed` / `isolation_failed` だけ。
- 再付記（同日、実 host の `bwrap: Can't find source path /proc/self/fd/8: Permission denied` を受けて）: bwrap 0.11 は bind の source を `realpath` で解決し、`--bind-fd` の `/proc/self/fd/N` も実 path（`session_root/<id>`）に展開して各段を辿る。内側 1000 は 0700 の `session_root` を辿れないので `--bind-fd` は使えない。代わりに spawn の子が `setns` の直後（まだ launcher の userns の 0 で capability がある間）に `unshare(CLONE_NEWNS)` → `/` を rprivate → `/tmp` に tmpfs（0755）→ `/tmp/celeris-session` に session dir を bind し、bwrap には `--bind /tmp/celeris-session /session` を渡す。この mount ns は launcher の userns の持ち物で、launcher 本体・host の mount ns は変わらない。`session_root` の 0700 はそのまま。
- 残る点: `setgroups` は D3 の 1 で `deny` のため、launcher の補助 group（systemd が付ける `B` の group）は Chrome に残る（userns 内では 65534 に見える）。

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

## 付記（2026-10-02、owner の鎖）

実 host で launcher の検査が `namespace owner 296608 (want 296608), parent owner 296608 (want 995)` で止まった。bwrap は `--dev` の devpts を張るために内側 0 で userns を作り、そのあと内側 1000 へ map し直す userns をもう 1 段作る。Chrome の userns は launcher の 2 map の userns から 2 段下にあり、鎖は `[S, S, B]` になる。脅威モデル（ADR-0115: daemon UID が Chrome の祖先 userns のどれの owner でもない）は段数に依らないので、構造は変えず検査を直す。

- launcher は Chrome の `/proc/<pid>/ns/user` から `NS_GET_PARENT` を辿り、launcher 自身の userns（`/proc/self/ns/user` と inode・dev が一致）の直前までの owner を集める。
- 鎖の先頭が `S`、末尾（launcher が作った userns）が `B`、全要素が `S` か `B`、`allowed_uids`（daemon UID）が鎖に無いこと、を `isolation_ok` と起動時の検査の条件にする。上の D3 の「親の owner が `B`」はこれに置き換える。

## 付記（2026-10-02、launcher の CDP と netns 内 listener）

実 host で Chrome の起動と owner/map 検査が通った後、daemon 側の `verify_isolation` が `CdpOnTcp` で止まった。`SessionFacts.listen_count` は Chrome と同じ private netns の TCP listener 数であり、sandboxd の egress proxy（127.0.0.1:3128）と shared-CDP relay（127.0.0.1:9223）を数える。Chrome 自身の CDP endpoint ではない。Chrome は引き続き `--remote-debugging-pipe` を使い、fd 3/4 と `CdpController` は launcher 側が保持する（ADR-0115）。

- daemon が観測を `RuntimeFacts` に変換するとき、`listen_count` から CDP endpoint を推定しない。launcher の固定 runtime の CDP は `Pipe` とする。
- launcher は `collect_facts` による netns 分離と `verify_isolation` を起動時と `isolation_ok` で検査する。daemon 側は `isolation_ok = false` を引き続き拒否する。host に露出する CDP TCP を許可する変更ではない。

## 付記 D-P（2026-10-02、socket 起動での launcher の身元確認）

背景: Attested task（01M3WV4BFJ71J9ZWJ020MP2Z4K）の host 実行で、daemon が launcher 接続の `SO_PEERCRED` を読むと uid 0 が返り、本番 admission の前提（launcher UID = `celeris-browser`）が成り立たなかった。launcher は systemd の socket 起動（`celeris-browser-launcher.socket`、`ListenStream=/run/celeris-browser/launcher.sock`、`Accept=no`）で動き、listen socket を `listen()` したのは systemd（root）である。Linux の `SO_PEERCRED` は、connect した側から見ると相手の socket が `listen()` された時点の資格情報を返すので、listen socket を受け継いだ launcher ではなく systemd を指す。daemon の判定は fail closed なので安全側には倒れるが、本番では機密能力を常に許可できない。

決定: daemon 側（`browser_launcher::client::LauncherClient`）は launcher の身元を `SO_PEERCRED` ではなく、**応答に kernel が付ける `SCM_CREDENTIALS`** で確かめる。

- client は接続に `SO_PASSCRED` を立てる。受け手が `SO_PASSCRED` を持つ AF_UNIX socket では、送り手が明示しなくても kernel が送信の時点の送り手の `{pid, uid, gid}`（thread group の pid と送り手の cred）を各 skb に付ける。送り手が明示的に `SCM_CREDENTIALS` を付ける場合も、kernel は自分の pid・自分の real/effective/saved uid 以外を拒否する（`CAP_SYS_ADMIN`/`CAP_SETUID` が無い限り。launcher は `celeris-browser` で動き、どちらも持たない）。値は受け手の pid/user namespace に変換される。
- client は応答を読む前に `recvmsg(MSG_PEEK)` で先頭の skb の資格情報を覗き、frame はこれまでどおり読む。AF_UNIX stream は資格情報の違う skb を 1 回の読みにまとめないので、先頭 skb の値はその応答を書いた process のもの。全応答で送り手が同じであることを要求し、資格情報の無い応答・送り手の違う応答が 1 度でもあれば以後 `None`（fail closed）。
- `LauncherSessionProof.launcher_uid` と `LauncherObservation.peer_uid` / `LauncherProofRegistration.peer_uid`（欄の名前は互換のため据え置く）は、この送り手の UID を指す。`verify_launcher_session` の照合規則（[ADR-0138](0138-browser-prod-admission-confidential-release.md) D-L。送り手の UID = 証明の UID = 設定上の launcher UID、root・daemon UID ではない）は変えない。
- launcher（server）側の検査は変えない。daemon は自分で `connect()` するので、launcher が accept した接続の `SO_PEERCRED` は daemon を正しく指す。protocol の版も変えない（launcher の binary は protocol v3 のままでよい）。

偽装への強さ: 値は kernel が送り手の process から採るもので、自己申告ではない。応答を書けるのは daemon の接続の相手側 FD を持つ process だけで、それは systemd が listen socket を渡した launcher（`User=celeris-browser`）である。root の systemd 自身が応答を書けば uid 0 になり拒否される。

検討した代替案:

- **`SO_PEERPIDFD`（kernel 6.5+）で pid を得て `/proc` で所有 uid を見る**: `SO_PEERPIDFD` も `SO_PEERCRED` と同じく listen した process（systemd、pid 1）を指すので、socket 起動では同じ問題が残る。
- **systemd の `MainPID` の uid を確かめる**: D-Bus か `systemctl show` が要り、daemon の run namespace では bus を覆っている（ADR-0095）。MainPID と応答の送り手が同じ process である保証も別に要る。
- **socket を launcher 自身が作る（socket 起動をやめる）**: `/run/celeris-browser/` の所有・`SocketUser=rmaeda` の mode を launcher が root 無しで再現できず、host 準備（`docs/ops/browser-launcher-host-setup.md`）と unit を作り直すことになる。daemon 側だけで直せる上の方法を採る。
- **launcher が accept 後に明示的に `SCM_CREDENTIALS` を送る**: kernel の検証は同じだが protocol と launcher binary の変更が要る。暗黙に付く資格情報で足りるので採らない。

試験: `browser_launcher::tests::client_identifies_the_responding_process_not_the_listener_creator` は socket 起動と同じ形（listen socket を作る process と、fork した子が accept して応答する）を作り、`SO_PEERCRED` が listen した process を指し、client の `responder()` が応答した子の pid を指すことを確かめる。`client_records_a_consistent_responder_across_requests` は in-process launcher の全応答で送り手が一致することを確かめる。host の実 session（`browser_launcher_ptrace.rs` の `launcher_chrome_denies_daemon_uid_ptrace`）は前提を「応答の `SCM_CREDENTIALS` の uid が `celeris-browser`」に置き換えた。host での再実行手順は `docs/ops/browser-launcher-admission-evidence-run.md`。

実証（2026-10-03）: main `3527c8e3`（protocol v3、本節の SCM 版）の launcher に対する host の 1 回の通常実行で、`launcher session proof: ... launcher_uid=995`、`ADMISSION[real-session]` の許可/拒否表（launcher-proof のみ両 admission allow）、`PTRACE_ATTACH ... errno=Some(1)`、`strace ... Operation not permitted`、`/proc/<pid>/{environ,mem}` の `errno=Some(13)` を確認した（`/var/tmp/launcher-evidence-main.log`、EXIT 0）。branch の protocol v4 版（launcher 側で SCM を付ける版）は main の v3 と重複したため採らず（merge `caf153c3`）、launcher の binary・protocol は v3 のまま。必須モードの host stutter 3 回と merge 後 HEAD の再取得は未確認。

## 付記（2026-10-09、launcher CredentialUse 解放決定）

人は launcher 経路の `CredentialUse` を ADR-0138 D-L の session 証明検証成功・owner が daemon UID でない・`isolation_ok` の全条件を満たす session に限り解放すると決定した。条件不成立時は接続前に fail closed とし、launcher の証明は credentiald の Attested injection IPC に渡して実 process facts を再検証する。controller に secret を見せず、`IdentityRestore` は今回解放しない。詳細は [2026-10-09 の決定](2026-10-09-browser-launcher-credential-release.md)。host 実証と本番昇格は別工程である。
