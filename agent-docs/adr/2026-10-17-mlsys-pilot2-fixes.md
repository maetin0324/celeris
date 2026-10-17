# ADR 2026-10-17: MLSys pilot-2 で見つかった campaign の欠け 6 件を直す

- 日付: 2026-10-17
- 状態: 採用・実装済み（crates/campaign）
- 関連: 2026-10-16-mlsys-pilot-wiring-fixes（D4 の「A に後付け制限を足さない」を本 ADR の D2/D3 で置き換える）、
  2026-10-16-campaign-budget-cap、2026-10-16-campaign-budget-reservation、2026-10-11-breaking-changes-by-default

## 背景
release 05a398f1efc9 の運用者 pilot-2（3 試行、A だけ実行、B/C は `POST /tasks` が 422）で、本測定の前に直す欠けが 6 件出た。
結果は `/local/celeris/data/scratch/mlsys-pilot-2/RESULTS.md`。

## 決定
### D1 外部入力は `--inputs-src` で hash 付きに固定して、全腕の workspace へ配る
- case.json の `inputs[]`（`name`, `sha256`）が正本。`--inputs-src <dir>` は `<dir>/<name>` の平らな配置。
  `campaign asset store` という架空の取得元は廃止し、case.json の `fetch` 文言も実際の手順に直す。
- `materials/` には足さない（`materials_sha256` は manifest で固定済みで、入力を混ぜると hash が変わる）。
  入力は別経路: run が `<out>/snapshot/inputs/<name>` に取り込み（sha256 を case.json の値と照合）、各試行の
  workspace の `inputs/<name>` へ、**その問題が宣言した入力だけ**複製する。複製後にもう一度 sha256 を照合する。
- 試行記録に `inputs`（name と sha256）を残す。A/B/C は同じ問題なら同じ入力を持つ。
- preflight は、選んだ全問題の入力について `--inputs-src` の存在・通常 file・sha256 一致を検査する。
  `initial`（初期 repo）を持つ問題があるのに `--repo-src` が無いのも不足とする。
- 取り込み元が欠けたまま run は始まらない（fail-closed）。名前が危険（`/`, `..`）な入力は case 読み込みで拒否する。
- 本測定の k4（`main-v3-n2-43706.tgz`）は同じ問題があったので同じ機構で解ける。k2 は入力ではなく初期 repo（`initial`）
  で、`--repo-src` の中身は人が用意する（preflight は指定の有無を見る）。
- 展開（`tar -xzf … -C inputs/`）は試行の仕事。grader は展開後の `inputs/` を見る。`no_artifact` の判定は
  `inputs/` を除いた木で行う（配った入力や展開だけでは成果物と見なさない）。

### D2 A 腕は bubblewrap の中で動かす（ファイルシステム隔離）
- A の command を `bwrap` で包む。見える物: 読み取り専用の `/usr`（と `/bin` `/lib*` `/sbin` の symlink／bind）・`/etc`、
  `/proc`、`/dev`、空の `/tmp`、試行の workspace（同じ path で書き込み可）、A のアカウント dir（D5、書き込み可）、
  claude の実行ファイルの実体（読み取り専用）。`/home` `/local` `/work`、`out/` の他の試行、src の
  `fixtures/grader-private`、運用者の staging、過去の workspace は **mount しない**（存在しない）。
- 環境変数は `--clearenv` のあと `PATH` `HOME`（workspace 内）`LANG` `TERM` `CLAUDE_CONFIG_DIR` だけ渡す。運用者の
  `CELERIS_CAMPAIGN_TOKEN` 等は A に見えない。network は残す（API 呼び出しに要る）。`--unshare-pid` `--die-with-parent`。
- 隔離は常に有効で、切る flag は無い。`bwrap` が使えない host では run が始まらない（preflight が実際に
  `bwrap … true` を走らせて確かめる）。ADR 2026-10-16-mlsys-pilot-wiring-fixes D4 の「sandbox を足さない」は
  この決定で置き換える（turn・許可リスト・tool の制限は引き続き足さない。隔離は読める範囲だけ）。
- B/C は Celeris の worker 隔離（ADR-0095）に従い、試行の workspace が `out/trials/…` の複製である点も A と同じ。
  campaign 側は B/C 用に追加の隔離を足さない（既存の worker sandbox の範囲）。確認結果は進捗に書く。

### D3 A 腕の token 予算を実行中に強制する
- A は `--output-format stream-json --verbose` で起動し、stdout を逐次読む。`message.id` ごとの最新 usage を足した
  生の累積（`token_basis` で数える）を `Executor::usage` として返す。
- `HarnessExecutor` は submit 時に監視 thread を起こし、約 1 秒ごとに累積を見て `TrialRequest.budget.max_tokens` を
  超えたら process group を kill して `final.json`（cancelled / `case_budget`）を書く。再開後は `poll` が同じ判定をする。
  最大超過は「監視 1 周期ぶんの使用量 + 1 メッセージ」で、`summary.json` の `overshoot_granularity` に書く。
- 最終の `result` 行が無い（kill された）試行の tokens は、生の累積を使う（欠測にしない）。

### D4 preflight が `POST /tasks` の 422 を投入前に検出し、campaign 専用案件を要求する
- preflight は `--api-url` に GET だけを送り（有料・書き込みなし）、`/api/v1/projects/{id}` と `/projects/{id}/repos` から
  task-ops `create` と同じ規則を評価する: executor は常に local `workspace` を渡すので、template の `repos`（無ければ案件の
  primary）に **リモートの repo が 1 つでもあれば拒否**。template に `workspace` / `cluster` を書くのも既に拒否している。
- 案件の `auto_integrate` が true なら拒否する（試行 task が自己改善の main に自動取り込みされないこと）。
- 専用案件は人が作る（本番への書き込みは worker／preflight はしない）。手順は run-final.md の §0.5:
  `POST /api/v1/projects`、ローカルの空 git repo を `POST /projects/{id}/repos`（local・primary）、`auto_integrate` は
  既定 false のまま。

### D5 A 腕は Celeris 管理のアカウントで起動し、消費を台帳と使用率の確認に入れる
- `--a-account <account_id>`（claude_oauth のプールのアカウント）が必須。`GET /api/v1/llm/accounts` から `dir` を引いて
  `CLAUDE_CONFIG_DIR` に渡す（既定の `~/.claude` は使わない）。logged_in でなければ preflight が拒否する。
  `observed.json` の `source` に `claude_oauth:<account_id>` を残す。
- 消費は provider 側の使用率に出るので、Celeris の 7 日使用率の観測にも入る。`celeris-campaign usage` が
  `GET /llm/accounts` の 7 日使用率と、campaign の腕ごとの tokens（台帳／summary）を並べて出し、上限（`--max-7d`、既定 0.45）超を非 0 で返す。

### D6 未実行・作成失敗の試行があれば complete=false
- checker は、各 (arm, problem, rep) の最初の記録が `executor_error`（task 作成失敗）または実行されなかった
  （`budget_stop` / `arm_budget` で workspace 無し）なら `unexecuted` に数え、`incomplete_reasons` に出す。
  `export` はこのとき `complete=false`・`real_performance=false`（`table.incomplete.*` のみ）。依存で blocked は従来どおり
  分母に残る結果で、未実行には数えない。

## 影響
- 旧い経路の削除: asset store の記述、`ClaudeCodeBuilder` の `--output-format json`、`HarnessExecutor::new` の無隔離起動。
- 新 flag: `--inputs-src`、`--a-account`、サブコマンド `usage`。run-final.md と runbook §5 を実コマンドに合わせた。
