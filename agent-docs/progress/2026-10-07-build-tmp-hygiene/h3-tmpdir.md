---
task: build-tmp-hygiene
wu: h3-tmpdir
status: done
completed: 2026-10-07
tasks: [01M4B4J92KBR73EQA5S7FWB21G]
---

# h3-tmpdir: 長い TMPDIR で browser_h3_injection が isolated_runtime_unavailable になる

## 原因（特定済み）
一時の診断出力で、失敗は `SharedCdp::start`（`crates/task-worker/src/browser.rs` の `cdp-relay.sock`）と確定。
`Error { kind: InvalidInput, message: "path must be shorter than SUN_LEN" }`。
session dir は `<workspace>/runs/<run_id>/browser`（TMPDIR ではなく workspace 基準）なので、本番でも workspace が深いと同じ失敗になり得る。
`Supervisor::start`・`ActionServer::start`（action socket は `/tmp/celeris-browser-<uid>/<hash>.sock` の固定長）は通っていた。

## 直したもの
- `crates/task-worker/src/browser_shared_cdp.rs`: `bind_unix_listener` — 上限超えのときは親 dir の fd を開き `/proc/self/fd/<fd>/cdp-relay.sock` で bind（実体は session dir、sandbox からは `/session/cdp-relay.sock`）。別名でも超えれば path と長さを含む明示の誤り。`start` / `start_with_mode`（launcher 経路）共通。
- ADR `2026-10-07-build-tmp-hygiene.md` に付記 1 段落。

## 証拠
| コマンド | 結果 |
|---|---|
| `cargo nextest run -p task-worker --lib tmpdir_long_path`（userns 不要） | 4 passed |
| 修正前 `CELERIS_USERNS_TESTS=1 TMPDIR=/tmp/abcdefghijklmnopqrstuvwxyz0123 cargo nextest run -p task-api --test browser_h3_injection production_h3` | FAILED（`H3 run result: Other("isolated_runtime_unavailable")`、再現） |
| 修正後 同 TMPDIR `cargo nextest run -p task-api --test browser_h3_injection` | 3 passed |
| 同 TMPDIR `cargo nextest run -p task-api --test browser_e2e` | 4 passed |
| 同 TMPDIR `cargo nextest run -p task-worker real_browser` | 5 passed |
| `TMPDIR=/tmp/celeris-test-parallel.XXXXXX/tmp` で `browser_h3_injection` | 3 passed |

この host の sandbox では userns（`unshare --user`）が使えたので上記は実機の userns 試験。release gate では `CELERIS_USERNS_TESTS=1` の `.gate-cargo-test.log` で `production_h3_injects_once_without_exposure` の pass を確かめる。

## 全体検査
- `bash scripts/dev/test-parallel.sh`: exit 0、4714 passed / 0 failed、tmp_leftovers 0
- `cargo clippy --workspace -- -D warnings`・`cargo clippy -p task-worker --tests -- -D warnings`: exit 0。`cargo fmt --all -- --check`・文書検査（layout・links・adr-numbers）: exit 0

## 未解決
- 無し。credentiald の `control.sock` 等は runtime dir（本番は XDG_RUNTIME_DIR 系）にあり今回の失敗とは無関係。
