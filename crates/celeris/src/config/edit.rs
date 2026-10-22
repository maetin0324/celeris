//! ADR 2026-10-22-dynamic-config D3: config.toml の本文を、コメントと並びを保ったまま 1 欄ずつ書き換える。
//! 欄の指定は `.` 区切りの path（`execution.max_replans`）。値は JSON で受け、`null` は欄を消す。

use serde_json::Value;
use toml_edit::{DocumentMut, Item, Table, TableLike};

/// `text` の `key` を `value` にした本文を返す（`value` が `null` なら欄を消す）。
pub fn set_field(text: &str, key: &str, value: &Value) -> Result<String, String> {
    let mut doc = text
        .parse::<DocumentMut>()
        .map_err(|e| format!("config.toml cannot be edited: {e}"))?;
    let path: Vec<&str> = key.split('.').collect();
    if path.iter().any(|s| s.is_empty()) {
        return Err(format!("invalid key: {key:?}"));
    }
    let (leaf, parents) = path
        .split_last()
        .ok_or_else(|| format!("invalid key: {key:?}"))?;
    let mut table: &mut dyn TableLike = doc.as_table_mut();
    for (i, segment) in parents.iter().enumerate() {
        if value.is_null() && !table.contains_key(segment) {
            return Ok(doc.to_string()); // 消す欄が元から無い
        }
        if !table.contains_key(segment) {
            let mut new = Table::new();
            new.set_implicit(true);
            table.insert(segment, Item::Table(new));
        }
        table = match table.get_mut(segment).and_then(Item::as_table_like_mut) {
            Some(t) => t,
            None => {
                return Err(format!("{} is not a table", parents[..=i].join(".")));
            }
        };
    }
    if value.is_null() {
        table.remove(leaf);
        return Ok(doc.to_string());
    }
    let item = to_item(value)?;
    match (table.get_mut(leaf), item) {
        (Some(Item::Value(old)), Item::Value(mut new)) => {
            *new.decor_mut() = old.decor().clone();
            *old = new;
        }
        (_, item) => {
            table.insert(leaf, item);
        }
    }
    Ok(doc.to_string())
}

/// JSON の値を TOML の `Item` にする（TOML に null は無い。配列の中の null は拒否）。
fn to_item(value: &Value) -> Result<Item, String> {
    let toml_value = toml::Value::try_from(value)
        .map_err(|e| format!("the value cannot be written as TOML: {e}"))?;
    let mut wrapper = toml::Table::new();
    wrapper.insert("v".into(), toml_value);
    let text = toml::to_string(&wrapper).map_err(|e| e.to_string())?;
    let doc = text.parse::<DocumentMut>().map_err(|e| e.to_string())?;
    doc.get("v")
        .cloned()
        .ok_or_else(|| "the value cannot be written as TOML".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const BASE: &str = "# 先頭のコメント\ndb = \"/x/db\"\n\n[execution]\n# replan の上限\nmax_replans = 5 # 本番\ngate = \"on\"\n\n[cos]\nmax_turns = 3\n";

    #[test]
    fn replacing_a_value_keeps_comments_and_order() {
        let out = set_field(BASE, "execution.max_replans", &json!(8)).unwrap();
        assert!(out.contains("# 先頭のコメント"));
        assert!(out.contains("# replan の上限"));
        assert!(out.contains("max_replans = 8 # 本番"), "{out}");
        assert!(out.find("[execution]").unwrap() < out.find("[cos]").unwrap());
    }

    #[test]
    fn new_nested_key_creates_tables_and_null_removes() {
        let out = set_field(BASE, "execution.tree.enabled", &json!(true)).unwrap();
        assert!(out.contains("enabled = true"), "{out}");
        let removed = set_field(&out, "execution.tree.enabled", &Value::Null).unwrap();
        assert!(!removed.contains("enabled"));
        assert_eq!(
            set_field(BASE, "jev.enabled", &Value::Null).unwrap(),
            BASE,
            "removing an absent key changes nothing"
        );
    }

    #[test]
    fn arrays_of_tables_and_bad_paths() {
        let out = set_field(
            BASE,
            "roles",
            &json!([{"id": "a", "max_turns": 3}, {"id": "b", "max_turns": 4}]),
        )
        .unwrap();
        assert_eq!(out.matches("[[roles]]").count(), 2, "{out}");
        assert!(set_field(BASE, "db.x", &json!(1)).is_err());
        assert!(set_field(BASE, "a..b", &json!(1)).is_err());
        assert!(set_field(BASE, "x", &json!([null])).is_err());
    }
}
