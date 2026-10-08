# WorkUnit の `checks` の書き方: 普通の check と範囲 check（`scope: true`）

WorkUnit（WU）の `checks` は daemon が決定的に流す検査で、worker の自己申告の代わりに完了を決める。
check には 2 種類あり、**いつ・どこで流れるかが違う**。範囲 check を普通の check や task の acceptance として
書くと、統合後や final review で他の変更を拾って必ず落ちる。決定の本文は
[ADR-0074 付記「範囲 check は WU の作業時だけ流す（`WorkUnitCheck.scope`、2026-10-05）」](../adr/0074-parallel-work-units-checkpoints-milestones-quota.md#付記-範囲-check-は-wu-の作業時だけ流すworkunitcheckscope2026-10-05)。

## 種類

| 種類 | 書き方 | 何を見るか |
|---|---|---|
| 普通の check | `{"cmd":"...","expect_exit":0}` | WU の成果が正しいか（試験・lint・生成物の一致など）。統合後に流しても意味が変わらないもの |
| 範囲 check | `{"cmd":"...","expect_exit":0,"scope":true}` | WU 自身の変更が許可範囲の path に収まるか。WU の作業ツリーで WU の base と比べるときだけ意味がある |

`scope` は既定 `false` で、`false` は JSON に書かない。`scope` の無い既存の計画は従来どおり（全 check が統合でも流れる）。

## いつ・どこで流れるか

| 場面 | 流れる check | 場所 |
|---|---|---|
| WU の作業時（worker が done を返した後、daemon が WU を commit する前） | その WU の全 check（普通 + 範囲） | WU の作業ツリー（WU 専用の worktree。無ければ Task の worktree、git の worktree が無い task は Task の directory） |
| 段の統合（D1.4 の 4、`start_integration`） | その段の生きた葉の WU の**普通の check**（`cmd` の重複を除く）と `workspace.toml` の check | Task の worktree（統合後の tree） |
| 子 task への昇格（`promote_to_task`、ADR-0079 D4 (3)） | **普通の check だけ**を子 task の command の acceptance に写す。残らなければ `done_when` → reviewer | 子 task の final review |

範囲 check は統合後・final review では流れない。統合は first-parent の merge で、各 WU の commit は WU の作業時に
範囲 check を通っている。したがって **範囲 check を task の acceptance に書かない**。範囲を見たいなら、その変更をする
leaf の `checks` に `scope: true` で置く。

repair に渡す「範囲外差分の check」（`repair_scope_from_units`・`review_repair_scope`）は `scope: true` の check を
正として集める（`cmd` に `git diff` を含むという従来の推定も互換のため残っている）。

## 環境変数

WU の checks と WU の run の環境に、daemon が次を入れる（定数は `task_core::execution_plan::{WU_BASE_ENV, WU_TARGET_ENV}`）。

- `CELERIS_WU_BASE` — 初回 run 開始時の追跡済み作業木を記録した一時 commit。先行 WU の未 commit 変更も含む。
  git がある直列・共有作業木にも渡る。統合用の `WorkUnitRow.base_commit` とは別で、retry でも同じ値。
- `CELERIS_WU_BASE_UNTRACKED` — 開始時に存在した未追跡 path 一覧の file（Git の quoted path 形式）。
- `CELERIS_WU_SCOPE_PATHS` — `sh "$CELERIS_WU_SCOPE_PATHS"` で開始時の全 file snapshot と現在の差分 path を出す補助。
  未追跡 file の追加・変更・削除、symlink・mode の変更、worker が commit した変更も扱う。
- `CELERIS_WU_TARGET` — 統合先 = Task のブランチ（`<prefix><task_id>`、通常は `celeris/<task_id>`）。git の worktree が
  あるときだけ。git の無い作業場所には snapshot の 3 変数も設定しない。remote は対象外。

planner は計画時に base の sha を知らないので、範囲 check は必ずこの変数を使う。ハードコードした sha や
`$(git merge-base HEAD main)` と比べない（main は task の途中で動き、merge-base は他の WU・main の変更を含みうる）。
worker も同じ変数で同じ check を自分で流し、done を返す前に範囲外の path が無いことを確かめられる。

## 範囲 check の既定の形

```sh
paths=$(if [ -n "${CELERIS_WU_SCOPE_PATHS:-}" ]; then sh "$CELERIS_WU_SCOPE_PATHS"; else git diff --name-only "${CELERIS_WU_BASE:-HEAD}" && git ls-files --others --exclude-standard; fi) || exit 1
out=$(printf '%s\n' "$paths" | sort -u | grep -vE '^(<許可 path の正規表現>)'); [ -z "$out" ] || { echo "out of scope:"; echo "$out"; exit 1; }
```

- `git diff --name-only "${CELERIS_WU_BASE:-HEAD}"` は追跡済みの base と作業木の差を出す。
  `git ls-files --others --exclude-standard` は先行 WU の未追跡 file も列挙するため、そのまま合わせる旧式は使わない。
  補助がある場合は必ず補助の出力を使う。一覧を除外するだけでは開始時の未追跡 file の編集・削除を見逃す。
- 補助の失敗は `|| exit 1` で伝播する。`sort` や `grep` の pipeline に直接入れて失敗を隠さない。
- snapshot は範囲比較用であり HEAD の祖先とは限らない。commit だけの確認には統合用 base を使う。
- 保存先は WU が実行される先頭リポジトリの git directory の `celeris-wu-bases/<wu-id>/`。
  snapshot と補助は実 index・HEAD・作業 file を変更しない。事後 check は保存済み snapshot を読み、再取得しない。

- 許可 path の正規表現には、Objective の範囲に加えて次を必ず入れる: WU 自身の記録（`agent-docs/progress/`、その WU の
  ADR `agent-docs/adr/`）、計画がその WU に書いてよいと言った path、その変更で再生成される file（`docs/protocol/` と
  `docs/api/v1/` の schema、`gui/`・`web/` の生成された型）。

## 落ちるときは path を出す

範囲 check は範囲外の path を標準出力に出してから非 0 で終える（上の形）。`test -z "$(…)"` の無言の形は書かない —
落ちてもログが空で、何が範囲外だったか分からない。daemon は `scope: true` の check が stdout・stderr とも空で落ちたとき、
判定文に「範囲 check が範囲外の path を出していない」旨の一文を足す（原因の特定を速くするためだけで、判定は変えない）。

## sh は dash

checks は `/bin/sh`（dash）で走る。`${s:0:12}`・`[[ ]]`・配列などの bash 専用の構文は使わない。上の形は POSIX sh で動く。

## 動機: 2026-10-04〜05 の本番の誤検出

どれも範囲 check が WU の作業時以外（段の統合後・final review の機械的再評価）で流れ、他の WU・main の変更を拾って落ちた。
repair → replan → 人の対処を招き、木の replan 上限に達する一因になった。

個々の経緯は ADR-0074 の付記の背景にある範囲の記録しか残っていない（下の 1 行はその要約）。

- 01M440S16A（受け入れ条件 3）— 範囲 check が WU の作業時以外で流れ、他の変更を拾って落ちた。
- 01M44FP87W — 同上（範囲 check の統合後・再評価での再実行による誤検出）。
- 01M4577C94（dispatch-context）— 同上。加えて `test -z "$(…)"` の無言の形だったため、ログが空のまま failed になった。
- 01M44C029S（fix-r2）— 同上。

関連: [試験の指針](testing.md)（CPU を焼く負荷を check に置かない）。
