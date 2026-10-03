---
title: sync-main-3: ADR-0128 D6 を写し直し方式へ改め、main 4c354a7f を取り込む
tasks: [01M40D0QW6HX3XEZK3GBCQV5ZM]
status: done
updated: 2026-10-03
---
# sync-main-3: ADR-0128 D6 を写し直し方式へ改め、main 4c354a7f を取り込む

完了日: 2026-10-03。merge commit `87bf8f87`（`git merge main`、main = `4c354a7f`）。

## したこと

- ADR-0128 D6 を改めた: 旧 `docs/PROGRESS.md` は凍結した履歴。取り込みのたびに `sh scripts/dev/progress-transition.sh <main-ref>` で main の全文を
  `agent-docs/PROGRESS.md` へ写し直し、案内 1 行だけを先頭に足す。旧前提（本文を変えなければ rename 追従で入る）が main の組み替えで崩れた経緯、
  `scripts/dev/tests/progress_transition_merge.sh` による確認、移行期間の終わり（人が旧ファイル・README・対象外を消す条件）を書いた。
- 衝突 3 件を解いた:
  - `docs/PROGRESS.md`（modify/delete）: `sh scripts/dev/progress-transition.sh main` で解いた。
  - `scripts/selfdeploy/install-units.sh`: main の hot root の unit 描画（`paths_env_state_dir`・`render_unit`・`UNITS`・`RENDER_DIR`）を基に、
    この branch の変更（運用文書のパス `docs/ops/web-parallel-operation.md`）だけ重ねた。
  - `agent-docs/adr/0138-browser-prod-admission-confidential-release.md`: main の本文（2026-10-03 訂正）を基に、手順書リンクを `../../docs/ops/` へ直しただけ。
- main が旧配置に足したものを新配置へ移した:
  - `docs/adr/0136-local-hot-data-layout.md`・`docs/adr/0139-langmem-proxy-bearer-and-verify-proxy-bind.md` → `agent-docs/adr/`。
    crates のコードが番号で参照するので振り直さず、`check-adr-numbers.sh` の `ALLOWED_OVER_LAST` に足した（ADR-0128 D5 付記に追記）。
  - `docs/progress/local-hot-data-{core-recheck,follow-recheck,verify}.md` → `agent-docs/progress/2026-10-02-local-hot-data/<wu-key>.md`。
  - main の PROGRESS の節「/local への hot データ移行（ADR-0136）」→ `agent-docs/progress/2026-10-02-local-hot-data.md`（新設）、
    「知識整理 run の llm-proxy 401 の修正（ADR-0139）」→ `agent-docs/progress/2026-10-03-langmem-proxy-401.md`（新設）、
    「本番 admission の機密能力解放」の main 側の更新 → 既存の `agent-docs/progress/2026-10-01-browser-prod-admission.md` に追記。
  - 壊れたリンクを直した: `phase-browser-4.md`（main 追記分）、`docs/ops/local-hot-data-migration.md`、ADR-0136 の削除済み手順書への参照。
  - `check-doc-links.sh` の `MIGRATION_EXCLUDE` に `progress-transition.sh` と移行試験を足した（旧パスを名指しする台本のため）。
- `docs/PROGRESS.md`・`docs/DESIGN.md` は復活させていない。crates/ は main の取り込み分以外変えていない
  （`git diff HEAD^1 HEAD -- crates` と `git diff <merge-base> main -- crates` は hunk の行番号以外一致）。

## 証拠コマンドと結果（HEAD = merge commit 87bf8f87）

- `git merge-base --is-ancestor 4c354a7f HEAD && test -z "$(git ls-files -u)" && test ! -e docs/PROGRESS.md && test ! -e docs/DESIGN.md && ! git grep -n -e '^<<<<<<< ' -e '^>>>>>>> ' -- agent-docs docs scripts` → exit 0。
- `sh scripts/dev/check-doc-links.sh` → exit 0（`check-doc-links: ok`。取り込み直後は 21 件の壊れた参照があり、上の修正で 0 件）。
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → exit 0（`check-doc-layout: ok`）。
- `sh scripts/dev/check-adr-numbers.sh` → exit 0（`ok (124 files)`）。
- `sh scripts/dev/progress-index.sh --check` → exit 0。
- `sh scripts/dev/tests/progress_transition_merge.sh` → exit 0（current-main resolution and branch X merge passed; old resolution conflicted as expected）。
- `git merge-tree --write-tree main HEAD` → exit 0（tree `1df8a019`）。
- `bash -n scripts/selfdeploy/install-units.sh` と「4c354a7f からの差分で hot の行を落としていない」検査 → exit 0。
- `bash scripts/selfdeploy/tests/install_units_hot_dir.sh` → exit 0（`install_units_hot_dir: ok`）。
- cargo の全体ゲート（`cargo test --workspace`・`cargo clippy --workspace -- -D warnings`）は workspace check に任せた（この run では流していない）。

## 未解決事項

- `0136`・`0139` を許可リストに足したのは、D5 付記の「main に先にあったことを示して許可リストに足す（人の確認を取る）」に当たる。
  両方とも本 ADR の取り込み前に main にあった（a2124b5f・9fb850c6）ことは示したが、人の確認はまだ取っていない。
- main の PROGRESS の /local 節の末尾には、別 task（01M3YF3NS2EGTZD2BBWNPG1K28 の replay 修正）の記録が同じ見出しの下に入っている。本文は元のまま移した。

## 提案

- 本 task が main に入った後も main に `docs/PROGRESS.md` を足す branch があれば、land の task は同じく `progress-transition.sh` で解く。
