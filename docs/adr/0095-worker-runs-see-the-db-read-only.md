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
