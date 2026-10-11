---
title: "credentiald の launcher UID を config に一本化"
tasks: [01M4NM5722YETPEYN74JR82ERD]
status: done
updated: 2026-10-11
---

# credentiald の launcher UID を config に一本化

## 現在地

2026-10-11 完了: install-units による手動 UID 引数の消失を修正し、全体検査が合格。
[ADR 付記](../adr/2026-10-09-browser-launcher-credential-release.md#付記-2026-10-11-launcher-uid-の正本と-unit-更新)に従い、
UID は daemon と同じ config.toml の `[browser].launcher_uid` のみに置く。
本番操作は実施していない。

## 変更

- credentiald は `serve --config <path> <daemon-pid>...` を必須とし、起動時に UID を読む。
  旧 `--launcher-uid` を撤去。不正 config・不正 UID は固定エラーで拒否する。
- unit は daemon と同じ config path を明示。home/hot root の両方で配布する。
- install-units は全 unit の差分を先に表示し、相違があれば全書込み前に停止する。
  差分確認後の `--replace-existing` だけが backup 付き置換を行う。mask/symlink は置換指定でも拒否する。
- migrate-to-local の root 切替は既存の全 unit backup・失敗時復旧の下で置換を明示する。
- unit 生成・手動引数付き旧 unit の保護、共通 config の読込み、生成 ExecStart による
  実 broker 起動、config の変更追従と不正値拒否を回帰試験で固定する。

## 本番への移行

以下は運用セッションが実行する。DB・events・vault・鍵は変更しない。UID はこの host では 995 と報告されているが、
実行時の `id -u celeris-browser` を正として照合する。新旧 CLI に互換経路はないため、unit と release を同時に切り替える。

1. 修正を含む checkout で release/verify を完了し、昇格可能な SHA を用意する。
   旧 `current/scripts` ではなく修正済み checkout の台本を使う。
   稼働 browser session が無く、credential 利用 task を再開しない時間帯に行う。

   ```sh
   W=<修正済み checkout の絶対パス>
   NEW=<検査済み新 release の sha12>
   STATE=/local/celeris/state
   OLD=$(basename "$(readlink -f "$STATE/current")")
   U="$HOME/.config/systemd/user"
   C="$HOME/.config/celeris/config.toml"
   SAVE="$HOME/.config/celeris/credentiald-unit-migration-$(date +%Y%m%d-%H%M%S)"
   install -d -m 0700 "$SAVE"
   cp -p "$C" "$SAVE/config.toml"
   cp -a "$U" "$SAVE/units"
   printf '%s\n' "$OLD" >"$SAVE/old-release"
   id -u celeris-browser
   systemctl --user cat "celeris@$OLD.service" "celeris-credentiald@$OLD.service"
   ```

2. `$C` の既存 `[browser]` 節を編集し、`launcher_uid = 995`（上で照合した値）を一度だけ置く。
   本番には既にあるため、一致していれば編集不要。0 や daemon UID を使わない。
   `runtime = "launcher"` と `launcher_socket` は既存値を維持する。
   config を編集した場合は `CELERIS_STATE_DIR="$STATE" bash "$W/scripts/selfdeploy/verify.sh" "$NEW"` を再実行する。
   秘密を含む config 全体をログへ出さない。

3. 先に差分を表示する。相違があれば非 0 で止まり、unit はまだ変わらない。
   daemon と credentiald の config path が同じで、削除される運用設定の行き先があることを確認する。
   drop-in に旧 UID 引数や ExecStart 上書きがあれば、退避済みであることを確認して新形式へ編集する。
   drop-in を含む最終形も `systemctl --user cat` で確認する。

   ```sh
   CELERIS_STATE_DIR="$STATE" bash "$W/scripts/selfdeploy/install-units.sh"
   # 差分を確認し、broker を止めてから明示的に置換する。
   systemctl --user stop "celeris-credentiald@$OLD.service"
   CELERIS_STATE_DIR="$STATE" bash "$W/scripts/selfdeploy/install-units.sh" --replace-existing
   systemctl --user cat "celeris@$NEW.service" "celeris-credentiald@$NEW.service"
   ```

   ExecStart は `serve --config .../config.toml` を含み、`--launcher-uid` が無いこと。
   新 unit で旧 broker を起動しない。自動 restart や rollback も旧 unit の復元後に行う。

4. 通常の [selfdeploy](../../docs/ops/selfdeploy.md)手順で昇格し、新 daemon の PID を得た後で broker を起動する。

   ```sh
   CELERIS_STATE_DIR="$STATE" bash "$W/scripts/selfdeploy/promote.sh" "$NEW"
   systemctl --user is-active "celeris@$NEW.service"
   systemctl --user restart "celeris-credentiald@$NEW.service"
   systemctl --user is-active "celeris-credentiald@$NEW.service"
   systemctl --user show "celeris-credentiald@$NEW.service" -p ExecStart -p MainPID
   "$STATE/releases/$NEW/bin/celerisctl" browser doctor
   ```

   `promote.sh` の完了と active daemon の release を確認してから broker を起動する。
   doctor の接続確認だけでは注入成功の証明にならないため、[credential 解放手順](../../docs/ops/browser-launcher-credential-release.md)の
   admission/台帳を確認し、既存の対象 task で人の承認付き CredentialUse が成功することを確認する。
   config の launcher UID を変更した場合も daemon と broker の双方を再起動する。

### 戻し方

失敗時は credential 利用 task を止め、新 broker を停止する。退避した config と unit（変更した drop-in も）を復元して
`daemon-reload` し、旧 release を rollback する。旧 CLI の引数は旧 unit の backup から戻す。新しい UID 設定経路を併存させない。

```sh
systemctl --user stop "celeris-credentiald@$NEW.service"
cp -p "$SAVE/config.toml" "$C"
cp -a "$SAVE/units/." "$U/"
# 今回新規作成した drop-in があれば個別に取り除く（旧 backup に存在しないもののみ）。
systemctl --user daemon-reload
# 昇格前に失敗して current が旧 release のままなら rollback は不要。
if [ "$(basename "$(readlink -f "$STATE/current")")" != "$OLD" ]; then
  test "$(basename "$(readlink -f "$STATE/previous")")" = "$OLD" || exit 1
  CELERIS_STATE_DIR="$STATE" bash "$W/scripts/selfdeploy/rollback.sh" || exit 1
fi
systemctl --user is-active "celeris@$OLD.service"
systemctl --user restart "celeris-credentiald@$OLD.service"
systemctl --user is-active "celeris-credentiald@$OLD.service"
```

vault の鍵を再初期化しない。今回の変更に DB migration はないため、DB を巻き戻さない。

## 検証

ログは task 成果物ディレクトリ
`/local/celeris/data/workspaces/01M4NM5722YETPEYN74JR82ERD/artifacts/` に保存。

- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh`: exit 0。
  `CELERIS_TEST_SUMMARY`: `passed=5169, failed=0, ignored=14, nextest_exit=0, doctest_exit=0`。
  nextest 5,166件（173 binaries）+ doc-test 3件。既定の opt-in/ignored は維持し、実 host の
  launcher 注入試験を実施したとは扱わない。`test-parallel.log`。
  長い run TMPDIR による Unix socket path 超過を避け、fixture の TMPDIR のみ `/tmp` に設定。
  CARGO_TARGET_DIR・コンパイラ設定は渡された値のまま。試験の残存一時項目4件は台本が削除した。
- `TMPDIR=/tmp cargo test -p celeris-credentiald --bin celeris-credentiald --test installed_unit --test browser_doctor --test broker`:
  exit 0、24件合格（初期の局所検証）。その後追加した旧 unit の置換ケースも上の全体試験で合格。
  `credentiald-tests.log`。
- `bash scripts/selfdeploy/tests/install_units_hot_dir.sh`: exit 0。
  手動 UID 引数を含む旧 unit の差分拒否・無変更、明示置換の backup・config 引数、再実行、
  home/hot root・paths.env、mask 保護を検証。`install-units.log`。
- `for test in scripts/selfdeploy/tests/*.sh; do bash "$test" || exit; done` 相当の逐次実行:
  exit 0、25本合格（`SUMMARY: 25 selfdeploy scripts passed`）。root 移行の自動復旧も合格。
  `selfdeploy-tests.log`。
- `cargo fmt --all --check`、`sh scripts/dev/check-doc-links.sh`、
  `sh scripts/dev/check-adr-numbers.sh`、`sh scripts/dev/progress-index.sh --check`:
  すべて exit 0。`fmt.log`・`doc-links.log`・`adr-numbers.log`・`progress-check.log`。
- `cargo clippy --workspace -- -D warnings`: exit 0、警告なし。
  `Finished dev profile [unoptimized + debuginfo] target(s) in 1m 21s`。`clippy.log`。

## 未解決事項

コード・回帰試験に未解決事項なし。本番への適用と承認付き CredentialUse の確認は、上記手順を運用セッションが実施する。
