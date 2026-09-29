# ADR-0084: Browser Phase 4 — isolated runtime・trusted injection・backend routing（P4-A〜C）

---
tasks: [01M3Q49ZTST3XQ9DGF6AGNR0XG]
---

- 日付: 2026-09-29
- 状態: **契約・拒否境界は Accepted（2026-09-29）、runtime 方式・H7 backend は決定待ち**。実 runtime の起動（bwrap の実行・UID の払い出し・filtering proxy の常駐）と本番の配線は未（下記「残り」）
- 関連: [ADR-0078](0078-browser-execution-capability.md) D8、[ADR-0080](0080-browser-phase2-policy-broker-approval.md) H3・H6・H7、[ADR-0083](0083-browser-phase3-identity-contract.md) D3・D4

## 範囲

P4-A / P4-B / P4-C はそれぞれ単独の成果・検査で受け入れる。どれも I/O を持たない判定（純関数）と、
その判定だけが開ける型（attestation・ticket）で「先に契約を確定し、後で利用可能にする」（phases-and-decisions の循環回避）。

## D1. P4-A isolated runtime（`task_core::browser_isolation`）

- runtime の起動側が観測した事実 `RuntimeFacts` を `verify_isolation` が検査し、違反を**全部**返す。要件:
  別 UID（host の UID でも 0 でもない）・user/pid/net/mount/ipc/uts の 6 namespace・root read-only・書き込みは `/session` 配下だけ・
  broker（`/run/celeris/credentiald`、`/var/lib/celeris/credentiald`、`/etc/celeris`、およびそれらの親）と host IPC
  （`/run/user`、`/dev/shm`、X11、dbus、docker.sock）が見えない・CDP は pipe か controller 専用 dir の unix socket（TCP は loopback でも拒否）・
  no_new_privs と capability の全落とし・専用 process group。
- 起動方式の既定案は bubblewrap（`bwrap_argv`: `--unshare-*`・`--die-with-parent`・`--new-session`・`--cap-drop ALL`・最後に `--remount-ro /`）。
  方式は検査に対して差し替え可能で、検査を通ったものだけが隔離を名乗る。
- egress（`check_egress`）: 許可は `host:port` の DNS 名だけ。IP literal（10 進・8 進・16 進の変種、`[::1]`）・大文字や末尾ドットの host・
  解決先の**どれか 1 つ**でも非公開（private・loopback・link-local・metadata 169.254.169.254・CGNAT・予約・multicast、v6 の ULA・link-local・
  v4-mapped / v4-compatible・NAT64・6to4・Teredo）・IPv6（既定で無効）・指定 resolver 以外への DNS（DoT の 853 を含む）・上位 proxy への連鎖を拒否。
- orphan 回収（`orphan_groups`）: 印（session label）付きで runtime の UID の process group のうち、生きている session に属さないものだけを返す。
  daemon の UID・root・印の無い process・pgid ≤ 1 には触らない。
- `IsolationAttestation` は `verify_isolation` からしか作れない（private field）。`isolation()` だけが `Isolation::Isolated` を返す。

## D2. identity 復元の配線（ADR-0083 D3/D4）

- `IdentityService::restore_isolated(…, &IsolationAttestation, …)` を追加。attestation を持つ時だけ `Isolated` で `authorize_use` を通し、
  封緘を開いて controller に返す。HTTP の `restore` は従来どおり trusted local で 403 `isolation_required`（HTTP から attestation は作れない）。
- agent-browser 0.38.1 の `--restore` / `--state` / `--profile` は使わない（D4 のまま）。開いた state の runtime への投入は controller（CDP）が行う。

## D3. P4-B trusted injection（`celeris_credentiald::injection`）

- `CredentialProvider` の trait は変えない。provider を呼べるのは `PeerRole::Injector`（isolated runtime の controller、peer UID で決める）だけで、
  `Worker` / `Agent` は `injection_worker_not_allowed`（`resolve_for_peer` と `TrustedInjector::prepare` の両方）。
- 2 段: `prepare` で session・frame 鎖（top → 注入先の全 frame が exact origin）・redirect 鎖（全部 exact origin）・要素の型
  （password は `type=password`、username は text/email）を検査して ticket を作る。`commit` は直前に読み直した対象と ticket を比べ、
  navigation・document・frame・要素・redirect の 1 つでも違えば**秘密を取り出さずに** `target_changed`（TOCTOU）。
- 認証区間（H3）の外では prepare も commit もしない（`auth_section_required`）。秘密は `InjectionSink` にだけ渡り、戻り値（receipt）に出ない。
- DOM 再表示: password を text input へ入れさせる罠は `redisplay_field`。注入後に page script が値を DOM に書き戻した観測は
  `RedisplayGuard`（salt 付き hash だけを持つ）が検出して観測ごと捨てる。Phase 3 の auth_section 配線（observation 停止）は変更しない。

## D4. P4-C backend routing（`task_core::browser_backend`）

- 能力ごとに要る適合 fixture（`required_cases`）。`certify` は宣言のうち fixture で裏付けられた能力だけを返し、version が違えば取り直し。
  **機密の能力（`CredentialInjection`・`IdentityRestore`）の宣言が P4-A（`IsolationSuite`・`EgressNegativeSuite`）/ P4-B（`InjectionAttackSuite`）・
  H3（`AuthSectionObservationStop`）の fixture で裏付けられない backend は丸ごと routing に使わない**。
- `route`: 明示 → browser-specialist → 既存の loop（同種は id 順）。要る能力を 1 つでも欠く backend は primary にも fallback にもならない
  （能力を落として代替しない）。前に失敗した backend は避ける。明示が使えなければ黙って替えずに `explicit_unavailable`。
- browser-specialist は人の決定（H7）まで `enabled = false` で登録し、既存の loop（ACP / 明示 Claude）を再利用する。
- 同一 task 評価（`rank_same_task`）: 同じ task fixture の結果だけを比べる（混ざれば `None`）。違反 0 → 受け入れ → 復旧の少なさ → 費用 → 時間。

## 残り（人の決定を含む）

## D5. 本番 routing の暫定境界（attempt 2、機密要求の扱いは D6 で訂正）

- worker の browser 起動直前に、実際の effective policy と adapter から P4-C の `route` を呼ぶ。既存 ACP / Claude loop は従来の Phase 1/2 非機密操作だけを候補にする。既存 suite で実行確認済みの操作に対応する fixture だけを登録し、credential 注入・identity 復元は登録しない。
- attempt 2 では非機密操作だけを検査し、旧 plugin bridge の秘密返却経路を Phase 2/3 結合テストのため残した。これは未適合 backend への機密要求を許すため、D6 で廃止する。選択結果が dispatcher が渡した adapter と異なる場合は暗黙の adapter 変更を行わず拒否する。
- H7 の browser-specialist は backend の選択と fixture 実行を人が決めるまで候補へ登録しない。既存 loop への fallback は同じ要求能力を満たす場合だけ候補になる。

- H7: browser-specialist の具体的 backend（Browser Use 等）の選定と有効化。比較は `rank_same_task` の同一 fixture で行う。
- P4-A の実 runtime: runtime 用 UID の払い出し（subuid 範囲）・bwrap の実行と `/proc` からの事実の採取・filtering proxy の常駐・orphan の killpg を worker に配線。
  内部 origin（celeris 自身の API 等）を egress に足す方針は人の決定（既定は足さない＝loopback は拒否のまま）。
- P4-B の実 sink（CDP `Input.insertText`）と broker IPC の peer UID → `PeerRole` の割り当て。

## D6. 未適合の機密要求は起動前に拒否する（attempt 3）

- D5 の「非機密のみの暫定 routing」を訂正する。effective policy の `CredentialUse` は必須能力 `CredentialInjection` に写像する。P4-A/B の適合がない既存 loop は、承認の有無にかかわらずその要求を受け付けない。古い Phase 2 の成功テストを維持する目的でこの判定を迂回しない。
- 判定は policy の確定直後、wait の読取り・承認要求作成・承認消費・substrate 起動より前に置く。拒否で一回承認を消費せず、broker lease を発行しない。公開操作だけの policy は既存 loop を継続利用できる。
- H3 の `auth_section` と Phase 3 の観測停止の実装は維持する。既存の trusted-local 認証成功を期待した結合テストは未適合拒否の負例に更新する。これを P4-B の実 injection 攻撃試験の代用にはしない。
- `IdentityRestore` を宣言する backend にも `InjectionAttackSuite` を要求する。identity だけを宣言して P4-B の適合を回避することはできない。
- 実 runtime 方式と UID の運用、H7 の backend 選択は未決。D1 の bubblewrap は候補であり採用済みではない。決定要求を run の `result.json` に記録し、P4-A の実配線とそれに依存する P4-B、H7 に依存する P4-C の backend 採用はその回答まで進めない。内部 origin は追加しない。
