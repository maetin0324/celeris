//! Framing for test HTTP/1.x clients (Content-Length; chunked is deliberately rejected).
#![allow(dead_code)]

pub const NOT_FOUND: &[u8] =
    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
pub const LIMIT: usize = 1024 * 1024;

pub fn complete(bytes: &[u8]) -> Result<Option<usize>, ()> {
    if bytes.len() > LIMIT {
        return Err(());
    }
    let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") else {
        return Ok(None);
    };
    let head = std::str::from_utf8(&bytes[..end]).map_err(|_| ())?;
    let mut lines = head.split("\r\n");
    let mut first = lines.next().ok_or(())?.split_whitespace();
    let method = first.next().ok_or(())?;
    let path = first.next().ok_or(())?;
    let version = first.next().ok_or(())?;
    if !method.bytes().all(|b| b.is_ascii_uppercase())
        || !path.starts_with('/')
        || !matches!(version, "HTTP/1.0" | "HTTP/1.1")
        || first.next().is_some()
    {
        return Err(());
    }
    let mut length = None;
    for line in lines {
        let (key, value) = line.split_once(':').ok_or(())?;
        if key.eq_ignore_ascii_case("transfer-encoding") {
            return Err(());
        }
        if key.eq_ignore_ascii_case("content-length") {
            if length.is_some() {
                return Err(());
            }
            length = Some(value.trim().parse::<usize>().map_err(|_| ())?);
        }
    }
    let size = (end + 4).checked_add(length.unwrap_or(0)).ok_or(())?;
    if size > LIMIT {
        return Err(());
    }
    Ok((bytes.len() >= size).then_some(size))
}

pub fn matches(raw: &str, method: &str, path: &str) -> bool {
    let mut words = raw.split_whitespace();
    words.next() == Some(method) && words.next() == Some(path)
}
