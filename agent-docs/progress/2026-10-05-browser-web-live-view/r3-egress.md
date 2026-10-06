---
tasks: [01M481T9T6AEH4N7MTGXFVKV7V]
status: done
completed: 2026-10-06
---

# R3: 試験専用 loopback egress 許可と拒否理由の記録

## 変更

- `docs/ops/browser-launcher-host-setup.md`: launcher TOML の `test_loopback_egress`（省略時 off）、loopback host:port の制約、本番 path/DB/socket での fail-closed、有効時ログ、session 記録の形式・場所と daemon/launcher 同一 release 更新を記載。
- `scripts/dev/browser-web-live-check.sh`: launcher config に許可ページの `127.0.0.1:<PAGE_PORT>` が明記されていることを起動前に検査。egress 証跡段だけを変更し、launcher state dir 配下の session `egress-denied.jsonl` を集め、範囲外 origin の `private_address`・`127.0.0.1:<DENIED_PORT>`・時刻・session id を検査したうえで `egress-denied.json` を生成する。許可 port が拒否記録に現れないことも確認。不許可ページに GET がない検査、settings URL、cleanup 等の他段は維持。
- `docs/ops/browser-web-live-check.md`: 試験 config の必要欄、実行前提、記録収集の証跡形式を更新。
- ADR `agent-docs/adr/2026-10-05-browser-department-web-live-view.md` に E4/E5 を追加し、close 変更と実装関数・試験名を突き合わせた。

## 各葉の証拠

- policy: `agent-docs/progress/2026-10-05-browser-web-live-view/r3-egress/policy.md`。記載の `cargo nextest ... egress_test_loopback`（6 passed）、`cargo clippy --workspace --all-targets -- -D warnings`、並列試験、文書検査はいずれも exit 0。
- launcher-cfg: `.../r3-egress/launcher-cfg.md`。`cargo nextest run --workspace -E 'test(/egress_test_loopback_/)'`（13 passed）、`cargo check --workspace --all-targets`、clippy、並列試験と文書検査は exit 0。
- denial-record: `.../r3-egress/denial-record.md`。`cargo test -p task-worker egress_denial_record_ --quiet`（4 passed）、egress process/relay 試験（8 passed）、clippy、並列試験、文書検査は exit 0。
- close の検査結果は下記のコマンド欄と `artifacts/result.json` を参照。

## 人が行う実機確認

この作業環境では root 所有 launcher config・別 UID launcher・user namespace を使う実機確認を行っていない。Fable は launcher/daemon を同じ最終 release から用意し、`docs/ops/browser-web-live-check.md` の隔離手順で `CELERIS_USERNS_TESTS=1` を設定して再実行すること。試験ページ port と launcher の `test_loopback_egress` は一致させ、`egress-denied.json` が launcher session 記録由来であること、範囲外 port の拒否と許可ページへの遷移が成立することを証跡に残す。本番 path/config/DB/socket で loopback 許可を有効にしてはならない。

## close 検査

文書検査 `sh scripts/dev/check-doc-links.sh`、`sh scripts/dev/check-adr-numbers.sh`、`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` はすべて exit 0。`bash -n scripts/dev/browser-web-live-check.sh` と `git diff --check` も exit 0。`git diff --name-only "$CELERIS_WU_BASE" -- crates/ web/ gui/` は空（exit 0）。範囲内の変更は host setup、Live View 手順、台本、ADR 付記、本進捗の5ファイル。台本の実機実行は userns と host 配置が必要なため人の確認として残す。
