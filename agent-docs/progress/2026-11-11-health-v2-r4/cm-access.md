---
title: "健全性v2 R4 cm-access: Dispatcher の field の読み書き file と共変更の実測"
tasks: [01M74VAMWFCC4JGZY4ENA92HJJ]
status: completed
updated: 2026-11-11
---
# 健全性v2 R4 cm-access: Dispatcher の field の読み書き file と共変更の実測

完了日: 2026-11-11

コードは変えていない。`change-map-measure.py` を作り、`change-map.json` を生成した。owner と equivalence は後の unit（cm-owners・cm-record）が足す。

## 変更
- `phase change-map: measure Dispatcher field access and co-change` — 次の 3 file を足した。
  - `agent-docs/progress/2026-11-11-health-v2-r4/change-map-measure.py`（python3 の標準 library だけ。build・試験は走らせない）
  - `agent-docs/progress/2026-11-11-health-v2-r4/change-map.json`（上の script の出力）
  - この進捗ファイル

呼び方（repo の root で）:

```sh
python3 agent-docs/progress/2026-11-11-health-v2-r4/change-map-measure.py --out agent-docs/progress/2026-11-11-health-v2-r4/change-map.json
```

## 数えた値（提出時の HEAD で実行）
- field: 116 件。`pub struct Dispatcher` の 4 字下げの `name:` 行で、slices.json の `dispatcher_fields` の awk と同じ名前の集合。そのうち `#[cfg(test)]` の field は 10 件。
- 読んだ file: 209 件（dispatcher.rs と dispatcher/ の下の *.rs）。そのうち試験の file は 139 件（dispatcher/tests/ の下と、試験の範囲で宣言された `mod` の先。cos_chat/harness_tests.rs・sink_tests.rs・workspace_files_tests.rs・tests.rs・launch/tests.rs・decision_log_wire/tests.rs を含む）。
- 共変更: 2026-09-15 00:00 UTC 以降の merge でない commit のうち、dispatcher.rs か dispatcher/ の下を変えた commit は 724 件。範囲の path は 227 件（消えた path を含む）。最上位の pairs は commit 数 3 以上の組 495 件。
  - pathspec 無しの `git log --no-merges --since=… --name-only` は 5100 件で、範囲で絞ると同じ 724 件になる。script は pathspec と `--full-history` で git に絞らせる（約 20 秒。pathspec 無しは約 60〜90 秒）。
- 書き手の file の数ごとの field 数: 0 件が 21（`#[cfg(test)]` の 10 件と、初期化の後に書かない `store`・`tx`・`account_books` など 11 件）、1 件が 77、2 件以上が 18。
- 試験の code から参照される field（test_access が空でない field）: 50 件。

## 出力の形
- `fields[]`: `name`・`decl`（`dispatcher.rs:行`）・`cfg_test`・`readers`・`writers`・`test_access`・`evidence`（代表の `path:行`、最大 6 件）・`co_change`（readers ∪ writers の file 同士の組の上位 5 件）。name の順。
- `co_change`: `since`・`commits`・`files`（path ごとの commit 数）・`pairs`（`{a, b, commits}`、commit 数の多い順）。
- `method`: field の数え方、読んだ範囲、参照と書きの判定規則、変更系 method の一覧（std と、型名ごとの自前の `&mut self` method）、共変更の規則、限界。
- 同じ HEAD なら同じ bytes を書く（2 回走らせて sha256 が一致した）。key は sort し、HEAD の hash は出力に入れない。

## 判定の規則（要点）
- 受け手は `impl Dispatcher {`（`impl super::Dispatcher`・`impl <trait> for Dispatcher` を含む）の中の `self`、型が `Dispatcher`・`&Dispatcher`・`&mut Dispatcher` の引数（cos_chat の `dispatcher: &mut Dispatcher` など）、関数名に dispatcher を含む呼び出しの結果を `let` で束ねた名前（試験の `let mut d = dispatcher(…)` など）。
- 書きは `=`・複合代入・`&mut <受け手>.f`・変更系 method（insert・remove・push・clear・entry・take・replace・get_mut・retain・extend・drain など。field の型に名前が現れる自前の型の `&mut self` method を含む）。
- 試験の code（dispatcher/tests/ の下、`#[cfg(test)]` の付いた item・文・field、試験の範囲で宣言された `mod` の先の file）の参照は readers・writers に入れず test_access に入れる。

## 限界
- 型を解かない字句の判定。macro の展開先・closure の引数・tuple の 2 つ目以降の束縛・struct に入れた Dispatcher（`self.dispatcher.f`）・`let this = &mut *self` などの別名経由の参照は拾えない。`let Dispatcher { .. } = self` の分解と `Dispatcher::new` の struct literal での初期化は参照に数えない。
- 範囲外（task_worker・crate::accounts など）で定義された型の `&mut self` method と、sub field（`self.f.g.m()`）の型の method は読みに数える。std の変更系 method の名前は型を見ずに当てるので、trait object の同名 method を書きと誤ることがある。
- `Mutex`・`RwLock` の中身の変更は `lock()` の後を追わず、読みに数える（atomic の `store`・`fetch_*`・`swap` は書き）。
- `#[path]` で dispatcher/ の外から組み込まれる 9 file（`health_review/check_item.rs`・`design_gate.rs`・`inventory.rs`・`policy_review.rs`・`review_limits.rs`・`plan_adoption.rs`・`worker_profile.rs`・`flow_tests.rs`・`scenario_tests.rs`）は読まない（objective の範囲）。そこでの field の参照は数に入らない。一覧は `method.scope.unread_mounted_modules`。
- 共変更は file 単位。1 commit の中で field に関係の無い変更をした file も組に数える。`--since` は committer date なので、rebase した commit は rebase の日時で数える。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `python3 agent-docs/progress/2026-11-11-health-v2-r4/change-map-measure.py --out agent-docs/progress/2026-11-11-health-v2-r4/change-map.json` → exit 0。fields 116 件、co_change.commits 724、最上位の key は co_change・fields・method。
- 同じ command で `$TMPDIR` に 2 回目を書き、sha256 を比べた → 一致。
- awk で取った field 名と JSON の name の集合を比べた → 116 件で一致。readers・writers に試験の file の path は 0 件。
- 検査コマンドの結果は result.json の evidence に書く（このファイルの commit の後に実行するため）。

## 未解決
- 無し。

## 提案
- cm-owners は `readers`・`writers` と `co_change` に加えて `test_access` を見て、試験の継ぎ目（`test-hooks`）の field を分けるとよい。`#[cfg(test)]` の 10 件は prod の参照が 0 件なので、そのまま `test-hooks` の候補になる。
- 範囲外の `health_review/` の 9 file にも Dispatcher の field の参照がある見込みだが未確認。owner の判定でこれらの module が効くなら、読む範囲を広げる別 unit が要る。
