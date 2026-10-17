# ADR 2026-10-17: campaign 試行の run 文脈を試行用 KB に固定する

- 日付: 2026-10-17
- 状態: 採用・実装済み（crates/task-dispatch）
- 関連: 2026-10-16-campaign-identity-propagation（試行の判定に使う）、2026-10-16-mlsys-trial-inputs（試行の `kb/`）、
  2026-10-17-mlsys-pilot2-fixes（A の bwrap 隔離）、ADR-0047（knowledge の mount と根の決め方）、ADR-0033（記憶・会話・認可）
- task: 01M54HYGQPFNRX6PNPJJG475EB（P0-3）。棚卸しは `agent-docs/progress/2026-10-17-mlsys-p0-3-trial-context.md`

## 背景
campaign runner は試行の作業場所に `materials/` と `kb/` を置くが、B/C の Celeris task の run 文脈（`run_extras`）は
通常の `config.knowledge.root` の索引（profile・`default_mounts`・案件 `projects/<slug>` の mount）、担当の記憶
（`memory_dir`）、会話履歴、永続の認可、通常 KB の skills を載せていた。worker の `celerisctl knowledge` も設定の根に落ちる。
棚卸しの R2/R3 は測定専用 config だけでは直らない（試行ごとに違う `<ws>/kb` を静的 config に書けない）。

## 決定
- D1 判定と KB: task の campaign 識別（自身か祖先。`task_campaign_resolve`）があれば「試行 task」。試行 task の knowledge
  mount は `dir:<試行の根 task の作業場所>/kb` の 1 つだけにし、profile・`default_mounts`・案件・`memory` の mount を使わない。
  `kb` mount が無い run では通常 KB の `index.json` に触れない（`mounted_knowledge`）。試行の根は `parent_id` の鎖の先頭
  （runner が `POST /tasks` で作った task）。根がローカルの作業場所でなければ試行用 KB も載せない（通常 KB に倒さない）。
  新しい flag・権限基盤は足さない。worker・planner・WU・木の子 task は全て `run_extras` を通るので同じ判定に乗る。
- D2 記憶・会話・認可: 試行 task には `memory`・`conversation`・`standing_rules` を載せない（測定 DB/config を分ける構成でも
  止まるが、同じ担当×同じ campaign 案件で他 arm・他反復の記憶が載る経路をコードで閉じる）。`absorb_memory`（書き込み）は
  変えない。読まれないので後続試行に届かない。
- D3 skills: 試行 task（worker 系と reviewer）には skills を渡さない。skills の根は通常 KB で、A arm は skills を受けない
  ので A/B/C で揃う。前置きの profile からも knowledge と skills_mounts を外す。「固定の内部運用共通知識」は Celeris の
  固定の前置き（役割の指示文・分野の manifest・成果物規則）だけとする。
- D4 worker の KB の根: 試行 task の run の env に `CELERIS_KNOWLEDGE_ROOT=<ws>/kb` を重ねる（WU の env と同じ口）。
  `celerisctl knowledge` は `--root` の次にこれを読むので、既定の設定 path に落ちない。`<ws>/kb` は git の KB ではないため
  `search`/`get`/`record` は「KB が無い」で止まる（通常 KB へは行かない、fail closed）。worker は前置きの索引の path を直接読む。
- 範囲外: worker の filesystem 直読み（棚卸し R8）・Claude の account dir（R10/R11）は人の決定・構成（preflight-kb）で扱う。
  reviewer に env は足さない（前置きに knowledge 節が無い）。

## 結果
- 試験 `crates/task-dispatch/src/dispatcher/tests/trial_context.rs`（実 Dispatcher の tick を通す）: 通常 KB・skill・記憶・
  会話・認可に置いた canary が試行の worker・planner・子 task・reviewer の `RunContext` に出ず、試行用 `kb/` の canary は
  knowledge index に出て env が `<ws>/kb` を指す。同じ fixture で識別の無い task には従来どおり全 canary が出る（不変）。
