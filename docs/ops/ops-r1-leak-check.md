# ops-r1 漏洩検査: grader-private・旧判定・旧 v1 入力が試行に届いていないことを確かめる

---
tasks: [01M62TJ5WBDD0VH5Z1EM264NWV, 01M66JYQ249SZHNXK0AEVTZNEC]
---

対象: [main-4-ops-r1](mlsys-main-4-ops-r1.md) の試行（A の直接 Claude Code、B の Celeris と CoS）。
道具は `scripts/campaign/check-ops-r1-leak.py`（Python 標準ライブラリだけ。LLM・daemon・DB を使わない）。
試験は `sh scripts/campaign/run-ops-r1-leak-tests.sh`（どの cwd でも通る。中で `cd scripts/campaign && python3 -m unittest test_check_ops_r1_leak`）。

## 1. 呼び方

```sh
python3 -I scripts/campaign/check-ops-r1-leak.py \
  --private <grader-private の private.json> \
  --forbidden-sha <旧 v1 tgz の sha256> --forbidden-sha <旧判定 file の sha256> \
  [--canary <private の写しに入れた canary 文字列>（ops-r1 のみ）] \
  --scan <公開入力> --scan <KB snapshot> --scan <memory> --scan <CoS 初期会話> \
  --scan <外部 operator への材料> --scan <run の作業場所> \
  --deny-readable-file <deny list の file> \
  [--eval-root <EVAL_ROOT>] \
  [--allow-string <公開入力にも載る文字列>] [--run-uid <uid> --run-gid <gid>] \
  [--runtime-probe <主体の probe JSON> --runtime-probe <L1 の probe JSON>] \
  --out <結果 JSON>
```

exit 0 は見つからなかったこと、1 は見つかったこと（または代わりの検出が無いこと）、2 は入力の誤り（scan 先が無い、hash が 64 桁の 16 進でない、probe JSON の schema が違う等）。
`--private`・`--forbidden-sha`・`--canary`・`--scan`・`--deny-readable-file`・`--runtime-probe` は何度でも書ける。
`--canary` は ops-r1 だけで使う。ops-r2 は private の写しに canary を置けないので渡さない（§4）。

deny list は file で渡す（`--deny-readable-file`）。UTF-8、1 行 1 絶対 path、空行は飛ばす。
絶対 path でない行と NUL を含む行は exit 2。件数の上限は無く、command line の長さに依らない（9000 件を超える全件の展開でも通る）。
結果 JSON の `checked.deny_readable_files[]` に file の path・sha256・件数を書く。
旧い `--deny-readable <path>` は廃止した。渡すと exit 2。
`--eval-root` は ops-r2 の EVAL_ROOT（絶対 path）。渡すと same_uid の外し方が ops-r2 の規則（§3）になる。
結果 JSON は scan 先の外に置く（`checked.private[].sha256` は private.json の hash なので、scan 先に置くと次の検査で漏洩として数える。`--out` の path だけは scan から外す）。

## 2. 何を見つけるか

| hit の kind | 条件 |
|---|---|
| `content` / `private_sha256` | private.json の sha256 の 16 進（大文字・小文字とも）が file に現れる |
| `content` / `private_string` | private.json の 12 字以上の文字列値が現れる（UTF-8 と JSON の `\uXXXX` 表記の両方） |
| `content` / `canary` | `--canary` の文字列が現れる |
| `content` / `forbidden_sha256` | `--forbidden-sha` の 16 進が現れる |
| `file_sha256` | file（または archive の member）の内容の sha256 が private.json か `--forbidden-sha` と一致する（改名した写し） |
| `symlink` | scan 先の symlink が private.json・deny list の path を指すか、それを含むディレクトリを指す |
| `deny_readable` | `--deny-readable-file` の path（ディレクトリなら中の file も）が run の uid/gid から読める（probe に載らない path。host の判定） |
| `deny_readable`（`source: runtime_probe`） | `--runtime-probe` のどれかで `readable=true` の path がある、runtime の probe の `mounts_outside_allow` が空でない（`member: mounts_outside_allow`）、`ok=false`（`member: ok=false`）。hit に `principal` と `probe_uid` を書く。L1 の `mounts_outside_allow` は hit にしない（§3） |
| `same_uid_without_canary` | 同じ uid の限界（§4）がある path があるのに `--canary` が無い（ops-r2 では §3 の 4 条件のどれかが欠けた path） |

- scan は file・ディレクトリを再帰で読み、tar（gz・bz2・xz）・zip・gzip の中身（入れ子は 3 段まで）と、`.git` を持つディレクトリの全 object（`git cat-file --batch-all-objects`。消した file の履歴も含む）を読む。
- 既定で JSON の `.schema`・`.case_id`・`.split` の値は公開の識別子として検査から外す。公開入力にも載る他の文字列（例: 公開入力 tgz の名前 `expected.input`）は `--allow-string` で外す。外したものは `checked.private[].excluded` に path・長さ・sha256 で残る。
- 結果 JSON の hit には文字列そのものを書かず、`needle_id`・`needle_len`・`needle_sha256` だけを書く。
- この process が読めなかった file は `checked.scan[].unreadable` と `limits` の `unreadable_in_scan` に出る（検査していない）。

## 3. runtime probe（`--runtime-probe`）

ADR `agent-docs/adr/2026-10-30-ops-r2-isolation-principal.md` の D3。
probe は試行の主体を起動するのと同じ包み方（A は `Sandbox::args`、B は `ops-r2-sandbox.sh`、外部 operator は D6 の起動 script）の中で
`celeris-campaign probe-read` が書いた JSON。形は次のとおり（欄が欠ける・型が違う・`schema` が違うと exit 2）。

```json
{"schema": "celeris.runtime-probe/1", "principal": "<主体>", "uid": 1000,
 "paths": [{"path": "<path>", "readable": false, "listable": false, "errno": 13}],
 "mounts_outside_allow": [], "ok": true}
```

- `principal` が `l1` か `l1:` で始まる probe は **L1 の確認**（人の uid の process から包まずに `EVAL_ROOT` の private を開いた結果）として扱う。
  それ以外は **runtime の確認**（その主体の包み方の中から開いた結果）。主体ごとに 1 file を渡す。
- deny list の path が probe の `paths[].path` に載っていれば（文字列の正規化か host の realpath が一致）、host の mode ではなく
  probe の `readable`・`listable` で判定する。`checked.deny_readable[]` は `method: runtime_probe` と、載った probe ごとの結果（`probes`）、
  `runtime_denied`（runtime の probe が 1 つ以上あり、どれも `readable=false`）、`l1_denied`（L1 の probe について同じ）を持つ。
- probe で判定した path も、host 側の stat で path と祖先のうち run の uid が持ち主のもの（root の run なら全部）を
  `owned_by_run_uid` に書く（最大 5 件）。
- **ops-r1（`--eval-root` なし）**: `same_uid` の limit を書かない（`same_uid_limit: false`、`same_uid_cleared_by: runtime_probe_and_l1`）のは、
  `runtime_denied` と `l1_denied` が両方 true で、かつ `owned_by_run_uid` が空の path だけ。
  `owned_by_run_uid` が空でなければ、両方の probe が拒否でも `same_uid` の limit を残す。同じ uid の chmod 000 は
  probe では拒否に見えるが、run は chmod・rename で読めるようにできるので、隔離とは数えない。
  片方の probe だけが拒否のときも持ち主の連鎖から `same_uid` を判定する。どちらの場合も canary が無ければ `same_uid_without_canary` の hit になる。
- **ops-r2（`--eval-root` あり、ADR `agent-docs/adr/2026-10-31-ops-r2-preflight-fixes.md` D5）**: 次の 4 条件が全部揃った path だけ `same_uid` を外す
  （`same_uid_cleared_by: runtime_probe_and_l1`）。canary は要らない（ops-r2 は private の写しに canary を置けない）。
  1. 渡した runtime の probe の全部がその path を持ち、どれも `readable=false`・`listable=false`。
  2. runtime の probe の全部が `ok=true` で `mounts_outside_allow` が空。
  3. L1 の probe が渡され、その全 path が `readable=false`・`listable=false`。
  4. path が `--eval-root` の下なら、L1 の probe にその path があり、`owned_by_run_uid` が空。
  1 つでも欠ければ、持ち主でなくても `same_uid` の limit を残し、`--canary` が無いので `same_uid_without_canary` の hit（exit 1）になる。
- **L1 の mount（ADR D6、付記 G6）**: L1 の probe は包まずに走るので host の mount が見え、`mounts_outside_allow` が空でないのは正常。
  hit にせず、結果の `checked.l1_mounts_outside_allow[]`（file と件数）に残す。L1 の `ok=false` は、`readable=true` でも `mounts_outside_allow` でも
  説明できないときだけ hit。runtime の probe の `mounts_outside_allow` と `ok=false` は従来どおり hit。
- probe に載らない path は今までどおり host から判定する（`limits` に `host_view` が残る。全部が probe で判定されたときは書かない）。
- hit になるのは、どの probe（L1 を含む）でも `readable=true` の path、runtime の probe の空でない `mounts_outside_allow`、説明のつかない `ok=false`。
  deny list に無い path でも probe が読めたと書いていれば hit にする。`listable` は結果に書くが、それだけでは hit にしない。
- canary の出現検査は probe があっても行う（読んだ後に書き出した痕跡を見る副次の証拠）。
- 結果の `checked.runtime_probe[]` に probe ごとの file・principal・uid・L1 か・ok・path 数・readable の数・mounts_outside_allow の数を書く。

## 4. 限界と代わりの検出

- **同じ uid では読めることを防げない。** run の uid が path か祖先ディレクトリの持ち主なら、mode を 000 にしても run は chmod・rename で読めるようにできる。
  検査はその path を「今は読めない」と判定するが、漏洩を防いだことにはならない。結果の `limits` に `kind: same_uid`（持ち主の path と代わりの検出）を書く。
- 代わりの検出は canary の出現検査（ops-r1 のみ。ops-r2 は §3 の 4 条件で外す）。private.json の写し（grader が読む側）に canary 文字列を入れ、その canary を `--canary` に渡す。
  試行の前（入力・KB・memory・CoS 初期会話・operator 材料）と後（run の作業場所・成果物・CoS 会話の export・KB の取り込み候補）の両方で scan し、canary が現れないことを確かめる。
  同じ uid の限界がある path があるのに `--canary` が無いと `same_uid_without_canary` の hit で exit 1 になる。
- 現状、本番の CoS と worker は人と同じ uid で動く（棚卸し g-grader-private-isolation）。B の試行では可読の判定より canary の出現検査が主な証拠になる。
  canary は読んだ後に書き出された痕跡しか見つけない。読んだが何も書かなかった場合は検出できない。
- probe に載らない path の可読の判定は host から見た今の mode・owner で行う。run の sandbox（bwrap の mount 等）が見える範囲を狭める効果は数えない（多めに読めると判定する側に倒れる）。
  run の uid/gid が検査の process と同じなら `os.access`（ACL も含む）で、違えば mode bit と祖先の x で判定する（後者では ACL を評価せず、ACL があれば `acl_present` を書く）。
- probe の結果は probe を走らせた時点・包み方のもの。この検査は probe JSON の中身を信じ、probe が本当にその包み方で走ったかは確かめない
  （確かめるのは preflight が probe を起動する経路）。
- 文字列の一致は完全一致だけ。言い換え・要約・一部の引用（12 字未満）は検出しない。

## 5. 結果 JSON（`celeris.ops-r1-leak-check/1`）

- `pass`: hit が 0 件なら true。
- `hit_count`・`hits`: 見つかったもの（`hits` は先頭 1000 件まで、`hit_count` は全件）。
- `checked`: `private`（path・sha256・検査した文字列数・外した文字列）、`forbidden_sha256`、`canary`（長さと sha256 だけ）、`scan`（file 数・byte 数・archive member 数・git object 数・symlink 数・読めなかった file）、`deny_readable`（存在・可読・owner・mode・判定方法・`same_uid_limit`。probe で判定した path は §3 の欄）、`deny_readable_files`（deny list の path・sha256・件数）、`eval_root`、`l1_mounts_outside_allow`、`runtime_probe`、`run_identity`。
- `limits`: `same_uid`・`unreadable_in_scan`・`git_objects_unscanned`・`host_view`。
