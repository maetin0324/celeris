//! RFC 4180 の CSV（書き出しと読み戻し）。comma・二重引用符・改行を含むセル（JSON の object など）を quoting して
//! 列数を崩さない。外部 crate を足さず、export の表と試験の読み戻しで同じ規則を使う。

/// 1 セル。comma・`"`・CR・LF を含むか、前後に空白があれば `"` で囲み、`"` は `""` にする。
pub fn field(s: &str) -> String {
    let needs = s.contains([',', '"', '\n', '\r']) || s.starts_with(' ') || s.ends_with(' ');
    if needs {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// 1 行（末尾 CRLF なしの LF 改行つき）。
pub fn line<S: AsRef<str>>(cells: &[S]) -> String {
    let mut out = cells
        .iter()
        .map(|c| field(c.as_ref()))
        .collect::<Vec<_>>()
        .join(",");
    out.push('\n');
    out
}

/// CSV 全体を行・セルへ読む。quote 内の改行・`""` を戻す。閉じない quote・quote 後の余計な文字はエラー。
pub fn parse(text: &str) -> Result<Vec<Vec<String>>, String> {
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut chars = text.chars().peekable();
    let mut quoted = false;
    let mut after_quote = false;
    let mut any = false;
    while let Some(c) = chars.next() {
        any = true;
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    cell.push('"');
                } else {
                    quoted = false;
                    after_quote = true;
                }
            } else {
                cell.push(c);
            }
            continue;
        }
        match c {
            '"' if cell.is_empty() && !after_quote => quoted = true,
            ',' => {
                row.push(std::mem::take(&mut cell));
                after_quote = false;
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
                after_quote = false;
                any = false;
            }
            _ if after_quote => return Err(format!("unexpected {c:?} after a closing quote")),
            _ => cell.push(c),
        }
    }
    if quoted {
        return Err("unterminated quoted field".into());
    }
    if any {
        row.push(cell);
        rows.push(row);
    }
    Ok(rows)
}
