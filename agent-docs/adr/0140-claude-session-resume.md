# ADR-0140: Claude Code の continuation は同一 session を resume し、条件が崩れたら checkpoint で新 session に倒す

---
tasks: [01M3Y2KXVXJ6CFH2XRS98W452J]
---

- 日付: 2026-10-02
- 状態: 採用（設計のみ。実装は後続の WorkUnit: session-key / reread-count / resume-wire / metrics-api）
- 関連: [ADR-0054 D1](0054-stateful-sessions-and-streaming-chat.md)（`node_sessions`・`--session-id`/`--resume`・`looks_like_resume_rejection`・UUID 必須）、
  [ADR-0072 D8/D9/D11/D19](0072-task-execution-decomposition.md)（checkpoint・continuation・実行 metrics）、
  [ADR-0074](0074-parallel-work-units-checkpoints-milestones-quota.md)（WU 並列と checkpoint）、
  [ADR-0043](0043-workspaces.md)（workspace・container 実行）、[ADR-0018](0018-remote-clusters-over-ssh.md)/[ADR-0019](0019-worktree-sync-for-large-repositories.md)（remote worktree）、
  [ADR-0079](0079-recursive-task-decomposition.md)（再帰統合）、[ADR-0118](0118-review-target-sync-and-merge-candidate.md)/[ADR-0120](0120-pre-review-sync-integration-repair.md)（review 前同期と IntegrationRepair）、
  [ADR-0121](0121-root-delivery-without-assignee.md)（root delivery）
- 番号: 着手時（2026-10-02、main `95595105`）に全 `refs/heads` と進行中 worktree の `docs/adr` を走査し最大は 0122。
  並行する Phase 4（routing）が最小の空き 0123 を取る見込みが高いため、衝突を避けて 0124 を使う（0116 は browser launcher 用に避ける）。
  2026-10-03: main の 0124-atomic-direct-route と番号が重なったため、全 ref の `docs/adr` を走査し最小の空き 0140 へ振り直した（旧番号 0124）。

## 背景

WU の worker run が `RunEnd::BudgetExhausted`（turns / wall_clock / context）や `RunEnd::Yielded` で終わると、
ADR-0072 D9/D11 の continuation が**新しい run を新しい session で**起こし、前置きに checkpoint（`checkpoint.json`）を
入れて続きをやらせている。新 session は直前の run が読んだファイル・grep 結果・試行錯誤を持たないので、
同じファイルを読み直し、同じ探索をやり直す。run 数・wall time・入力 token の多くがこの再探索に使われている。

ADR-0054 D1 は CoS の対話と部門長のレビュー run について、`node_sessions` と claude-code の
`--session-id <uuid>` / `--resume <uuid>` による継続を既に実装している（resume 拒否の検出、アカウント・アダプタ
変更での作り直し、rollover も含む）。本 ADR はこの経路を **同じ WU の continuation** にだけ広げる。

## 決定

### D1. 判断表（continuation の run をどの session で起こすか）

判断は dispatcher の純粋関数（ADR-0054 の `sessions::decide` と同じ層。LLM を使わない）で、次の表を**上から順に**見る。

| # | 条件 | 判断 | fallback 理由（記録値） |
|---|------|------|------------------------|
| 1 | run の role が `planner` または `reviewer`（final review・部門長 review・合成 review task を含む） | **常に fresh**（本 ADR の継続 session を引かない。部門長 review の `Lead` session は ADR-0054 のまま） | `role_fresh` |
| 2 | 新しい WU の最初の run（continuation ではない。独立 WU・repair WU・retry 後の新 WU を含む） | **fresh**（他 WU の session を引き継がない） | `independent_wu` |
| 3 | 直前の run の終わり方が continuable でない（`Completed` で review fail → 再作業、`Failed`、`Crashed`、`Waiting` 明け以外の再試行） | **fresh** + checkpoint 前置き（従来どおり） | `not_continuation` |
| 4 | 人・planner・設定が明示的に fresh context を要求（`fresh_context = true`、`[sessions] continuation_resume = false`） | **fresh** + checkpoint | `fresh_requested` |
| 5 | adapter が `claude-code` 以外（codex / acp / aider 等。continuation の resume は claude-code だけを対象にする） | **fresh** + checkpoint | `adapter_unsupported` |
| 6 | 保存された session の adapter が今回の adapter と違う | **fresh** + checkpoint、旧 session を retire | `adapter_changed` |
| 7 | 保存された session の account または provider が今回選ばれたものと違う（プール枯渇で別 account に倒れた等） | **fresh** + checkpoint、旧 session を retire | `account_changed` |
| 8 | 実行面が resume を保証できない（D3: container 実行、cwd が前回と違う） | **fresh** + checkpoint | `surface_unsupported` |
| 9 | 保存された session が無い・壊れている（daemon crash/restart 後に行が無い、session id が UUID でない、session ファイルが無い） | **fresh** + checkpoint | `session_missing` |
| 10 | `approx_tokens` が `[sessions] rollover_tokens` 以上、または直前の終わりが `BudgetExhausted{kind: context}` | **fresh** + checkpoint、旧 session を retire | `context_rollover` |
| 11 | 上のどれでもない: 同一 Task・同一 WU・adapter `claude-code`・同一 account/provider で、直前の run が `BudgetExhausted{turns|wall_clock}` / `Yielded` / recoverable continuation（[ADR-0090](0090-durable-wait-for-cluster-jobs.md) の `Waiting` 明けを含む） | **resume**（ADR-0054 の `--resume <session_id>`） | —（`resumed`） |

- resume した run が「session が無い・拒否された」で失敗したとき（ADR-0054 の `looks_like_resume_rejection`、
  `EventSink::session_resume_failed`）は、その session を retire し、**同じ continuation を checkpoint 前置きの fresh で
  1 回だけやり直す**（理由 `resume_rejected`）。このやり直しは continuation の回数・attempts に数えない
  （供給側の失敗であり仕事の失敗ではない。[review 停止経路で attempts を消費しない](0118-review-target-sync-and-merge-candidate.md)のと同じ考え）。
- resume 時の前置きは**差分だけ**（ADR-0054 D1 の差分前置きと同じ考え）: 「前回の run は予算切れ/yield で止まった。
  checkpoint の `next_action` から続けよ」と、直前 run 以降に変わったこと（人の回答・決定、target の同期）だけを載せる。
  checkpoint は resume 時も前置きに**要約として**載せる（session 内の記憶と食い違ったら checkpoint を正とする）。
- `context_rollover`（#10）は「context が逼迫したから止まった」run を同じ session で続けても同じ所で止まるため、
  resume しない。turns / wall_clock の予算切れは context に余裕があるので resume の対象になる。

### D2. session key と task-core での保存

- **key** = `(task_id, work_unit_id, adapter, account_id)`。provider は account に従属する（[ADR-0012](0012-multi-account-and-worker-run.md) の account は
  provider を 1 つ持つ）ので key には入れないが、行に `provider` を記録し判断表 #7 で比べる。
  atomic な Task（WU を持たない）は `work_unit_id = NULL` を「Task 全体で 1 本」として扱う。
- **保存先は `node_sessions` の拡張**（新しい表は作らない）:
  - `SessionKind::Continuation`（`kind = 'continuation'`）を足す。`node_id` は run の担当ノード（課）。
  - migration **0038 以上**（`crates/task-core/migrations` には現在 `0037` が 2 本ある）で
    `node_sessions` に `task_id TEXT NULL`, `work_unit_id TEXT NULL`, `provider TEXT NULL`, `cwd TEXT NULL` を足し、
    `(kind, task_id, work_unit_id) WHERE retired_at IS NULL` の索引を張る。既存の `conversation` / `lead` 行は NULL のまま。
  - 不変条件「key ごとに現役（`retired_at IS NULL`）は高々 1 行」はストアで強制せず、dispatcher が
    retire → create の順で保つ（ADR-0054 Phase 67 と同じ。ストアに判断を入れない）。
  - 既存の `runs.session_id`（migration 0026）には、各 run が使った session id を書く（resume か fresh かは
    run の metrics に記録。D4）。
  - Task が終端（done / cancelled / failed）になった、または WU が done / superseded / cancelled になったら、
    その key の現役行を retire する（session の寿命 = WU の寿命）。
- claude-code の初回は celeris が UUID を発行して `--session-id` に渡す（ADR-0054 Phase 67b の UUID 必須をそのまま守る）。

### D3. session の保存場所・account isolation・実行面ごとの可否

- **保存場所**: Claude Code は session を `$CLAUDE_CONFIG_DIR/projects/<cwd を変換した名前>/<session_id>.jsonl`
  （既定 `~/.claude/projects/...`）に書く。中身は会話全文と tool 出力で、worktree のファイル内容や
  コマンド出力を含む。celeris はこの jsonl を**読まない・複製しない・成果物に載せない**（`--resume` に id を渡すだけ）。
  jsonl は account の config dir の中にあり、ほかの account・ほかの OS ユーザーからは見えない（権限は CLI の既定のまま）。
- **account isolation**: resume は「保存された行の `account_id` と今回の account が一致する」ときだけ行う（判断表 #7）。
  別 account の `CLAUDE_CONFIG_DIR` に対して `--resume` を渡すことはしない（その id は別 account の dir には存在せず、
  仮に存在しても他 account の会話を読むことになる）。account が変わったら必ず checkpoint で fresh に倒す。
  session id を API・GUI に出すのは run 詳細の既存欄（`runs.session_id`）だけで、jsonl の path は出さない。
- **cwd**: session は cwd に紐づくので、resume は WU の worktree path（`node_sessions.cwd`）が前回と同じときだけ行う。
  WU の worktree は WU の寿命の間固定なので通常は一致する。違えば `surface_unsupported`。
- **container 実行（ADR-0043 D3）**: `CLAUDE_CONFIG_DIR` は**読み取り専用**でマウントされる（`container.rs` の
  `CREDENTIAL_ENV_DIRS`）ので、container 内の claude は session jsonl を config dir に永続化できず、container 内の
  cwd もホストと異なりうる。よって **container 実行の run は resume しない**（`surface_unsupported`、checkpoint fallback）。
  config dir を書き込み可能でマウントするのは認証情報の保護（ADR-0043）を弱めるので採らない。
- **remote worktree 実行（ADR-0018/0019、`ssh.rs`）**: LLM（claude）は**手元で動き**、クラスタにはコマンド実行と同期だけを
  出す。session jsonl は手元の config dir に、cwd は手元の WU worktree になるので **resume できる**。
  remote 側の worktree・同期の意味は変えない。ただし手元の cwd が run ごとに変わる構成（sync mode の都合で一時 dir を
  使う等）では `surface_unsupported` に倒す。
- **daemon の crash/restart**: `node_sessions` は DB にあるので restart 後も行は残る。行はあるが jsonl が無い
  （config dir の掃除等）場合は claude の resume 拒否として検出され D1 の `resume_rejected` 経路で fresh に倒れる。

### D4. 指標（before/after を比べるための定義）

run ごとに `RunMetrics`（`runs.metrics`）へ足し、Task 単位で `ExecutionMetrics`（`GET /tasks/{id}/execution`、
`GET /metrics/execution`）に集計する。比較は「本 ADR の resume を無効にした run 群（before、`continuation_resume = false`
または導入前）」と「有効にした run 群（after）」を同じ定義で並べる。

| 指標 | 定義 | 単位・集計 |
|------|------|-----------|
| run 数 | Task（または WU）の worker run の件数（planner/reviewer は role 別に別枠。既存 `runs_by_role`） | 件、合計 |
| 総 wall time | 各 run の `wall_ms` の和（並列 WU は重なっても和を取る。壁時計の経過時間は別に Task 作成→終端も出す） | ms、合計 |
| 入力 token | run の usage の `input_tokens + cache_read_input_tokens + cache_creation_input_tokens`（内訳も保持。cache_read の比率で resume の効果を見る） | token、合計と内訳 |
| 再探索の重複 | run 内の tool_use のうち `Read`（同じ正規化 path）・`Grep`/`Glob`（同じ pattern + path）の 2 回目以降の回数。continuation をまたいだ重複は「同じ WU の前の run で既に同じ key を読んだ」回数として別に数える | 回、run 内 / WU 内（run をまたぐ） |
| resume/fresh の内訳 | continuation の run の `session_mode = resumed | fresh` の件数 | 件 |
| fallback 理由 | fresh になった continuation の D1 の理由（`role_fresh` … `resume_rejected`）ごとの件数 | 件、理由別 |

- 再探索の計数は adapter の event stream（claude-code の `tool_use` の `name`/`input`）から決定的に数える
  （path は worktree 相対に正規化。中身は記録しない）。取れない adapter は `None`。
- 指標は観測であり、判断表の入力には使わない（rollover だけは既存の `approx_tokens` を使う）。

### D5. 変えないもの

- `task-ops` `changes.rs` の `rebase_and_advance` / `sync_onto_target`、`review.rs`（review run の組み立て）、
  `review_verdict.rs`（ReviewRepair の判定・repair WU の作成）の意味。review fail 後の再作業は continuation ではない
  （判断表 #3）ので fresh のまま。
- ADR-0043 の workspace・衝突解消タスク・container の認証情報の読み取り専用マウント。
- ADR-0079 の再帰統合（子 task は親ブランチへ、子は独立した Task なので session を共有しない。判断表 #2）。
- root delivery（`crates/celeris/src/delivery.rs`、ADR-0121）と selfdeploy。
- remote worktree（ADR-0018/0019）の同期と実行の意味。
- ADR-0054 の `conversation` / `lead` session の判断・rollover・差分前置き。
- planner の routing（Phase 4。planner は判断表 #1 で常に fresh なので触れない）。

## 採らない

- 全 run（初回の WU run・review fail 後の再作業を含む）を同じ session に載せる: review の独立性と WU の分離を崩す。
- 別 account の session jsonl をコピーして resume する: account isolation を破る。
- container に config dir を書き込み可能でマウントする: 認証情報の保護を弱める。
- LLM に resume すべきか判断させる: 判断は表の決定的な条件だけで行う。
- codex / acp の continuation resume: ADR-0054 で resume の安定性に既知の問題（codex の `thread/resume` 未実装等）があるため、
  まず claude-code だけで効果を測る。

## 受け入れ条件（後続 WorkUnit が満たす）

- `session_reuse` を名前に含むテスト: 同一条件の continuation が同じ session id を `--resume` で渡すこと、
  adapter/account 変更・session 喪失・resume 拒否・fresh 要求で checkpoint 前置きの fresh になること。
- `fresh_session` を名前に含むテスト: planner / reviewer / 独立 WU が fresh のままで、別 account の session を resume しないこと。
- `continuation_metrics` を名前に含むテスト: D4 の指標（run 数・wall time・入力 token・再探索重複・resume/fresh 内訳・fallback 理由）が
  出力されること。
- テストは偽 adapter で行い、実 claude・外部ネットワーク・systemd を使わない。

## 付記（2026-10-03、remote-resume）

- D3 の remote worktree の可否は `session_resume_remote_*` 試験（`dispatcher/tests/session_resume.rs`）で固定した:
  同じ account・同じ手元 cwd なら resume、別 account（`account_changed`）と resume 拒否（`resume_rejected`）は
  checkpoint fallback で、他 account の session id を `--resume` に渡さない。判断表・実装の変更は無し。
- 試験のために `Dispatcher::set_cluster_ssh_command_override`（remote の `SshSettings` の ssh/rsync を偽に差し替える
  継ぎ目。本番は呼ばない）を足した。
