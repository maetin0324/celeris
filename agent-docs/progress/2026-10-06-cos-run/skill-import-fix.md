# CoS skill 取り込み試験の修正

tasks: [01M47HJ72B4A5FZPNHS5K1RW0J]

`ui_ux_skills_config_skills_import_into_a_temp_kb` は ui-ux の4件だけを `names` で指定して初回取り込みする一方、冪等性確認では空の `names` を渡して `config/skills` 全件を取り込んでいた。CoS 用 skill が追加されると再取り込みで新しい項目が入り、KB の HEAD が進むため `head_before` の比較に失敗する。

再取り込みにも初回と同じ `names` を渡すよう変更した。これにより、試験の対象を変えず「同一内容を再度取り込んでも HEAD が進まない」という意図を保つ。検索した `crates/` と `tests/` の他の `config/skills` 取り込み箇所は、ui-ux の名前を明示しており全件数に依存していなかった。`skills_list` の件数確認もこの一時 KB に限定されている。

## 確認

- `cargo test -p task-ops skill_import`
- `cargo test -p celeris --test ui_ux_skills_delivery`
