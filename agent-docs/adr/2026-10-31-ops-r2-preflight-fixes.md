---
tasks: [01M68D4ECCN4V367TYHAKRQZMF]
---
# ADR 2026-10-31: K2 ops-r2 の開始前チェック（§2〜§6）を実機で通すための修正

状態: 採用（設計）。実装は同じ task の後続 unit（cfg・leak・probe・manifest・docs）。この ADR ではコードを変えない。
上位の決定は [ADR 2026-10-30-ops-r2-isolation-principal](2026-10-30-ops-r2-isolation-principal.md)（D1〜D7、付記 L1・G1〜G6）。
この ADR はその付記として読み、矛盾するところはこの ADR の D1〜D8 が上書きする（上書きする箇所は各決定に書く）。

## 背景

K2 ops-r2 pilot（task 01M669QGFJRZ2MY7VRA1BSSZ5H）は人の承認が出た。外部 operator が `docs/ops/ops-r2-k2-preflight.md` の
§1〜§6 を実機で実行し、§4 で止まった。有料の試行は 0 件、測定 daemon は起動していない。
証跡は task 01M669QGFJRZ2MY7VRA1BSSZ5H の `artifacts/ops-r2/preflight/`（`steps.tsv` ほか）。要点は次のとおり。

| 段 | 結果 | 原因 |
|---|---|---|
| §1 | 合格 | `EVAL_ROOT` の private の sha256 が承認票と一致、release dir に private 系の file 0 件 |
| §2 `ops-r1-measure-config.sh` | exit 0 だが不備 | `[cos.triage] policy_version = "6"` を固定で書く（承認票は 10）。`--claude-accounts <b>` が `[accounts] claude_dir` になり、b の subdir（backups・cache・plugins・sessions）が account と見なされ、login 済みの account が 0 |
| §3 preflight | exit 1 | deny list 7129 件を bwrap の引数に展開して `Exceeded maximum number of arguments 9000`。3276 件に絞っても、NFS 上の claude binary の mount が A の `deny_readable` に当たる。ops-r2 の K2 manifest が無く、ops-r1 の manifest・`private file hash mismatch` 等 8 件が `missing` |
| §4 probe | B は `--allow` なしの手順書の command で `mounts_outside_allow` 6（`--allow` 付きで 0）。L1 は readable 0・`mounts_outside_allow` 19 |
| §4 漏洩検査 | exit 1 | `same_uid_without_canary` 3275 件（ops-r2 は canary を置けない）と、L1 probe の `mounts_outside_allow` を `deny_readable` の hit にした 1 件。内容の hit は 0 |
| §5・§6・§8 | 未実行 | §4 で止めた |

## 決定

### D1. 方針版は `ops-r1-measure-config.sh` の必須引数 `--policy-version` で決める

- `ops-r1-measure-config.sh` に `--policy-version <数字>` を必須で足し、`[cos.triage] policy_version` にその値を書く。固定の `"6"` は消す。
  省略・空・数字以外は usage で止める（既定値は置かない。release ごとの既定を推測すると承認票と食い違ったときに気づけない）。
- 生成時に `--skills-src` の `cos-inbox-triage/SKILL.md` の front matter の `version` と引数を比べ、違えば止める。
  daemon が渡す方針版と CoS が読む skill の版が別の値になる測定 daemon を作らない。
- operator は承認票の方針版（K2 ops-r2 では 10）を引数に渡す。開始前チェック 5 と停止規則 §7-6 は今までどおり
  測定 daemon の `/api/v1/health` の値と承認票の値を比べる。K4（ops-r1）の固定値 6 には依存しない。
- 試験は、引数の値が config に出ること、省略と skill の版との不一致で止まることを確かめる（cfg unit）。

### D2. B の account pool は生成時に作る 1 entry の pool dir にする

- `--claude-accounts <dir>` を消し、`--claude-account <dir>`（1 つの account dir。`.credentials.json` を直に持つ）に替える。
  旧い引数は usage で止める（ADR 2026-10-11-breaking-changes-by-default）。
- 生成時に `$STATE/claude-pool/` を新しく作り、その中に account dir の basename を名前にした symlink を 1 つだけ置く
  （例 `$STATE/claude-pool/b -> /local/celeris/measure-accounts/b`）。`[accounts] claude_dir` は `$STATE/claude-pool` を指す。
  `$STATE/claude-pool/` が既にあって中身が違えば止める。
- symlink を選ぶ理由: Celeris の account の走査（`task_api::accounts::scan_accounts`、`model_discovery::first_logged_in`）は
  `Path::is_dir` で symlink の先を辿るので、symlink の entry が 1 account になる。bind mount は host 側で mount の権限が要る。
  測定 daemon は `ops-r2-sandbox.sh --account <b>` が b の実 path をそのまま mount するので、bwrap の中でも symlink の先が解決する。
- 生成時の拒否: account dir に `.credentials.json` が無い、`projects/` か `memory/` がある（`ops-r2-sandbox.sh` と同じ規則）、
  basename が account id として無効、実 path が `~/.claude` か `~/.claude` の下。
- a（A 腕の account）と operator（実験専用 operator session）の account dir は pool に入れない。pool の entry は引数の 1 つだけなので、
  別の dir が載る経路は無い。
- 試験は、生成した config を読み込み、Celeris の account の走査が `b` 1 件・login 済みだけを返すことを確かめる（cfg unit）。

### D3. deny list は file で渡す

- `celeris-campaign probe-read` は `--deny-file <path>`（1 行 1 絶対 path）で deny list を読む。繰り返しの `--deny` は消す。
  L1 の probe も 1 行の file を渡す。
- `celeris-campaign preflight` は既存の `--deny-list` の file を、probe を起動するときに file のまま bwrap の中へ ro で bind し、
  probe には `--deny-file` で渡す。deny list の path を bwrap・probe の引数に展開しない（bwrap の引数上限 9000 に当たらない）。
- `check-ops-r1-leak.py` は `--deny-readable-file <path>` で読み、`--deny-readable` は消す（渡せば入力の誤りで止まる）。
- file の形は 3 つで同じにする: UTF-8、1 行 1 path、空行は飛ばす、絶対 path でない行・NUL を含む行は入力の誤りで止まる。
  出力の JSON に file の sha256 を書き、承認票の deny list と照合できるようにする。
- 試験は 9000 件を超える deny list で probe・preflight・漏洩検査が走ることを確かめる（probe unit・leak unit）。

### D4. 測定用の claude は local の固定 copy を使い、probe の照合は変えない

- 測定（A 腕・B の測定 daemon・実験専用 operator session）で使う claude は、operator が NFS 上の install から local の dir
  （例 `/local/celeris/measure-bin/claude-<版>/`）へ写した固定 copy にする。写した後は書き換えない。
- operator は copy の `--version` の出力が承認票の Claude Code 版と一致すること、copy の sha256 を steps に記録する。
  `--claude-bin`（config 生成）と A 腕の Toolchain 宣言はこの copy を指す。
- probe の mount 照合（mount の source が許可表の外なら `deny_readable`）は変えない。NFS の source を許可に足す変更はしない。
  NFS の mount は他の利用者の書き換えを受け、許可表の外の木を見せるため。
- 版を上げるときは新しい dir に写し、承認票の版と sha256 を人が改める。

### D5. ops-r2 には canary を置かず、same_uid は runtime probe と L1 probe の合格で外す

付記（2026-10-30、L1 は持ち主の uid で判定）の「`--canary` が無ければ `same_uid_without_canary` の hit」を次のとおり改める。

- **canary を置かない理由**: K2 の新しい private は `EVAL_ROOT`（持ち主 `celeris-eval`、0700）にあり、sha256 が承認票に結ばれている。
  canary の文字列を足すと private の sha256 が変わり、承認票と照合できなくなる。repo の旧 private（漏洩済み）に足すのは repo と
  git の履歴を変える。どちらも ops-r2 では行えない。
- **same_uid を外す条件**（漏洩検査が deny path ごとに判定。満たせば `same_uid_limit: false`、`same_uid_cleared_by: runtime_probe_and_l1` を記録）:
  1. 渡した runtime probe（l1 以外の主体）の全部がその path を持ち、どれも `readable: false`・`listable: false`。
  2. その runtime probe の全部が `ok: true` で `mounts_outside_allow` が 0（許可表の外の木を mount していない）。
  3. L1 probe が渡され、その全 deny path が `readable: false`・`listable: false`。
  4. path が `EVAL_ROOT` の下なら、L1 probe にその path があり、`owned_by_run_uid` が空（付記どおり）。
- **根拠**: same_uid の limit は「run の uid は chmod・rename で読めるようにできる」ことを指す。`EVAL_ROOT` の外の deny path
  （repo の clone の旧 private・旧 grade-case、過去の試行の出力、本番 KB、`~/.claude` 等）は人の uid が持つので L1 では塞げず、
  L2（bwrap の mount 許可表）で塞ぐ（上位 ADR D1）。試行の主体は probe と同じ包み方の bwrap の中で動き（D3 の「同じ包み方」）、
  その中では path が存在せず、mountinfo に許可表の外の source が無い（条件 2）。存在しない path の inode には chmod も rename も
  届かないので、`owned_by_run_uid` が空でなくても試行の主体からは読めるようにできない。`EVAL_ROOT` の下は L1（持ち主の分離）が
  塞ぐので、付記の条件（`owned_by_run_uid` が空）をそのまま残す。
- **緩めない点**: 4 条件の 1 つでも欠ければ limit を残し、canary が無いので `same_uid_without_canary` の hit で exit 1 にする
  （probe に載っていない path、`mounts_outside_allow` のある runtime probe、L1 probe なしは通らない）。内容の検査
  （`--private` の文字列・`--forbidden-sha`）は変えない。bwrap の外の人の uid の process（普段の operator・人の shell）が読める点は
  付記 G5 の限界のままで、この規則では外さない（承認票と D5 の照合で扱う）。
- 漏洩検査が `EVAL_ROOT` の下かを判定できるよう、`check-ops-r1-leak.py` に `--eval-root` を足す（leak unit）。

### D6. L1 probe の `mounts_outside_allow` は hit にしない。B の probe の手順書の command に `--allow` を入れる

- 付記 G6 を漏洩検査にも当てはめる。L1 probe（`principal: l1`）は包まずに走るので host の mount が見え、`mounts_outside_allow` が
  出るのは正しい状態である。漏洩検査はその値を報告欄（例 `l1_mounts_outside_allow`）に残し、`deny_readable` の hit にしない。
  runtime の主体の `mounts_outside_allow` は今までどおり hit にする（D5 の条件 2 でも使う）。
- 手順書 §4 の B の probe の command は、preflight が B の probe に渡すのと同じ `--allow`（`ops-r2-sandbox.sh` が mount する
  release・state・account・`--ro` と system の dir）を含む形に直す。`--allow` の無い command で `mounts_outside_allow` 6 が出たのは
  手順書の誤りで、probe の判定の誤りではない（`--allow` 付きで 0 を確認済み）。

### D7. sirius への接続は operator が実機で確かめ、使えなければ止めて人に返す

- K2 は B の worker が sirius で計測する。測定 daemon は bwrap の中で動き、許可表に `~/.ssh` も ssh の control master の socket も無い。
  bwrap の中から sirius に届くかは未確認。
- 開始前チェックに operator の手順を足す（docs unit）: `ops-r2-sandbox.sh --state "$S" --release "$R" --account <b> -- ssh -o BatchMode=yes -o ConnectTimeout=10 <sirius の host 名> true`
  の exit code と stderr の 1 行目を steps に記録する。既存の control master を使う設定なら、その socket が bwrap の中に見えるかも記録する。
- exit 0 でなければ、operator は試行を始めず、結果を人に返す。ssh の鍵・config・socket を許可表に足すかは credential を
  試行の主体に見せる判断なので人が決める。operator とコードは許可表を広げない。
- コードの試験では確かめない（外部 network に出ない規則）。

### D8. 測定 daemon は試行と無関係に LLM を呼ぶ経路を config で切る

- `ops-r1-measure-config.sh` は `[maintenance.stagnation]` の 4 つの閾値（`project_idle_secs`・`human_wait_secs`・
  `release_unpromoted_secs`・`task_unscheduled_secs`）を 0 にして書く。停滞の finding が出ないので、それを CoS の triage に回す
  LLM の run も起きない。
- 既に切っている経路（`[knowledge.langmem]`・`[knowledge.gc]` の `enabled = false`、`[memory]`・`[llm_proxy]`・`[[mcp.listeners]]`・
  `[[cron.seed]]` を置かない）は残し、生成した config にその一覧を注記する。
- CoS の受信箱 triage は config で切れない。測定 daemon の受信箱に載るのは試行の task から出た待ちだけになるので、その triage は
  B の処理の一部として試行に計上する。試行と無関係な起点（停滞の finding・cron の定期見直し等）の CoS run が測定 daemon に
  現れたら、その試行を正式な試行にせず、消費を別に記録する（docs unit が停止規則に書く）。
- cfg unit は、上の一覧の他に daemon が account の credential で外部の LLM を呼ぶ常駐の経路が無いかを実装から確かめ、
  あれば config で切るか、切れない理由と記録の仕方を進捗に書く。
- 試験は、生成した config で停滞の閾値が 0、langmem・gc が無効であることを確かめる（cfg unit）。

## 後続 unit の作業範囲

| unit | 決定 | 主な file |
|---|---|---|
| cfg | D1・D2・D8 | `scripts/campaign/ops-r1-measure-config.sh`・`test_ops_r1_measure_config.py`、account の走査を使う試験 |
| leak | D3（漏洩検査）・D5・D6（漏洩検査） | `scripts/campaign/check-ops-r1-leak.py`・`test_check_ops_r1_leak.py`、`docs/ops/ops-r1-leak-check.md` |
| probe | D3（probe・preflight）・D4 | `crates/campaign/src/runtime_probe.rs`・`preflight.rs`・`main.rs` とその試験 |
| manifest | 不備 3（K2 ops-r2 の manifest） | `crates/campaign/fixtures/` の manifest と preflight の受け入れ |
| docs | D4・D6（手順書）・D7・D8（停止規則）、本番への移行 | `docs/ops/ops-r2-k2-preflight.md`・`ops-r2-isolation.md`、進捗 |

## 人の準備・決定が要る点

- D4: local の固定 copy を置く dir と、承認票の Claude Code 版と copy の版が合うこと（operator が確かめ、合わなければ人に返す）。
- D7: bwrap の中から sirius に届かないとき、ssh の鍵・config・socket を許可表に足すか（人の決定）。
- 本番設定・本番 daemon は変えない。測定 daemon の config は試行ごとに生成する。

## 関連

- [ADR 2026-10-30-ops-r2-isolation-principal](2026-10-30-ops-r2-isolation-principal.md)（D1〜D7、付記 L1・G1〜G6）
- ADR 2026-10-27-project-stagnation-inbox（`[maintenance.stagnation]`）、ADR 2026-10-11-breaking-changes-by-default
