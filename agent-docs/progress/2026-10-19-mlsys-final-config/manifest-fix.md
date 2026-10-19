---
tasks: [01M58VFRBA6HG0SSF3CG5BKM91]
unit: manifest-fix
status: done-pending-operator-input
updated: 2026-10-19
---
# 本測定 manifest の確定（B/C run_harnesses・根拠表なし）と固定 hash

## 変更（`agent-docs/progress/2026-10-16-mlsys-final/` の既存 final.json / task-template.json を更新。複製なし）
- B/C の `routing.models = ["claude-sonnet-5-5"]`・`routing.run_harnesses = ["claude-code"]` を同一に明示。A（claude-code / claude-sonnet-5-5）は集合の内側。内部 probe 用 `cap-probe-*` は含めない（測定 daemon 側の probe 宣言は `cap-probe-claude-code`、別 unit）。
  - 理由: 本測定の実行モデルは Anthropic 1 種で、codex は shadow のみ（実使用 harness は claude-code だけ）。B/C とも同じ `lower_policy=mlsys-min-1` で方策（fixed / jev-1）だけが違う。C を単一モデルへ pin する別設定ではなく、A/B/C とも同一モデル。A は弱めていない。
- C: `evidence_table_id` の placeholder を除き `no_evidence_table=true`（effective_evidence_mode = none）。B は根拠表欄なし = none 明示（`effective_evidence_mode` が none。latest への暗黙 fallback なし）。新しい明示決定は無いため現行型の範囲。
- task-template.json: `project_id` を案件 id `01M2WTS3DKNZBSZ2JMVB4CZMBW` に確定（秘密値ではない）。
- 比較構成: primary = A vs B、secondary = B vs C（subset は K2・K4 の 2 problem）。
- K2 `judged_scope = [implementation, experiment]`、analysis / review は `not_judged`（case.json で確認、変更なし。ADR 2026-10-18-k2-judged-scope）。K4 は analysis。

## 固定 hash（sha256）
| 対象 | sha256 |
|---|---|
| final.json（ファイル、placeholder 残り） | aac6791e2f1369a18ba9098c1cd4ce0bfa27a22e5fc0e7036b070528eb370f5c |
| task-template.json（ファイル） | 26899657bf15f2471d69b7085b53a5efb636ccf3783bfe2ddece65a07f1caab8 |
| final-input-manifest.json（内部 sha256、manifest の `input.sha256` と一致） | 6fa8f0ecf197d731a36ea7c7152b088be99a18c0cf229cdf01340aec28b06e4b |
| snapshot.materials_sha256（変更なし） | dab3ca05d01adf1da268c8ec35e24f33249ca3cc02083e0a2e6b487d0c5a800d |
| snapshot.kb_snapshot（変更なし） | 741f24146f5c02f44acf5291e23806416b9956809e55a1474488ecbcc9fe0702 |
| `celeris-campaign validate` の campaign sha256（operator 値を仮置き `claude-code@0.0.0-dummy` / `0123456789ab` にした検証用写しで取得。実値で再取得が要る） | 5b3bf549457f1aff7e04e0be7700c166333cd9f00770925e35195569e1766b29 |

## validate の結果
- 現ファイル: `celeris-campaign validate --manifest .../final.json` → exit 1, `config A: harness_version is an unresolved placeholder`。H1 後にしか決まらない値が残っているため**意図どおり拒否**（最初の有料実行前の gate）。
- 仮値の写し（`$TMPDIR`、追跡外）: exit 0、上表の sha256。run_harnesses 一致・C の no_evidence_table・A が B/C 集合内の検査を通る。
- materials・入力 manifest・`input.sha256` は変更していない（hash 不変）。env-lock/3・campaign lock はこの unit の範囲外（実 release 昇格後に `lock` で生成。旧測定の out への上書きはしない: `data/final-out` は存在せず本 unit は書いていない）。

## operator 入力待ち（H1 昇格後にだけ決まる。秘密値は書かない）
1. B・C の `harness_version`: 昇格した release sha12（12 桁 hex）。final.json 26・44 行。
2. A の `harness_version`: 実際の claude-code 版（`claude-code@<version>`、alias 不可）。final.json 18 行。
3. account ID（測定に使う claude アカウントの識別。参照のみ、認証値は書かない）。
4. 測定専用 daemon の config 配置（`cap-probe-claude-code` 宣言）— 別 unit（daemon-decl）の雛形を operator が配置。
5. 置換後に `validate` を再実行し、campaign sha256 を本表へ追記してから `lock`。

旧 placeholder 手順（`evidence_table_id` を `celerisctl routing evidence` の版へ置換する手順）は不要になった。`docs/ops/measure-run-final.md` の該当記述は runbook unit が直す。
