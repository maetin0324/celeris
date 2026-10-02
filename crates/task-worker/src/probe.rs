//! OpenAI 互換エンドポイントの**到達性の検査**（ADR-0052 D1。Phase 64）。
//!
//! 知識整理 run（`knowledge` ハーネス = `langmem` アダプタ）の設定された接続先を、dispatch の
//! **直前**に `GET <base_url>/models` で検査する（ADR-0132 D4）。通常は proxy の到達性を見て、
//! 届かなければ tier `cheap` の汎用ハーネスへ倒す（ADR-0052 D2）。proxy に届く限り、
//! 個別の LLM source の障害と fallback は proxy が処理する。
//!
//! **LLM は呼ばない**（CLAUDE.md「ディスパッチャやストアに LLM 呼び出しを入れない」）。ここがやるのは
//! ADR-0043 D3 のコンテナ runtime の probe と同じ種類の、決定的な 1 回の HTTP GET だけ。
//!
//! 依存を増やさないため、`std::net::TcpStream` に最小限の HTTP/1.1 を自分で書く
//! （リクエストは 1 行 + ヘッダ、応答はステータス行だけ読む）。`http://` だけを見る:
//! `https://` や書き方の壊れた `base_url` は [`Reachability::Unknown`] にして、**従来どおり**
//! `langmem` で走らせる（検査できないことを「落ちている」と決めつけない）。
//!
//! `[llm_proxy]`（ADR-0053）を `[knowledge.langmem].base_url` に向けたとき、
//! `GET /v1/models` は Bearer トークンが無いと 401 を返す（`/healthz` を除く全エンドポイントが
//! 認証を要求する）。401/403 は接続先の到達不能を意味しない
//! （トークンが未設定・不一致というだけ）ので、[`Reachability::Unreachable`] にせず
//! [`Reachability::Unknown`]（= 従来どおり `langmem` で走らせる）にする。呼び出し側が
//! `[knowledge.langmem].api_key_secret` から解決した平文のトークンを渡せば、`Authorization: Bearer`
//! ヘッダを付けて検査する。**トークンの値はどのログにも出さない**。

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// ADR-0052 D1: 検査の制限時間（3 秒）。
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// ADR-0052 D1: 検査の結果をキャッシュする時間（60 秒。tick ごとに叩かない）。
pub const PROBE_CACHE_TTL: Duration = Duration::from_secs(60);

/// ステータス行を読むときの上限（これを超えたら壊れた応答とみなす）。
const MAX_STATUS_LINE: usize = 512;

/// [`probe_models`] の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reachability {
    /// `GET <base_url>/models` が 2xx を返した。
    Ok,
    /// 接続できない・時間切れ・2xx 以外。`reason` は人が読む 1 行（進行イベントに出す）。
    Unreachable { reason: String },
    /// 検査できない（`base_url` が無い・`https://`・書き方が壊れている）。**従来どおり**扱う。
    Unknown { reason: String },
}

impl Reachability {
    /// フォールバックすべきか（`Unreachable` のときだけ）。
    pub fn should_fall_back(&self) -> Option<&str> {
        match self {
            Reachability::Unreachable { reason } => Some(reason),
            Reachability::Ok | Reachability::Unknown { .. } => None,
        }
    }
}

/// `base_url` を `(host, port, path)` に分解する（`http://host[:port][/path]`）。
fn split_http_url(base_url: &str) -> Result<(String, u16, String), String> {
    let trimmed = base_url.trim();
    let rest = match trimmed.strip_prefix("http://") {
        Some(rest) => rest,
        None if trimmed.starts_with("https://") => {
            return Err("https は検査しない".to_string());
        }
        None => return Err("http:// で始まっていない".to_string()),
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].trim_end_matches('/').to_string()),
        None => (rest, String::new()),
    };
    if authority.is_empty() {
        return Err("ホストが空".to_string());
    }
    // IPv6 のリテラル（`[::1]:8000`）も読めるようにする。
    let (host, port) = if let Some(end) = authority.strip_prefix('[').and_then(|a| a.find(']')) {
        let host = &authority[1..=end];
        match authority[end + 2..].strip_prefix(':') {
            Some(p) => (host, p),
            None => (host, ""),
        }
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h, p),
            None => (authority, ""),
        }
    };
    let port: u16 = if port.is_empty() {
        80
    } else {
        port.parse()
            .map_err(|_| format!("ポートが数でない: {port}"))?
    };
    if host.is_empty() {
        return Err("ホストが空".to_string());
    }
    Ok((host.to_string(), port, path))
}

/// ADR-0052 D1（Phase 65b で `bearer_token` を追加）: `GET <base_url>/models` を `timeout` で
/// 1 回だけ当てる。`bearer_token` があれば `Authorization: Bearer <token>` を付ける
/// （celeris の `llm-proxy` のように、`/healthz` 以外の全エンドポイントが認証を要求する上流のため）。
///
/// ネットワーク I/O はここだけ。返るのは決定的な 3 値（[`Reachability`]）で、判断は呼び出し側
/// （`task_dispatch::Dispatcher`）がする。**`bearer_token` の値はログに出さない。**
pub fn probe_models(base_url: &str, timeout: Duration, bearer_token: Option<&str>) -> Reachability {
    let (host, port, path) = match split_http_url(base_url) {
        Ok(parts) => parts,
        Err(reason) => return Reachability::Unknown { reason },
    };
    let target = format!("{path}/models");
    let deadline = Instant::now() + timeout;

    let remaining = |deadline: Instant| deadline.saturating_duration_since(Instant::now());
    let addrs = match (host.as_str(), port).to_socket_addrs() {
        Ok(addrs) => addrs.collect::<Vec<_>>(),
        Err(e) => {
            return Reachability::Unreachable {
                reason: format!("名前を引けない: {e}"),
            };
        }
    };
    let Some(addr) = addrs.into_iter().next() else {
        return Reachability::Unreachable {
            reason: "名前に対応する住所が無い".to_string(),
        };
    };

    let left = remaining(deadline);
    if left.is_zero() {
        return Reachability::Unreachable {
            reason: "時間切れ（接続する前）".to_string(),
        };
    }
    let mut stream = match TcpStream::connect_timeout(&addr, left) {
        Ok(s) => s,
        Err(e) => {
            return Reachability::Unreachable {
                reason: format!("接続できない: {e}"),
            };
        }
    };
    let left = remaining(deadline);
    if left.is_zero() {
        return Reachability::Unreachable {
            reason: "時間切れ（接続の直後）".to_string(),
        };
    }
    if stream.set_read_timeout(Some(left)).is_err() || stream.set_write_timeout(Some(left)).is_err()
    {
        return Reachability::Unreachable {
            reason: "ソケットの時間切れを設定できない".to_string(),
        };
    }
    let request = match bearer_token {
        Some(token) => format!(
            "GET {target} HTTP/1.1\r\nHost: {host}:{port}\r\nAccept: */*\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
        ),
        None => format!(
            "GET {target} HTTP/1.1\r\nHost: {host}:{port}\r\nAccept: */*\r\nConnection: close\r\n\r\n"
        ),
    };
    if let Err(e) = stream.write_all(request.as_bytes()) {
        return Reachability::Unreachable {
            reason: format!("要求を送れない: {e}"),
        };
    }
    let _ = stream.flush();

    // ステータス行（`HTTP/1.1 200 OK`）だけ読む。本文は読まない。
    let mut line = Vec::with_capacity(64);
    let mut byte = [0u8; 1];
    loop {
        if remaining(deadline).is_zero() {
            return Reachability::Unreachable {
                reason: "応答が時間内に来ない".to_string(),
            };
        }
        match stream.read(&mut byte) {
            Ok(0) => {
                return Reachability::Unreachable {
                    reason: "応答が無いまま閉じられた".to_string(),
                };
            }
            Ok(_) => {
                if byte[0] == b'\n' {
                    break;
                }
                if byte[0] != b'\r' {
                    line.push(byte[0]);
                }
                if line.len() > MAX_STATUS_LINE {
                    return Reachability::Unreachable {
                        reason: "応答のステータス行が長すぎる".to_string(),
                    };
                }
            }
            Err(e) => {
                return Reachability::Unreachable {
                    reason: format!("応答を読めない: {e}"),
                };
            }
        }
    }
    let status_line = String::from_utf8_lossy(&line).trim().to_string();
    let code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse::<u16>().ok());
    match code {
        Some(code) if (200..300).contains(&code) => Reachability::Ok,
        // Phase 65b: 401/403 は「到達性が無い」ではなく「認証が合っていない」。LLM が落ちている
        // わけではないので、フォールバックさせない（従来どおり langmem で走らせる）。
        Some(code @ (401 | 403)) => Reachability::Unknown {
            reason: format!("HTTP {code}（認証エラーは到達性の欠落として扱わない）"),
        },
        Some(code) => Reachability::Unreachable {
            reason: format!("HTTP {code}"),
        },
        None => Reachability::Unreachable {
            reason: format!("応答が HTTP ではない: {status_line}"),
        },
    }
}

#[cfg(test)]
mod tests;
