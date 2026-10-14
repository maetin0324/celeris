# ADR 2026-10-14: クラスタ画面のローカルマシン行と、store に置くローカル設定（workspace_root・max_concurrency）

- 日付: 2026-10-14
- 状態: 採用（実装は backend・web の葉）
- 関連: ADR-0059 D6（`cluster_settings`・`PUT /clusters/{id}/settings`）、ADR-0042 D3（`workspace_root` の既定）、
  ADR-0041 D1 / ADR-0043 D2（`<workspace_root>/<task_id>` の足回り）、ADR-0040 D3（CLI の `--workspace-root` 上書き）、
  ADR-0095 / ADR-0126（db_guard の本番 DB・releases 保護）、ADR-0075 / ADR-0136（scratch）、
  ADR 2026-10-07-build-tmp-hygiene（`[maintenance]`）、migration 0051（`org_browser_events` = task を持たない監査の流れ）、
  ADR 2026-10-11-breaking-changes-by-default

## 1. 文脈

人の依頼: 「クラスタ画面で celeris 自身が動いているローカルマシンの設定（ワークディレクトリなど）も設定できるように」。

現状:

- `web/features/ops/clusters-screen.tsx` は `GET /api/v1/clusters` の `[[clusters]]`（リモート）だけを並べ、
  `PUT /api/v1/clusters/{id}/settings` で `work_dir` を DB（`cluster_settings`、migration 0025）に上書きできる。
  優先順位は「DB の行があれば DB、無ければ設定ファイル」で、応答の `work_dir_source` が `settings` / `config` を返す。
- ローカルの `workspace_root`（`crates/celeris/src/config/mod.rs`、既定 `~/.local/celeris/workspaces`）は設定ファイルと
  CLI の `--workspace-root`（verify.sh の staging 用）でしか決まらず、reload の対象外（`daemon/admin.rs` の
  `reload_providers` の注記）。画面には出ない。
- **`workspace_root` は task ごとに記録されていない。** task の足回りは至る所（`task_ops::workspace::local_dir`、
  `Dispatcher::task_dir` / `task_workspaces_for`、knowledge_curation、doc_gardener、delivery、task-api の
  artifacts・docs・changes など 130 箇所余り）で `config.workspace_root.join(task_id)` と都度計算される。
  そのまま全体の値を変えると、既存 task と走行中 run の作業場所を見失う（artifacts・worktree.json・runs/ が
  旧い root に残り、新しい root の空 dir を見に行く）。

## 2. 決定

### D1. 画面から変える項目の範囲

| 項目 | 採否 | 理由 |
|---|---|---|
| `workspace_root` | **採る（必須）** | 依頼そのもの（ワークディレクトリ）。リモートの `work_dir` と同じ意味の値で、ディスクの空き（`/local` への移動など）のため人が変えたい。D5 の task ごとの固定で既存 task を壊さずに変えられる。 |
| `max_concurrency` | **採る** | リモート行の `concurrency` に対応するローカルの同時実行数で、画面で並べて比べる値。dispatcher が tick ごとに `self.config.max_concurrency` を読むだけなので、次の tick から安全に効く（走行中の run は止めない。下げても超過分は自然に終わるのを待つ）。 |
| `[scratch]`（`CARGO_TARGET_DIR` の pool の root・上限） | 採らない（表示のみ） | ADR-0075 / ADR-0136 で `/local/celeris/data/scratch` に固定した host 運用の値。root を変えると走行中 run の target と seed・semantic GC の対象が分かれ、host の容量計画（thin pool）と結び付くため設定ファイルで人が変える。 |
| `[maintenance]`（`target_sweep` の roots・上限、`disk_watch`） | 採らない（表示のみ） | 掃除の対象 path を画面から増やせると、誤った path を消す危険が大きい（削除系）。build-tmp-hygiene の監視閾値も host 管理の値。 |
| `workspace` の worktree 設定・`tick_ms`・`lease_grace_secs` 等 | 採らない（表示しない） | daemon の内部の調整値で、ローカルマシンの「設定」として人が触る対象ではない。 |
| `db`・`[api]`・`[selfdeploy]`・`[storage]` | 採らない（表示しない） | 本番 DB・待ち受け・release の置き場。画面から変えると daemon 自身を壊す。設定ファイルと再起動が正。 |

画面の行に出す読み取り専用の値は `scratch_root`（`[scratch]` の root、未設定なら `null`）と、
`workspace_root`・`max_concurrency` の設定ファイル値（上書きを消したときに戻る値）。それ以外は出さない。

### D2. store の表（migration 0079）

`crates/task-core/migrations/0079_local_settings.sql`（main の 0078 は eval_trials.instance_id。main 取り込み時に 0079 へ振り直した）:

```sql
-- ローカルマシンの上書き（1 行だけ。id = 1）。NULL の欄は設定ファイルの値を使う。
CREATE TABLE IF NOT EXISTS local_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    workspace_root TEXT,          -- 正規化済みの絶対 path（`~` は PUT 時に展開して保存）
    max_concurrency INTEGER,      -- 1..=256
    updated_at TEXT NOT NULL
);
-- task ごとの足回りの root の固定（D5）。INSERT だけ、更新しない。
CREATE TABLE IF NOT EXISTS task_workspace_roots (
    task_id TEXT PRIMARY KEY,
    root TEXT NOT NULL,
    pinned_at TEXT NOT NULL
);
-- 変更の監査（追記専用。org_browser_events と同じく task を持たない流れ）。
CREATE TABLE IF NOT EXISTS local_settings_events (
    id INTEGER PRIMARY KEY,
    ts TEXT NOT NULL,
    actor TEXT NOT NULL,          -- "admin" | "cos:<operation id>"
    before_json TEXT NOT NULL,
    after_json TEXT NOT NULL
);
```

- `local_settings` は欄ごとの上書きを持つ（`cluster_settings` と同じく「上書きだけ」を持ち、解決は celeris 側）。
  全欄が `NULL` になったら行を消す（`cluster_settings` の「上書きを消すときは行ごと削除」と同じ流儀）。
- 監査: 書き込みと同じ transaction で `local_settings_events` に before/after を 1 行 INSERT する。この表と
  `task_workspace_roots` は UPDATE・DELETE をコードに持たない（events は追記専用の方針。task の `events` 表は
  `task_id NOT NULL` なので使わず、0051 の前例に倣って別の流れにする）。CoS 経由（`cluster.settings_put` と同じ
  操作の流れ）は既存の `OperationAudit::apply_checked` の中で同じ store 関数を呼び、`cos_operations` にも残る。

### D3. 設定ファイル値との優先順位

欄ごとに、上から最初に値があるものを使う:

1. **CLI の `--workspace-root`**（ADR-0040 D3。verify.sh の staging が本番 config を読みつつ作業場所を差し替える）。
   この上書きがあるときは store の `workspace_root` と `task_workspace_roots` の固定を**どちらも無視**し、全 task を
   CLI の root で解決する。staging の DB は本番の写しなので、固定を信じると staging が本番の作業場所に書くため。
   `max_concurrency` には CLI の上書きが無い。
2. **store（`local_settings` の欄が非 NULL）**。
3. **設定ファイル**（`workspace_root` / `max_concurrency`、無ければ既定値）。

応答の各欄に `source`（`"cli" | "settings" | "config"`）を返す（`work_dir_source` と同じ語彙に `cli` を足す）。
`reload` で設定ファイルの `max_concurrency` を読み直しても、store に値があれば store が勝つ。

### D4. API — `GET /clusters` にローカル行を含め、`PUT /clusters/local/settings` で変える

既存の流儀（一覧は `GET /clusters`、変更は `PUT /clusters/{id}/settings`）に合わせる:

- `GET /api/v1/clusters` の応答に **`local`（`LocalMachineView`、常に 1 つ）** を足す。`items`（リモート）の
  `ClusterView` には混ぜない（`host`・`sync`・`auth`・接続状態などリモート専用の欄を持たないため、型を分ける）。
  画面は `local` を先頭の行として `items` と同じ並びで描く。
  ```json
  "local": {
    "id": "local",
    "hostname": "<gethostname>",
    "in_use": 3,
    "workspace_root": {"value": "/local/celeris/data/workspaces", "source": "settings",
                       "config_value": "/home/u/.local/celeris/workspaces", "apply": "new_tasks"},
    "max_concurrency": {"value": 6, "source": "settings", "config_value": 4, "apply": "next_tick"},
    "scratch_root": "/local/celeris/data/scratch",
    "updated_at": "2026-10-14T…Z"
  }
  ```
- `PUT /api/v1/clusters/local/settings`（管理系。`token_file` 未設定でも 401。`require_admin`）:
  body `{"workspace_root": "<abs or ~/…>" | null, "max_concurrency": <1..=256> | null}`。PUT なので全置換で、
  `null` または省略の欄は上書きを消す（設定ファイルの値に戻る）。応答は上の `LocalMachineView`（変更後）。
  CoS の操作は `cluster.settings_put` と同じ経路に `local.settings_put` を足す。
- `local` は予約 id。`[[clusters]] id = "local"` は**設定の読み込みエラー**にする（D4 の route と衝突するため。
  axum の route は静的な `/clusters/local/settings` が `/clusters/{id}/settings` より優先される）。
- **`apply` 欄**（応答と画面に出す反映の時期）:
  - `workspace_root` → `"new_tasks"`: 即時に store に入り、**この後に足回りを初めて用意する task から**効く（D5）。
    daemon の再起動は要らない。
  - `max_concurrency` → `"next_tick"`: API が admin チャネルで dispatcher に再読込を知らせ、次の tick から効く。
- `docs/api/v1/api.md`・schema・gui/web の生成型を同じ変更で更新する。

### D5. 既存 workspace・走行中 run への影響と path 検証

**task ごとの root の固定**（store を変えても既存 task を動かさない仕組み）:

- dispatcher が task の足回り（`<root>/<task_id>`）を**初めて用意するとき**に、その時点の実効の root を
  `task_workspace_roots` に INSERT する（`INSERT OR IGNORE`、1 回だけ）。root task は D3 の実効値、子 task
  （`parent_id` あり）は**親の固定された root**を継ぐ（WU の worktree・親ブランチの写しが同じ root に揃う）。
- 足回りの path の解決は、全て `config.workspace_root.join(task_id)` をやめて **resolver**
  （`WorkspaceRoots::task_root(task_id)` = 固定があればそれ、無ければ設定ファイル／CLI の root）を通す。
  固定の無い既存 task（この ADR 以前に作られた task）は設定ファイルの root で解決され、従来どおり。
  `Local { path }` の相対 path の基準も同じ resolver の値にする。旧い `config.workspace_root` 直参照は残さない
  （破壊的変更の方針）。
- 結果: 既存 task の作業場所は**動かさない・写さない**。走行中 run は自分の task の固定 root を使い続けるので
  壊れない。新しい root に移るのは、設定変更後に初めて足回りを用意する task だけ。旧い root の掃除
  （workspace_prune・target_sweep）は固定に従って task ごとの root を見る。
- 退けた案: (a) 設定変更を daemon 再起動後にだけ効かせる — 再起動後に既存 task が新しい root を見て壊れる点は同じ。
  (b) 旧い root に `<task_id>` の symlink を置いて新しい root を指す — 130 箇所を触らずに済むが、prune の
  `remove_dir_all`・sandbox の bind・canonicalize で symlink の扱いが分かれ、消し漏れと誤削除を招く。

**path 検証**（PUT の時点で決定的に。失敗は 422 `validation`、`field: "workspace_root"`）:

1. `~` / `~/…` は daemon の `$HOME` で展開し、展開後が絶対 path であること。`..` を含まないこと（正規化後に比較）。
2. 書き込めること: `create_dir_all` してから一時 file を作って消せること（作れなければ 422）。
3. 本番の大事な場所を指さないこと: 次のどれかと**同じか、その中か、それを含む**なら拒否 —
   `db` の file とその dir、`[selfdeploy] releases_dir`、`~/.local/celeris/releases`、`/local/celeris/state/releases`、
   `~/.config/celeris`、設定ファイルのある dir。`/` と `$HOME` そのものも拒否。
   照合は canonicalize 後の path で行う（symlink 経由のすり抜けを防ぐ）。db_guard（ADR-0095 / 0126）の保護 path
   一覧と同じ定義を共有する。
4. `max_concurrency` は 1..=256 の整数。

本番の config ファイルは書き換えない（API は store にだけ書く）。

## 3. 結果

- 画面からローカルの作業場所と同時実行数を変えられ、変更は監査の流れに残る。設定ファイルは種と既定に戻る。
- `workspace_root` の参照は resolver 1 本に寄る（backend の葉で 130 箇所余りを置き換える。大きいが機械的）。
- staging（verify.sh）は CLI の上書きで本番の固定を無視するので、本番の作業場所に書かない。
