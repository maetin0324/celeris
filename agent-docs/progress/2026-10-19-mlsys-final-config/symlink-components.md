---
task: 01M58VFRBA6HG0SSF3CG5BKM91
wu: symlink-components
status: done
completed: 2026-10-19
---
# symlink-components: component ごとの symlink 展開

- `crates/campaign/src/snapshot.rs` の `check_inner_symlink` を、component ごとの実体解決（symlink はその場で再帰展開、`..` は展開後の実体に適用、各段階で root 内、展開 40 回上限）に書き換えた。hash_tree / copy_tree / isolation の `copy_inner_symlink` は共通経路でこれを使う。
- 試験 `crates/campaign/tests/inner_symlink.rs`: reviewer の `alias -> .` + `link -> alias/../tree/a/real.txt` を hash・copy 双方で拒否、入れ子 alias 経由の途中脱出・file 通過を拒否、安全な入れ子・連鎖は成功し複製後の全 link が複製 root 内に閉じる。
- K2 正解資産・リンク字面・既存 hash は変更なし。ADR `2026-10-19-campaign-inner-symlink.md` を実装に合わせた。
- 実 K2 の通し検証は main-config-verify が担当。
