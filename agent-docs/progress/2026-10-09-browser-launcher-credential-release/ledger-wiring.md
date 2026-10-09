# ledger-wiring: launcher credential 証拠を release と browser-ledger.sh につなぐ

ADR: `agent-docs/adr/2026-10-09-browser-launcher-credential-release.md`（台帳と host 条件の節）。

## 変更

- `scripts/selfdeploy/lib.sh`（`sd_browser_ledger` 3b）:
  - 生成器を `--credential-runtime launcher` で呼ぶ。`release.sh` と `browser-ledger.sh` は同じ関数を呼ぶので両方に効く。
  - launcher socket は `SD_BROWSER_LAUNCHER_SOCKET`（既定 `/run/celeris-browser/launcher.sock`、`docs/ops/browser-launcher-host-setup.md` の socket path）。存在しなければ空にして `CELERIS_BROWSER_LAUNCHER_SOCKET` を空で生成器へ渡す。生成器は `launcher_unavailable` と理由を返す。
  - 生成器が失敗（launcher 無し等）しても、生成器が書いた台帳（credential claim を消したもの）を採る。これで credential が空になり、公開台帳は残る。
- `scripts/selfdeploy/tests/browser_ledger_launcher_credential.sh`（新規）: 偽の生成器・agent-browser・celerisctl だけを使う。
  - launcher 無し: 台帳は ok で置かれ、`credential_evidence` は `launcher_unavailable` と理由付き、`credential_backends` は空、古い credential claim は消え、log に理由が残る。`sd_browser_ledger` は 0 を返す。
  - launcher 有り: 生成器が `--credential-runtime launcher` と socket 付きで呼ばれ、credential 証拠（ok）が採られる。

## 判断

- launcher の有無は生成器に任せず、呼ぶ側（lib.sh）が socket の存在で決める。生成器は env の有無だけを見る（既存の生成器の契約を変えない）。
- 既存の daemon の credential 段（`--credential-runtime` 既定 daemon）は呼ばなくなった。daemon の証拠を launcher の適合へ流用しない、という ADR の条件を満たすため。

## 検証

- `bash scripts/selfdeploy/tests/browser_ledger_launcher_credential.sh` → exit 0（`browser_ledger_launcher_credential: ok`）
- `bash scripts/selfdeploy/tests/browser_ledger_release_stages.sh` → exit 0（`PASS: browser_ledger_release`）
- `bash scripts/selfdeploy/tests/browser_ledger_credential_evidence.sh` → exit 0（同じ fixture を exec する）
- `bash -n scripts/selfdeploy/lib.sh` と試験 script → 構文 OK。`git diff --check` → 問題なし。
- 範囲: 変更は `scripts/selfdeploy/lib.sh` と新規試験の 2 file だけ。

Rust のコードは変えていないので、`cargo clippy` と `bash scripts/dev/test-parallel.sh` は統合後の close-out 工程で回す（この WU では未実行）。

## 未解決事項

- 本番 host では `/run/celeris-browser/launcher.sock` が存在するときだけ launcher 証拠が生成される。実際の launcher 経路の credential 実証（必須モード stutter 3 回・HEAD での ADMISSION 再取得）は人の host 操作で、`ops-runbook` の手順（`docs/ops/browser-launcher-credential-release.md`）に従う。この branch の base には未取り込みのため、ここでは確認していない。
- `release.sh` 自体には変更なし（`sd_browser_ledger` を呼ぶだけ）。launcher socket の環境変数を release の外から渡す場合は `SD_BROWSER_LAUNCHER_SOCKET` を使う。

## 提案

- 本番の launcher 証拠を release ごとに取るか、人が `browser-ledger.sh` で作り直すかは運用で決める。現状は release の度に launcher が有れば取る。
