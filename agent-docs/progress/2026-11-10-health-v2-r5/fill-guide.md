---
title: 健全性v2 R5 — map leaf の共通手順（試験関数に契約を付け、組を判断する）
tasks: [01M7388T31FEX7W469M2TJS236]
status: done
updated: 2026-11-10
---
# 健全性v2 R5 — map leaf の共通手順（試験関数に契約を付け、組を判断する）

map leaf（`map-api-a`・`map-api-b`・`map-api-c`・`map-celeris-src`・`map-cli`）と `old-impl` がこの手順に従う。
台本は [`contracts.py`](contracts.py)、走査の結果と件数は [`scanner.md`](scanner.md) にある。
下のコマンドは repo の root で実行する（`C=agent-docs/progress/2026-11-10-health-v2-r5/contracts.py`）。

## 1. 手順
1. 雛形を作る: `python3 $C --scaffold <name>`。`mapping/<name>.json` に、scope の file・試験・この fragment が持つ組が入る。既にあれば上書きしないで止まる。作り直すときは file を消してから実行する。
2. file ごとに `capability` と `contract` を書く。1 つの file の中で契約が違う試験だけ、`fns` に試験ごとの上書きを書く（3 節）。
3. この fragment が持つ組（`groups`）を全部判断する（4 節）。
4. `python3 $C --check-fragment agent-docs/progress/2026-11-10-health-v2-r5/mapping/<name>.json` が 0 で終わるまで直す。不備は 1 行に 1 件ずつ stdout に出る。
5. 進捗 `agent-docs/progress/2026-11-10-health-v2-r5/<leaf key>.md` に、試験の数・capability ごとの数・採った組と採らなかった組の数・`extra_capabilities` の一覧を書く。commit は件名で書く。

fragment の scope（`contracts.py` の `FRAGMENTS`。path の正規表現）:

| name | scope |
|---|---|
| `api-a` | `^crates/task-api/tests/[a-c]`（`tests/common/` を含む） |
| `api-b` | `^crates/task-api/tests/[d-r]` |
| `api-c` | `^crates/task-api/(tests/[s-z]\|src/)`（`tests/support/` を含む） |
| `celeris-src` | `^crates/celeris/src/` |
| `cli` | `^crates/(celeris/tests/\|celerisctl/)` |

## 2. test id の形
- 試験の id は `<repo 相対の file path>::<file の中の inline mod の path>::<fn 名>`。inline mod の外にある fn は `<path>::<fn 名>` になる。
  - 例: `crates/celeris/src/lib/tests.rs::nested::case`、`crates/task-api/tests/browser_waits.rs::waits_are_listed`。
- 同じ id が 2 つ以上あるとき（`#[cfg]` で分けた同名の fn）は、2 つ目から `#2`・`#3` を付ける。
- helper の id も同じ形（`crates/task-api/tests/common/mod.rs::env`）。
- `mapping/<name>.json` の `files.<path>.tests` と `fns` の key には、id から `<path>::` を除いた部分（`nested::case` など）を書く。
- crate を含む module の path は別の欄 `mod_path` にある（`lib::delivery::tests`、`test:browser_waits`、`bin:celerisctl::commands::query::tests`）。id には使わない。

## 3. 契約の書き方
- `capability` には B0 の [capability-map.json](../2026-11-07-health-v2-b0/capability-map.json) の `capabilities[].id` を書く。雛形の `b0_hint` は、試験の file か試験 module の持ち主の src file を B0 の `tests`・`owners` が指す capability の一覧で、候補として使う。
- B0 に合う capability が無いときは `extra_capabilities` に足す: `{"id": "<a-z0-9->", "name": "<1 行>", "owner_paths": ["crates/<crate>/src/..."], "reason": "<B0 に無い理由、1 行>"}`。id は B0 の id と重ねない。`owner_paths` は crates/ の下に実在する path にする。ほかの fragment の `extra_capabilities` も参照できる（`--check-fragment` は `mapping/*.json` の全部を読む）。
- `contract` は 1 行で 200 字以内。「何が何をすると何になるか」を書く。試験名の言い換えや「〜を試験する」で終わる文にしない。
  - 例: 「承認待ちの wait は人の承認が無い間 credential を worker に渡さず、承認後に broker が注入する」。
  - 試験の doc comment の 1 行目（scan の `doc`）が契約を言い当てていれば、それを短くして使う。
- 優先順は試験ごとの上書き（`files.<path>.fns.<id の file 内部分>`）> file の値。上書きには `capability` と `contract` の片方だけを書いてもよい。書かなかった欄は file の値になる。
- scope の全試験が `capability`（知っている id）と `contract`（1 行）を持つまで `--check-fragment` は 0 にならない。

## 4. 組の判断
`groups` に入るのは、この fragment が持つ組（先頭の member の path がこの fragment の scope にある組）だけ。組の member は走査が決め、map leaf は変えない。

- `duplicate_setup`: 2 つ以上の file にある helper（試験 code の中の、impl・trait の外の試験でない fn。本体が空の `{}` は除く）で、コメント・空白を除いた本体の sha256 が同じ組。member は helper の id。雛形の `users` は、その helper を setup に持つ試験。
- `equivalent_scenario`: 同じ file か、同じ B0 capability（`b0_hint`）の試験で、literal（文字列・数・bool）を伏せた本体の sha256 が同じ組。member は試験の id。不変条件の試験（`invariant=true`）は走査の段階で組から除いてある。

各組に次の欄を書く:

| 欄 | 書くこと |
|---|---|
| `adopt` | 候補として採るなら `true`、採らないなら `false` |
| `reason` | 採る・採らない理由（1 行）。例: 「同じ 3 行の HTTP 要求の helper を 14 file が写している」「本体は同じだが別の server を起動する fixture で、意味が違う」 |
| `keep_tests` | 採るとき: 残す試験の id（1 件以上。実在する試験）。`equivalent_scenario` は member から選ぶ。`duplicate_setup` は `users` から選ぶ（`users` が空なら member の file の試験から選ぶ） |
| `action` | 採るとき: `delete`（消す）か `merge`（束ねる。helper を tests/common へ移す・表駆動の 1 本にする） |
| `plan` | 採るとき: 消すか束ねるかの具体の案（1 行） |
| `mutation_target` | 採るとき: 整理の後に mutation で確かめる対象の src module。crates/*/src/ の下に実在する、試験でない file（`tests.rs`・`*_tests.rs`・`tests/` の下・cfg(test) の module は不可） |

採るかどうかの基準:
- 見た目が同じでも意味の違うもの（違う fixture を起動する・違う契約を守る）は採らない（汎用規約 8）。
- 本体が短く（数 token）、写しを 1 か所にまとめても build・読みやすさが変わらないものは、理由を書いて採らない。
- 候補は判断の材料で、この段では試験を消さない。

## 5. old_impl（unit `old-impl` が書く）
`candidates/old-impl.json` は `{"schema": "celeris.health-v2-r5.old-impl/1", "candidates": [...]}`。1 件は:

```json
{"id": "old-<slug>", "members": ["<試験 id>"],
 "evidence": {"adr": "agent-docs/adr/<file>.md", "what": "<消した仕組み・旧い API や挙動、1 行>"},
 "keep_tests": ["<代わりに残す試験 id>"], "action": "delete", "plan": "<1 行>",
 "mutation_target": "crates/<crate>/src/<file>.rs",
 "not_replay_reason": "<DB に残る旧い shape の event を読む replay の契約でない理由、1 行>"}
```

- `evidence` は `adr`（実在する ADR の path）か `commit_subject`（HEAD の履歴にある commit の件名そのまま）のどちらか。
- DB に残る旧い shape の event・行を読む試験は replay の契約なので old_impl にしない。
- `keep_tests` と `members` を重ねない。

## 6. 禁止事項
- 不変条件の試験（scan の `invariant=true`）を、どの組・候補の `members` にも入れない。`--check-fragment` と `--check` が落とす。
- 試験（crates/ の下）と `contracts.py` を変えない。map leaf が書くのは `mapping/<name>.json`（unit `old-impl` は `candidates/old-impl.json`）と自分の進捗だけ。
- ほかの fragment の `mapping/*.json` を変えない。
- 走査の結果（id・hash・組の member）を手で直さない。走査が誤っていると思ったら直さずに 7 節で申告する。

## 7. 埋められないとき
- capability も `extra_capabilities` も決められない試験がある、走査の誤り（試験の取りこぼし・誤った組）を見つけた、scope の外の file を変える必要がある、のどれかに当たったら、`mapping/<name>.json` を埋めたところまで commit し、result.json に `plan_issue` を書いて止まる。何が埋まらないか（test id と理由）を `plan_issue` と進捗に書く。
- 回避のために scope の外を変えたり、検査を弱めたりしない。
