---
tasks: [01M7KATBGCRQRZ5NWXHXVWYPZ8]
---
# ADR 2026-11-16: 許可の一覧から行を消すだけの変更を repo health の policy.changed に数えない

- 日付: 2026-11-16
- 状態: Accepted（実装済み）
- 付記先: [ADR 2026-11-07 repo health の契約](2026-11-07-repo-health-contract.md) D2（`policy.changed` の定義。付記 2026-11-16 がこの ADR を指す）
- 関係: [ADR 2026-11-07 health review integration](2026-11-07-health-review-integration.md) D6.1（`policy_mixed`）・D6.4（policy の変更の人の承認）・D6.6（policy の file だけの branch）、[ADR 2026-11-07 Celeris profile](2026-11-07-repo-health-celeris-profile.md)
- task: 01M7KATBGCRQRZ5NWXHXVWYPZ8（人の決定 2026-11-16。operator が中継した）。policy の file・その試験・ADR・進捗だけを変える policy の branch（先例 01M76BYGKY7R8SBZ22AZGJ72SP、D6.6）

## 背景
LoC 削減 v3 E（01M7J3NKVAH9NG53C5FEFNQJYK）は daemon が呼ばない notify の旧い走査を消した。そこにあった通知の文言 3 個の許可行が `scripts/dev/human-text-lint.allow` に残り、残すと `scripts/dev/check-human-text.py` が `no longer found` で exit 1 になる。許可行を消すと policy の file を変えたことになり、repo health の `policy.changed`（必須、severity error）が fail になる。コードと同じ branch なので dispatcher の `policy_mixed`（D6.1）が review を差し戻す。許可行の削除とコードの削除は別の branch に分けられない（どちらを先にしても片方の検査が落ちる）。

人は 3 行の削除を承認し、仕組みとして「許可の一覧から行を消すだけの変更は policy の変更に数えない」を選んだ。

## 実装前 brief
1. 根本原因: `policy.changed` は policy の file の base と head の blob の違いだけで決まる（`health/engine.py` の `build_report` と `rules/policy_changed.py` が別々に同じ式を持つ）。許可の一覧は行を消すと許可が減り、検査は厳しくなる方向にしか変わらない。それでも blob が違うので、検査を緩める変更と同じ扱いになる。症状は、許可の一覧の掃除がコードの変更と同じ branch に入ったときの `policy_mixed` と D6.4 の人の承認の待ちとして出る。
2. 責務 owner: `policy.changed` の判定を持つ `scripts/dev/repo-health/health/engine.py`。どの file が許可の一覧かは repo 固有なので profile（`scripts/dev/repo-health/profiles/celeris.toml`）が持ち、欄の解釈は `health/profile.py` が持つ。
3. 既存の再利用先: profile の正規表現の一覧の検証 `_check_regexes`（`policy_paths`・`exclude_paths` と同じ）、policy の file の一覧 `_policy_files`、`Context`（rule への入力）、`Tree.has`・`Tree.read_bytes`。試験は `scripts/tests/test_repo_health_core.py` と同じ一時 git repo の作り方と `validate_report.py`。
4. 選択肢:
   - (a) profile に許可の一覧の path を書き、engine の 1 か所で「行を消すだけ」を判定し、report と rule の両方がその結果を使う。採用。
   - (b) 許可の一覧を `policy_paths` から外す。行を足す変更（検査を緩める変更）も人の review なしに通る。採らない。
   - (c) dispatcher（Rust）の `policy_mixed` で許可の一覧の削除を見逃す。report の `policy.changed` と dispatcher の判定が食い違い、`check-health.sh`（D6.2）・reviewer の prompt・`policy_review_gate` がそれぞれ別の判定を持つ。採らない。
   - (d) 検査器のコードに 2 つの file の path を書く。検査器は repo 固有の名前で分岐しない（契約 ADR の適用範囲）。採らない。
5. 新しい状態・分岐・fallback・port: profile の任意の欄 `policy_allowlists`（既定は空、組み込み generic には無い）と、engine の判定関数 1 つ、`Context.policy_changed`（数える policy の file。rule が読む）。旧い判定と新しい判定を切り替える flag・互換分岐は置かない。
6. 撤去対象: `rules/policy_changed.py` が自分で持っていた blob の比較（`ctx.policy_files` を `base_blob != head_blob` で絞る式）。rule は engine の結果 `ctx.policy_changed` だけを読む。
7. 不変条件: (i) 行を足す・書き換える・file を足す・消す・読めない内容は今までどおり `policy.changed=true`。(ii) 判定に使う `policy_allowlists` は base 側（初回は bootstrap の head）の profile のもので、head が profile に欄を足しても、その変更は判定に使わない。(iii) report の `policy.changed` と rule `policy.changed` の finding は同時に在る・同時に無い（`validate_report.py` の S8）。(iv) report の schema（`policy.files` の形）は変えない。試験は `scripts/tests/test_repo_health_allowlist_shrink.py`。

## 決定
- D1. profile の欄: top-level に任意の `policy_allowlists = ["<regex>", ...]`。repo 相対 path に当てる正規表現の一覧で、検証は `policy_paths`・`exclude_paths` と同じ（文字列の配列で、各要素が正規表現として読める）。既定は空。組み込み generic には置かない。profile は今までどおり policy の側（base、初回は bootstrap）で読む。`policy_paths` に当たらない file は policy の file でないので、この欄は policy の file のうち許可の一覧とみなすものを選ぶだけになる。
- D2. 判定（engine の 1 か所）: policy の file（`policy.files` の各行）は、base と head の blob が違えば数える。ただし次を全部満たすときだけ数えない。
  1. path が `policy_allowlists` のどれかに当たる。
  2. base と head の両方の tree に file がある（追加・削除は数える）。
  3. 両方の内容が UTF-8 として読める（strict に decode する。読めなければ数える）。
  4. head の内容の行の集合が、base の内容の行の集合に含まれる。内容の行は、内容を `'\n'` で分けた行のうち、`strip()` して空になる行と、列 0 が `#` の行を除いたもの。行は改行以外を変えずに比べる（行末の空白・`'\r'` も比べる）。
  report の `policy.changed` は数えた file が 1 つでもあれば true。rule `policy.changed` の finding の evidence は数えた file だけにする。`policy.files` には今までどおり policy の file の全部と両側の blob を書く（形は変えない。数えなかった file も base と head の blob が違う行として残る）。
- D3. Celeris の profile: `policy_allowlists = ['^scripts/dev/(adr-allowed-duplicates\.txt|human-text-lint\.allow)$']`。検査器のコード（`health/`・`rules/`・`repo_health.py`）には repo 固有の path を注釈も含めて書かない。
- D4. dispatcher（Rust）は変えない。`policy_mixed`（D6.1、`crates/task-dispatch/src/health_review/verdict.rs`）と D6.4 の関門（`policy_review.rs` の `policy_review_gate`）は report の `policy.changed` が true のときだけ働き、検査器の判定を写すだけで自分では判定しない。`check-health.sh` の D6.2（`engine.policy_review_files`）も report の `policy.changed` を読む。`HealthPolicyState.files`（`input.rs`）は blob の違う policy の file の一覧なので、数えなかった許可の一覧もそこに載る。reviewer の prompt（`task-worker` の `change_rules/review.rs`）には `changed=false` と並んでその file が出るが、`changed=false` の repo では D6.3 の `health.policy` を求めず、D6.1・D6.4 も働かない。
- D5. 試験: `scripts/tests/test_repo_health_allowlist_shrink.py`。消すだけ → false・finding 無し、1 行足す → true、1 行書き換える → true、許可の一覧でない policy の file（profile）から行を消す → true、head が profile の `policy_allowlists` に path を足しつつ行を消す → true（base 側の profile で判定し、その file も evidence に入る）、許可の一覧の file を消す → true。各 report は `validate_report.py` を通る。

## 許可の一覧の読み手（行を消すと検査が厳しくなる方向にしか変わらないことの確認）
2026-11-16 に repo の中の読み手を `git grep` で探し、次の 3 つだけだった。どれも行を独立の許可として読み、行の間に依存（順序・前の行の続き・節）が無い。列 0 の `#` の行と空白だけの行は、どれも許可として読まない。D2 は「内容の行」をこれより広く取る（例: 字下げした `#` の行・行の途中の `#` を内容の行として比べる）ので、内容の行を減らすだけの変更は、どの読み手でも許可の集合を減らすか変えない。

| 読み手 | 読み方 | `#`・空行 |
|---|---|---|
| `scripts/dev/check-human-text.py` の `allow_list` | 1 行を `rule<TAB>snippet<TAB>理由` に分け、`(rule, snippet)` を集合に入れる。形が違う行は exit。finding が集合に無ければ fail、集合のうち使われない許可は `no longer found` で fail | `not raw.strip()` と `raw.startswith("#")` の行を飛ばす |
| `scripts/dev/check-adr-numbers.sh` | `sed -e 's/#.*//' -e 's/[[:space:]]//g'` の後の空でない行を、許可する ADR の file 名の一覧にする。同じ番号の ADR が全部一覧にあるときだけ重複を許す | `#` から行末を消し、空になった行を捨てる |
| `crates/task-dispatch/src/auto_resolve/classify.rs` の `allowed_adr_duplicates` | 1 行ごとに `#` より前を `trim()` し、空でないものを集合に入れる（統合の番号重複 resolver が使う） | 同上 |

- `check-human-text.py` では、使われない許可を消すと `no longer found` が消える。これは使われていない許可が減るだけで、許す finding は増えない。
- 空白だけの行の扱い: D2 は Python の `str.strip()`（Unicode の空白も除く）を使う。`check-adr-numbers.sh` の `[[:space:]]` は locale によって全角空白を消さないが、全角空白だけの語は ADR の file 名に当たらないので、許可は増えない。
- `#` の行・空行だけを足す・書き換える変更も数えない（内容の行の集合が変わらない）。どの読み手も、その行を許可として読まない。

## 採らなかった案
- head の profile の `policy_allowlists` で判定する案。候補が任意の policy の file（例 profile 自身・baseline）を許可の一覧として登録し、同じ branch で行を消して検査を緩められる。契約 D2 と同じく base 側で判定する。
- 行の多重集合（同じ行の数）で比べる案。どの読み手も同じ行の重複を 1 つの許可として読むので、集合で足りる。
- 許可の一覧の形（`rule<TAB>snippet<TAB>理由` など）を検査器が解釈する案。読み手ごとの形を検査器が持つと、repo 固有の解釈が検査器に入る。行の集合の包含は形を問わず、どの読み手でも許可を増やさない。
