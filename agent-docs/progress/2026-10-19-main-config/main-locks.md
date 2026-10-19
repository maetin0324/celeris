---
tasks: [01M5A76P7RHE7TGBE14XGE6YJR]
status: done
completed: 2026-10-19
---
# main-locks: 実 root に env-lock/3 と campaign lock を生成・照合

## 実施
- check-main-pins.py: lock 内容検証（A binary path/sha256、worker hash、B/C の Sonnet-only・Jev 無効・C 呼出し 0、cap_env 一致、K2 commit、両 lock の campaign_sha256 一致）。test_check_main_pins.py に lock 改変・hash 不一致・binary 経路不一致の fixture 試験（29 件）。
- 承認済み実証跡 pinned/evidence/main-llm-models.json（sha256 c9babb76…45ab6、人の承認済み）で `celeris-campaign offline-lock` を実行。
  - 承認済み pinned/bin の binary は offline-lock 追加前の build で subcommand が無いため、同 repo の同 crate を `cargo build -p celeris-campaign` した binary で実行した（pin は変更していない）。
  - `--out` は空 dir が必須（既存 data は lock に取り込まない）なので、空の pinned/lock-gen に生成し、env-lock.json・lock.json を pinned/main/ に複写。pinned/main を `--out` にした `--verify` が verified（campaign_lock / env_lock とも verified、paid_calls_made 0、campaign_sha256 8ca0e198…a999、lock_sha256 311714ae…70f9）。
- `check-main-pins.py --root <artifacts>`: exit 1、不足は pinned/main/evidence/k2-preflight.json のみ（env-lock.json・lock.json は missing に無い）。

## 未解決（operator）
- 実 K2 preflight は測定 daemon の起動が要る。operator が測定 daemon（pinned/main/daemon.toml）を起動して preflight を実行し pinned/main/evidence/k2-preflight.json を保存する。この task では daemon・実 LLM・watch を行っていない。
