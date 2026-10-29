# ops-r1 漏洩検査: grader-private・旧判定・旧 v1 入力が試行に届いていないことを確かめる

---
tasks: [01M62TJ5WBDD0VH5Z1EM264NWV]
---

対象: [main-4-ops-r1](mlsys-main-4-ops-r1.md) の試行（A の直接 Claude Code、B の Celeris と CoS）。
道具は `scripts/campaign/check-ops-r1-leak.py`（Python 標準ライブラリだけ。LLM・daemon・DB を使わない）。
試験は `python3 -I scripts/campaign/test_check_ops_r1_leak.py`。

## 1. 呼び方

```sh
python3 -I scripts/campaign/check-ops-r1-leak.py \
  --private <grader-private の private.json> \
  --forbidden-sha <旧 v1 tgz の sha256> --forbidden-sha <旧判定 file の sha256> \
  --canary <private の写しに入れた canary 文字列> \
  --scan <公開入力> --scan <KB snapshot> --scan <memory> --scan <CoS 初期会話> \
  --scan <外部 operator への材料> --scan <run の作業場所> \
  --deny-readable <private.json> --deny-readable <旧 bundle の写し> \
  [--allow-string <公開入力にも載る文字列>] [--run-uid <uid> --run-gid <gid>] \
  --out <結果 JSON>
```

exit 0 は見つからなかったこと、1 は見つかったこと（または代わりの検出が無いこと）、2 は入力の誤り（scan 先が無い、hash が 64 桁の 16 進でない等）。
`--private`・`--forbidden-sha`・`--canary`・`--scan`・`--deny-readable` は何度でも書ける。
結果 JSON は scan 先の外に置く（`checked.private[].sha256` は private.json の hash なので、scan 先に置くと次の検査で漏洩として数える。`--out` の path だけは scan から外す）。

## 2. 何を見つけるか

| hit の kind | 条件 |
|---|---|
| `content` / `private_sha256` | private.json の sha256 の 16 進（大文字・小文字とも）が file に現れる |
| `content` / `private_string` | private.json の 12 字以上の文字列値が現れる（UTF-8 と JSON の `\uXXXX` 表記の両方） |
| `content` / `canary` | `--canary` の文字列が現れる |
| `content` / `forbidden_sha256` | `--forbidden-sha` の 16 進が現れる |
| `file_sha256` | file（または archive の member）の内容の sha256 が private.json か `--forbidden-sha` と一致する（改名した写し） |
| `symlink` | scan 先の symlink が private.json・`--deny-readable` の path を指すか、それを含むディレクトリを指す |
| `deny_readable` | `--deny-readable` の path（ディレクトリなら中の file も）が run の uid/gid から読める |
| `same_uid_without_canary` | 同じ uid の限界（§3）がある path があるのに `--canary` が無い |

- scan は file・ディレクトリを再帰で読み、tar（gz・bz2・xz）・zip・gzip の中身（入れ子は 3 段まで）と、`.git` を持つディレクトリの全 object（`git cat-file --batch-all-objects`。消した file の履歴も含む）を読む。
- 既定で JSON の `.schema`・`.case_id`・`.split` の値は公開の識別子として検査から外す。公開入力にも載る他の文字列（例: 公開入力 tgz の名前 `expected.input`）は `--allow-string` で外す。外したものは `checked.private[].excluded` に path・長さ・sha256 で残る。
- 結果 JSON の hit には文字列そのものを書かず、`needle_id`・`needle_len`・`needle_sha256` だけを書く。
- この process が読めなかった file は `checked.scan[].unreadable` と `limits` の `unreadable_in_scan` に出る（検査していない）。

## 3. 限界と代わりの検出

- **同じ uid では読めることを防げない。** run の uid が path か祖先ディレクトリの持ち主なら、mode を 000 にしても run は chmod・rename で読めるようにできる。
  検査はその path を「今は読めない」と判定するが、漏洩を防いだことにはならない。結果の `limits` に `kind: same_uid`（持ち主の path と代わりの検出）を書く。
- 代わりの検出は canary の出現検査。private.json の写し（grader が読む側）に canary 文字列を入れ、その canary を `--canary` に渡す。
  試行の前（入力・KB・memory・CoS 初期会話・operator 材料）と後（run の作業場所・成果物・CoS 会話の export・KB の取り込み候補）の両方で scan し、canary が現れないことを確かめる。
  同じ uid の限界がある path があるのに `--canary` が無いと `same_uid_without_canary` の hit で exit 1 になる。
- 現状、本番の CoS と worker は人と同じ uid で動く（棚卸し g-grader-private-isolation）。B の試行では可読の判定より canary の出現検査が主な証拠になる。
  canary は読んだ後に書き出された痕跡しか見つけない。読んだが何も書かなかった場合は検出できない。
- 可読の判定は host から見た今の mode・owner で行う。run の sandbox（bwrap の mount 等）が見える範囲を狭める効果は数えない（多めに読めると判定する側に倒れる）。
  run の uid/gid が検査の process と同じなら `os.access`（ACL も含む）で、違えば mode bit と祖先の x で判定する（後者では ACL を評価せず、ACL があれば `acl_present` を書く）。
- 文字列の一致は完全一致だけ。言い換え・要約・一部の引用（12 字未満）は検出しない。

## 4. 結果 JSON（`celeris.ops-r1-leak-check/1`）

- `pass`: hit が 0 件なら true。
- `hit_count`・`hits`: 見つかったもの（`hits` は先頭 1000 件まで、`hit_count` は全件）。
- `checked`: `private`（path・sha256・検査した文字列数・外した文字列）、`forbidden_sha256`、`canary`（長さと sha256 だけ）、`scan`（file 数・byte 数・archive member 数・git object 数・symlink 数・読めなかった file）、`deny_readable`（存在・可読・owner・mode・判定方法・`same_uid_limit`）、`run_identity`。
- `limits`: `same_uid`・`unreadable_in_scan`・`git_objects_unscanned`・`host_view`。
