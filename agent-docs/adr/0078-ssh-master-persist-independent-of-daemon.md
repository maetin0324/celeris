# ADR-0078: クラスタの ssh master の寿命を celeris の停止から切り離し、無駄な再接続をやめ、切断を数えて知らせる

---
tasks: [01M3MQQ9CQT3Q1NQGKPCT4H0BR]
---

- 日付: 2026-09-28
- 状態: **Accepted**（方針のみ。実装は後続の WorkUnit `fix` で行う）
- 関連: ADR-0018 D2（`-O check` と `BatchMode=yes`）、ADR-0032 D2 の訂正（`ControlPersist` で master は自分を
  切り離す）/ D4（TOTP の中継）/ §3（totp の自動維持はしない）、ADR-0040 D4（ライブ切替）/ D5（self ブランチ）、
  ADR-0053 D3（トンネルと `ClusterLoginNeeded`）、ADR-0060（master を celeris の cgroup の外で起こす）、
  **ADR-0062 D2 を一部改める**（probe の失敗で `-O exit` しない。D3 を見よ）

## 1. 文脈

人間の依頼「pegasus の TOTP ログイン要求が多過ぎる。なぜ頻繁に ssh セッションが終了するのか、原因を特定して直して」。
2026-09-28 の調査（journal `--user` の 09-26〜09-28、`ssh -G pegasus`、コード。pegasus への新規ログインは無し）で分かったこと:

| 記号 | 原因 | 区分 | 確度 |
|---|---|---|---|
| C1 | **主因**。`~/.ssh/config` の `ControlPersist 10` を celeris が上書きしない。master は「mux のクライアントが 10 秒いない」と自分で終了する。celeris は 5 秒ごとの `-O check`（`CLUSTER_LIVENESS_INTERVAL`）と remote-exec だけで master を生かしている | 人の設定 × Celeris | 高 |
| C2 | daemon の停止→起動（drain を含めて約 15 秒）の間は `-O check` が止まる → C1 で master が消える。ライブ切替（ADR-0040 D4）は新旧が重なるので生き残った（09-28 12:05 の観測） | Celeris | 高 |
| C3 | tick ループが 10 秒を超えて止まる（host の負荷、実通信 probe の timeout 10 秒 `LIVENESS_PROBE_TIMEOUT` がそのまま tick を塞ぐ）→ C1 で master が消える。観測した 3 回の消失（09-26 03:43、09-28 12:13、17:17 UTC）は全部これで、どれも idle（`in_flight: 0`）のとき | Celeris | 高 |
| C4 | 実通信 probe が 1 回失敗（遅いだけの timeout を含む）すると `-O exit` で**生きている master を自分で閉じる**（`dispatcher.rs` `refresh_cluster_liveness`、ADR-0062 D2） | Celeris | 中（潜在。今回の 3 回には無関係） |
| C5 | `auth = "totp"` のクラスタでも、トンネルのために約 6 秒ごとに鍵認証の新しい接続を張る（`ensure_cluster_master_for_tunnel` → `cluster_connector`）。TOTP が要るので成功しない。2 日で 7,461 回 | Celeris | 高（切断の原因ではない） |
| P1 | C5 の大量の認証失敗に対する pegasus の sshd 側の制限（MaxStartups 等）が、GUI の TOTP 接続で「プロンプトが来ない」4 回を起こした可能性 | pegasus 側 | 低〜中（pegasus のログが無く未確認） |
| P2/P3 | pegasus の idle timeout・MaxSessions | pegasus 側 | 低（keepalive 30 秒で idle にならない。寿命 2h22m / 1h09m / 4m31s が不揃いで固定の上限と合わない） |
| S1 | ControlPath を `~/.ssh/mux-…` → `/run/user/1001/ssh-mux-…` に変えた（09-25、1 回限り） | 人の設定 | 低（継続的な原因ではない） |

cgroup による巻き添え（ADR-0060）と keepalive（ADR-0062 D1）は既に直っている。残っているのは「master が celeris の
**mux 接続の途切れ**に寿命を握られている」ことで、ADR-0060 が直した「celeris の**プロセス**に寿命を握られている」とは別の経路である。

## 2. 決定

### D1. master の argv で `ControlPersist=yes` を明示する（C1・C2・C3。主因）

- `crates/task-worker/src/cluster_login.rs` の master の argv（`start_publickey` / `start_totp` の両経路、`-M -N` の前）に
  `-o ControlPersist=yes` を足す。組み立ては `keepalive_args` と同じく純関数（例 `persist_args`）にする。
  コマンドラインの `-o` は `~/.ssh/config` より優先されるので、**人の設定を変えずに celeris が張る master にだけ効く**
  （ADR-0062 D1 の keepalive と同じ流儀。ADR-0018 D7「ssh の設定は人の `~/.ssh/config` が正」の例外は、
  この 2 つの寿命に関する値だけに限る）。
- `yes` にすると master が終わるのは次の場合だけになる: 明示的な切断（`DELETE /clusters/{id}/connect` →
  `ClusterMaster::kill` + `ssh -O exit`）、keepalive による断の検出（30 秒 × 3 = 最大 90 秒）、pegasus 側からの切断、
  手元 host の再起動・user manager の停止。**daemon の停止→起動、ライブ切替、tick の停止には左右されない**。
- `[[clusters]] control_persist`（既定 `"yes"`。`"yes"` か正の秒数だけを受ける。それ以外は `Config::validate` のエラー）を
  逃げ道として置く。既定を変える運用は想定しない。
- `ControlPersist=no` で celeris が子を抱え続ける案は採らない（§3）。
- `scripts/cluster-login.sh`（人が張る経路、ADR-0032 D7）の `ssh -M -N -f` にも `-o ControlPersist=yes` と
  keepalive（`-o ServerAliveInterval=30 -o ServerAliveCountMax=3`）を足す。

#### daemon 再起動・リリース切り替えでの扱い（D1 と ADR-0060 の合成）

| 出来事 | 修正前 | 修正後 |
|---|---|---|
| 停止→起動（同じ sha・別 sha） | cgroup の巻き添えは ADR-0060 で解消済みだが、`-O check` が 10 秒以上止まるので C1 で消える | 残る。新しい daemon は起動直後の `-O check`（ADR-0018 D2 / ADR-0060 D3）で既存の master を見つけ `connected=true` にする |
| ライブ切替（ADR-0040 D4） | 重なりがあれば残る（偶然に依存） | 残る |
| tick の 10 秒超の停止 | 消える（観測 3 回） | 残る |
| host 再起動 | 消える | 消える（避けられない。TOTP 1 回） |

### D2. ControlPath は人の設定のまま使い、起動時に置き場所を検査するだけにする

- celeris は `ControlPath` を上書きしない。人の `ssh`・`scripts/cluster-login.sh`・celeris が**同じ master を共有**する
  ことが ADR-0018 D2 の前提で、celeris だけ別のパスにすると人が張った master を借りられなくなる。
- 現在の `/run/user/1001/ssh-mux-…` はリリースのディレクトリ（`~/.local/celeris/releases/<sha>`）と無関係で、
  リリース切り替えでは変わらない。`/run/user/<uid>` は tmpfs だが `Linger=yes` なのでログアウトでは消えない。
- 起動時（`wire_cluster_liveness_hooks` と同じ本番の起動経路だけ）に、クラスタごとに `ssh -G <host>`（設定の展開だけで
  通信しない）で実効の `controlpath` / `controlmaster` / `controlpersist` を読み、次を **warn で 1 回**残す（起動は止めない）:
  - `controlpath` が `none`、または `controlmaster` が `no`（celeris が借りられない）
  - パスの長さが 90 バイトを超える（unix socket の上限 108 から、ssh が作成時に付ける一時の接尾辞 17 文字を引いた余裕）
  - 親ディレクトリが自分の所有・0700 でない
  - パスが `XDG_RUNTIME_DIR` の下で、`loginctl show-user` の `Linger` が `yes` でない（ログアウトで消える）
  - 人の設定の `controlpersist` の値（D1 で上書きしていることを明示するため。info）
- 判定は純関数（`ssh -G` の出力 → 警告の列）にしてテストする。

### D3. 無駄な再接続と、生きている master を閉じる操作をやめる（C4・C5・P1）

1. **totp のクラスタでは、鍵認証による自動再接続を切断 1 回につき 1 回までにする**（`ensure_cluster_master_for_tunnel`）。
   `connected` が true→false に変わった直後に 1 回だけ試し（ADR-0032 D3「TOTP の前に鍵認証」を保つ）、失敗したら
   `login_needed` を立てて、**人が GUI で接続するか `-O check` が生存を返すまで再試行しない**。
   `-O check`（ローカルの unix socket を見るだけ）は 5 秒ごとに続けるので、人が端末で張った master も従来どおり拾える。
   これは ADR-0032 §3「totp に接続の自動維持を広げない」と揃えるもので、`try_auto_connect_cluster` の判断（`auth != "publickey"`
   は自動接続しない）とも一致させる。
2. **publickey のクラスタの自動再接続は指数バックオフ**（6 秒から倍々、上限 5 分。接続が戻ったらリセット）。
3. **実通信 probe の失敗では `-O exit` しない**（ADR-0062 D2 を改める）。probe が失敗したら `connected=false` と cooldown
   だけにし、master には触らない。連続 3 回失敗したときだけ「死んだ」と確定して D4 の通知を出す。
   本当に TCP が死んでいれば master は keepalive（D1 と ADR-0062 D1）で 90 秒以内に自分で終了するので、`-O exit` による
   片付けは要らない。遅いだけの timeout で TOTP が要る master を捨てる害の方が大きい。
4. **probe を tick から外す**（C3 の一因）。`LIVENESS_PROBE_TIMEOUT`（10 秒）の probe は別スレッドで走らせ、結果だけを次の tick で
   拾う。tick が probe を待って止まらないようにする。D1 の後はこれが master の寿命には効かないが、tick の停止そのものを減らす。

### D4. 切断を検知したら人に分かる形で 1 回知らせる

- 検知の点は 1 か所: dispatcher の `cluster_connected` の **true→false の遷移**（`-O check` 失敗・probe の連続失敗・
  `ClusterMasterExited` のどれでも通る）。切り離された master の終了は `ClusterMasterExited`（子を見る方式）では
  捕まらないので、この遷移で拾う必要がある。
- 知らせ方は既存の経路に乗せる（新しい通知の種類は作らない）:
  - totp のクラスタ: 既存の `NotificationKind::ClusterLoginNeeded`（ADR-0053 D3。outage ごとに 1 件、復旧すれば次は新しい報告）。
    本文に「いつ切れたか」「接続していた時間」「推定の理由（D5 の `cause`）」「GUI の『クラスタ』画面から TOTP を入れて再接続」を書く。
  - publickey のクラスタ: D3-2 のバックオフで 3 回続けて戻らなかったときだけ、同じ `ClusterUnavailable` の報告（ADR-0062 D3）を出す。
- GUI の「クラスタ」画面の各クラスタに、D5 の回数と「最後に切れた時刻・理由」を出す（表示の追加だけ。画面の構成は変えない）。

### D5. 切断・再接続の回数を log と記録に残す

- **log（tracing。journal で数えられる固定の文言と構造化フィールド）**:
  - `cluster ssh master connected` (info): `cluster`, `host`, `method = totp|publickey|borrowed`（borrowed = 人が張った master を `-O check` で見つけた）, `attempt`
  - `cluster ssh master lost` (warn): `cluster`, `host`, `cause = check_failed|probe_failed|master_exited|explicit`, `uptime_secs`, `last_tick_gap_ms`（直前の tick 間隔。C3 の再発を見分ける）
  - `cluster key-auth reconnect attempt` (info、totp は切断ごとに最大 1 行): `cluster`, `ok`, `backoff_secs`
  - `cluster totp prompt relayed` (info): `cluster`（コードやプロンプトの中身は書かない）
- **記録（DB の Event。daemon 再起動をまたいで数えるため）**: 既存の `ClusterMasterExited` / `ClusterUnavailable` に加えて、
  `ClusterConnected { cluster, method }` と `ClusterMasterLost { cluster, cause, uptime_secs }` の 2 つを足す
  （`EVENT_TYPES` の要素数のテストを合わせて更新する）。
- **metrics**: `GET /api/v1/clusters` の各クラスタに `stats` を足す。直近 24 時間と起動以降の
  `connects_totp` / `connects_publickey` / `connects_borrowed` / `losses`（`cause` 別）/ `key_auth_attempts` /
  `last_lost_at` / `last_lost_cause`。24 時間の値は D5 の Event から数える（in-memory のカウンタは再起動で消えるため）。
  Prometheus などの外部の仕組みは入れない。

## 3. 採らない

- **`ControlPersist=no` で celeris が master を子として保持する**。celeris が再起動すると stderr のパイプの読み手が消え、
  forward の失敗などで ssh が stderr に書いたときに SIGPIPE で落ちうる。寿命がまた celeris に結び付く。
- **master 用に別の常駐 unit（`celeris-ssh-master@.service`）を作る**。ADR-0060 の scope で cgroup は既に分かれており、
  D1 で idle による終了も無くなるので、unit を増やす利点が無い。TOTP の中継（ADR-0032 D4 の FIFO）も unit 越しでは複雑になる。
- **celeris が `ControlPath` を自分の場所に上書きする**（D2）。
- **人の `~/.ssh/config` を celeris が書き換える**。変更は人に提案するだけ（§4）。
- **TOTP の種を預かる**（ADR-0032 §3 のまま）。

## 4. 人の ssh 設定と Celeris 側の切り分け

**Celeris 側だけで済む（必須の修正はすべてこちら）**: D1〜D5。人の `~/.ssh/config` を変えなくても、celeris が張る master は
`ControlPersist=yes` と keepalive がコマンドラインで効く。

**人の設定（任意。適用するかは人が決める。celeris は書き換えない）**: 人が端末で `ssh pegasus` したときの master は、今は
10 秒で消える。これも長く持たせ、celeris が借りられるようにしたい場合の変更案（sirius も同じ構成なので同じ変更を勧める）:

```
Host pegasus
    ...
    ControlPersist 8h          # 10 → 8h（または yes）
    ServerAliveInterval 30     # 0 → 30
    ServerAliveCountMax 3
```

ControlPath は今の `/run/user/1001/ssh-mux-…` のままでよい（長さ 57 文字、0700、Linger=yes）。変えると、その瞬間に
それまでの master が見えなくなる（1 回だけ TOTP が要る）。

## 5. TOTP の要求の見込み（修正前 → 修正後）

- **修正前**: 接続は「celeris の mux 接続が 10 秒途切れる」まで（観測では 4 分〜2 時間半）。daemon の停止→起動でも消える。
  09-28 の約 8 時間で TOTP の成功 2 回、消失 2 回、プロンプトが来ない時間切れ 4 回。
- **修正後**: TOTP が要るのは、初回の接続、網の断（keepalive で 90 秒以内に検出）、pegasus 側の切断、手元 host の再起動、
  明示的な切断のあとだけ。**celeris の再起動・リリース切り替え・負荷による tick の停止が理由の TOTP はゼロになる見込み**。
  観測した 3 回の消失はどれも celeris 側の途切れなので、同じ条件なら TOTP は「1 日に初回の 1 回」程度になる。
  C5 の鍵認証の連打（2 日で 7,461 回）は切断ごとに最大 1 回になり、P1 が当たっていれば「プロンプトが来ない」失敗も消える。
  D5 の `losses` と `connects_totp` で、この見込みが当たったかを後から数えて確かめる。

## 6. テスト方針

テストは実 ssh・pegasus に出ない（CLAUDE.md、ADR-0062 D2）。既存の偽 ssh / 偽 `systemd-run` と、dispatcher のフック差し替えを使う。

1. **argv（純関数）**: `start_publickey` / `start_totp` の両方で `-o ControlPersist=yes` が `-M -N` より前にある。
   `control_persist` の設定値が反映され、不正値は `Config::validate` のエラー。keepalive の既存テスト（`keepalive_args`）と同じ形。
2. **`scripts/cluster-login.sh`**: 偽 `ssh`（受け取った argv を書き出すだけ）を `PATH` の先頭に置いて実行し、`ControlPersist=yes` と
   keepalive が渡ることを確かめる。
3. **ControlPath の検査（D2、純関数）**: `ssh -G` の出力の例から、`none`・90 バイト超・Linger 無しがそれぞれ警告になり、正常な例は警告なし。
4. **dispatcher（フック差し替え）**:
   - probe が 1〜2 回失敗しても disconnector（`-O exit`）が呼ばれず、3 回連続で初めて「lost」になる。
   - totp のクラスタで `-O check` が false のまま 100 tick 回しても、connector の呼び出しは 1 回だけ。publickey は間隔が倍々で 5 分で頭打ち。
   - true→false の遷移 1 回につき、`ClusterMasterLost` の Event 1 件、通知 1 件、`stats.losses` +1。false のままの tick では増えない。
     false→true で `ClusterConnected` 1 件（`method` が正しい）。
   - probe が tick を塞がない（probe フックが 10 秒眠っても tick が即座に戻る）。
5. **再起動をまたぐ回数**: Event から数えた 24 時間の `stats` が、dispatcher を作り直した後も同じ値を返す。
6. **本番での確認（人の操作が要る。TOTP は 1 回だけ）**: 検証済みのリリースを人が昇格したあと、人が GUI で pegasus に 1 回接続する。
   その後 `systemctl --user restart celeris@<sha>`（停止→起動）を 1 回と、次のライブ切替 1 回を経ても、`GET /api/v1/clusters` の
   pegasus が `connected=true` のまま、`stats.connects_totp` が増えず、journal に `cluster ssh master lost` が出ないことを確かめる。
   確認に使う ssh は `ssh -O check pegasus`（ローカルの socket を見るだけ）に限る。

## 7. 実装での補足（2026-09-28、WorkUnit `fix`）

実装で次の 3 点を D の文言から具体化した（方針は変えない）。

1. **D5 の「記録」は `events` ではなく新しい表 `cluster_connection_log`（migration 0030、schema 30）に置く**。
   `Event` は `append_event(task_id, …)` でタスクに紐づけて積む作りで、クラスタの接続・切断はどのタスクにも属さない
   （idle のときにこそ起きる。観測した 3 回はどれも `in_flight: 0`）。追記だけの表にして、`GET /clusters` の
   `stats.last_24h` はそこから数える。`ClusterConnected` / `ClusterMasterLost` の Event と `EVENT_TYPES` の変更は行わない。
   行の `kind` は `connected`（`method` = `totp|publickey|borrowed`）/ `lost`（`cause`、`uptime_secs`）/ `key_auth_attempt`（`cause` = `ok|failed`）。
2. **D3-3 の「probe の失敗で `connected=false`」は、連続 3 回に達したときにだけ行う**。1〜2 回の失敗は warn の log と数だけにして
   接続中のまま扱う（D4 の検知点〈true→false の遷移〉を 1 か所に保つため。遅いだけの timeout で dispatch を止めない）。
   3 回で lost（`cause = probe_failed`）にした後は、`-O check` が通っても probe が 1 回成功するまで接続中に戻さない。
   `-O exit` の片付けフック（`Dispatcher::set_cluster_disconnector`）は使い道が無くなったので外した。
3. **D4 の totp の通知**は、forward を持つクラスタでは鍵認証の 1 回の試行が失敗した時点で、forward を持たない
   （`ensure_cluster_master_for_tunnel` を通らない）クラスタでは true→false の遷移の時点で、`ClusterLoginNeeded` を 1 件出す。
   `auth = "manual"` は totp と同じ扱い（切断 1 回につき鍵認証 1 回）。`method = totp` は「GUI の接続が終わった直後の
   false→true」で判定する（`set_cluster_connect_pending(false)` が印を立てる）。
