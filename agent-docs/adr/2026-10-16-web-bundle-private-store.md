# ADR 2026-10-16: web 配布物の pnpm store は専用の空 store に prod 依存だけを fetch する

- 日付: 2026-10-16
- 状態: 採用・実装済み
- 関連: ADR-0135（web release の依存保証）、2026-10-11-breaking-changes-by-default

## 背景
2026-10-16、`release.sh` の web-bundle 段が 3 回中 2 回落ちた（04:17Z・77ba8dd4: `tar: dist: Cannot mkdir`・
`.pnpm-store/v11/index.db: Cannot open`・`Skipping to next header`、05:42Z・99a7967d: offline prod install が
`ERR_PNPM_NO_LOCKFILE`）。どちらも再実行では通った。

`web/scripts/release.sh` は `corepack pnpm store path` の共有 store（`/local/.pnpm-store/v11`、344M・約 1.6 万 file、
index.db は SQLite）を `cp -a` で配布物へ丸写ししていた。この store は host 全体で共有され、worker・reviewer の
`pnpm install --frozen-lockfile` が同時に書く。tarball の約 1.7 万 entry のうち 1.68 万が store だった。

## 原因（証拠）
- 共有 store は host の全 pnpm が書く可変物。2 回目の失敗時刻 05:42 と store ディレクトリの mtime（10/16 05:42）が一致する。
  つまり丸写しの最中（または直前）に別の pnpm が store を更新していた。
- 丸写しは SQLite の index.db と files/ を別々の時点で読むので、壊れた/不整合な写しが配布物に入る。
  tar はその巨大な tarball（entry の 99% が store）を展開するため、壊れた入力に当たる確率が配布物の大きさに比例する。
- 失敗の log そのものは gate-logs に残っておらず（失敗時の log は FAILED_DIR にあるが回収されていない）、
  同時書き込みの瞬間を事後に再生はできない。CPU を焼く再現は禁止なので行っていない。
  根拠は上記の構造（共有・可変・丸写し・mtime の一致）である。丸写しを無くせば同じ競合は構造上起こらない。

## 決定
- D1: `web/scripts/release.sh` は共有 store を写さない。`$STAGE/.pnpm-store`（`pnpm-workspace.yaml` の
  `storeDir`）を空から作り、`pnpm fetch --prod` で配布物の lockfile が要る prod 依存だけを入れ、続けて
  `install --prod --offline --frozen-lockfile` で store が自足していることをその場で確かめる。
- D2: 旧い共有 store の `cp -a` は消した。互換 flag は無い。
- D3: `bundle_web`（selfdeploy/release.sh）は変えない。展開・offline install・server import の検査は従来どおり。

## 効果
tarball は約 1.7 万 entry から約 7.6 千 entry（11MB）。共有 store を読まないので他の pnpm と競合しない。
fetch は network を使う（release の web 段は既に `pnpm install` で network を使う）。

## 移行
config 欄の削除は無い。本番への移行作業は無い。
