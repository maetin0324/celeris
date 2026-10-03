# ADR-0105: P4-A isolated runtime は同一 host UID の bubblewrap で組み、別 UID までは機密解放しない

- 状態: 採用（2026-09-29、人の決定 p4a-uid に従う）
- 関連: ADR-0101 D3、ADR-0102 D1/D4、ADR-0103 D1、ADR-0104

## 文脈

ADR-0103 D1 は bubblewrap + 専用 UID を選んだ。この host では rmaeda の subuid（165536:65536）が親 user namespace の
uid_map の外にあり、newuidmap が EPERM になる。人の決定 p4a-uid は「この host では同一 host UID のまま
user/pid/net/mount/ipc/uts namespace・read-only root・egress・分離・orphan 回収を実装して試験し、別 host UID の
実証は docs/ops の手順書に切り出す。restore_isolated など機密能力は別 UID の実証まで拒否のまま」とした。

## 決定

- D1 起動: `task_worker::browser_runtime` が bwrap の argv を組む。`--unshare-user --unshare-pid --unshare-net
  --unshare-ipc --unshare-uts --unshare-cgroup-try --die-with-parent --new-session --cap-drop ALL --clearenv`、
  sandbox 内 UID/GID は 1000。root は tmpfs で、`/usr`・`/etc` の必要な file（fonts/ssl/passwd/ld.so.cache 等。`/etc` 全体は `/etc/celeris` を含むので bind しない）・browser dir だけを read-only bind し、`/bin` 等は
  symlink。書ける所は host の session dir を bind した `/session` だけで、最後に `--remount-ro /`。`/run`・`/tmp`・
  `/run/user`（agent-browser の control socket 置き場）・broker の path は bind しない。
- D2 事実採取: `--info-fd` の child-pid について host 側で `/proc/<pid>/ns/*` を自 process と比べ、`status` の
  Uid/NoNewPrivs/CapEff/CapPrm、`mountinfo` の root の ro と書ける mount（proc と `/dev` 配下の pseudo fs を除く）を
  読み、`RuntimeFacts` を作って既存の `verify_isolation` に渡す。runtime_uid は host から見た実 UID をそのまま
  入れる。この host では host_uid と等しいので `SameUid` になり attestation は出ない（= 決定どおり機密拒否）。
  検査を弱める分岐は入れない。
- D3 CDP は `--remote-debugging-pipe`（fd 3/4）だけ。fd は controller が持ち、browser の netns の TCP には何も
  listen しない（試験で `/proc/<pid>/net/tcp{,6}` の LISTEN 0 件を確認）。
- D4 orphan 回収: bwrap は `PR_SET_PDEATHSIG=SIGKILL` と独立 process group で起動し、sandbox 内は bwrap の
  `--die-with-parent`。daemon 再起動の回収用に controller dir へ `pid starttime` を記録し、起動時の
  `reap_recorded` が `/proc/<pid>/stat` の starttime 一致を確かめてから process group を SIGKILL する
  （PID 再利用で他 process を殺さない）。
- D5 identity 復元の結合: `LiveIsolation`（task-core）を稼働中 session が実装し、呼ぶたびに事実を採り直して
  attestation を返す。`BrowserIdentityService::restore_for_session` はこれを経由してだけ `restore_isolated` を呼び、
  session 死亡・session id 不一致・隔離違反なら拒否する。agent-browser 0.38.1 の `--restore/--state/--profile`
  は使わない。
- D6 egress: browser の唯一の出口は `--proxy-server` の netns 内 loopback で、その先を ADR-0104 の
  celeris-browser-egress（継承 socket）に繋ぐ。netns 内 listener と host 側 unix socket の中継は本 ADR の次段で、
  中継が無い間の browser は loopback 以外へ出られない（閉じている側に倒れる）。

## 結果

- 同一 UID のため、別 UID の他 process からの ptrace・/proc 読み・同 UID の file への到達を host 側で防ぐ保証は無い
  （sandbox 内からは mount namespace で見えないだけ）。これが機密解放を拒否のままにする理由。
- 別 host UID での確認手順は `docs/ops/browser-isolated-runtime-subuid.md`。
