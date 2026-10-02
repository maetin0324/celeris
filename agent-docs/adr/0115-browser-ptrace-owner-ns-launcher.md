# ADR-0115: browser の user namespace 所有者を分離する launcher

---
tasks: [01M3W96K79NKX7B4ZM7X9E684S]
---

- 日付: 2026-10-01
- 状態: 採用（設計のみ。実装・実 process 攻撃試験・本番 admission の解放は未）
- 関連: [ADR-0102](0102-browser-phase4-isolation-injection-routing.md) D1/D2、[ADR-0103](0103-browser-phase4-runtime-selection.md)、[ADR-0105](0105-browser-p4a-same-uid-bwrap-runtime.md)、[ADR-0108](0108-browser-p4a-relay-supervisor-launch-restore.md)、[subuid 手順](../ops/browser-isolated-runtime-subuid.md)、[Phase 4 記録](../progress/phase-browser-4.md)、[A13 後続](../progress/browser-followups.md)

## 背景と決定

現行の本番経路は `crates/task-worker/src/browser.rs` の `run_with_executable_attempt` から `crates/task-worker/src/browser_supervisor.rs` の `Supervisor::start` → `runtime_thread` → `crates/task-worker/src/browser_runtime.rs` の `IsolatedRuntime::launch_with` → `bwrap_args` へ進む。daemon 側の thread が `bwrap --unshare-user` を起動し、`facts` / `attest_with` が `task_core::browser_isolation::verify_isolation` に事実を渡す。`crates/task-worker/tests/browser_runtime_isolated.rs` の基線は sandbox 内 UID 1000 → daemon の host UID という単一 map であり、検査結果は `SameUid` になる。

後続の別 host UID 試験では Chrome の host Uid が 165536、`verify_isolation` が `Ok`、`/proc/<pid>/environ` の読取りが拒否され、本番 `Attested` admission で identity 復元も成功した。しかし daemon UID 1001 の独立 process（worker を含む）から Chrome への `ptrace` attach は成功した。これは旧 `browser_runtime_isolated.rs` の同一 UID 基線とは別の試験結果である。`docs/progress/phase-browser-4.md` と `browser-followups.md` の A13 欄は 2026-09-30 時点の記録で、実 process 試験を未解決としている。後続試験の結果はまだ両文書へ反映されておらず、A13 の broker `peer_uid_mismatch` 判定と ptrace 隔離の判定も区別する。この四つの正例は ptrace 隔離の証明ではない。

原因は UID map の値でなく **user namespace を誰が作ったか** にある。Linux は作成時の親 namespace における creator の effective UID を子 user namespace の owner として記録し、その owner UID の親 namespace 側 process に子 namespace 内の全 capability を認める。daemon UID 1001 が namespace を作ると、別 host UID の Chrome に対しても daemon UID の別 process は子 namespace の `CAP_SYS_PTRACE` を持つ。`ptrace_may_access` の `ns_capable` 判定を通るため、Chrome 側の `CapEff=0`、`NoNewPrivs=1`、`PR_SET_DUMPABLE=0` や host の Yama `ptrace_scope=2` だけではこの owner 権限を遮断できない。

browser launcher を root が配置した専用サービスとして設け、**専用 host user `celeris-browser` の process が user namespace を作る**。daemon はサービスへの制限付き要求者に留まり、namespace を作らず、入らず、UID/GID map にも現れない。launcher 自身とその資格情報を持つ process は信頼境界の内側に置く。

## 脅威モデル

- 攻撃者は daemon UID 1001 で動く別 process、worker、または侵害された非特権 run。PID を知り、host の `/proc`、`ptrace`、同 UID の filesystem/IPC へアクセスを試す。daemon UID に root や host 初期 namespace の `CAP_SYS_PTRACE` は与えない。
- 防ぐ対象は Chrome のメモリ、環境、開いた FD、profile と CDP 経路を daemon UID の別 process が直接読むこと。`SO_PEERCRED` の broker 拒否だけで ptrace を代替しない。
- root、初期 namespace に `CAP_SYS_PTRACE` を持つ管理者、`celeris-browser` の資格情報または launcher 自体の侵害はこの境界の外。専用 user の login と任意コード実行を禁止する。
- 現行の daemon 内 controller が機密 state または CDP pipe を保持したままなら、同 UID の worker が controller を読む別経路が残る。後続実装では CDP pipe と機密 state の処理を専用サービス側に閉じ、daemon には action の結果と秘密を含まない receipt だけを返す。これを含めて検証するまで機密能力を本番解放しない。

## UID/GID map

- 例: host 初期 namespace の `celeris-browser` を UID/GID `B`、daemon を UID/GID 1001、専用 user に新規割当てする subuid/subgid の先頭を `S` とする。内側のセットアップ用 UID/GID 0 → host `B`、Chrome UID/GID 1000 → host `S` の二つの map を使い、Chrome 起動前に内側 UID/GID 1000 へ下げる。host daemon UID/GID 1001 は **写像しない**。既存記録の 165536:65536 は `rmaeda` の割当てと Chrome の実測値であり、`celeris-browser` 用に再利用しない。`B` と `S` は host の実割当てから決め、固定値にしない。
- launcher は host の `celeris-browser` 資格情報になった後で user namespace を作る。namespace 生成、二つの map の設定、mount 等のセットアップ、Chrome への降格を分離して実装する。map はこの user 自身の ID と許可された subordinate 範囲だけを `newuidmap` / `newgidmap` で設定し、GID map 前の `setgroups` 処理を含めて失敗時は起動を止める。現行の `bwrap_args` の `--unshare-user --uid 1000` をそのまま呼び出しても二つの map にはならないため、後続実装で namespace の生成・map・bubblewrap への引渡しを明示的に変更する。host の親 namespace の map が subuid/subgid 範囲を含まなければ拒否する。
- namespace owner は `B` であり、daemon UID 1001 ではない。daemon は owner UID でも Chrome に写像された UID でもなく、子 namespace の capability を持たない。この条件を UID の数値差だけから推論せず、launcher の実行資格情報・namespace の生成経路・`uid_map` / `gid_map` を照合する。
- `RuntimeFacts.runtime_uid != host_uid` と既存の `verify_isolation` は必要条件として維持するが、owner の検査を含まない。後続実装で launcher の証明と `verify_isolation` の両方を本番 admission の条件にする。`SameUidHarness` は試験専用のままとする。

## IPC とライフサイクル

- root が配置する launcher executable は systemd の専用 service で `User=celeris-browser` / `Group=celeris-browser` として動かす。daemon は systemd 管理の Unix socket に接続するだけとし、`SO_PEERCRED` と固定 protocol で要求者、session、許された browser action を検査する。同 UID の worker は `SO_PEERCRED` だけでは daemon と区別できないため、session/lease の照合と操作範囲の制限を必須とし、socket 到達を機密 state の受領権限とみなさない。daemon から任意 argv、bind mount、環境、UID map、FD 番号、実行パスを指定させない。要求のサイズ・同時数・期限も制限する。
- launcher が `bwrap` / sandboxd / Chrome を起動し、CDP pipe と egress 中継の制御 FD を所有する。既存の `--remote-debugging-pipe`、netns 内 loopback listener、接続ごとの proxy、H3 の認証区間・injection-only broker 契約を保つ。CDP pipe、profile FD、開封済み state を daemon UID に返さない。broker は専用 service の peer UID と稼働 session/lease を検証し、秘密を daemon に返さない。
- session は service 側で登録し、starttime と instance id を伴う PID 記録を専用 user だけが書ける場所に置く。daemon 接続の切断、lease 失効、service 停止では CDP と channel を閉じ、process group / cgroup の全子を停止・回収する。再起動時は記録した starttime を照合して孤児だけを回収し、PID 再利用先や稼働中の別 instance を殺さない。launcher が落ちた時は browser を残して継続せず、admission を閉じる。
- helper の socket と状態 dir を Chrome の mount に bind しない。service 側の応答は固定の状態・receipt・非機密の観測だけとし、失敗や診断 log に cookie、credential、CDP payload を残さない。

## 権限境界

| 主体 | host 資格情報と許す操作 | 禁止する操作 |
|---|---|---|
| daemon / worker | UID 1001。launcher socket へ制限付き要求 | browser 用 userns の生成、`celeris-browser` への UID 切替、CDP/機密 state FD の受領、helper の書換え |
| launcher / trusted controller | 専用 UID `B`。namespace 生成、map 設定の依頼、CDP・browser の管理 | daemon の任意コマンドの実行、root 権限での常駐 |
| Chrome / sandboxd | host subuid `S`。内側 UID 1000、必要な `/session` への書込み | host の helper・broker socket・daemon 状態 dir への到達、外向きネットワーク直結 |
| map helper | root が管理する `newuidmap` / `newgidmap` の短時間の特権 | browser 処理、CDP・秘密の保持、任意の未割当て map |

owner UID の process は子 user namespace で `CAP_SYS_PTRACE` を得るため、`celeris-browser` 自体も信頼対象として絞る。`--cap-drop ALL` と `no_new_privs` は Chrome 自身の権限縮小には必要だが、親 namespace にいる owner の権限を消す手段ではない。daemon が `sudo`、setuid helper の汎用インターフェース、systemd の任意 unit 起動権限を通じて `celeris-browser` としてコードを走らせられる設定は採らない。

## ホスト側の準備

以下は root が行う配置・設定作業であり、この ADR の記述だけでは host に適用しない。

1. login 不可の専用 user/group `celeris-browser` を作り、daemon UID 1001 や worker の補助 group と共有しない。launcher の状態 dir は `B` のみ、Chrome の `/session` に bind する dir は割当て先 `S` が書けるよう別々に所有者と mode を決める。socket dir と home も用途別に制限し、browser subuid から helper の executable・設定・秘密へ書けないことを確認する。
2. `/etc/subuid` と `/etc/subgid` に **`celeris-browser` 用**の非重複範囲を新たに割り当てる。既存の `rmaeda:165536:65536` を流用しない。親 `uid_map` / `gid_map` に範囲が入る host であること、`newuidmap` / `newgidmap` の setuid または file capability と root 所有・書換え不可の状態を確認する。map helper の特権は map 設定時だけ使う。
3. launcher executable と固定設定を root 所有、daemon・worker・`celeris-browser` から書換え不可の mode で配置する。launcher を setuid root の汎用実行器にせず、systemd が専用 user で起動する。必要な file capability がある場合は map helper に限定し、launcher / daemon / Chrome に host `CAP_SYS_PTRACE` を付与しない。
4. systemd の `.socket` / `.service` を root が配置する。socket は daemon からの接続だけを許し、service は `User=` / `Group=celeris-browser`、資源制限、専用 cgroup、状態 dir、再起動と停止時の回収を設定する。mapping に setuid helper が要る段階では `NoNewPrivileges` などでそれを妨げないよう段階を分け、map 完了後の browser には `no_new_privs` と capability 全落としを適用する。設定値は実 host で検証して確定する。
5. daemon 側から helper executable・unit・socket path・subuid 範囲を変更できないこと、host UID 1001 の process が `celeris-browser` として実行できないことを権限表と実 process で確認する。

## 移行手順

1. 専用 user / subuid / systemd socket と launcher を準備し、既存の daemon 起動 `bwrap --unshare-user` 経路とは別に試験環境で起動する。新経路がない host では閉じて失敗させ、機密要求を旧経路へ fallback しない。
2. launcher 側の Chrome / sandboxd / egress / controller、FD の所有と `RuntimeFacts` 採取を結線する。既存の `verify_isolation` の全条件、CDP pipe、proxy 経由の正例と直結の負例、復元の H3 観測停止を維持する。
3. `uid_map` / `gid_map`、host Uid、owner の実行資格情報を記録し、daemon UID 1001 の独立 process と worker から Chrome への `ptrace` attach、`/proc/<pid>/environ`・`mem`・`fd` 読取り、helper socket の越権要求を実 process で試す。root/専用 user の正の対照も取り、試験自体が機能していることを示す。A13 の broker `peer_uid_mismatch` と、この ptrace 拒否は別々に判定する。
4. SIGKILL、daemon/launcher 再起動、lease 失効、PID 再利用を含む回収と本番 `Attested` identity 復元の成功経路を試験する。結果を ops / progress と適合記録へ反映し、別途本番 admission の判断を行う。

## 後続段階

この ADR は設計の決定だけであり、launcher の実装、host 設定、systemd unit の配置、実 process 攻撃試験は後続段階で行う。この ADR を根拠に本番 admission の機密能力 `CredentialInjection` / `IdentityRestore` を解放しない。`verify_isolation Ok` と本番 `Attested` 復元成功だけでは十分ではない。daemon UID からの ptrace 拒否、A13 の実 process 試験、controller の秘密非露出、全 lifecycle の回収を揃えてから別の適合・昇格判断を行う。

## 検討した代替案

- **daemon が subuid に写像した userns を直接作る**: Chrome の host UID は変わり、`/proc/<pid>/environ` も拒否されるが、daemon UID が owner のままで `CAP_SYS_PTRACE` を得る。今回の実測で棄却。
- **Yama `ptrace_scope=2` または `PR_SET_DUMPABLE=0` に依存する**: owner の namespace capability による例外を閉じないため棄却。追加の防御として使うことは妨げない。
- **`--cap-drop ALL`、`no_new_privs`、uid_map の変更だけに依存する**: Chrome 自身の capability と owner の capability は別であり、棄却。
- **daemon に host `CAP_SYS_PTRACE` を与える、または root 常駐 launcher にする**: 境界をさらに広げるため棄却。root は配置と限定された map helper のみに使う。
- **container / VM に全面移行する**: 別の隔離境界にはなり得るが、現行の CDP・egress・broker 契約と lifecycle を維持したまま owner 問題を解く最小の変更として、まず専用 user の launcher を採る。VM 方式を将来の選択肢から除外しない。
