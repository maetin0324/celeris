---
tasks: [01M4G0GPW9XHJ0MC236F1R8FG9]
---
# browser launcher 経路の credential 解放手順（人が行う）

この手順は運用セッションで人が実行する。worker は root 操作、本番設定変更、資格情報の入力をしない。秘密値を shell history、ログ、台帳、成果物へ書かない。解放条件と fail-closed の境界は[決定記録](../../agent-docs/adr/2026-10-09-browser-launcher-credential-release.md)および[ADR-0138](../../agent-docs/adr/0138-browser-prod-admission-confidential-release.md)に従う。

## 1. 配送後の確認と launcher 入れ替え（root）

更新を含む release が配送・承認されてから実行する。launcher は task-worker の同じ release HEAD から作り、既存 binary を退避して hash を記録する。稼働 browser session がないことを確認し、socket/service を停止してから入れ替える。

```sh
W=<配送された agent-platform worktree>
L=/usr/local/libexec/celeris/celeris-browser-launcher
cd "$W"
git rev-parse HEAD
cargo build --release -p task-worker --bin celeris-browser-launcher
T=$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
sha256sum "$T/release/celeris-browser-launcher" "$L"
install -o root -g root -m 0755 "$L" "$L.pre-credential-release"
pgrep -u celeris-browser -a
systemctl stop celeris-browser-launcher.socket celeris-browser-launcher.service
install -o root -g root -m 0755 "$T/release/celeris-browser-launcher" "$L"
sha256sum "$L"
systemctl start celeris-browser-launcher.socket
systemctl status celeris-browser-launcher.socket --no-pager
```

新 binary の hash が build した値と一致することを確認する。続けて daemon の実行 user で launcher の必須実証を取り、結果を保護された運用記録へ保存する。出力に秘密は含めない。

```sh
cd "$W"
sh crates/task-worker/scripts/launcher-admission-evidence.sh --stutter 3 /tmp/launcher-admission-stutter.log
sh crates/task-worker/scripts/launcher-admission-evidence.sh --credential /tmp/launcher-credential-tests.log
```

両台本が exit 0 であることを確認する。stutter log は各回に `RUN[stutter-N] required launcher admission + SIGSTOP stutter`、`STUTTER[stutter-N]: stops=<n>`（`n` は 0 より大きい整数）、`EXIT[stutter-N]: 0` があり、3 回分の後の最終行が `EXIT: 0` であることを確認する。試験 group の終了と競合した signal の失敗（ESRCH）は失敗にしない（その回の停止は終わっており、合否は `stops` と試験の終了コードで決まる）。pgid を取れないうちに試験が終わった回は `stops=0` となり失敗する。ログの行形式は次のとおり（停止数は実行ごとに変わる）。

```text
RUN[stutter-1] required launcher admission + SIGSTOP stutter
STUTTER[stutter-1]: stops=81
EXIT[stutter-1]: 0
RUN[stutter-2] required launcher admission + SIGSTOP stutter
STUTTER[stutter-2]: stops=81
EXIT[stutter-2]: 0
RUN[stutter-3] required launcher admission + SIGSTOP stutter
STUTTER[stutter-3]: stops=82
EXIT[stutter-3]: 0
EXIT: 0
```

これは `sh crates/task-worker/scripts/tests/launcher-admission-evidence-stutter.sh` の実行で得た行形式の例である。各 stutter run に `ADMISSION[real-session]` と ptrace 拒否があること、launcher responder の PID/UID が service と `celeris-browser` に一致することも確認する。PID と UID は実行時に確認し、ログから人が読める保管先へ必要最小限の結果を写す。`LAUNCHER_EVIDENCE_TEST_CMD` は台本の試験専用 hook であり、本番手順では設定・使用しない。

## 2. 台帳を再取得

更新済み release の build worktree/HEAD を指定して、既存の selfdeploy 台帳生成経路を実行する。これは本番変更を伴うため運用セッションで実行する。

```sh
bash scripts/selfdeploy/browser-ledger.sh <sha12> --force
celerisctl browser ledger check --file ~/.local/celeris/releases/<sha12>/browser/conformance.json
celerisctl browser doctor
```

`ledger-status.json` が `ok: true`、`browser doctor` の ledger と backend が OK で、credential 証拠が launcher runtime として当該 backend に結び付いていることを確認する。証拠が欠落・失敗・daemon runtime 由来なら解放を中止する。台帳やログの内容を開いて credential 値を探さない。

## 3. policy と manaba task の再開

owner session を使い、対象 browser policy に `credential_use` capability を明示的に付与し、対象 backend と origin を必要最小限に制限する。既存 policy を更新する手段は管理 API/UI を使い、秘密値を policy に書かない。`celerisctl browser doctor` で policy/ledger/backend の状態を確認する。

続いて task `01M4FPA6ADFH639XYP894ES87J` を再開する。task の入力・credential prompt でのみ人が資格情報を入力し、通常の task log、chat、event に貼らない。credential 操作が承認待ちになった場合は owner session で内容を確認して一度だけ承認する。完了後、task 詳細・受信箱・browser doctor で成功を確認する。

## 4. 戻し方

どの確認でも不一致、拒否、秘密露出の疑いがあれば task を停止し、policy から `credential_use` を外してから launcher を戻す。root で session が終了したことを確認し、socket/service を止めて退避版を戻す。

```sh
pgrep -u celeris-browser -a
systemctl stop celeris-browser-launcher.socket celeris-browser-launcher.service
install -o root -g root -m 0755 "$L.pre-credential-release" "$L"
sha256sum "$L"
systemctl start celeris-browser-launcher.socket
systemctl status celeris-browser-launcher.socket --no-pager
```

hash が入れ替え前の記録と一致することを確認する。退避版が利用できない場合は、承認済みの直前 release から launcher を再 build して戻す。台帳を前の release SHA で再生成し、policy の `credential_use` が無効であることを確認する。原因を解消し、必須 admission 実証と台帳検査を再度通すまでは task を再開しない。
