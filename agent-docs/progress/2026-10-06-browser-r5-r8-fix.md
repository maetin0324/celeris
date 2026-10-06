---
title: 3 回目の実機確認で見つかった R5〜R8 と launcher session dir の修正
tasks: [01M4896B1Q1617Q95NT2BNDNAF]
status: done
updated: 2026-10-06
completed: 2026-10-06
---

# 3 回目の実機確認で見つかった R5〜R8 と launcher session dir の修正

統合 HEAD `ec41de3d`（`integrate wu/launcher-launch (phase fix)`）で検証した。R5〜R8 の各変更は `bf9c4175`（台本）、`3be7566f`（egress）、`ec41de3d`（launcher）の統合 commit に含まれる。

## 対応

- **R5 — launcher attach policy:** session の agent-browser `policy.json` に CDP attach 用 `launch` を加えた。許可 origin の `navigate` を launcher policy 経由で通す試験を追加し、許可外 port は拒否する。launcher の起動失敗・通常停止時に session dir を回収し、subuid 所有の中身は runtime 停止後に mapped subuid で片付ける。
- **R6 — HTTP egress:** egress proxy が `http://` の絶対 URI を持つ GET/HEAD を受け、CONNECT と同じ origin 許可、DNS 解決後の全アドレス検査、SSRF 拒否、拒否記録を通す。Host と URI の一致、hop-by-hop header の除去、1 接続 1 要求、`Connection: close` を固定し、拒否理由を `DenialRecorder` の allowlist に追加した。https 証明書を使わない loopback 上流試験で許可・拒否を確認する。
- **R7 — session ID:** 実機確認台本は launcher の `Started` 応答にある session ID で `state_dir/sessions/<id>/egress-denied.jsonl` を探す。task 作成以降の daemon log から ID を一意に抽出する。
- **R8 — egress 拒否の発生:** 許可ページ内の `#forbidden-link` を snapshot の参照でクリックし、Chrome 自身から許可外 origin（port 17731）へ遷移させて proxy の拒否記録を採る。wrapper に許可外 URL を直接渡す方式は使わない。
- **残存 session dir:** launcher session の所有 guard が通常停止と失敗経路で session dir を片付ける。

## 検証結果

- `cargo test -p task-worker` — exit 0、808 passed、0 failed、4 ignored。launcher policy、HTTP GET/HEAD forward、拒否記録、browser egress・launcher 関連試験を含む。
- `cargo test --workspace --no-run` — exit 0。
- `cargo clippy --workspace -- -D warnings` — exit 0。
- `bash -n scripts/dev/browser-web-live-check.sh` — exit 0。
- `sh scripts/dev/progress-index.sh --check` — exit 0。
- `sh scripts/dev/check-adr-numbers.sh` — exit 0（147 files）。
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` — exit 0。
- 範囲確認 `git diff --quiet "$CELERIS_WU_BASE" HEAD -- . ':(exclude)agent-docs/progress/2026-10-06-browser-r5-r8-fix/**'` — exit 0。

## 未解決事項

- R5/R6 の実装は、host の `/usr/local/libexec/celeris/celeris-browser-launcher` と egress binary を人が入れ替えるまで本番では有効にならない。daemon・launcher の本番差し替えや再起動はこの検証では行っていない。
- 4 回目の実機確認は Fable が実施する。sandbox では launcher/Chrome の実機確認を行えないため、ここでは task-worker 試験と compile/clippy で検証した。
- 人が実施する launcher/egress の配置・再起動手順と確認方法は、運用側で実行する。
