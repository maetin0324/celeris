---
title: docs 再構成 — gui-api.md §1・§2 を router に合わせ、overview.md を統合して削除（cleanup-api / api-list）
tasks: [01M3YNFVFZG5AFFM0RS05X22ZT]
status: done
updated: 2026-10-02
---
# docs 再構成 — gui-api.md §1・§2 を router に合わせ、overview.md を統合して削除（cleanup-api / api-list）

完了日: 2026-10-02。`docs/api/v1/gui-api.md` を API の説明の唯一の文書にした。crates/・web/・gui/・scripts/・CLAUDE.md・.claude/・
`scripts/dev/docs-layout.tsv`・`*.schema.json` は読んだだけで変えていない。

## 処理した文書

| 処理(削除/統合/移動/修正) | 旧パス | 新パス or - | 理由 | 最後の commit |
|---|---|---|---|---|
| 統合 | `docs/api/v1/overview.md` | `docs/api/v1/gui-api.md` §3.125 | 同じ API（実行・計画・木・決定・撤去した入口）を 2 つ目の文書で説明していた。§3.125.1〜15 に移して `git rm` | `a42f9a54` |
| 修正 | `docs/api/v1/gui-api.md` | - | §2 を router の全 route に合わせ、§1 を middleware・problem と照らして直した。冒頭の overview.md へのリンクを §3.125 への案内に変えた | `01fd1f91` |
| 修正 | `docs/README.md` | - | `api/` の行から `overview.md` を外し、説明は gui-api.md の 1 本と書いた | `01fd1f91` |
| 処理なし | `docs/gui/` | - | move-docs で `agent-docs/gui/` へ移動済み（`docs/gui/` はもう無い。記録は `agent-docs/progress/2026-10-02-docs-layout/move-docs.md`） | - |

## 直した点

### §2 エンドポイント一覧（95 → 174）

- `crates/task-api/src` の `.route(…)`（`handlers.rs` の router と、`merge` した各モジュールの `routes()`。`browser_control.rs` の
  `BASE` + `format!` を含む）から 146 本のパス・174 組の（メソッド, パス）を取り出し、表を 1 組 1 行に揃えた。
- 足した行: #96〜100（console/instruct・llm/sources・console/new-conversation・mcp/clients・mcp/calls。§3.107〜3.111 が
  既に番号で指していた欠番）と #108〜174（accept・retry・rereview・tasks pause/resume・accounts 7 本・clusters settings・
  org memory・reports 4 本・approvals / standing-rules 5 本・projects plan / project-plan decide（410）・docs/maintenance 2 本・
  execution / execution-plan / tree/adopt / phase-gate / plan-gate / decompose / task-tree・metrics/execution・metrics/scratch・
  decisions 5 本・browser 系 24 本）。応答型は各ハンドラの `json_response(…)` の型と状態コード、`task_ops` の関数の戻り値で書いた。
- 実在しない行は無かった（表の 101 行はすべて route にあった）。
- 410 を返す 9 本（#17・48・49・54・78〜80・131・132）の応答型を `410 Problem（旧 …）` にし、壊れていた
  `docs/celeris-api-v1.md` への案内を §3.125.8 に変えた。
- 見出しを「（174 = 表 168 + browser 制御 6）」にし、番号の意味・パスの書き方・browser 系の詳細の在りかを表の前に書いた。

- （2 回目の run）browser 制御の 6 本（旧 #169〜174）は番号付きの表から外し、表の直後の別表（番号なし）に置いた。
  route が定数 `BASE` と `format!` で登録されていて、計画の check（`.route( "…"` の文字列リテラルだけを拾う）からは見えず
  `stale` と判定されたため。行は残っているので、全 route が §2 にあることは変わらない。
- （2 回目の run）gui-api.md の冒頭と §3.125 の見出しにあった「旧 `overview.md` を統合した」の文言を消した（経緯はこの記録にある）。
- （2 回目の run）`docs/protocol/worker-protocol.md` 3〜4 行目の ADR へのリンク 2 件を `../../agent-docs/adr/` に直した（パスだけ。
  protocol WU と同じ行に触れるので、統合で衝突したら protocol 側の版を採ればよい）。

### §1 全体（`middleware.rs`・`problem.rs`・`lib.rs` と照合）

- 1.1: 設定の型の場所（`crates/celeris/src/config/api.rs` の `ApiConfig`、task-api の `ApiSettings`）、トークンは SHA-256 で持つこと、`MAX_STREAMS`。
- 1.2: `OPTIONS` の 405 は認証より前。ページングの既定・最大はエンドポイントごと（`/tasks` 100/500、イベント系 500/5,000）、`limit=0` は 400。
  Content-Type の検査は `POST`/`PUT`/`PATCH`（本文が空でも）、`DELETE` は検査しない。413 は middleware（`Content-Length`）とハンドラ（読み込み中）の両方。
- 1.3: Bearer の scheme は大文字小文字を区別しない、比較は SHA-256 の値どうし。「変更系は 1 つ残らず管理系」を、`require_admin` を使わない
  browser の Live View・制御（署名 assertion / daemon bearer。token が無い構成では 403 `browser_control_disabled`）の例外つきに直した。
- 1.4: Host は `Host` ヘッダと absolute-form の authority の両方を照合、`Host` が無い・複数・非 ASCII も 400、検査の順。Origin の拒否は `PUT` を含む全変更系。
- 1.5: `removed_by_adr_0079`（410）・`replay_in_progress`（503）を足し、`validation` の `field` の推定先に `title`/`objective`/`parent` を足した。
  `OpsError` の写像表に `ProjectNotFound`・`MilestoneNotFound`・`InvalidLifecycle`・`DecisionNotFound`・`DecisionNotOpen`・`TreeAdopt`・`ProjectPlan*` を、
  `StoreError::InUse` の 409 を足した。エンドポイント固有のコードは §3 の各節にあると明記した。

### §3.125（旧 overview.md）

- overview.md の 23 節を §3.125.1〜15 に並べ直して移した（execution・execution-plan GET / POST・PUT・tree/adopt・metrics/execution・accept と retry・
  decompose・撤去した入口（410）・tasks pause/resume・task-tree・phase-gate・plan-gate・決定の要求 5 本と受信箱・案件の GET/PATCH の現行の形・`stages_hint`）。
- 実装と照らした点: `accept` の本文 `ReopenBody`、`RetryBody.accept` の既定 true と `execution`、`EXECUTION_METRICS_GROUP_BY` の 5 値、`task-tree` の `root`、
  `TaskPauseResult` の欄、`AdoptRequest`・`DecisionAnswerBody` の欄、410 の入口 7 種（`ApiProblem::gone` の 7 か所）と `instead`、`include_frozen`、`GET /tasks?milestone=`。
  食い違いは見つからなかった。overview.md の壊れたリンク（`api/v1/api-v1.schema.json`・`gui/api.md`）は移すときに落とした。
- 重複: 410 の入口の説明は §3.125.8 の 1 か所にし、§2 の 410 の行はそこを指す。

## 証拠

| コマンド | 結果 |
|---|---|
| `python3 artifacts/routes.py crates/task-api/src`（router の `.route` を括弧の対応で読む。tests.rs と `#[cfg(test)] mod tests` を除く） | 174 組・146 パス |
| §2 の表の（メソッド, パス）と上の 174 組の `diff` | 差分ゼロ（`MATCH`）。番号 1〜174 は重複・欠番なし |
| `sh scripts/dev/check-doc-links.sh docs/api docs/README.md` | `check-doc-links: ok`、exit 0 |
| `sh scripts/dev/check-doc-links.sh`（全体） | exit 1、45 件。うち本 WU 由来は `gui/docs/celeris-api-v1.md:3: ../../docs/api/v1/overview.md` の 1 件（下の未解決）。残り 44 件は docs/guides・docs/ops・docs/protocol の既存の壊れたリンクで本 WU の範囲外 |
| `git diff --stat HEAD` | `docs/README.md`・`docs/api/v1/gui-api.md`・`docs/api/v1/overview.md`（削除）・この記録だけ |

| 計画の check 1（route ⇔ §2 表、`missing`/`stale`） | 2 回目の run で exit 0 |
| 計画の check 2（overview.md 無し・参照無し・`check-doc-links.sh docs/api docs/protocol docs/README.md`） | 2 回目の run で `check-doc-links: ok`、exit 0 |

## 未解決

- `gui/docs/celeris-api-v1.md:3` が `docs/api/v1/overview.md` を指したまま壊れる。gui/ はこの task では変えられない。
  このファイルは gui-api.md の古い写し（同じ見出し・改訂履歴）で、「同じ API を説明する文書は 1 つ」に反する。
- §3 の既存の節（§3.47・§3.48・§3.49・§3.61・§3.63・§3.84〜3.91）は 410 になった入口や旧い挙動をまだ現役のように書いている。
  §3.125.8・§3.125.14 が現行。直すのは api-s3a / api-s3b の範囲。
- browser の Live View（#165〜168）と制御（#169〜174）は §3 に節が無く、`docs/guides/browser-capability.md` にも説明が無い（表の行だけ）。
- `cargo test --workspace` / `cargo clippy` はこの WU では回していない（Rust・schema に差分が無い文書だけの変更）。

- 計画の check 1 は `format!` で組み立てた route（browser 制御 6 本）を拾えない。そのため 6 本は番号付きの表の外に置いてある。

## 提案

- `gui/docs/celeris-api-v1.md` を消すか、1 行目以降を `docs/api/v1/gui-api.md` への案内だけにする（gui/ を触れる task で）。
- `crates/task-api/src/lib.rs` の冒頭のコメント「`/api/v1` 配下の 26 エンドポイント」は古い（今は 146 パス）。crates を触れる task で直す。
- §2 の表と router の一致を決定的な検査にする（このとき使った `routes.py` 相当を `scripts/dev/` に置き、CI か leaf の check で回す）。
- browser の Live View・制御の API の説明を `docs/guides/browser-capability.md` か gui-api.md §3 に足す。
