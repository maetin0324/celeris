---
tasks: [01M6TYVJSRM3SFQ3VHWQE4MZ9M]
---
# ADR 2026-11-07: 汎用 repo health の契約（入力・profile・状態・report v1）

- 状態: 採用（契約）。この ADR では検査器 `scripts/dev/repo-health/repo_health.py` を実装しない。実装は健全性 v2 G1 の後続 WorkUnit（core・rules-structure・rules-dup・rules-archmap・celeris-profile）。この WorkUnit が置いたのは report の形 `scripts/dev/repo-health/schema/report-v1.schema.json`、検証器 `scripts/dev/repo-health/validate_report.py`、妥当な例 `schema/examples/*.json`、不正な例 `schema/bad-examples/*.json`、試験 `scripts/tests/test_repo_health_contract.py`。
- 親: 健全性 v2 の親 task 01M6TYV9DTZESAJJMXRQVGZ56V、slice G1（[ADR 2026-11-07 健全性 v2 の基準](2026-11-07-health-v2-baseline.md) D2）。
- 関係: [ADR-0083](0083-source-size-guardrail.md)（source-size の baseline と例外）、[ADR-0128](0128-docs-layout.md)（文書検査）、[2026-10-11 破壊的変更を既定にする](2026-10-11-breaking-changes-by-default.md)、[2026-10-20 全体試験は release だけ](2026-10-20-full-tests-at-release-only.md)。
- 適用範囲: Git 管理下のソフトウェア repo 一般。検査器は repo 名・案件名・self_improve などの flag で振る舞いを分岐しない。Celeris 固有の名前（TaskStore・Dispatcher・crate の層・architecture-map の path）と上限は `scripts/dev/repo-health/profiles/celeris.toml` にだけ書く。

## 結論
- 検査器は「repo・base・head・policy 版・profile」を入力に、1 つの JSON report（`celeris.repo-health.report/1`）を書き、exit code 0（合格）・1（fail）・2（error・欠測・使い方誤り）で返す。同じ入力からは byte 単位で同じ JSON が出る。
- policy（profile・baseline・検査器）は **base 側の内容**で評価する。候補（head）が policy の file を変えていれば `policy.changed` finding（必須 rule、severity error）になり、その report は合格しない。policy の変更は別の review で承認する。base に policy が無い初回は `policy.source=bootstrap` と明示し、警告にとどめる。
- rule の状態は pass・fail・error・unknown・not_applicable・unsupported の 6 つ。必須 rule の fail は exit 1、error・unknown・unsupported は exit 2。合格にできるのは必須 rule が全部 pass か not_applicable のときだけ。
- 既存負債との差分: base にも同じ fingerprint の finding があれば `existing`、値が悪化していれば `worsened`、無ければ `new`。fail に数えるのは severity error で delta が new か worsened、例外の無い finding だけ。
- 静的推定の finding（dup.*・link.unlinked・migration.remnant）は `candidate=true` で、severity は warning か info。検査器も後続の機構も候補を削除・書き換えしない。
- 形の正しさは schema で、集約・一致の規則（S1〜S12）は `validate_report.py` で機械的に確かめる。examples は 6 状態・doc-only・shallow・dirty worktree・policy.changed・bootstrap・組み込み generic を含む。

## 決定

### D1. 入力と CLI（契約 (1)）
```
python3 scripts/dev/repo-health/repo_health.py --repo <path> --base <rev> --head <rev|worktree> \
    --policy-version <ver> [--profile <name|path>] --out <report.json>
```
- `--repo`: 1 つの Git 作業ツリー（または bare でない repo）の root。root でない path は `git rev-parse --show-toplevel` で root に寄せる。Git 管理下でなければ report を書かず exit 2（`not_a_git_repo`）。
- `--base`: 比べる基準の commit（ref 名か sha）。task では `git merge-base HEAD main`、WorkUnit では `$CELERIS_WU_BASE` を渡す。検査器は merge-base を自分で計算しない（呼び出し側が基準を明示する。進捗の規則と同じ）。
- `--head`: 候補。commit（ref か sha）か `worktree`（作業ツリー。未 commit の変更と ignore されない未追跡 file を含む）。
- `--policy-version`: 呼び出し側が期待する policy の版。base 側 profile が宣言する `version` と違えば report を書かず exit 2（`policy_version_mismatch`）。review の証跡に「どの版の policy で合格したか」を残すための欄で、report の `inputs.policy_version` と `policy.version` は常に同じ。
- `--profile`: 名前（`profiles/<name>.toml`）か TOML file の path。省略時は D2 の順で解決する。
- `--out`: report の書き先。stdout には人向けの要約（rule ごとの status と finding の数）だけを出す。report は `--out` にだけ書く。
- 採らなかった: `--mode doc-only` の flag。mode は diff から検査器が決める（D7）。手で指定できると構造 rule を外して合格できる。

### D2. profile の解決順と policy の評価側（契約 (2)）
- 解決順: `--profile` > repo 内の既定 `scripts/dev/repo-health/profiles/default.toml` > 組み込み `generic`。名前は対象 repo の `scripts/dev/repo-health/profiles/` を先に、無ければ検査器自身の `profiles/` を見る。report の `inputs.profile` に requested・resolved・source（cli・repo・builtin）を記録する。
- profile は TOML。`version`（policy 版）、`extends`（別 profile 名。1 段だけ）、rule ごとの `required`・閾値・例外（`reason` と出典行が必須）、言語 adapter の有効化、`policy_paths`（policy とみなす path の正規表現）、`doc_only.paths`、`arch_map.path` を持つ。項目の正本は core WorkUnit が profile の schema として置く。Celeris の既定は `profiles/default.toml` が `extends = "celeris"` で `celeris.toml` を指す（celeris-profile WorkUnit）。
- **policy は base 側で評価する**: profile・baseline（例: `scripts/dev/source-size-baseline.json`）・検査器（`scripts/dev/repo-health/**` と包んだ既存検査の script）は `git show <base>:<path>` で読む。head・worktree 側の内容は評価に使わない。
- `policy_paths` の既定（組み込み generic）は `^scripts/dev/repo-health/` だけ。Celeris の profile はこれに既存検査の script と baseline を足す。
- head が policy の file を 1 つでも変えていれば `policy.changed=true` とし、file ごとに base と head の blob を `policy.files` に書き、rule `policy.changed`（必須）の finding（severity error、`delta=new`、evidence は変えた file）を出す。結果は fail（exit 1）。policy の変更は、その変更だけを対象にした別の review（人か、G2 で reviewer に渡す policy review）で承認し、承認後の main を base にして再評価する。
- base に profile が無い初回: `policy.source=bootstrap`。head の profile で評価し、`policy.changed` の finding を severity warning で出す（合格を妨げない）。report を読む側は「初回の基準」として扱う。profile が base にも head にも無ければ `policy.source=builtin`（組み込み generic）。base が読めなければ `policy.source=unavailable`（D7 の shallow）。
- 採らなかった: head 側の policy で評価して差分だけ警告する案。候補が閾値を上げるか例外を足せば自分を合格にできる。親 task の「検査器・policy・baseline を弱めて合格できない」に反する。
- 採らなかった: repo root に `.repo-health.toml` を置く案。G1 の write_set（`scripts/dev/repo-health/` 以下）の外で、Celeris 以外の repo でも tool の dir と離れる。既定 profile も `profiles/` に置く。

### D3. 状態の意味と集約・exit code（契約 (3)）
| status | 意味 | 必須 rule のとき |
|---|---|---|
| pass | 評価でき、fail 級の finding が無い | 合格 |
| fail | 評価でき、fail 級の finding（D6）が 1 件以上ある | verdict fail、exit 1 |
| error | 評価を試みたが完了しなかった（検査器の例外・timeout・入力の parse 失敗）。`reason` は `crashed:<where>` など | verdict error、exit 2 |
| unknown | 評価は完了したが判定できない（baseline が読めない・値が比べられない）。`reason` は `baseline_unreadable` など | verdict error、exit 2 |
| not_applicable | この repo・profile・diff には評価対象が無い（doc-only、adapter の言語が無い repo で profile が rule を無効にした、対象 path が 0） | 影響しない |
| unsupported | 評価が必要だが、この環境では行えない（`tool_missing:<name>`、`adapter_missing:<lang>`、`base_unreachable`） | verdict error、exit 2 |

- 集約: 必須 rule に error・unknown・unsupported が 1 つでもあれば verdict `error`（exit 2）。無ければ fail が 1 つでもあれば `fail`（exit 1）。どちらも無ければ `pass`（exit 0）。error が fail に勝つのは、error の report は結果が揃っておらず「この fail だけ直せば合格」と読めないため。
- 必須でない rule（既定では候補 rule dup.*・link.unlinked・migration.remnant）の status は verdict に影響しない。report にはそのまま残す。
- `summary.reviewable`: head が commit で、dirty でなく、base に届くときだけ true。false の report（worktree・dirty・shallow）は手元の確認用で、review の証跡にしない。
- exit 2 は report が書けないときにも使う（使い方誤り・non-Git・policy 版の不一致）。その場合 stderr に理由を出し、`--out` には書かない。
- 採らなかった: fail が error に勝つ案（exit 1 を優先）。fail を直して再実行すると exit 2 に変わり、合格までの手順が増える。先に環境を直させる方が速い。
- 採らなかった: unknown を warning 扱いで合格にする案。欠測を合格にしないのは親 task の条件。

### D4. finding の欄（契約 (4)）
| 欄 | 内容 |
|---|---|
| `rule_id` | D5 の ID |
| `fingerprint` | `<rule_id>:<hash>`。rule と正規化した evidence（path・symbol・表名。行番号と行の内容は含めない）から決める安定な鍵。base との突き合わせ（D6）に使う |
| `severity` | error・warning・info。fail に数えるのは error だけ |
| `delta` | new・worsened・existing（D6） |
| `candidate` | 静的推定なら true。候補は削除・書き換えの対象にせず、severity は warning か info（検証器 S4） |
| `evidence[]` | `path`（repo root からの相対。先頭 `/` と `..` は不可）、`line`・`end_line`、`ref`（base・head）、`symbol`、`excerpt`（400 文字まで）。1 件以上 |
| `snapshot` | base sha（shallow なら null）・head sha・dirty・shallow。report の `snapshot` と一致する（S1）。finding を report から切り離して引用しても対象が分かるようにする |
| `scope` | `kind`（repo・crate・package・module・file・doc）と `id` |
| `exception` | null か `{reason, source: {path, line}}`。base 側 profile の例外。例外付きは fail に数えない |
| `rationale` | 判定根拠。閾値・baseline の値・比べた対象を文で書く |
| `baseline` | 任意。値の悪化を見る rule の base の値・head の値・閾値 |

- report 全体の `snapshot` は base（sha・reachable）、head（kind・sha・dirty）、shallow、submodules（path と両側の sha）。
- 時刻・絶対 path・乱数・実行時間は report に入れない（schema は追加欄を拒む）。実行時間が要るなら report と別の file に書く。

### D5. rule ID（契約 (5)）
| ID | 内容 | 候補 | 既定 required |
|---|---|---|---|
| `deps.violation` | profile の層・許可表に反する crate/module 依存 | no | yes |
| `deps.cycle` | module/crate の循環 | no | yes |
| `size.responsibility-growth` | 巨大責務の悪化（fn 宣言数・field 数・行数が base より増えて閾値超え） | no | yes |
| `dup.contract` | 同じ契約（型・schema・wire 仕様）の重複 | yes | no |
| `dup.state` | 同じ状態（表・file）の複数の書き手 | yes | no |
| `dup.public-surface` | 同じ公開口（API・CLI・event kind）の重複 | yes | no |
| `link.unlinked` | 参照されない候補（未参照 fn・module・doc） | yes | no |
| `dup.test-fixture` | 試験・fixture の重複 | yes | no |
| `migration.remnant` | 移行の残骸（deprecated・互換分岐・新旧切替 flag・退役した path への参照） | yes | no |
| `arch-map.<kind>` | architecture-map との食い違い。kind は `missing_row`・`missing_test`・`wrong_owner`・`missing_invariant`・`stale`・`wrong_path` | no | yes |
| `doc.links`・`adr.numbers`・`progress.index`・`human-text`・`source-size` | 既存検査の包み（`rules[].wraps` にその script の path）。既存 script の非 0 exit を finding に写す | no | yes |
| `policy.changed` | D2 | no | yes |

- profile は `required` を上書きできる（base 側の内容で）。ID の一覧は schema の pattern が固定する。ID を足すときは schema・検証器・この ADR を同じ変更で更新する。
- 包んだ既存検査は repo_health.py の中で同じ入力（root・base）を受け取って動く。各 script が自分で root・base を解決する重複は celeris-profile WorkUnit で消す（slices.json の G1 `remove`）。

### D6. 既存負債との差分（契約 (6)）
- 検査器は同じ rule を base にも適用し、fingerprint で突き合わせる。base にあれば `existing`、`baseline.value` より `head_value` が悪ければ `worsened`、無ければ `new`。base で見つかって head に無いものは report に入れない（`summary.counts` にも数えない。直した数は base の report と比べれば分かる）。
- fail 級 = severity error かつ delta が new か worsened かつ exception が null。rule の status は fail 級の有無で決まる（検証器 S3）。existing は report に残るが合格を妨げない。
- 突き合わせに使う base の評価は、base の policy・base の tree で行う。source-size の frozen baseline（ADR-0083）は base 側の file をそのまま読む。
- 採らなかった: 既存負債を report から外す案。負債の全体量（B0 の計測）と review の材料が失われる。

### D7. 軽量経路と環境の扱い（契約 (7)）
- doc-only: base..head（worktree なら作業ツリー）の変更 path が全部 profile の `doc_only.paths`（既定 `\.md$` と `^docs/`・`^agent-docs/`）に当たれば `inputs.mode=doc-only`。構造 rule（deps.*・size.*・dup.*・link.unlinked・migration.remnant・source-size）は `not_applicable`、文書 rule（doc.links・adr.numbers・progress.index・human-text・arch-map.*）と policy.changed だけ評価する（検証器 S10）。cargo・言語 adapter を起動しない。
- tool 未導入: adapter が必要な外部 tool（cargo・node など）が無ければ、その rule は `unsupported`（`tool_missing:<name>`）。必須なら exit 2。tool が無いことを pass にしない。
- 複数 repo・submodule: 1 回の実行は 1 つの Git 作業ツリー。submodule は `snapshot.submodules` に両側の sha を記録するだけで中に入らない（submodule は別の実行の `--repo` にする）。複数 repo の task は repo ごとに実行し、report は結合しない。検証器は複数 report を引数に取り、全件妥当で exit 0。
- dirty tree: `--head worktree` だけが dirty になりうる。`snapshot.head.dirty=true`、`summary.reviewable=false`。commit の head は dirty にならない（S7）。
- shallow clone: base の object が無い（`git cat-file -e <base>^{commit}` が失敗）なら `snapshot.shallow=true`・`base.sha=null`・`base.reachable=false`、policy は `unavailable`、全 rule は `unsupported`（`base_unreachable`）、verdict error（S9）。深さを足してから再実行する。
- non-Git: report を書かず exit 2（D1）。
- 採らなかった: shallow のとき head だけで評価する案。差分も policy も base 無しでは決まらず、既存負債との区別がつかない。

### D8. report の形・検証器・決定性
- 形は `schema/report-v1.schema.json`（JSON Schema 2020-12 の部分集合）。検証器 `validate_report.py` は標準ライブラリだけで形と意味規則 S1〜S12 を確かめる。使い方は `python3 scripts/dev/repo-health/validate_report.py <report.json>...`、全件妥当で exit 0、1 件でも不正なら `file:JSON pointer: 内容` を出して exit 1。
- 意味規則: S1 finding の snapshot の一致、S2 rule_id の存在、S3 status と fail 級 finding の整合、S4 候補の severity、S5 verdict・exit_code の集約、S6 counts、S7 reviewable、S8 policy.changed と finding の同時存在（bootstrap は error でない）、S9 base 不達は error、S10 doc-only の not_applicable、S11 fingerprint の一意性と rule_id 接頭、S12 pass 以外の reason。
- 決定性: 同じ repo の内容・base・head・policy 版・profile からは byte 単位で同じ JSON（key は schema の順、findings は fingerprint 順、evidence は path・line 順、`ensure_ascii=False`・indent 2）。core WorkUnit は同じ入力で 2 回実行して diff が空であることを試験にする。
- examples は 12 件（pass・fail・error・unknown・doc-only の not_applicable・unsupported 2 件・shallow・dirty worktree・policy.changed・bootstrap・組み込み generic）、bad-examples は 21 件（必須欄欠け・未知の status・必須 fail/unknown/unsupported を pass と報告・候補の error・snapshot 不一致・未知の rule・絶対 path・dirty で reviewable・policy.changed の finding 欠け・時刻の追加欄・fingerprint 重複・doc-only で構造 rule 評価・existing を fail・shallow で pass・counts 不一致・evidence 無し・reason 無し・policy 版不一致・bootstrap の error）。
- 採らなかった: jsonschema package を使う案。worker と release gate に外部 package を入れない（Python 3 標準ライブラリだけ）。
- 採らなかった: report に実行時刻・所要時間を入れる案。決定性（同じ入力で同じ byte）を崩し、review の証跡の比較を難しくする。

## 後続 WorkUnit への引き継ぎ
- core: D1 の CLI、D2 の解決と base 側読み、D7 の mode・snapshot、D8 の決定性と report 出力。出力は必ず `validate_report.py` を通す試験を持つ。
- rules-structure・rules-dup・rules-archmap: D5 の rule を実装し、finding は D4 の欄を埋める。候補は candidate=true・severity warning 以下。
- celeris-profile: `profiles/celeris.toml` と `profiles/default.toml`、`check-health.sh` からの呼び出し、既存 script の root・base 解決の削除。
