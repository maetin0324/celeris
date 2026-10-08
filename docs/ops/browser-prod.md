# browser 適合台帳の生成・配置・検査（運用手順）

---
tasks: [01M4CH1MPV1KB69SYKXS79E0SP]
---

browser task を本番で動かすには、その release 用の適合台帳（`conformance.json`）が要る。設計は
[ADR 2026-10-08-browser-prod-enablement](../../agent-docs/adr/2026-10-08-browser-prod-enablement.md) D1・D2。
本番 host の操作（release dir への書き込み・daemon の再起動）は人が実行する。

前提の点検と不足項目の修正、運用セッションの有効化手順は [本番 browser 実行の有効化と点検](browser-prod-enablement.md)を参照。

## 1. release が台帳を作って置く流れ

- `scripts/selfdeploy/release.sh` の `browser-ledger` 段（`lib.sh` の `sd_browser_ledger`）が、release の組み立て中に台帳を作る。
  1. host の `agent-browser --version` を確認（無い・版違いなら台帳を作らず理由だけ残す）。
  2. `scripts/browser-conformance.py --protocol-scripted --fallback-scenario --celeris-release <sha12>` で公開能力の台帳を作る。実 LLM は要らない。実 agent-browser は要る。
  3. `--p4b-evidence`（backend `claude-code`・`browser-specialist`）で credential 注入の証拠を足す。失敗しても公開能力だけの台帳は残る（`credential_use` を含む task は止まる）。
  4. `celerisctl browser ledger check` で検査し、通れば `browser.partial` を `browser/` に rename する。
- 置き場所: `~/.local/celeris/releases/<sha12>/browser/conformance.json`。結果は同じ dir の `ledger-status.json`、release の `manifest.json`・`gate.json` の `browser_ledger {ok, code}`、ログは `gate-logs/browser-ledger.log`。
- 段は非 blocking（上限 `SD_BROWSER_LEDGER_TIMEOUT`、既定 1800 秒）。生成に失敗しても release は失敗しない。
- `promote.sh` も台帳が無くて止まらない。`promoted.json` とログに `browser_ledger {ok, code}`（無ければ `missing`）を写す。
- rollback しても、その release 自身の台帳が使われる。

## 2. daemon への渡し方

台帳の path は daemon 起動時に次の順で決まる。

1. env `CELERIS_BROWSER_CONFORMANCE_FILE`（試験・開発用の上書き。空は無い扱い）
2. `--release <sha12>` で動くとき: 実行ファイルの `../browser/conformance.json`（release dir）
3. どちらも無ければ未配置（daemon は起動する）

path は tick ごとに `(mtime, size)` だけを見直すので、台帳を置き換えても daemon の再起動は要らない。

## 3. 台帳が使えないときの止まり方（ledger-gate）

browser task だけが dispatch の直前に `blocked`（reason `browser_prerequisite`）で止まり、`BrowserPrerequisiteBlocked` event と受信箱に code と文が出る。infra_requeue は繰り返さない。台帳が有効になれば自動で `ready` に戻る。

| code | 意味 | 直し方 |
|---|---|---|
| `missing` | 未配置 | 新しい release を作るか §4 |
| `invalid` | 壊れている（読めない・64 KiB 超・symlink・schema 不一致など） | §4 で作り直す |
| `stale_release` | 別の release 用（`generated_for` の無い旧台帳を含む） | §4 |
| `stale_agent_browser` | agent-browser の版が台帳と違う | host の版を合わせ §4 |
| `agent_browser_missing` | PATH に agent-browser が無い | 導入してから §4 |
| `no_conformant_backend` | certify された公開 backend が無い | §4 の出力を確認 |
| `ledger_lacks_credential` | task が `credential_use` を要するが注入の証拠が無い | P4-B 証拠付きで §4 |

## 4. 人が台帳を作る・作り直す（本番 host で人が実行）

release を作り直さずに台帳だけ作り直す:

```sh
# 今の台帳が有効なら何もしない。--force で強制。
bash scripts/selfdeploy/browser-ledger.sh <sha12> [--force]
```

build worktree を `<sha12>` に合わせて `sd_browser_ledger` を回し、成功すれば `releases/<sha12>/browser/` を置き換える。失敗時は既存の台帳を残して exit 1。daemon は再起動なしに拾う。

`celerisctl` に台帳を生成するコマンドは無い。あるのは検査だけ:

```sh
celerisctl browser ledger check --file ~/.local/celeris/releases/<sha12>/browser/conformance.json \
  --release <sha12> [--agent-browser <path>] [--no-host-probe] [--json]
```

- `ok` は exit 0、台帳の不備はすべて exit 3。`--json` は `{ok, code, message, release, agent_browser, backends, credential_backends}` の 1 行。
- `--release` を省くと release の比較をしない。`--no-host-probe` は host の agent-browser の実行を省く。DB・daemon 接続は不要。

確認: 上の `check` が exit 0、`releases/<sha12>/browser/ledger-status.json` が `ok: true`、`celerisctl browser doctor` の `ledger`・`ledger-backends` が OK（`credential` が空なら公開 task のみ動く）。

## 5. 実機確認（人）

昇格後、manaba の task を 1 回通す（資格情報が人のものなので人が行う）: owner session → web で task を起票 → credential form → 承認。成否は task 詳細の browser 節・受信箱・`celerisctl browser doctor` で見る。
