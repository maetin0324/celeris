# 継いだ Remote の作業場所は、担当未定でも受け手が居なければ子の作成時に Local へ落とす

- 日付: 2026-10-27
- 状態: 採用・実装済み
- task: 01M5XEE1ZRQ0VJZA7SG94F9F3X
- 関連: [ADR-0062](0062-ssh-master-keepalive-and-cluster-tool-routing.md) B1（remote は `cluster:<id>` を持つノードだけ）・
  D6（継承した Remote の降格）、[ADR-0046](0046-organization-as-agent-profiles.md) D5（matching）、
  [ADR-0069](0069-routing-four-layers.md) D1（委譲・計画が書いた担当は捨てる）

## 背景

ADR-0062 D6 の `downgrade_inherited_remote_if_needed` は、継いだ Remote を Local に落とすのを「担当が決まっていて、その
担当が `cluster:<id>` を持たない」ときだけ行い、担当が未定なら「matching が `cluster:<id>` を持つノードだけを候補に
するので後で矛盾しない」として Remote のまま返していた。ADR-0069 D1 以降、委譲・計画の子は担当を捨てるので担当は
常に未定で、この降格は一度も起きなくなっていた。その結果、BenchFS の Sirius 作業場所を持つ親が web-research の子を
作ると、子は Remote(sirius) を継ぎ、web-research を受けて `cluster:sirius` も持つノードが組織に無いため matching が
`Unroutable`（「担当が見つからない: harness web-research / … / 道具 cluster:sirius」）で blocked にした
（2026-10-26 の 01M5WC8PPW、2026-10-27 の 01M5XEA3EE。どちらも人が PATCH で workspace を local に直して進めた）。

## 決定

- D1. 判定は**子の作成時**に行う。matching では行わない。
  - 「明示された workspace か、継いだものか」は作成時にしか分からない（`Task` に出自の欄は無い）。明示の Remote は
    落とさない（matching が人に聞く）という規則を守るには、作成時に判定するしかない。
  - matching（`task_ops::matching::decide`）は担当を決めるだけで、タスクの作業場所を書き換えない（matching が
    workspace を変えると、作業場所の変更が担当決定の副作用として隠れる）。
- D2. 判定の関数は `task_core::downgrade_inherited_remote(task: &mut Task, was_explicit, org)` の 1 つ
  （旧 `downgrade_inherited_remote_if_needed` は消した）。組み立て終わった子（`genre`・`skills`・`assignee` が
  決まった `Task`）に 1 回呼ぶ。
  - 明示（`was_explicit`）・Local・組織が空なら何もしない。
  - 担当が決まっていれば従来どおり、その担当が `cluster:<id>` を持つか。
  - 担当が未定なら、matching が走る子（`matching_harness`: harness があり組み込みの裏方ハーネスでない）について、
    **matching の候補の条件**（根でない・harness を許す・browser の条件）を満たし `cluster:<id>` を持つノードが
    1 つも無ければ落とす。matching が走らない子は Remote のまま（matching が blocked にしないので矛盾しない）。
  - 落とした先は `Local{path: <task_id>}`。理由（`workspace inherited as local: no node that takes harness <h> has
    cluster:<id>` / `… assignee <id> has no cluster:<id>`）を `Task.routing.workspace_downgrade` に書く。
    `Event::Created` は task 全体を載せるので、理由は events に残る。委譲・計画の経路は従来どおり
    `on_downgrade` で tracing にも 1 回残す。
- D3. 候補の条件は `task_core::matching_candidates`（`matching_harness`・`required_cluster_tool`・`accepts`・
  `any_candidate_with_tool`）に 1 つだけ置き、`task_ops::matching::decide` と D2 の両方が使う。片方だけ直すと、作成時に
  「受け手が居る」と見て Remote に残した子が matching で誰にも割り当たらない、が再発するため。
- D4. 呼び出し箇所（子・タスクの作成経路のすべて）:
  - 委譲の子: `task_core::delegate::materialize_delegated_logging`（明示 = 提案の `workspace` がある）。
  - 計画の子: `task_core::plan::materialize_logging`（明示 = 計画の `workspace` がある）。
  - 木の子（`task_ops::tree`）: `materialize_delegated` は skills を空で組むので、そこでの判定を捨て、子の skills を
    決めた後に親から継いだ作業場所で 1 回だけ判定する（browser の条件が skills を見るため）。
  - `POST /tasks`・`celerisctl add`・CoS の `create_task`（`task_ops::add::build_task`）: `cluster` も `workspace` も
    書かず、選んだリポジトリの置き場から Remote を継いだときだけ判定する（parent の有無を問わない。継いだ Remote は
    同じ問題を起こすため）。落とした Local とリモートのリポジトリの組み合わせは、委譲の子と同じく許す
    （web-research などリポジトリを使わない仕事が対象。「Local + remote repo は 422」の検査は明示の Local にだけ効く）。
- D5. 同じ判定の写しを消す: `task_ops::actions::create_task_action` に手で写してあった「明示の担当が `cluster:<id>` を
  持つか」の検査を `task_ops::matching::assignee_has_cluster_tool` の呼び出しにした。dispatcher の
  `task_may_use_cluster`・`block_task_missing_cluster_tool` は実行時に担当の道具を確かめる別の判定で、作業場所を
  書き換えないので触らない。celerisctl に同じ判定の写しは無い（`worker --cluster` の上書きは別物）。

## 影響

- 本番 config・org の変更は不要。web-research / literature-research を受けて `cluster:<id>` を持つノードが無い構成では、
  remote の案件から作られた調査の子は Local で作られ、そのまま web-research などに割り当たる。
- API: `TaskRouting.workspace_downgrade`（任意の文字列）を足した（`docs/api/v1/*.schema.json`・web の生成型を再生成）。
