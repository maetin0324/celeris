---
tasks: [01M3YB21F07GQKTRYPRVN184AR]
---
# ADR-0126: 試験用一時 DB の daemon は worker run の中で userns を要求しない、userns の要る試験は opt-in（ADR-0095 付記）

- 日付: 2026-10-02
- 状態: 実装済み。葉 guard-scope（A）・e2e-harness（A の試験側）・userns-optin（B）・gate-env（B の gate）・
  prompt-rule（C）を実装し、worker sandbox の中で `cargo test -p e2e --test api_scenarios`・
  `cargo test -p celeris --test instance_handoff`・`cargo test --workspace`・`cargo clippy --workspace -- -D warnings` を
  実行して確認した（証拠: [docs/progress/phase-test-db-userns.md](../progress/phase-test-db-userns.md)）。
- 関連: [ADR-0095](0095-worker-runs-see-the-db-read-only.md) D1/D5（worker db guard、fail-closed）と付記 D-a〜D-d、
  ADR-0079 付記「R7-6」（daemon 起動時の guard）・「R7-12」D4（`CELERIS_ISOLATION_TESTS` と環境の preflight）、
  ADR-0045 D2（既定の設定 `~/.config/celeris/config.toml`）。DESIGN.md は変えない。

## 文脈

worker run の sandbox の中（Claude Code / codex の sandbox。user namespace を作れない）で
`cargo test -p e2e --test api_scenarios` や `crates/celeris/tests/instance_handoff.rs` を走らせると、試験が起動する
celeris daemon が `install_worker_db_guard`（`crates/celeris/src/daemon/bootstrap.rs`）で `task_worker::db_guard::probe` を呼び、
`unshare -U` が `uid_map: Operation not permitted` で落ち、D5 の fail-closed で daemon が起動しない。e2e と daemon 試験は全滅し、
planner が WU の check に置くと必ず落ちて replan が続いた（2026-10-02 handoff 認可 task の `docs/progress/phase-R.md`
『main 取り込みと受け入れ検査』『phase3 control flaky 再実行』）。

worker db guard が守るのは**本番の DB**（本番 config の `[db] path` と `state_dir`、例 `/var/lib/celeris`）と、それを通じた本番 API
である。試験ごとの一時 DB を使う daemon にそれを要求しても守るものが増えない。一方、実 browser・実 runtime・launcher の試験は
本当に user namespace が要り、sandbox では原理的に走らない。両者を分ける。

## 決定

### A. worker db guard の判定を「守る対象」と「worker run の中か」で決める

#### A1. 本番の守る対象（protected set）

`install_worker_db_guard` は `[db] worker_read_only = true`（既定）のとき、まず守る対象の集合 P を作る:

1. **worker run の印**: 本番 daemon の `db_guard::apply`（namespace を作る側）が、子の環境に
   `CELERIS_WORKER_DB_GUARD=<正規化した守る DB のディレクトリ>` を入れる（token の値は入れない。path だけ）。
   付記 D-a の `host_config_read_only_paths` と同じ準備（fork 前）で作る。
2. **本番 config**: `getpwuid(getuid())` の home（`$HOME` は使わない。試験が書き換える）の
   `.config/celeris/config.toml`（ADR-0045 D2 の既定）。存在すれば読んで、`[db] path` の DB ファイルとそのディレクトリ、
   `state_dir`、`[api] token_file` を P に入れる。付記 D-a で `~/.config/celeris` は namespace の中でも読み取り専用で読める。
3. P の各 path は `canonicalize`（symlink・相対 path・`..` を解く）し、さらに `(st_dev, st_ino)` を控える。

本番 config が**存在するのに読めない・parse できない**、印の path が canonicalize できない、のどれかなら P を決められない
とみなし、A3 の免除をしない（従来どおり userns を要求＝fail closed）。

#### A2. 試験 daemon の判定（拒否が先、どれか 1 つに当たれば拒否）

起動しようとしている daemon の config から、DB ファイル・DB のディレクトリ・`state_dir`・`[api] token_file`（とその内容）を
同じく canonicalize と `(st_dev, st_ino)` で求め、次の**どれか 1 つ**に当たれば「本番を使う daemon」とする:

- DB ファイルが P の DB ファイルと同じ path、または同じ `(st_dev, st_ino)`（hard link・bind mount）。
- DB のディレクトリか `state_dir` が、P の DB ディレクトリ・`state_dir` と同じか、その中か、それを含む（canonical path の
  component 単位の前方一致。文字列の前方一致ではない。`/var/lib/celeris2` は `/var/lib/celeris` の中ではない）、
  または同じ `(st_dev, st_ino)`。
- `token_file` が P の `token_file` と同じ path か同じ inode、または**内容が本番 token と一致**（定数時間比較。値はログに出さない）。
  本番 token が P にあるのに読めず内容を比べられないとき、試験 daemon が `token_file` を持てば拒否する。
- 試験 daemon の path が canonicalize できない（DB は `build_dispatcher` が作った後なので存在するはず）。

本番を使う daemon は**今まで通り**扱う: userns を probe し、作れなければ起動しない（D5 の文言のまま。worker run の中なら
`worker db guard: this daemon uses the production DB/token inside a worker run (ADR-0126)` を足して失敗する）。

#### A3. 免除（userns を要求しない）

次の**全部**を満たすときだけ、probe をせず guard を入れずに起動する（`db_guard::install(None)`、
`tracing::warn!` で「test DB inside a guarded worker run; worker db guard not installed (ADR-0126)」と出す）:

1. A2 の拒否に当たらない（試験用の一時 DB・一時 config・本番 token 無し）。
2. worker run の中である: `CELERIS_WORKER_DB_GUARD` があり、**その path が今の mount namespace で読み取り専用**
   （`statvfs` の `ST_RDONLY`、かつ書き込み試行が `EROFS`）であることを kernel で確かめられる。
   印があっても書ける（偽の印・guard の外）なら免除しない。
3. 印の path が P に入っている（A1 の 1 と 2 が食い違えば、両方を P に入れて A2 を当てる）。

worker run の外（印が無い・印が裏付けられない）では、試験用 DB でも**今まで通り** probe する（host では userns が使えるので
従来の挙動が保たれ、本番 daemon が `--config` を既定以外に置いて P から漏れても guard を外さない）。

#### A4. 本番 DB を守る強さを弱めないことの論証

- 免除は A3-2 により「外側の本番 daemon の guard が既に効いている namespace の中」に限る。その中では本番 DB のディレクトリは
  kernel が読み取り専用にしており（ADR-0095 D1）、mount namespace は子に継承され、userns を作れない sandbox の中からは外せない。
  内側の daemon が guard を入れても入れなくても、本番 DB への書き込み可能性は変わらない。
- 内側の daemon が本番 DB・本番 `state_dir`・本番 token を使う場合は A2 で拒否するので、worker run から本番 DB に繋がる
  daemon（本番 token で本番 API を叩く経路を含む）は起動できない。path の正規化（symlink・相対 path・`..`・bind mount・hard link）
  は canonicalize と inode の両方で比べるので迂回できない。
- 判定できないとき（本番 config が読めない、印が裏付けられない、path が解けない）は全て免除しない側（fail closed）。
- `[db] worker_read_only = false` の opt-out は従来のまま（この ADR は免除を足すだけで、既存の拒否を 1 つも外さない）。

#### A5. 固定する試験（葉 guard-scope。`worker_db_guard_` を前置き）

判定は userns・実 mount に依らない純関数（P・試験 daemon の path・「印の path が読み取り専用か」を注入）に切り出し、一時 dir で試す:

- `worker_db_guard_refuses_production_db_path_inside_worker_run`
- `worker_db_guard_refuses_production_db_via_symlink`
- `worker_db_guard_refuses_production_db_via_relative_path`
- `worker_db_guard_refuses_production_db_via_hard_link`（同じ inode）
- `worker_db_guard_refuses_production_state_dir_subpath`
- `worker_db_guard_refuses_production_token_file_and_copied_token`
- `worker_db_guard_refuses_when_production_config_is_unreadable`（fail closed）
- `worker_db_guard_refuses_test_db_when_marker_is_not_read_only`（偽の印 → 従来どおり probe を要求）
- `worker_db_guard_allows_test_db_inside_worker_run`
- `worker_db_guard_allows_test_db_with_test_token`
- `worker_db_guard_requires_userns_for_test_db_outside_worker_run`（印なし → 従来どおり）
- `worker_db_guard_sibling_dir_is_not_production`（`/var/lib/celeris2` の型の component 比較）

e2e harness（葉 e2e-harness）は daemon を試験ごとの一時 dir の DB・config と試験専用の token で起こし、`CELERIS_WORKER_DB_GUARD`
を子の環境から**消さない**（`env_clear` するなら明示的に通す。消すと A3-2 が満たせず sandbox では起動できない）。
sandbox の中で e2e と instance_handoff が通ることは葉 verify が worker run の記録で示す。

### B. userns の要る試験は既定で skip、`CELERIS_USERNS_TESTS=1` のときだけ走る

#### B1. 対象

unshare / `CLONE_NEWUSER` / newuidmap / bwrap の user namespace を前提にする試験。少なくとも
`crates/task-worker/tests/` の `browser_injection_wire.rs`・`browser_runtime_isolated.rs`・`browser_shared_cdp.rs`・
`browser_egress_relay.rs`・`browser_h3_wire.rs`・`browser_cdp_sink.rs`・`browser_runtime_supervisor.rs`・
`browser_injection_attacks.rs`・`browser_restore_deliver.rs`、`crates/task-api/tests/` の `browser_h3_injection.rs`・
`browser_restore_deliver.rs`・`browser_restore_live_session.rs`、unit 試験のうち実 namespace を作るもの（`db_guard_tests.rs`、
`scratch/tests.rs`・`container/tests.rs`・`browser_tests.rs`・task-dispatch の `git_workspace.rs`・`browser_fallback.rs` の該当試験）。
`browser_launcher_ptrace.rs` は今は無い。足すときも同じ規則に従う。葉 userns-optin が `grep -l 'unshare\|CLONE_NEWUSER\|newuidmap'`
で棚卸しし、実際に namespace を作る試験だけに掛ける。A により `instance_handoff.rs` と e2e は対象**ではない**（既定で走る）。

#### B2. 規則

- `CELERIS_USERNS_TESTS=1` → 走らせる。**環境が無ければ（userns を作れない・道具が無い）skip ではなく fail**
  （明示的に求めたのに黙って飛ばさない。`CELERIS_ISOLATION_TESTS=require` と同じ向き）。
- 未設定・他の値 → 試験の始めで抜け、stderr に 1 行
  `SKIPPED (userns test, not passed): set CELERIS_USERNS_TESTS=1 to run (ADR-0126)` を出す。
- 判定は各 crate の小さな helper 1 つ（同じ文言・同じ優先順位）に寄せる。

#### B3. 既存の環境変数との関係（互換）

優先順位は上から:

1. `CELERIS_ISOLATION_TESTS=skip` → 従来どおり skip（`SKIPPED (not passed): CELERIS_ISOLATION_TESTS=skip`）。`CELERIS_USERNS_TESTS=1`
   より強い（人が明示的に外したものを gate の既定で戻さない）。
2. `CELERIS_USERNS_TESTS=1`、または従来の `CELERIS_ISOLATION_TESTS=require` / `CELERIS_DB_GUARD_TESTS=require` → 走らせ、
   環境が無ければ fail（既存の呼び出し側をそのまま opt-in として扱う）。
3. どれも無い → skip（B2 の 1 行）。従来の「未設定なら走らせ、環境が無ければ preflight で skip / fail」は、未設定時の既定が
   skip に変わる。R7-12 D4 の preflight（環境があるかの機能 probe）は 2 のときの診断として残す。

#### B4. gate で付けるか

- `scripts/selfdeploy/release.sh` は `export CELERIS_USERNS_TESTS="${CELERIS_USERNS_TESTS:-1}"` を
  `CELERIS_ISOLATION_TESTS` の export の隣に足す（葉 gate-env）。release gate は host（userns が使える）で走るので、
  既定で外したことで今まで release で守っていた退行の検出（browser 隔離・db guard の実 namespace 試験）は消えない。
  userns が使えない host で release を走らせたら B2 により **fail**（skip にしない）。そういう host で release するなら人が
  `CELERIS_USERNS_TESTS=0` を明示する（gate の記録に残る）。
- `scripts/selfdeploy/verify.sh` は cargo 試験を走らせない。daemon の verify モードの起動時 probe（ADR-0095 D5）は本番 config で
  動くので A3 の免除に当たらず従来どおり実 userns を試す。verify.sh には変数を足さない。
- 人・統合検査が host（worker sandbox の外）で `cargo test --workspace` を走らせ userns 試験も確かめたいときは
  `CELERIS_USERNS_TESTS=1` を付ける。worker の WU の check には付けない（C）。

### C. planner の check 指針に足す 1 行

`crates/task-worker/src/claude_code/prompt.rs` の check 作成指針（`Checks run with /bin/sh (dash)` の並び）に次の 1 行を足す
（葉 prompt-rule。文言試験も足す）:

> Do not put tests that need a user namespace (real browser/runtime/launcher, unshare/CLONE_NEWUSER/newuidmap) in WU checks: the worker sandbox cannot create one. They skip unless CELERIS_USERNS_TESTS=1; if they must run, run them with CELERIS_USERNS_TESTS=1 in the daemon's integration check (release gate), not in a leaf.

（意味: userns の要る試験は WU の check に入れない。入れるなら環境変数付きで、daemon の統合検査に置く。）

## 却下した案

- **sandbox の印（`CODEX_SANDBOX`・uid_map の形）で guard を外す**: R7-12 D4 と同じ理由で却下。印は偽れ、このホストは LXC の
  userns の中で uid_map の形が既定でない。免除は kernel で裏付けた読み取り専用 mount（A3-2）に限る。
- **e2e で `[db] worker_read_only = false` を書く**: 試験は通るが、本番 DB を指す config でも外れる経路を試験が作る。A2 の拒否を
  試験で固定できない。
- **userns 試験を `#[ignore]` にする**: `--ignored` は他の ignore 試験も巻き込み、skip の理由も出ない。環境変数で選ぶ。
- **免除の判定に path だけを使い、worker run の中かを見ない**: `--config` を既定以外に置いた本番 daemon が P から漏れると
  guard が外れる。worker run の外は従来どおりにする。

## 残る穴（受け入れる）

- worker run の外で、本番 config を既定以外の場所に置いた本番 daemon を人が起こす場合は、A の判定に関係なく従来の probe が
  掛かる（弱めない）。
- 本番 token が環境変数など `token_file` 以外の経路で渡る形が将来足されたら、A2 の比較対象にそれも足す（この ADR の不変条件:
  「worker run の中で本番 token を持つ daemon は免除しない」）。

## 付記（final review 指摘の修正）

final review（2026-10-02）で、この ADR の決定（A〜C）が次の 3 点で実装に反映されていないことを指摘された。この付記は
その修正方針を記す（実装は葉 fix-refuse・fix-userns-lib・fix-e2e-guard が別ブランチで行う。本付記はコードを変えない）。

### 1. worker run 内の本番 daemon は probe せず即時拒否する（fail closed。probe の成否に依らない）

現状の `install_worker_db_guard`（`crates/celeris/src/daemon/bootstrap.rs`）は `GuardDecision::RefuseProduction`
（起動する daemon が A2 の「本番 DB・本番 `state_dir`・本番 token・印の path」のどれかに当たると判定したとき）でも、
ログを出した後に `task_worker::db_guard::DbGuard::new` と `probe` を呼び、userns が作れれば（host など）そのまま起動して
しまう。これは A4 の論証（「worker run の中から本番 DB に繋がる daemon は起動できない」）と食い違う: worker run の外
（userns を作れる host）で本番 config を指す daemon を worker run の印つきで起こすと、probe が通って起動してしまう。

修正方針:

- `GuardDecision::RefuseProduction` を判定したら、worker run の中（`CELERIS_WORKER_DB_GUARD` の印があり A3-2 の
  読み取り専用確認が成り立つ場合に限らず、印があるだけで）probe を呼ばずに `DaemonError::DbGuard` で即時に起動を拒否する。
  判定は probe の成否・userns が作れるかどうかに依らない。
- worker run の外（印が無い、または印が裏付けられない）で `RefuseProduction` になった場合は、A2 の本文どおり**従来の
  probe 経路**を保つ（host では probe が成功して守られたまま起動し、userns が無い環境では D5 の fail-closed が従来どおり
  掛かる）。worker run の中かどうかの判定は、印の path を A1-3 と同じ canonicalize（symlink・相対 path・`..` を解く）の
  後に行い、迂回できないことを次の起動試験で固定する:
  - `worker_db_guard_refuses_production_db_inside_worker_run_without_probing`（probe が成功する環境でも、worker run の
    印がある限り即時拒否されることを固定する。symlink・相対 path 経由の本番 path でも同様に拒否）
  - `worker_db_guard_still_probes_production_db_outside_worker_run`（印が無ければ従来どおり probe 経路を通ることを固定する）

### 2. task-worker の lib 内 userns 試験は B2 の既定 skip 規則に従う

棚卸し（B1）は `tests/` 配下の統合試験ファイルに留まっていたが、`crates/task-worker/src/browser_tests.rs` の
`production_action_path_reaches_fixture_through_real_browser_and_egress`（lib 内 unit 試験）は `/usr/bin/unshare --user
--map-root-user --net --` を直接呼んでおり、B2 の `CELERIS_USERNS_TESTS` gate を経ない。sandbox の中では
`unshare: uid_map: Operation not permitted` でこの試験が失敗する。

修正方針:

- B2 の判定 helper（既定 skip・`SKIPPED (userns test, not passed): set CELERIS_USERNS_TESTS=1 to run (ADR-0126)` を
  stderr に 1 行・`CELERIS_USERNS_TESTS=1` で環境が無ければ fail）を、この試験の先頭（`unshare` を呼ぶ前）に適用する。
  他の lib 内 unit 試験で `unshare` / `CLONE_NEWUSER` / `newuidmap` を使うものも同じ規則にする。
- 葉 userns-optin の棚卸し grep（`grep -l 'unshare\|CLONE_NEWUSER\|newuidmap'`）の対象を `crates/*/src/` にも広げ、
  今後 lib 内に追加される userns 試験も同じ漏れをしないようにする。

### 3. e2e harness は guard を有効のまま、免除経路（Exempt）を実際に通す

現状の e2e harness（`tests/e2e/tests/api_scenarios.rs`）は daemon の config に `[db] worker_read_only = false` を書いて
guard そのものを外しており（A3 の免除経路を検証していない）、daemon を起こす `command()` は `CELERIS_` 前置きの環境変数を
すべて消していて worker run の印 `CELERIS_WORKER_DB_GUARD` も残らない。これでは「worker db guard は本番 DB・本番 token を
守ったまま、試験用 DB だけ免除する」という A の主張を e2e が確かめていない。

修正方針:

- e2e harness の daemon config は既定の `[db] worker_read_only = true` のままにする（`worker_read_only = false` を書かない）。
- `command()` で `CELERIS_` 前置きの環境変数を消すときも、harness 自身の一時 DB のディレクトリを指す
  `CELERIS_WORKER_DB_GUARD=<canonicalize した一時 DB ディレクトリ>` だけは残す（他の `CELERIS_*`、特に本番 token を運ぶ
  変数は消したまま）。これにより A3 の免除（Exempt）が実際の判定経路を通る。
- 印が無い場所（release gate や verify.sh の daemon 起動など worker run の外）では、この変更後も従来どおり probe 経路
  になることを維持する（B4 の記述と整合）。

修正後は、1〜3 のいずれも「worker run の中で本番 DB・本番 token・印の path に当たる daemon は起動できない」こと（1）と
「試験用 DB の daemon は guard が有効なまま免除される」こと（3）を起動試験・e2e 実行で固定する。
