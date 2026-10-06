---
tasks: [01M47QXZR0QMCYZM9KAZC81BCD]
---
# launcher egress allow の origin 変換

`SessionPolicy.allowed_domains` は現在、`https://example.com` や `http://localhost:8123` のような origin を受け取る。古い grant 由来の bare host は `https://host`、port 443 として読む。daemon 経路は `browser_policy::origin_host_port` を使い、origin を egress proxy 用の `host:port` に変換する。

修正前の launcher は全要素へ `:443` を追加していた。単体試験 `launcher_egress_uses_origin_scheme_and_port` を先に追加して実行したところ exit 101。実際の allow は `https://example.com:443`、`http://localhost:443`、`http://127.0.0.1:8123:443`、`https://billing.example.com:8443:443`、`legacy.example.com:443` となり、期待した `host:port` と一致しなかった。この試験は userns や実 launcher を使わず、backend が組み立てる `EgressPolicy` を直接検査する。

launcher も daemon と同じ `origin_host_port` を使うようにした。試験の対象は HTTPS 既定 443、HTTP 既定 80、HTTP/HTTPS の明示 port、legacy bare host である。なお、`check_egress` は IP literal と private address を別途拒否する。この変更は allow の形式を正し、loopback への実通信を解放する変更ではない。実通信の可否は Fable の実機再確認で判定する。

修正後の確認: `cargo test -p task-worker --lib launcher_egress_uses_origin_scheme_and_port -- --nocapture` は 1 passed、`cargo test -p task-worker --lib browser_policy::tests -- --nocapture` は 6 passed。`cargo fmt --all -- --check`、`cargo clippy -p task-worker --lib -- -D warnings`、`bash scripts/dev/progress-index.sh --check` は exit 0。

## 人が行う host launcher 入れ替え

このコードだけでは host の `/usr/local/libexec/celeris/celeris-browser-launcher` は更新されない。統合した commit の作業ツリー `W` で release build し、同じ binary の hash を配置前後で照合する。稼働中の browser session が終わった後、root が socket と service を止めて入れ替える。詳しい host 前提と復旧手順は [launcher の host 手順](../../../docs/ops/browser-launcher-admission-evidence-run.md)を参照する。本 run では以下を実行していない。

```sh
# 通常の host shell。W は修正を統合した作業ツリー。
cd "$W"
git rev-parse HEAD
cargo build --release -p task-worker --bin celeris-browser-launcher
T=$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
sha256sum "$T/release/celeris-browser-launcher"

# 同じ host shell で続ける。既存 session が残っていれば終了を待つ。
L=/usr/local/libexec/celeris/celeris-browser-launcher
pgrep -u celeris-browser -a
sha256sum "$L"
sudo install -o root -g root -m 0755 "$L" "$L.pre-egress-fix"
sudo systemctl stop celeris-browser-launcher.socket celeris-browser-launcher.service
sudo install -o root -g root -m 0755 "$T/release/celeris-browser-launcher" "$L"
sha256sum "$L" # build 済み binary と一致すること
sudo systemctl start celeris-browser-launcher.socket
systemctl status celeris-browser-launcher.socket --no-pager
```

配置後、Fable が更新台本で実 session を再確認する。異常時は socket と service を止め、退避した `$L.pre-egress-fix` を同じ mode/owner で `$L` に戻してから socket を起動する。
