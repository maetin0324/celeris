# ADR-0099: 担当の無い root task の delivery と見送り通知

---
tasks: [01M3VT5BJZX8JSME7ZQJGTVGEN]
---

- 日付: 2026-10-01
- 状態: Accepted（ADR-0051 の付記。実装は後続 WorkUnit）
- 関連: [ADR-0051](0051-supervised-delivery.md)、[ADR-0079](0079-recursive-task-decomposition.md) D6

## 背景

2026-10-01、browser capability の root task `01M3PAX6RVE7AX8Z6118KADME3` は最終 review に合格したが、`assignee` が無かった。`task_ops::delivery::begin` は `task.assignee` から `department_of` で部を引けなければ `Ok(None)` を返すため、delivery は作られず、成果ブランチの 139 commit が main に入らなかった。人が別 task で取り込み直した。対象案件の root が完了しても取り込みが見送られたことを人が知れないのが問題である。

現在 `begin` は `review_spawn` が独立 Reviewer run を起こす直前に呼ぶ。delivery ができた時だけ、同じ review に部署内のマージ可否条件が加わる。`celeris::delivery` は保存された delivery を進める。したがって見送りの判定と通知は `begin` の入口で行い、既存の merge/release/verify の判定を迂回しない。

## D1. delivery の部署を決める順序

対象案件の root task に限り、次の順に `org_list` と `task_core::department_of` で**部の ID**へ正規化する。存在しない組織ノードや秘書など、部へ辿れない候補は飛ばす。課の ID を delivery の部署として保存しない。

1. root の `task.assignee`。
2. **現在有効な計画を作った担当**。`execution_plans.planner_run_id` があれば、その run に対応する `RoutingDecided.record.org_node` を使う。無ければ、計画を作った plan task を特定できる場合はその `Task.assignee` を使う。run ID や plan task との対応を確証できない場合、同じ task の別 run や単なる最新担当を推測して使わない。`origin = human/repair/fixture` の計画は planner の根拠としない。
3. 木の子 task と WU run の実担当から得られる部。子は `tree.parent_unit` の祖先を辿ってこの root に属するものだけを集め、各子の `Task.assignee` を候補にする。WU はこの root の `work_units` に結び付く worker run だけを集め、run ID に対応する `RoutingDecided.record.org_node` を候補にする。`WorkUnitSpec` と `RunRow` 自体には `assignee` 欄がないため、計画が書いた担当や run を起こした時点の root の現在値を代用しない。対応する routing 記録が無い run は数えない。各子を 1 票、各 WU run を 1 票として部ごとに数え、最多票を選ぶ。同票なら部 ID の辞書順で小さい方を選ぶ。古い計画の WU run もこの root に紐付く実行の証拠として数える。
4. 案件の既定部署。celeris の `[selfdeploy] delivery_default_departments` は案件 ID をキー、組織の部 ID を値にする map とし、`DeliveryPolicy` に読み込む。設定された ID が現在の組織で `department_of` により部へ解決できる場合だけ使う。省略時は空 map で、暗黙の `engineering` 等は置かない。

どの段でも部が得られなければ **department unresolved** とする。部署の選択はレビュー担当と delivery 記録にだけ使い、root や子の `assignee` を書き換えない。

## D2. 見送りの境界と理由

`begin` が delivery を作らない条件のうち、対象案件の root について次の理由は報告する。理由は安定した機械可読コードにし、`detail` に対象の repo ID、path、branch、ref など判定に必要な情報を入れる。安全上の理由で `head` がまだ解決できない場合は `null` とする。

| 理由コード | 条件 |
|---|---|
| `multiple_repos` | task の repo がちょうど 1 件ではない、または marker の repo がちょうど 1 件ではない |
| `no_marker` | task worktree の marker が読めない |
| `marker_repo_mismatch` | marker の repo が task の repo と対応しない、または登録元 repo と一致しない |
| `repo_row_missing` | task の repo ID に対応する案件の repo 行が無い |
| `repo_not_local` | 登録 repo が local ではない |
| `repo_path_mismatch` | 登録 path が存在しない、`policy.repo` と異なる、または marker の source と異なる |
| `not_git` | marker の kind が `git` でない、または対象 path が Git repo でない |
| `no_branch` | marker に branch が無い |
| `branch_name_mismatch` | branch がこの task ID で終わらない |
| `refs_unresolvable` | 既定ブランチまたは対象ブランチの ref を SHA に解決できない |
| `department_unresolved` | D1 の全候補で部が決まらない |

複数条件が同時にある場合、上表の順に最初の理由を記録し、次回の再判定で別の理由が明らかになればその理由を記録する。部署が先に解決できなくても repo 側の具体的な不備を隠さない。`marker_repo_mismatch` は repo 行・path と照合できる時点で判定する。

次は従来どおり**通知せずに対象外**とする: 案件が無い、案件 ID が `[selfdeploy] delivery_projects` にない、support task、`tree.parent_unit` を持つ木の子、同じ head の delivery が `Merging` / `Preparing` / `Ready` にある場合。`Merging` / `Preparing` 中は既存の delivery を重複作成しない（保存済み head を使用する）。

## D3. task event と受信箱

見送りには新しい task event `DeliverySkipped { reason, detail, head: Option<String> }` を当該 root に積む。これは task の状態を変えない監査イベントである。再起動、review の再試行、設定 reload で同じ事象を繰り返さないよう、保存済み event を調べて **(task ID, reason, head)** ごとに高々 1 件とする。`head = null` は同じ理由の未解決 head として 1 件にまとめる。新しい head なら再通知できる。

`task_ops::inbox::build_attention` は event から `AttentionItem::DeliverySkipped` を派生し、task、理由、詳細、head、時刻を表示用データに入れる。完了済み root も走査対象にし、失敗 task 専用の 24 時間制限は適用しない。該当 head の delivery が後で作られ、取り込み済みまたは進行中なら古い見送り item を消す。人には「完了したが main への取り込みを開始できなかった」と分かる文面と、event の具体的な理由を示す。event が正本であり、受信箱のための別の永続状態は設けない。

## D4. 維持する動作

変更するのは、対象案件の root の部署解決と、取り込みを開始できない理由の可視化だけである。木の子は親のブランチへ統合し、単独の delivery を持たない。対象外の案件と support task は従来どおり何もしない。既存の `task.assignee` が部に解決できる場合の優先順位、独立 Reviewer run の合否、固定 SHA、fast-forward merge、release/verify、最終デプロイの人による操作は ADR-0051 のまま変えない。
