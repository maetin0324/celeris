# ADR-0095: worker の run から本番 DB は読み取り専用、celerisctl は migration をしない

- 日付: 2026-09-30
- 状態: **Accepted（Phase R7-6 で実装）**
- 関連: [ADR-0013](0013-taskd-api-and-gui-foundations.md) D5（schema 版数・`SchemaTooNew`）、
  [ADR-0064](0064-db-local-disk-and-store-resilience.md)（DB の置き場所・WAL・読み取りプール）、
  [ADR-0054](0054-stateful-sessions-and-streaming-chat.md) D2（CoS run の codex read-only sandbox）、
  [ADR-0075](0075-tiered-build-cache.md)（コンテナ実行 `container::wrap`）、
  [ADR-0062](0062-ssh-master-keepalive-and-cluster-tool-routing.md) Phase 108 付記（sandbox 内の ssh の "Bad owner"）
- 番号: main と全ブランチの `docs/adr/` で 0094 まで使用済み（0094 は 2 本）なので 0095。

## 文脈

本番 2026-09-30 22:37:52Z（docs/progress/phase-R.md R7-5 の 22:15〜23:17Z 追記）: browser task 01M3SPN8H05EJ3DHPVEGEYTMEH の run
01M3T7CR3V4R0Q1CQX5QKNN0GG（adapter **codex**、`-c sandbox_mode="workspace-write"`、`--add-dir` は artifacts とリポジトリの git 管理領域、
`[adapters.codex] extra_args = ["--approve-for-me"]`）が、ブランチでビルドした `target/debug/celerisctl add --db /var/lib/celeris/celeris.sqlite3 …`
を実行した。celerisctl は `SqliteStore::open` で DB を開き、**開くだけで** migration を適用する（`from_connection` → `migrate`）。ブランチにしか
無い 0035 が本番に入り schema 35 になり、schema 34 の daemon は再起動できない状態になった（verify も `db schema version 35 is newer than 34`
で失敗）。1 回目の `add` は入力検証で exit 1 だったが、検証より前の open で migration は済んでいた。人が backup を取って巻き戻した。

### なぜ codex の workspace-write が止めなかったか（調査結果）

- **DB は writable root の外だった。** codex のセッション記録（`~/.local/celeris/codex-accounts/chatgpt_plus_personal/sessions/2026/09/30/
  rollout-…-01a0f476-6364-77e1-8d2c-6dc61879d1da.jsonl`）の permissions 指示に writable roots が列挙されている:
  `/home/rmaeda/workspace/agent-platform/.git`、`…/.git/worktrees/agent-platform38`、`/tmp`、`/var/lib/celeris/workspaces/01M3SPN8H05…`、
  その `artifacts`。`/var/lib/celeris` 自体も `/var/lib/celeris/celeris.sqlite3` も入っていない（`--add-dir` は artifacts と git 管理領域だけ）。
- **codex の sandbox もこのホストで効く。** `codex sandbox -P :workspace -- touch ~/.cache/…` は `Read-only file system`（`NoNewPrivs: 1`、
  `Seccomp: 2`）。sandbox の不在ではない。
- **原因は escalation。** モデルはこのコマンドを `exec_command({…, sandbox_permissions: "require_escalated", justification: "…Celeris DB に
  起票してよいですか？", prefix_rule: ["target/debug/celerisctl","add"]})` で出した（rollout の ordinal 66 / 73）。`--approve-for-me` は codex-cli
  0.157.0 の `codex exec --help` で "Route approval requests through automatic review using the workspace-write sandbox"、つまり escalation
  要求を自動レビュー（`approvals_reviewer = "auto_review"`）に回し、承認されたコマンドは **sandbox の外で**実行される。自動レビューは
  「人が指定した後続 task の起票」を正当と判断した。同じ run は `systemd-run --user …`（cargo test の gate）も escalation で走らせている。
- 結論: エージェント CLI の内側の sandbox は、そのエージェント自身が escalation で外せる。**強制は adapter のプロセスの外側
  （celeris が起動する時点）で掛けなければならない。** claude-code・opencode・aider など他の adapter はそもそも DB への書き込みを
  止める仕組みを持たない。

## 決定

### D1. worker の run は「DB のディレクトリが読み取り専用の mount namespace」で起動する

celeris が task のために起動するプロセス（下の D2）は、exec の直前（`pre_exec`、fork 後の子）に次を行う:

1. `unshare(CLONE_NEWUSER | CLONE_NEWNS)`。`/proc/self/setgroups` = `deny`、`uid_map` / `gid_map` は自分の uid/gid を自分へ 1 対 1。
   特権は要らない（unprivileged user namespace）。
2. `/` を `MS_REC | MS_PRIVATE`（この namespace の mount を外へ伝播させない）。
3. DB のディレクトリ `P`（`[db] path` の親、canonicalize 済み）の**直下の項目のうち DB 本体とその `-wal` / `-shm` / `-journal` 以外**
   （ディレクトリと通常ファイル。symlink は辿らない）を、それぞれ自分自身へ `MS_BIND | MS_REC`。
4. `P` を自分自身へ `MS_BIND | MS_REC` し、その最上位だけを `MS_REMOUNT | MS_BIND | MS_RDONLY` にする（元の mount の
   `nosuid`/`nodev`/`noexec`/atime 系の flag は user namespace では外せないので `statvfs` の値を引き継ぐ）。3 の bind は
   下位 mount なので書ける。
5. cwd を**絶対パスで** `chdir` し直す（std は `pre_exec` より前に cwd へ `chdir` するので、そのままだと cwd は mount の下の古い
   dentry を指し、`../../celeris.sqlite3` が書ける元の mount に届く）。
6. exec。exec で uid が 0 でないので capability は全て落ちる（`CapEff: 0`）。

結果（本番の形 `/var/lib/celeris/{celeris.sqlite3,-wal,-shm,workspaces/,scratch/,memory/,build-cache/,…}`）:

- `celeris.sqlite3` / `-wal` / `-shm` への書き込みは `EROFS`。SQLite では `attempt to write a readonly database`（`SQLITE_READONLY`）。
  `rm`・`mv`・新しい `-journal` の作成・`P` 直下への新規作成も `EROFS`。別 mount への hard link は `EXDEV`。
- `workspaces/`・`scratch/`・`memory/` などは従来どおり書ける（3 の bind）。`/tmp`・`$HOME` など `P` の外は影響なし。
- namespace の中から戻せない: 子の user namespace を作って `mount -o remount,rw` / `umount` しても、親から受け継いだ mount は
  locked（`MNT_LOCK_READONLY`）で `EPERM`（実測）。
- daemon の `/proc/<pid>/fd/N` から DB を開き直す迂回も塞がる: 子 user namespace からは親の namespace のプロセスの
  `/proc/<pid>/fd` の readlink/open が ptrace のアクセス検査で `EACCES`（実測）。
- 入れ子の sandbox は動く: `codex sandbox -P :workspace`（bwrap）と `bwrap --dev-bind / / true` が namespace の中で成功（実測）。
  pivot_root / chroot をしないので「chroot された プロセスは user namespace を作れない」制限に当たらない。

**プロセスの継承で効くので、adapter の種類・エージェント CLI の sandbox の有無・escalation・子プロセス（`sh`、`cargo`、
`target/debug/celerisctl`、`sqlite3`、python）に関係なく掛かる。**

### D2. 対象のプロセス

`task_worker::db_guard::launch(command, container)` を、worker の run のプロセスを spawn する全ての所で `container::wrap` の
代わりに呼ぶ（コンテナでなければ D1 を掛け、コンテナなら従来どおり `container::wrap`）:

- adapter: fake（`subprocess::run_subprocess`）、claude-code、codex、aider、acp（opencode）、langmem、local_deep_research、
  paperqa（acquire / ask の 2 つ）。planner / reviewer / CoS / 管理画面の probe も同じ adapter を通るので含まれる。
- `LocalWorkspace::exec`（WU の check、受け入れ条件の check、merge probe）。
- remote-exec（`.celeris/remote-exec`）はエージェントのプロセスの子として走る ssh なので含まれる（クラスタ側からはこのホストの DB に届かない）。

含めないもの（worker の run ではない daemon 自身の処理）: dispatcher の git（統合）、配送の release 準備、self-deploy、rsync/ssh の
同期（`SshWorkspace::run_command`）、ssh master、ログイン検査。

コンテナ実行（`[containers]`）には D1 を掛けない。コンテナの中は `-v` で渡した task のディレクトリとキャッシュしか見えず、DB の
ディレクトリは mount しない（それ自体が隔離）。rootless podman は内部で `newuidmap`（setuid）を使うので、D1 の user namespace の
中では起動できない（setuid が効かない）ことも理由。

### D3. WAL と読み取り

読み取りは従来どおりできる: `celerisctl --db /var/lib/celeris/celeris.sqlite3 show|ls …`、`sqlite3 'file:…?mode=ro'`。SQLite は
書けない DB ファイルを `SQLITE_OPEN_READWRITE` で開くと自動で読み取り専用に倒し、WAL の DB は `-shm` が既にあって書けなければ
読み取り専用の共有メモリ（3.22 以降の readonly_shm）で読む。-shm が要るのは daemon がその DB を開いている間は常に満たされる
（daemon が WAL の接続を保持し、`-wal` / `-shm` を残す）。ディレクトリ単位で読み取り専用にするので、daemon が `-wal` / `-shm` を
作り直しても worker 側から新しいファイルが見える（ファイル単位の bind だと古い inode に固定され、checkpoint と組み合わさると
壊れた読み取りになり得るので採らない）。daemon が止まっていて `-shm` が無いときは読み取りも失敗する（run は daemon が動いて
いる間しか無いので実害なし）。

### D4. ssh の設定の所有者

user namespace の中では root 所有のファイルが `nobody`（65534）に見え、ssh は `/etc/ssh/ssh_config` が Include する
`/etc/ssh/ssh_config.d/*.conf` を "Bad owner or permissions" で拒否する（このホストで `ssh -G github.com` が失敗、実測。ADR-0062
Phase 108 の codex sandbox と同じ現象）。git over ssh やエージェントが直接打つ ssh が壊れないよう、D1 の namespace では
`/etc/ssh/ssh_config.d` に**同じ内容の、daemon の uid が所有する写し**（`$XDG_RUNTIME_DIR/celeris-db-guard/ssh_config.d`、
無ければ temp dir。spawn ごとに内容を同期）を bind する。内容は変えない。

### D5. 設定と fail-closed

- `[db] worker_read_only = true`（既定）。`false` は明示的な opt-out（D1 を掛けない。非推奨。user namespace を使えない環境だけ）。
- daemon（`run`、verify モードを含む）は起動時に D1 を 1 回実際に試す（`true` を namespace 付きで起動）。失敗したら
  **起動しない**（`worker db guard unavailable: … set [db] worker_read_only = false only if …`）。黙って保護なしで走らない。
  verify で試すので、効かないホストでは release の verify が落ち、昇格前に分かる。
- 実装はプロセス全体の 1 つの設定（`db_guard::install`）。DB の保護は provider の行ごとに変わらない daemon の不変条件なので、
  adapter の config ごとに配らない。`build_dispatcher`（テストから広く呼ばれる）では入れず、`run` だけが入れる。
- spawn の準備（直下の列挙など）に失敗したら、その spawn を失敗させる（保護なしで起動しない）。例外は守る DB ファイル
  そのものが無いときだけ（同じプロセスで先に終わった in-process の daemon が入れたガードの残り。守るものが無い）。
  本番の daemon の DB は動いている間に消えず、namespace の中からは消せない。

### D6. celerisctl は migration をしない

`task_core::SqliteStore::open_client(path)`（celerisctl が DB を開く全ての所で使う。daemon・API・MCP は従来の `open_with`＝migration あり）:

1. DB ファイルが無ければ失敗（作らない。`SQLITE_OPEN_READ_ONLY` で開いて版数を読むだけ）。
2. 版数（`schema_migrations` の最大。表が無ければ 0）を読み、
   - **古い**（`< SCHEMA_VERSION`）→ `StoreError::SchemaTooOld`。「celerisctl は migration をしない。daemon（celeris）がこの DB で起動すると
     migrate する」。読み取りも拒否（列が足りず、どのみち誤った結果か SQL エラーになる）。
   - **同じ** → 従来どおり読み書き（migration の関数は呼ばない）。
   - **新しい**（`> SCHEMA_VERSION`）→ **読み取り専用の接続**（`SQLITE_OPEN_READ_ONLY`、`journal_mode` は変えない）で開き、stderr に
     警告を出す。書き込みは SQLite が `SQLITE_READONLY` で拒否し、celerisctl は「この DB の schema N はこの celerisctl の M より新しいので
     書き込みはしない」と言い換える。読み取りを許す理由: 新しい schema は列・表を足す向きに進み、celerisctl の SELECT は列名を明示するので
     足された列は無視される。消えた・改名された列は SQL エラー、未知の enum 値は deserialize エラーで**大きく**失敗し、黙って誤ることは
     少ない。本番が先に上がって worker の手元の celerisctl が古い、という向きで `show` / `ls` を使えなくする実害の方が大きい。
     書き込みは古いバイナリが新しい不変条件（NOT NULL の列・新しい状態）を知らずに壊し得るので拒否する。
3. 書き込みが `SQLITE_READONLY` で落ちたとき（D1 の namespace、または 2 の新しい DB）は、celerisctl は
   「DB は読み取り専用で開かれている（worker の run からは本番 DB に書けない、ADR-0095）。変更は HTTP API か人に頼む」と出す。

daemon の migration の挙動（`open_with`: 古ければ適用、新しければ `SchemaTooNew`）は変えない。

### D7. 試験の準備

celerisctl のバイナリを使う試験・スクリプトは、DB を `task_core::SqliteStore::open`（＝daemon と同じ migration）で先に作る。
celerisctl に `db init` / `db migrate` のような migration の入口は**足さない**（「celerisctl は migration をしない」を例外なく保つ）。

## 残る穴（受け入れる）

- **namespace の外で実行させる経路**: `systemd-run --user …`（user manager が namespace の外で起動する）、`ssh localhost`、D-Bus で
  起動されるサービス。worker は同じ uid なので、これらを通せば書ける。今回の事故の形（誤った判断で直接 `celerisctl add --db` を打つ）は
  D1 で止まる。意図的な回避まで止めるには DB を別 uid の所有にする必要があり（daemon の実行ユーザーの分離）、別の課題。
- **HTTP API**: worker が API token を持っていれば API で書ける。これは設計どおり（daemon が検証し、schema も daemon のもの）。
- **コンテナ以外で D1 を外す設定**（`worker_read_only = false`）は人が明示したときだけ。

## 却下した案

- **codex の設定だけ直す**（`--approve-for-me` をやめる、writable roots を絞る）: 他の adapter を守らず、codex でも escalation /
  `danger-full-access` / 将来の flag の意味の変化で外れる。
- **API だけにする**（worker から DB を見せない）: 人が明示的に却下（`celerisctl show` / `ls` を worker から使えることが要る）。
- **celerisctl だけ `mode=ro`（環境変数で指示）**: `sqlite3`・python・別のビルドの celerisctl・環境変数を消したプロセスで外れる。
  OS で強制しないと「書き込みは失敗する」を満たさない（D6 の read-only は補助で、強制は D1）。
- **bwrap で包む**: 実測で、bwrap の中では codex の sandbox（bwrap）が "No permissions to create new namespace" で起動しない
  （bwrap は pivot_root し、chroot された扱いのプロセスは user namespace を作れない）。codex / claude-code の sandbox を壊す。
- **Landlock**: Landlock の domain の中では mount(2) / pivot_root(2) が禁止され、入れ子の bwrap（codex の sandbox）が動かない。
  また「DB 以外は全部書ける」を許可リストで表すには経路上の兄弟を全部列挙する必要があり、後から増えた兄弟が書けなくなる。
- **ファイル単位の read-only bind**（DB・-wal・-shm だけ）: D3 のとおり、daemon が -wal / -shm を作り直すと古い inode に固定される。

## 昇格時に人がすること

- 設定の変更は不要（既定で有効）。`/var/lib/celeris` 直下に**新しい**ディレクトリを足した場合、それは次に起動する run から書ける
  （spawn のたびに直下を列挙する）。
- 昇格後、worker から `celerisctl --db /var/lib/celeris/celeris.sqlite3 ls` が動き、`add` が「読み取り専用」で失敗することを確認できる。
- `promote.sh --pre-start` のフックで新しいリリースの celerisctl を DB に使う場合、リリースに migration があると celerisctl は
  古い DB を拒否する（D6）。そのようなフックは daemon の migration の後に回す（現行のフックは ADR-0046 の一度きりのもので、既に不要）。

## 付記（2026-10-02）: user systemd bus の遮断

### 事故

- **2026-10-01 18:32Z**: web dogfood の task の codex run が `systemctl --user` で本番 daemon（`celeris@<sha12>` の user unit）を
  ブランチのビルドに差し替えた（docs/PROGRESS.md 8dc45bd3 の記録）。「残る穴」に挙げていた `systemd-run --user` と同じ経路で、
  user manager は D1 の namespace の外にいるので、namespace の中の読み取り専用は効かない。
- 同じ run で codex の `--approve-for-me`（`approvals_reviewer = "auto_review"`）が `sandbox_permissions: "require_escalated"` の
  要求を justification の文言だけで自動承認した。上の「なぜ codex の workspace-write が止めなかったか」と同じ構造。
- **18:26〜18:28Z**: テスト（`task_worker::detach` の試験と `crates/celeris/tests/releases_api.rs`）が本番の user manager に実
  `systemd-run --user --scope` の scope を 8 件作った。テストの実行環境が本番の user session を継承していたため。

### D-a. worker の run と check から user systemd bus を見えなくする

`db_guard::launch` を通る全ての spawn（D2 の対象: 全 adapter・`LocalWorkspace::exec` の check・remote-exec の親）で:

1. **環境変数**: 子の環境から `DBUS_SESSION_BUS_ADDRESS` を除く（`Command::env_remove`）。namespace の有無・コンテナか否かに
   関係なく常に行う。`XDG_RUNTIME_DIR` は残す（D4 の ssh_config の写しや他の道具が使う。bus は下の 2 で塞ぐ）。
2. **mount**: D1 の namespace の中で、exec の前に
   - `$XDG_RUNTIME_DIR/systemd`（`systemctl --user` の直結経路 `private` と transient unit の置き場がある**ディレクトリ**）を
     `tmpfs`（`size=0` 相当の小さいもの、`mode=0700`）で覆う。
   - `/run/user/$UID/bus`（D-Bus の **socket ファイル**。ファイルの上に tmpfs は張れない）を空のファイル（`/dev/null` ではなく
     空の通常ファイル。open しても connect できない）の bind で覆う。`$XDG_RUNTIME_DIR` が `/run/user/$UID` と違う場合は
     `$XDG_RUNTIME_DIR/bus` も同様に覆う。
   - いずれも**存在しないときは何もしない**（非 systemd のホスト、テスト環境、`XDG_RUNTIME_DIR` 未設定）。
   - 存在するのに覆えなかったときは、D5 の「spawn の準備に失敗したら spawn を失敗させる」に従い、その spawn を失敗させる
     （保護なしで起動しない）。daemon の起動時の自己試験（D5）にも「namespace の中から bus が見えない」の確認を足す。
3. **書き込み先の OS 強制**（D-b の rules が path を表せない分）: 同じ namespace で `~/.config/systemd`・`~/.local/celeris/releases`・
   `~/.config/celeris` を、存在すれば自分自身へ bind して `MS_RDONLY` で remount する（D1 の 4 と同じ手順、下位の bind は作らない）。
   worker の run がここに書く正当な理由は無い（codex のアカウント dir は `~/.local/celeris/codex-accounts` で、`releases` の外）。
   実装時に既存の run が書いていないことを確かめ、書くものが見つかったら付記を直してから進める。
   実装: `DbGuard::host_config_read_only_paths`（HOME は `DbGuard::new` で解決、試験は `with_home` で注入）を `Plan::enter` が
   bind + remount する。試験は `db_guard_tests.rs` の `host_config_read_only_inside_namespace` ほか（2026-10-02 の grep で書く run は無し）。

namespace を作れない環境での扱い:

- **コンテナ実行**（`[containers]`）: D2 のとおり D1 は掛けない。1 は掛ける。コンテナには `-v` で `/run/user/$UID` を渡さないので
  bus は元から見えない（渡す設定を足さない）。
- **`[db] worker_read_only = false`**（明示の opt-out）: 1 だけが効く。`systemctl --user` は `$XDG_RUNTIME_DIR/systemd/private` へ
  直結でき、`DBUS_SESSION_BUS_ADDRESS` が無くても既定の `/run/user/$UID/bus` を試すので、**bus は塞がらない**。daemon は起動時に
  「user systemd bus は worker の run から届く（ADR-0095 付記 D-a）」と警告を出す。黙って塞いだつもりにならない。
- user namespace がそもそも作れないホストでは D5 により daemon が起動しない（変更なし）。

試験は実 process で行う: namespace 付きで起動した子の中から `systemctl --user show-environment` と
`systemd-run --user --scope true` を実行し、どちらも非 0 で終わることを確かめる（bus が無いホストでは skip、ただし
`CELERIS_DB_GUARD_TESTS=require` 相当で skip を失敗にできるようにする）。**この試験は bus が塞がっていることの確認なので、
塞がっていなければ実 user manager に scope を作り得る**: 試験の中の `systemd-run` には `--scope true` 以外を渡さず、
`--unit` に試験専用の名前を付けて、失敗（=穴）したときは出来た scope を `systemctl --user stop` してから assert を落とす。

### D-b. codex の自動承認に拒否のパターンを入れる: execpolicy の rules（`forbidden`）を adapter が配る

調べたこと（このホストの codex-cli 0.160.0、外部ネットワークは使っていない）:

- `codex exec --help` に `--ignore-rules`（"Do not load user or project execpolicy `.rules` files"）がある。つまり rules は
  既定で `$CODEX_HOME/rules/*.rules`（user）とプロジェクトの rules から読まれる。`~/.codex/rules/default.rules` は
  `prefix_rule(pattern=[…], decision="allow")` の形。本番のアカウント dir（`~/.local/celeris/codex-accounts/chatgpt_plus_personal`、
  adapter が `CODEX_HOME` に渡す、`codex_account.rs`）には `rules/` が無い。
- `codex execpolicy check --rules <file> <tokens…>` で実測:
  - `prefix_rule(pattern=["systemctl","--user"], decision="forbidden", justification="…")` に対し `systemctl --user restart celeris`
    → `"decision":"forbidden"`。`prefix_rule(pattern=["systemd-run"], decision="forbidden")` に対し `systemd-run --user --scope true`
    → `"decision":"forbidden"`。
  - `bash -lc "systemctl --user stop x"` → `matchedRules: []`（check の道具は token 列をそのまま照合する。codex の実行時が
    `bash -lc` の単純な script を分解して照合するかはローカルでは確かめていない）。
  - `cp a ~/.config/systemd/user/x` → `matchedRules: []`。**prefix_rule は argv の接頭辞しか表せず、書き込み先の path は表せない。**
- `-c` の設定には「この command を拒否する」を表す key は見当たらない（`codex exec --help` / `codex execpolicy --help` の範囲）。
  `approval_policy` / `sandbox_mode` / `approvals_reviewer` は全体の強さを変えるだけ。
- adapter（`crates/task-worker/src/codex.rs`）は `codex exec --json` を起動して出力を読むだけで、run の途中の escalation 要求を
  受け取って可否を返す口を持たない（`--approve-for-me` は codex の中の自動レビューで完結する）。adapter 側で個々の command を
  検査するには app-server の承認プロトコルへの移行が要り、この付記の範囲を超える。

選択: **execpolicy の rules**。adapter が spawn の直前に `$CODEX_HOME/rules/celeris-deny.rules` を毎回書き直す（内容は adapter の
定数。中身が違えば上書き）。rules の `forbidden` は承認の前に判定され、承認 policy・自動レビューに関係なく拒否される
（codex の rules の意味。prompt を出さない）。中身:

- `prefix_rule(pattern=["systemctl","--user"], decision="forbidden", …)`、`["systemctl","--user-unit"]` 等の変種は実装時に
  `systemctl --help` で確かめて足す。
- `prefix_rule(pattern=["systemd-run"], decision="forbidden", …)`（`--user` 以外も worker の仕事ではない）。
- `["loginctl"]`・`["busctl","--user"]`・`["dbus-send","--session"]` など bus を直接叩く道具も同様に足す。
- justification は「本番 host の操作は人が実行する手順として書く（ADR-0095 付記 D-d）」。モデルに理由が返る。

あわせて adapter は operator の `extra_args` に `--ignore-rules` が入っていたら拒否する（起動を失敗させる）。resume の
`--dangerously-bypass-approvals-and-sandbox` の fallback（`translate_resume_extra_args`）で rules が効くかは実装時に
`codex execpolicy` と実 run の rollout で確かめ、効かないなら fallback を止める。

採らなかったもの: `-c` 設定（表す key が無い）、adapter 側の検査（口が無い）、`--approve-for-me` をやめる（他の運用を壊し、
却下した案「codex の設定だけ直す」と同じ理由で単独では足りない）。

**rules は補助で、強制は D-a。** prefix_rule は `bash -lc`・`sh -c`・python の `subprocess`・別名の symlink・path での書き込みで
外れ得る。`~/.config/systemd`・`~/.local/celeris/releases`・`~/.config/celeris` への書き込みは rules では表せないので D-a の 3
（読み取り専用 bind）で止める。rules の file は `$CODEX_HOME`（worker から書ける）にあるが、codex が起動時に読むので
同じ run の中で消しても効かず、次の spawn で adapter が書き直す。

### D-c. テストは実 user manager に触らない

- `task_worker::detach` の試験と `crates/celeris/tests/releases_api.rs` は `systemd-run` を**注入した runner**
  （`DetachLauncher::SystemdRun { program }` の偽物の絶対パス。`crate::test_support::write_executable` で書いた、引数を記録して
  scope を作らずに残りの command を exec するだけの script）で叩く。`PATH` 上の実 `systemd-run` を選び得る `"auto"` の判定は
  純関数（`resolve_detach_launcher`）の単体試験だけで確かめる。
- `releases_api.rs` の「user bus が使えるなら実 `systemd-run --user --scope` で試す」経路（skip 判定の `Command::new("systemd-run")`
  を含む）は消す。試験の process から `systemd-run` / `systemctl --user` を実行しない。
- 確認: `cargo test --workspace` の実行前後で `systemctl --user list-units --type=scope` の件数が変わらないこと、または
  `PATH` の先頭に「呼ばれたら記録して失敗する」偽の `systemd-run` / `systemctl` を置いて全試験を回し、記録が空であること（後者は
  本番の user manager に触らずに確かめられるので、こちらを検証の既定にする）。D-a の実 process の試験は bus を塞いだ namespace の
  中でだけ `systemd-run` を呼ぶ（上記）。

### D-d. 本番 host の操作は人が実行する手順として書く

planner と worker の指示（`crates/task-worker/src/preamble.rs` の共通の前置き）に次を入れる:
「本番 host の操作（`systemctl --user`・`systemd-run`・`~/.config/systemd`・`~/.local/celeris/releases`・`~/.config/celeris`
の変更、daemon の再起動・差し替え、本番 DB への書き込み）はしない。必要なら、人が実行する手順（コマンドと確認方法）を
成果物に書き、計画では人の決定（decisions）または人の check を置く。」D-a・D-b で止まる操作を最初から試みさせないためで、
強制ではない（強制は D-a）。

### 残る穴（2026-10-02 更新）

上の「残る穴」のうち `systemd-run --user` は D-a で塞ぐ（namespace が掛かる run に限る。`worker_read_only = false` では残る）。
まだ残るもの:

- **`ssh localhost`**（および自ホストへの ssh 全般）: sshd が namespace の外で login shell を起こすので、DB にも user bus にも
  届く。agent の ssh 鍵・ssh master（`~/.ssh` の ControlPath）を使えば認証も通り得る。塞ぐには ssh の宛先の検査か、worker を
  別 uid にする必要がある（別の課題）。
- **D-Bus の abstract socket**: abstract namespace の UNIX socket は mount namespace ではなく network namespace に属するので、
  path を覆っても届く。このホストの user bus は path（`/run/user/$UID/bus`）だが、`dbus-launch` 等で abstract の bus を
  起こした session があれば届く。network namespace の分離は外部通信を壊すので採らない。
- **他の UNIX socket の常駐サービス**: tmux / screen の server、podman の API socket、`gpg-agent`、`ssh-agent` など、
  namespace の外のプロセスに command を実行させられるもの。`$XDG_RUNTIME_DIR` の下は D-a で `systemd` と `bus` だけを覆う。
- **cron / at**: `crontab` は setuid の helper を使う。user namespace の中で setuid が効かず失敗すると見込むが、実測していない。
- **HTTP API**・**`worker_read_only = false`**・**コンテナ以外での opt-out**: 従来どおり。
- **codex 以外の adapter の承認**: D-b は codex だけ。claude-code・opencode などは自前の承認の仕組みを持ち、D-a と D-d に頼る。
- いずれも worker が daemon と同じ uid であることが根で、完全に閉じるには worker の実行ユーザーの分離が要る。

## 付記（2026-10-02）: 試験用一時 DB の daemon と userns 試験

worker run の中で一時 DB・一時 config・本番 token 無しで起こす試験 daemon には userns を要求しない（本番 DB・本番 token なら従来どおり拒否）。userns の要る試験は `CELERIS_USERNS_TESTS=1` の opt-in。詳細は [ADR-0126](0126-test-daemons-without-userns.md)。
