---
tasks: [01M7MZTKBPZPQE4XN451F8X84F]
---
# ADR 2026-11-19: アーキ v4 の計測の規則の付記（C4 の補助の指標 conn_open_files、WorkUnit の run の監査の基準、C10 の窓の下限）

- 日付: 2026-11-19
- 状態: Accepted
- 関係: アーキ v4 A の baseline.md §6・§7（A の run の成果物 `/local/celeris/data/workspaces/01M7MZTKAXM8GJGK8BPE13CQRM/artifacts/baseline.md`。
  B〜H の計測の規則の正本）、H の [final-report.md](../progress/2026-11-19-arch-v4-h/final-report.md) の提案 7・12・13、
  [struct-store.md](../progress/2026-11-19-arch-v4-h/struct-store.md) §7・§8、[prompt-audit.md](../progress/2026-11-19-arch-v4-h/prompt-audit.md) の提案 2、
  [ADR 2026-11-18-knowledge-lifecycle-tx-boundary](2026-11-18-knowledge-lifecycle-tx-boundary.md)（F。自前の接続を消した変更）。
- task: 01M7MZTKBPZPQE4XN451F8X84F（アーキ v4 H。replan の R7b、WorkUnit r7-rules）

## 背景

- A の C4 は「`rusqlite` を書く production file」と「SQL 文字列を持つ production file」を `git grep -l` で数える（A §6 の 8）。
  この正規表現は、file の中で接続を開く行と、呼び手の `&Transaction` を受けるだけの行を区別しない。F は `crates/task-api/src/knowledge.rs` の
  自前の接続（`Connection::open`・`TransactionBehavior`・`busy_timeout`）を消したが、file は `rusqlite` を書き続けるので、C4 の数は 74 → 75 で減らず、
  改善が数に出なかった（struct-store.md §7）。
- H の prompt-audit は、過去の WorkUnit の run の prompt を今の `work_units` 行と比べると 7 run が「切れた」ように見えることを見つけた。
  計画の改訂が同じ id の WorkUnit の行の objective・done_when を続きの run 用の文に書き換えるためで、run の開始時に有効だった計画の版
  （`execution_plans` の `units[]`）と比べると欠けは 0 件だった（prompt-audit.md「今の work_units 行と比べた場合」）。
- H の loc-cost は A の C10（修正の波及）を B0 の後の窓に当てたが、production を変えた通常の修正が 1 件しかなく比べられなかった（loc-cost.md §3）。

## 決定

- D1（C4 の補助の指標 `conn_open_files`）: A の C4 と同じ file の範囲（`crates/` の `.rs`。A の試験の除外 `X`・評価基盤の除外 `P`・
  `crates/task-core/src/store/` の除外も同じ）で、`Connection::open`・`TransactionBehavior`・`busy_timeout` のどれかを含む行を 1 行以上持つ
  production file の数を `conn_open_files` として数える。A §6 の 8 と同じく `git grep` の近似で、inline の `#[cfg(test)]` の中も数える。

  ```sh
  X='(^|/)(tests?|benches|examples)/|(^|/)(tests|[^/]*_tests?|test_support)\.rs$|_fixture\.rs$'; P='^crates/(campaign|routing-eval|jev-classifier)/'
  git grep -lE 'Connection::open|TransactionBehavior|busy_timeout' $B -- 'crates/*.rs' | sed "s/^$B://" \
    | grep -vE "$X" | grep -vE "$P" | grep -v '^crates/task-core/src/store/' | wc -l
  ```

- D2（両方の値を並べる）: `conn_open_files` は A の規則と別の数え方なので、A の C4 の 2 つの数を置き換えない。before/after の表では、
  A の C4（`rusqlite` の file・SQL 文字列の file）の行に並べて `conn_open_files` の行を足し、checker の欄で区別する。A との比較の合否は A の C4 の数で決め、
  `conn_open_files` は接続を開く場所が減ったかを示す補助に使う。この ADR の時点の値（2026-11-19 に数えた）:

  | 指標 | B0（`bc665168`） | main（件名 `integrate wu/close (phase close)`） | 差 |
  |---|---|---|---|
  | A の C4 `rusqlite` の production file | 74 | 75 | +1（D の `org_write.rs`） |
  | `conn_open_files`（D1） | 48 | 47 | −1（F の `crates/task-api/src/knowledge.rs`） |

  残る 47 file には `crates/task-api/src/chat/attachments.rs`・`crates/task-api/src/cos/triage_view.rs` など、H の提案 4（F の提案 1）が移す候補が入る。

- D3（WorkUnit の run の監査の基準）: 過去の WorkUnit の run の prompt に objective と受入条件が入ったかを監査するときは、今の `work_units` 行ではなく、
  run の開始時に有効だった計画の版の `units[]` と比べる。版は同じ task の `execution_plans` の行のうち `created_at <= runs.started_at` を満たす最大の `version`、
  比べる値はその版の `units[]`（`audit.py` は `work_units[]` の名の配列も同じに読む）で `key` が一致する unit の `objective` と `done_when`。
  該当する版が無いときだけ今の `work_units` 行と比べ、どちらと比べたか（`plan_v<N>` か `wu_row`）を結果の行に残す。
  WorkUnit の無い worker・planner の run は task の objective と `acceptance[].text`、reviewer の run は review する task の objective と受入条件と比べる。
  本番 DB は `mode=ro` の URI でだけ開く（prompt-audit.md の「再現」の `audit.py` がこの規則の実装）。

- D4（run ごとの spec）: run が受け取った spec を run ごとに残す仕組みは、この ADR では足さない。足すかどうかは人が決める（下の「提案」）。
  それまでは D3 の規則で監査し、計画の改訂の前後で同じ id の unit の文が変わったことは `execution_plans` の版から読む。

- D5（C10 の窓の下限）: A の C10 の snippet（A §7 C10）を A の窓と別の窓に当てて割合を比べるのは、その窓の no-merges の
  「production を変えた commit」が 30 件以上のときだけにする。30 件に満たなければ、その窓の項目は「未達（窓の件数が不足）」と書き、窓が閉じる日時を書く。
  with-merges の数は並べて書くが、merge 1 件が WorkUnit や task の変更を束ねるので、no-merges の代わりにしない。
  B0 の後 2 週間の窓（SINCE 2026-11-17T00:00:00Z、REV `bc665168..main`）は 2026-12-01T00:00:00Z に閉じる
  （2026-11-19 の時点の値は [c10-window.md](../progress/2026-11-19-arch-v4-h/c10-window.md)。no-merges 1 件で未達）。

- D6: A の規則（baseline.md の C1〜C12 と §6）は変えない。この ADR は規則を足すだけで、A の数の定義・snippet・checker は B0 のまま使う。
- ADR 番号は採らない（日付+slug、ADR-0128 D5）。

## 理由

- 接続を開く行は、transaction の境界・busy の扱い・接続の数を file が自分で決めている印で、F の変更が消したのはこの行だった。
  A の正規表現の数を変えると A との比較が崩れるので、別の指標として並べる。
- `work_units` 行は計画の改訂で同じ id のまま上書きされる。run が受け取った文を後から確かめられる記録は、版ごとに残る `execution_plans` の `units[]` だけである。
- C10 の割合は件数が少ないと 1 件で大きく動く（1 件なら 0 % か 100 %）。A の窓の 194 件と比べる意味があるのは数十件からで、下限を 30 件に置いた。

## 提案（人が決める。起票しない）

- run ごとの spec を残すか: 例えば `runs/<run id>/request.json` の work_unit の写しを監査の正本にする、または `runs` の行に開始時の計画の版を記録する。
  残せば D3 の版の推定が要らなくなり、開始直後に lease を失って prompt.txt の無い run（H の prompt-audit の 4 run）についても、何が渡るはずだったかを確かめられる。
  残さなければ D3 の規則で監査を続ける。起票するかは人が決める（H は起票しない。final-report.md の提案 7）。
- D5 の窓が 2026-12-01T00:00:00Z に閉じても no-merges が 30 件に満たないとき、窓を延ばすか、with-merges を通常の修正の単位として数え直す規則を足すかを人が決める。

## 撤去対象

無し（計測の規則を足すだけで、置き換える旧い仕組み・code・config は無い）。

## 試験・検査

code を変えないので試験は足さない。D1・D5 の数は上の command と c10-window.md §5 の command で、D3 は prompt-audit.md の `audit.py` で再現できる。
