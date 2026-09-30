# ADR-0088: P4-A 実結合 — netns 内中継・runtime supervisor・起動時回収・production 起動経路・稼働中 session への restore 結合

---
tasks: [01M3QGRC542ZC23996DNCTHZF5]
---

- 日付: 2026-09-29
- 状態: Accepted（設計。実装と実 runtime の証拠は後続 unit。この ADR 自体は受入の証拠ではない）
- 関連: [ADR-0084](0084-browser-phase4-isolation-injection-routing.md) D1/D2/D6、[ADR-0085](0085-browser-phase4-runtime-selection.md) 1/2/5、[ADR-0086](0086-browser-egress-transport.md) 1/4/7、[ADR-0087](0087-browser-p4a-same-uid-bwrap-runtime.md) D1〜D6、人の決定 p4a-uid

## 文脈

ADR-0087 の run で、実 bwrap runtime（6 namespace・ro root・`/session` だけ rw・CDP pipe）と事実採取は実プロセスで
確認できた。reviewer は次の 3 点を未達とした。

1. browser の `--proxy-server=http://127.0.0.1:3128` の先に何も無く、「fixture へは egress proxy 経由でだけ届く」
   正例が無い。経路が無いことで閉じているだけで、出口が proxy に限られることの証明になっていない。
2. SIGKILL・再起動の試験の中身が `/bin/sleep` で、browser と proxy を一体で回収していない。daemon 起動時に
   `reap_recorded` を呼んでいない。
3. `restore_for_session` の試験が fake attestation（`FakeLive`）で、HTTP・daemon・稼働中 session の registry に
   結合されていない。

加えて production の起動経路（`task_worker::browser::run` → agent-browser）は隔離されていない。agent-browser 0.38.1
本体はこの host に無い（`which agent-browser` が空）。決定 p4a-uid により、この host では同一 host UID のまま
隔離し、restore 等の機密能力は別 UID の実証まで拒否する。

## 決定

### D1. netns 内 listener → controller → 接続ごとの celeris-browser-egress

- sandbox の最初の process を新しい小さな実行ファイル `celeris-browser-sandboxd`（task-worker の bin。host の
  実体を sandbox 内 `/opt/celeris/` へ ro bind）にする。sandboxd は起動直後に netns の `127.0.0.1:3128` を
  bind/listen し、listen できてから browser（または D4 の agent-browser）を子として起動する。listen 失敗なら
  browser を起動せずに固定コードで終了する（browser が出口の無い状態で動き出す競合を作らない）。
- controller と sandboxd の間は、controller が `socketpair(AF_UNIX, SOCK_SEQPACKET)` で作り、一端を継承 FD
  （FD 6。CDP の FD 3/4、`--info-fd` 5 と重ねない）として渡す **request channel** だけとする。
  - sandboxd は TCP 接続を 1 本 accept するごとに channel へ 1 byte の要求を送る。
  - controller は要求ごとに `socketpair(AF_UNIX, SOCK_STREAM)` を作り、一端を FD 3、管理者 policy（D4 参照）を
    stdin にして `celeris-browser-egress` を起動し（ADR-0086 D7 の契約そのまま）、もう一端を `SCM_RIGHTS` で
    sandboxd へ返す。sandboxd は accept した TCP とその unix stream の間で byte を中継するだけで、HTTP を解釈しない
    （検査は proxy だけが行う。sandboxd が侵害されても得られるのは「proxy を 1 本起動させる」能力だけ）。
  - controller は runtime ごとに同時 egress 数（既定 32）と要求 rate を制限し、超過分は unix stream を返さずに
    閉じる（ADR-0086 D4 の「接続数（呼出し側）」をここで実装する）。channel の EOF は runtime 終了として扱う。
- **ADR-0086 D1「proxy は host listener を開かない」を維持する**: proxy は相変わらず継承 FD 3 しか持たない。
  host 側には TCP listener も path を持つ unix socket も作らない（channel は無名 socketpair）。listener は
  browser の netns の loopback にだけあり、host の netns からも worker からも到達できない。
- browser の netns には `lo` しか無いので、proxy を通らない通信（private IP・IPv6・DNS 直叩き・`--no-proxy`
  相当の迂回）は経路が無く失敗する。**出口が proxy だけであることは「経路が無い」ことと「proxy 経由の正例が届く」
  ことの両方で示す**。
- 却下: (a) sandbox に bind した path 付き unix socket に browser/relay が connect する — host の同一 UID の
  任意 process が同じ path に connect でき、host 側に listener を開くのと同じになる。(b) sandboxd が accept した
  TCP fd 自体を SCM_RIGHTS で controller へ渡し、proxy の FD 3 にする — ADR-0086 D7 の「FD 3 は接続済み
  AF_UNIX stream」を変えることになり、proxy の試験済み入力面が変わる。(c) slirp4netns/pasta で netns に
  外向き経路を作り proxy を host の port に置く — host listener が要り、proxy 迂回の経路も生まれる。
  (d) proxy を sandbox 内で動かす — DNS resolver への到達に netns の外への経路が要る。
- 試験方針（実 runtime。fake を証拠に数えない）:
  - **fixture 網**: 試験が `unshare --user --map-root-user --net`（newuidmap を使わない単一 map。この host で動く）
    で外側の試験用 netns を作り、その中で `lo` に公開扱いの IPv4（例 `93.184.216.34/32`）を付け、そこに HTTPS
    fixture と TCP/53 の DNS fixture を置く。controller・proxy・runtime はこの試験用 netns の中で動かす。
    外部ネットワークには経路が無い（2026-09-29 この host で `unshare --user --map-root-user --net sh -c 'ip link set lo up && ip addr add 93.184.216.34/32 dev lo'` → exit 0 を確認済み）。本番 policy・内部 origin は変えず、allowlist と resolver は試験の stdin
    policy にだけ書く。
  - 正例: 実 chrome-headless-shell を D2 の supervisor で起動し、CDP pipe で `https://<fixture 名>/` を開いて
    fixture の固有文字列を読み、fixture 側で接続元が proxy（試験用 netns 側の socket）であること、proxy の
    起動回数が 1 以上であることを確かめる（証明書は試験で作った自己署名の SPKI を
    `--ignore-certificate-errors-spki-list` にだけ渡す）。
  - 負例（同じ runtime、sandbox 内の probe と browser の両方）: fixture の IP literal へ直接 connect、private
    IP・`::1`・IPv6 literal、fixture DNS への UDP/TCP 53 直叩き、proxy を外した直接 navigation、allowlist 外の
    名前 — すべて失敗し、fixture 側に接続が記録されないこと。
  - 前提（`unshare -r`・`ip`・bwrap・browser）が無い環境では試験を失敗させ、成功にも ignore にもしない。
    理由と人への手順を PROGRESS に書く。

### D2. browser・sandboxd・proxy を 1 runtime とする supervisor

- `task_worker::browser_runtime::Supervisor` を追加する。1 runtime = 1 本の**専用の長寿命 std::thread**
  （`celeris-browser-rt-<session>`）。bwrap の spawn、channel の受信、egress の spawn、`waitpid`、停止はすべて
  この thread が行い、thread は bwrap を回収し終えるまで終了しない。
  - 理由: `PR_SET_PDEATHSIG` は親 **thread** の終了で発火する。tokio の `spawn_blocking` の thread は idle で
    終了するので、そこから起動すると runtime が理由なく殺される。async 側は `Supervisor` の handle
    （mpsc と状態）だけを持つ。
- 起動: bwrap（ADR-0087 D1 の argv。command は sandboxd）を `process_group(0)` と `PR_SET_PDEATHSIG=SIGKILL`
  で起動し、pre_exec で `getppid()` が spawn 前に控えた pid と違えば即 `_exit`（prctl 前に親が死んだ競合を塞ぐ）。
  egress は pre_exec で `setpgid(0, bwrap_pgid)` により同じ process group に入れ、ADR-0086 D7 の自前の
  PDEATHSIG も保つ。sandbox 内は bwrap の `--die-with-parent` と pid namespace（内側 pid 1 の死で全 process が
  SIGKILL される）で回収される。
- 記録: sandboxd の ready（channel 上の 1 byte）を受けたら、D3 の記録 dir に `<session>.pid`（bwrap の
  `pid starttime`。egress は同じ pgid なので個別記録しない）を tmp+rename で書き、その後に registry（D5）へ登録
  する。記録前に起動失敗したら自分で killpg して終わる。
- 停止: registry から外す → CDP/channel を閉じる → SIGTERM → 猶予（5 秒）→ `killpg(bwrap_pgid, SIGKILL)` →
  `waitpid` → 記録を消す。egress の子も同じ thread が回収する。
- 異常時: daemon が SIGKILL されると全 thread が消え、bwrap と egress の PDEATHSIG が発火し、sandbox は pid
  namespace ごと消える。発火を取りこぼした場合（prctl 前の競合・pgid 外へ出た process）は D3 の起動時回収が拾う。
- 却下: tokio の `Command`/`spawn_blocking` からの起動（上記の thread 寿命問題）、runtime ごとの別 supervisor
  process（supervisor 自体の orphan 回収がもう 1 段要る）、bwrap の `--new-session` を外して sandbox を同じ
  pgid に置く（TIOCSTI 対策を失う。pid namespace で十分回収できる）。
- 試験方針（実プロセス）: D1 と同じ実 runtime（実 browser + sandboxd + fixture を通した実 egress 接続を 1 本
  張った状態）で、(a) supervisor を持つ子 process を SIGKILL、(b) supervisor thread を持つ子 process を
  再起動して D3 の起動時回収を通す、のそれぞれの後に、事前に控えた bwrap・sandboxd・browser・egress の pid が
  `/proc` から消え（または starttime が変わり）、fixture への接続が切れていることを確かめる。中身が `sleep` の
  runtime は証拠に数えない。

### D3. 起動時回収の位置

- 記録 dir は daemon 所有の状態 dir の下の `browser-runtime/<instance_id>/`（workspace・run dir の外。sandbox に
  bind しない）。instance ごとに分けるのは、ADR-0040 の release handoff で旧 instance が drain 中に新 instance が
  起動するため。
- `celeris::run` の `instance::Supervisor::start` が `Started::Running` を返した直後（`set_orphan_takeover` と同じ
  所）、dispatcher の最初の tick より前に呼ぶ。`Started::Duplicate` と `--mode verify` では呼ばない（他の稼働中
  instance の runtime を殺さない）。対象は「`daemon_instances` の freshness 切れ、または記録された pid が
  `instance::pid_alive` で死んでいる instance」の dir だけで、各記録に ADR-0087 D4 の starttime 照合付き
  `reap_recorded` を適用し、空になった dir を消す。自分の dir は新規なので空。
- 回収した数は `tracing::info!` に出す（pid と session id だけ。URL・policy は出さない）。
- 却下: `shutdown_and_exit` での回収（SIGKILL では走らない）、dispatcher の orphan 回収 tick への相乗り（最初の
  tick までの間、旧 runtime が egress を張れる）、instance を区別しない単一 dir（handoff 中の旧 instance の
  runtime を殺す）。
- 試験方針（実プロセス）: 実 daemon の起動関数（`celeris::run` 相当の起動経路の関数を切り出して呼ぶ。試験が
  自分で `reap_recorded` を呼ぶのは証拠に数えない）に、死んだ instance の記録（実 runtime を起動して記録した後、
  その supervisor を SIGKILL したもの）と生きている instance の記録を置き、前者だけが消え後者の runtime が
  生きていることを確かめる。

### D4. production 起動経路の切替

- `task_worker::browser::run` は agent-browser を host で直接起動しない。すべての browser run は D2 の supervisor
  で isolated runtime を起動し、その中で agent-browser を動かす。host 直起動への fallback は無い。
- agent-browser は sandbox の中で動かす: host の `agent-browser` を realpath で解決し、その install dir と同梱
  browser の dir だけを ro bind する（`$HOME` は bind しない）。env は `--clearenv` の後に `HOME=/session/home`、
  `XDG_RUNTIME_DIR=/session/run`、`AGENT_BROWSER_NAMESPACE=celeris` だけを設定し、agent-browser の daemon・control
  socket は sandbox 内に置く（host の `/run/user` は見えない。ADR-0087 D1 のまま）。version 検査も sandbox 内で行う。
- harness 側 shim（`browser_cli.py`）は agent-browser を直接 exec せず、run dir（0700）の `action.sock` を通して
  supervisor に action を送る。supervisor は `policy.json` で action を検査し直し、channel 上の別 message 種別で
  sandboxd に渡す。sandboxd は固定の argv 形（`--config /session/upstream.json --session <id>
  --action-policy /session/policy.json --json <action> …`）だけを組み、`--restore`・`--state`・`--profile`・
  `--cdp`・`--executable-path` などの未許可 flag を含む要求は固定拒否する。**agent-browser 0.38.1 の
  --restore/--state/--profile は使わない**（ADR-0084 D2 のまま）。
  - `action.sock` は同一 UID の process なら connect できるが、受け付けるのは worker がすでに shim で実行できる
    action の語彙だけで、broker・CDP・agent-browser の control socket には届かない。
- egress policy: allowlist は task の effective policy の `allowed_domains`（443 のみ）から作り、resolver は
  新しい任意の設定 `[browser.egress] resolver` から読む。**既定値は置かず、未設定なら run を起動前に固定コード
  `isolated_runtime_unavailable` で失敗させる**（bwrap・sandboxd・egress の実行ファイルが無い場合も同じ）。
  この ADR も後続実装も本番の設定ファイルは変更しない。設定は人が手順書に従って行う。
- live view: isolated runtime では host から browser に届く経路が無いので、live view URL は出さない（現行の
  credential 利用時と同じ扱い）。live view の再配線は本 ADR の範囲外。
- 却下: host で agent-browser を動かし `--cdp` で sandbox の browser に繋ぐ（CDP を worker 側の process に渡す
  ことになり ADR-0084 D1 の CDP 分離に反する）、controller が setns で sandbox に入って agent-browser を実行する
  （bwrap の cap drop・seccomp の外で動く process ができる）、切替を設定の既定 off で導入する（production が
  隔離なしのまま残る）。
- 未検証の前提: agent-browser 0.38.1 が host に無いため、その browser 起動時の proxy 指定方法・CDP の持ち方は
  確認できない。安全性はそれに依存しない（netns に lo しか無く、proxy を指定しなければ出られないだけ）。
- 試験方針: `browser::run_with_executable` を production と同じ起動経路（supervisor → bwrap → sandboxd）で実行し、
  executable には試験 dir の小さな script（実 chrome-headless-shell を起動して D1 の fixture を開く）を与えて、
  shim → `action.sock` → sandboxd → 実 browser → sandboxd 中継 → 実 egress → fixture が通ることと、禁止 flag の
  拒否を確かめる。**これは起動経路の結合の証拠であり、agent-browser 0.38.1 の適合の証拠ではない**。
  agent-browser での確認は、導入された host で人が実行する手順を `docs/ops/` に書き、未解決に残す。

### D5. restore の HTTP・稼働中 session への結合

- task-core に `LiveSessionRegistry` trait（`get(session_id) -> Option<Arc<dyn LiveSessionEntry>>`）を置く。
  entry は `LiveIsolation` と runtime 種別（`Isolated` / `NotIsolated`）を返す。daemon は `celeris::run` で registry を
  1 つ作り、dispatcher（D2 の supervisor が登録・削除）と API の `ApiState` の両方に渡す。store と dispatcher の
  判断に LLM は入れない。
- `POST /api/v1/browser/identities/{id}/restore` の body に `session_id` を足す。無ければ従来どおり
  `isolation_required`。有れば handler は registry から entry を引き、次の順で判定する（先に外側の条件を決めて、
  実 session の試験で拒否理由を区別できるようにする。秘密の開封は必ず最後）:
  1. `sweep_expired` と identity の存在・project・origin・期限（`not_found`・`other_project`・`expired` 等、
     既存の `authorize_use` の非隔離部分）。
  2. session が registry に無い、または `NotIsolated` → `isolation_required`。
  3. `current_attestation()` を採り直し、失敗（この host の `SameUid` を含む）または session id 不一致 →
     `isolation_required`。
  4. 3 を通った場合だけ `restore_isolated` で開封し、平文 state は runtime の controller（CDP 側）にだけ渡し、HTTP
     応答は 204 で本文を返さない。controller に投入口が無い runtime（agent-browser 駆動で CDP pipe を controller が
     持たない場合）は `isolation_required`。
- 決定 p4a-uid によりこの host では 3 で必ず拒否される。検査を緩める分岐・試験用の attestation 生成口は作らない。
- **H3 と auth_section を変えない**: `AuthSectionObservationStop`、Phase 3 の auth_section 配線、ADR-0080 D3 の
  認証後の観測停止、ADR-0084 D6 の未適合機密要求の起動前拒否、ADR-0085 3/4 の旧 IPC 固定拒否はそのまま。
  将来 restore が成功した session は認証済みとして扱い、ADR-0080 D3 と同じく観測 action を止める（弱める方向の
  変更はしない）。
- 却下: dispatcher が run 開始時に restore を自動実行する（人の承認と session の対応が消える）、`FakeLive` の
  成功を restore 結合の証拠にする、HTTP 応答で state を返す。
- 試験方針（実 runtime）: 実 daemon の router と実 registry に D2 の supervisor で起動した実隔離 session を登録し、
  HTTP で (a) 別 identity・別 project → `not_found`/`other_project`、(b) 期限切れ → `expired`、(c) 正しい
  identity でこの host の同一 UID → `isolation_required`、(d) 登録されていない session id・停止後の session →
  `isolation_required`、(e) `NotIsolated` の session → `isolation_required` を確かめ、いずれでも state が開封
  されない（sealer の open が呼ばれない）ことを確かめる。成功例は別 UID の host でだけ得られるので手順書に回す。
  `FakeLive` の単体試験は残してよいが、受入の証拠に数えない。

## 既存 ADR との関係（置換範囲）

- ADR-0087 D3「browser の netns の TCP には何も listen しない」を、「netns 内の LISTEN は sandboxd の
  `127.0.0.1:3128` だけ（agent-browser 駆動時は agent-browser 自身が netns loopback に開くものを列挙して記録する）。
  host の netns・worker からはどれにも届かない」に置き換える。CDP を controller が持つ場合は pipe だけ、は維持。
- ADR-0087 D4 の記録場所を D3 の instance 別 dir に、回収対象を bwrap の process group（egress を含む）に具体化する。
  starttime 照合は変えない。
- ADR-0087 D5 の判定順を D5 のとおり具体化する（拒否されるものは変わらない。開封は attestation の後のまま）。
- ADR-0087 D6 の「次段の中継」を D1 で決める。ADR-0086 の決定は変えない（D1 は ADR-0086 D1/D7 の範囲内で配線する）。
- ADR-0084 D1（CDP・broker・control IPC の分離）、D2（`--restore/--state/--profile` 不使用）、D6、ADR-0085 の各項は
  変えない。ADR-0085 1 の専用 UID は、決定 p4a-uid によりこの host では同一 UID のまま（ADR-0087 と同じ）。

## 変えないもの

- H3・Phase 3 の auth_section 配線・観測停止、機密要求の起動前拒否、旧 resolve IPC の固定拒否。
- 本番の設定ファイル・本番昇格・内部 origin（追加しない）。`[browser.egress] resolver` は schema を足すだけで、
  本番では未設定のまま（= isolated runtime を使う browser run は起動前に失敗する）。

## 結果

- 良い: 出口が proxy だけであることを正例と負例の両方で実証でき、browser・sandboxd・proxy が 1 つの process
  group と pid namespace で回収される。restore の拒否が実 session・実 HTTP 経路で区別して試験できる。
- 悪い: production で browser を使うには resolver の設定と agent-browser の導入が要る（それまで fail-closed）。
  live view は isolated runtime で出ない。同一 UID の限界（ADR-0087 結果）は残り、機密解放は別 UID の実証まで拒否。
- 後続 unit: egress-relay（D1）、restore-binding（D5）、supervisor（D2）、daemon-reap（D3）、prod-launch（D4）、
  evidence（実 runtime 証跡と PROGRESS・手順書）。

## 追記（2026-09-30、P4-B gate-recheck）: 停止時の残存 0 の待ち

D2 の「正常停止（TERM → 猶予 → KILL → wait）で戻った時点で残存 0」は、bwrap（外側）の回収までしか待っていなかった。
bwrap の子である pid namespace の init（記録上の `bwrap-init`）とその下は、bwrap の回収後に PDEATHSIG と
pid namespace の後始末で非同期に消えるため、高負荷の評価器では `stop()` の直後に `bwrap-init` がまだ生きて
見えることがあった（`browser_runtime_supervisor` の (c) が 8 回中 1 回 `left=[bwrap-init]` で失敗）。

決定: `stop_runtime` は bwrap と egress を回収した後、記録にある本人（pid+starttime 一致）が全て消えるまで
`STOP_GRACE` を上限に待ってから記録を消す。追加の signal は送らない（pid 再利用の保護は `same_process_alive`
の starttime 照合のまま）。停止の意味・signal の順序・起動時回収・記録の形式は変えない。
