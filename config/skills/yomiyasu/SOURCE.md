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
- scripts は Python 標準ライブラリだけで動く。Celeris の検査は `scripts/dev/check-human-text.py` が
  `scripts/yomiyasu_lint.py` をネットワークなしで呼ぶだけ（`scripts/dev/check-health.sh` から走る。ADR の D4）。
  Rust の試験からは呼ばない。
- Celeris での使い方（どの run に何を載せるか）は
  [ADR 2026-10-27-yomiyasu-human-text](../../../agent-docs/adr/2026-10-27-yomiyasu-human-text.md)。
  人に返す文面の短い規則（`task_worker::human_text`）だけを prompt に入れる。この skill の本体は CoS・worker の
  run に mount しない（ADR の D2）。長い報告を本体で推敲する使い方は、必要になった時に別に決める。
- upstream を更新するときは、この SOURCE.md の commit と、ADR の token の実測を一緒に直す。
