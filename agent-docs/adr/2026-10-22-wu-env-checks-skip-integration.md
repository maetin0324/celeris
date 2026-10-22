# ADR 2026-10-22: WU の環境変数を使う check は段の統合の検査で流さない

## 背景
本番 2026-10-22 11:40Z、moshi-brewed の task 01M5HJ4XEG で、WU の check
`git diff --quiet "$CELERIS_WU_BASE" -- crates/ android/ …`（`scope` の印は無い）が WU の作業時には通ったのに、
段の統合の検査（`phase_integration::start_integration`）で `fatal: bad revision ''`（exit 128）になり task が止まった。
`CELERIS_WU_BASE` / `CELERIS_WU_TARGET`（と snapshot の `CELERIS_WU_BASE_UNTRACKED` / `CELERIS_WU_SCOPE_PATHS`）は
WU の run と WU の checks にだけ入る（`work_unit_scope_env`・`wu_base::scope_env`、ADR-0074 付記 2026-10-05 D3・
ADR 2026-10-08-work-unit-scope-snapshot）。統合の検査は Task の worktree で env 無しに流すので、変数は空になる。
統合の検査から外していたのは `scope: true` の check だけで、planner が `scope` を付け忘れると必ずこの形になる。
人が回答しても同じ検査が再実行され、同じ失敗を繰り返す。

## 決定
- D1: `CELERIS_WU_*` は「その WU が自分の作業を始めた時点」を指す変数で、段の統合の後の Task の worktree には対応する値が無い。
  統合の検査ではこれらを設定しない（WU の `base_commit` も段の基点も渡さない）。
  - WU の `base_commit` を渡すと、同じ段の他の WU の変更（統合で入る）を「この WU の差分」として拾い、正しい WU を落とす。
  - 段の基点を渡すと、check の意味が「この WU の差分」から「段全体の差分」に変わる。複数 WU の段では同じく誤って落ちる。
- D2: `cmd` が `CELERIS_WU_` を含む WU の check は、`scope: true` と同じく WU の作業時だけで判定し、段の統合の検査
  （`integration_checks_for_phase`）に入れない。判定は `task_core::WorkUnitCheck::is_work_unit_only`
  （`scope || cmd に CELERIS_WU_ を含む`）の 1 か所。WU の作業時の判定は変えない（従来どおり env 付きで流れ、落ちれば WU が落ちる）。
  - 統合は WU の commit を merge するだけで WU 自身の差分は変わらないので、WU の作業時の判定がそのまま残る。統合で壊れたかは、
    env を使わない check と `workspace.toml` の check（`default_checks`）が統合後の tree で見る。
  - `${CELERIS_WU_BASE:-…}` のように既定値を持つ形も外す（規則を文字列 1 つで決め、値の無い変数を残さない）。
    `bash scripts/dev/test-affected.sh` のように変数を中で読むだけの check は `cmd` に名前が無いので従来どおり統合でも流れ、
    変数が無いときの既定（`git merge-base HEAD main`）で動く。
- D3: 子 task の acceptance への写し（`promote_to_task`）も同じ判定にする（env の無い場所で流れる点は統合の検査と同じ）。
- D4: planner 指示（`PLANNER_CHECK_GUIDANCE`）と `agent-docs/guides/work-unit-checks.md` に「`$CELERIS_WU_*` を使う check は
  `scope` の有無によらず WU の作業時だけで流れる」と書く。ADR-0074 付記 2026-10-05 の 5（互換: `scope` の無い計画は全 check が
  統合でも流れる）はこの ADR で置き換える。

## 影響
- 既に止まっている task（01M5HJ4XEG）は、この変更の release の後に統合をやり直せば当該 check を流さずに進む。
- 変数を使う check に他の検査（`cargo test … && git diff "$CELERIS_WU_BASE" …`）を連ねた場合、その全体が統合で流れなくなる。
  統合後に流したい検査は別の check に分けて書く（planner 指示に書く）。

## 検証
`crates/task-dispatch/src/dispatcher/tests/scope_checks.rs`: 一時 repository・偽 adapter で、`scope` の無い
`git diff --quiet "$CELERIS_WU_BASE" -- b/` を持つ WU `a`（`a/` だけ変える）と `b/` を変える WU `b` を同じ段に置く。
check は WU の作業時に env 付きで流れて通り、統合の検査には入らず、task は done になる（`bad revision` で落ちない）。
純粋関数 `integration_checks_for_phase` の試験も足す。
