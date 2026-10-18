# A 腕の認証と可変状態を試行 identity ごとに分離する

- 日付: 2026-10-18
- 状態: 実装済み
- tasks: [01M56HCNQX7S26ZYGY071A19E4]

## 背景
`crates/campaign/src/harness_exec.rs` の `Sandbox::args` は A のアカウント dir 丸ごとを書込可で bind し、
`CLAUDE_CONFIG_DIR` に指していた。transcript・projects・自動記憶・設定が全試行（case/arm/repetition）で
共有され、前試行の識別文字列が次試行から読め得る。ADR 2026-10-17-mlsys-trial-context-isolation の
「試行ごとの文脈分離」が A のアカウント可変状態に届いていなかった。

## 決定
- D1 認証の渡し方: アカウント dir から渡すのは `.credentials.json` 1 件だけ。複製せず、log にも出さず、
  bwrap の `--ro-bind <acct>/.credentials.json <state>/.credentials.json` で試行 state dir の中に重ねる
  （read-only。state dir 側には空の mount 先 file を置く）。アカウント dir 全体は bind しない。
  read-only なので token の sandbox 内更新は acct に戻らない（測定は短く、更新は Celeris 側の管理）。
- D2 場所: `<--out>/trial-state/<campaign_id>/<arm>/<case>/rep<N>/`。各要素は英数・`-_.` 以外を `%xx` に
  エスケープし（`..` を作れない）、同じ試行は常に同じ path、別の campaign/arm/case/rep は互いに素。
  `CLAUDE_CONFIG_DIR` はこの dir を指す。transcript・projects・memory・設定はここに書かれる。
- D3 marker: state dir 直下の `trial-identity.json`
  `{campaign_id, manifest_sha256, arm, case, rep, account_id}`。初回に fsync して書き、以降は一致を確かめる。
  不一致、または marker が無いのに内容がある dir（混入）は起動前に拒否する（削除して続行しない）。
- D4 再開: 同一試行の再投入・再開は state dir を保持して再利用する。他試行の state dir は bind しない
  （bwrap 引数に現れるのは自分の dir だけ）。
- D5 旧経路の削除: `Sandbox.config_dir`（account dir 全体の書込 bind）を削除し、`auth_dir` + `state_root` に置換。
  互換分岐は残さない。`main.rs` の構築は `state_root = <out>/trial-state`。

## 試験
`crates/campaign/tests/a_arm_hardening.rs`: bwrap 引数の bind 集合（account dir 全体なし・credentials は ro）、
state dir の安定性と互いに素であること、marker の書込・再開保持・他試行/混入の拒否。実 LLM は呼ばない。
preflight の新規/再開検査への接続は別 WorkUnit（preflight-split）。
