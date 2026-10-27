source_url: https://github.com/nanaism/yomiyasu/tree/41e5aec228f080fe24e5f795efee2ac3fda61839/skills/yomiyasu
commit: 41e5aec228f080fe24e5f795efee2ac3fda61839 (v2.4.0)
fetched: 2026-10-27
license: MIT
license_file: LICENSE
modified / excluded: `assets/algo-artis.png`（作者の画像。SKILL.md から参照されない）と `.codex-plugin/plugin.json`（Codex の plugin 登録用。Celeris は skill を自前で届ける）を写していない。ほかのファイルは upstream のまま。

## Notes

- 写したもの: `skills/yomiyasu/` の `SKILL.md`・`references/`・`scripts/`・`LICENSE`・`UNICODE-LICENSE.txt`。
  SKILL.md・scripts/yomiyasu_lint.py・scripts/yomiyasu_check.py・references/slop-catalog.md の sha256 が
  upstream の同じ commit の raw と一致することを取り込み時に確かめた。
- `UNICODE-LICENSE.txt` は scripts が使う Unicode のデータの license（upstream のまま）。
- scripts は Python 標準ライブラリだけで動く。Celeris の試験は `scripts/yomiyasu_lint.py` を
  ネットワークなしで呼ぶだけ（`crates/task-worker/tests/human_text_lint.rs`）。
- Celeris での使い方（どの run に何を載せるか）は
  [ADR 2026-10-27-yomiyasu-human-text](../../../agent-docs/adr/2026-10-27-yomiyasu-human-text.md)。
  人に返す文面の短い規則は prompt に入り、この skill の本体は CoS の run に mount して長い報告の推敲にだけ使う。
- upstream を更新するときは、この SOURCE.md の commit と、ADR の token の実測を一緒に直す。
