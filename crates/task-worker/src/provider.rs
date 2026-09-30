//! 供給側失敗の文字列分類（ADR-0010 D5）。`claude-code`/`codex` のエラー文面（結果メッセージ・
//! `stderr` 末尾）を、決定的な部分一致規則で `ProviderFailure` に写す純粋関数。LLM を呼ばない・
//! ネットワークを見ない（原則: ディスパッチャ/アダプタに協調判断の LLM 呼び出しを入れない）。

use crate::protocol::ProviderFailure;

const EXHAUSTED_PATTERNS: &[&str] = &["usage limit", "quota", "credit balance"];
const THROTTLED_PATTERNS: &[&str] = &["rate limit", "rate_limit", "overloaded"];
const THROTTLED_CODES: &[&str] = &["429", "529"];
const AUTH_FAILED_PATTERNS: &[&str] = &[
    "invalid api key",
    "authentication",
    "not logged in",
    "/login",
];
const AUTH_FAILED_CODES: &[&str] = &["401"];

/// `text` を大文字小文字を無視した部分一致で分類する。判定順は Exhausted → Throttled → AuthFailed
/// （`usage limit` のような文言が `rate limit` 等と紛れないよう、より具体的な枯渇を先に見る）。
/// どれにも当たらなければ `None`（例: モデル非対応のような celeris 側では対処しようのないエラー。
/// ADR-0010 D5）。
///
/// HTTP ステータス（`429` 等）は独立したトークンとして、かつ「ステータスの文脈」にあるときだけ一致
/// させる（前後が英数字・`.`・`:` でない、かつ `HTTP 529` / `status:429` / `Error 529` / `(529)` /
/// `401 Unauthorized` のようにステータスであることを示す語や記法が前後にある）。stderr 末尾の
/// スタックトレースに含まれる `cli.js:4291:17` のような位置情報や、Python の rich トレースバックに
/// 現れる `pypdf/_page.py:529 in __getitem__` のようなファイル名:行番号を供給側失敗と誤分類し、
/// attempts を消費しない requeue を無期限に繰り返すのを防ぐ（Phase 7 監査の指摘、Phase 34 実機の
/// U34-1 / P-87）。
pub fn classify_provider_failure(text: &str) -> Option<ProviderFailure> {
    let lower = text.to_lowercase();
    if EXHAUSTED_PATTERNS.iter().any(|p| lower.contains(p)) {
        return Some(ProviderFailure::Exhausted);
    }
    // コード（`429` 等）の判定は数字そのものは大文字小文字の影響を受けないので、`has_status_context`
    // の「直後が大文字始まりの語」判定のために元の大文字小文字を保った `text` をそのまま渡す。
    if THROTTLED_PATTERNS.iter().any(|p| lower.contains(p))
        || THROTTLED_CODES.iter().any(|c| contains_code(text, c))
    {
        return Some(ProviderFailure::Throttled {
            retry_after_secs: 60,
        });
    }
    if AUTH_FAILED_PATTERNS.iter().any(|p| lower.contains(p))
        || AUTH_FAILED_CODES.iter().any(|c| contains_code(text, c))
    {
        return Some(ProviderFailure::AuthFailed);
    }
    None
}

/// `code` が独立したトークンとして、かつステータスの文脈で現れるか。
///
/// まず前後が英数字・`.` なら数字列の一部とみなす。`:` は位置情報（`file.js:429:17` の `:17`、
/// `12:429` の `12:`）を作る場合、つまり `:` の向こう側が数字のときだけ一部とみなす。加えて、`:` の
/// 手前が拡張子付きのファイル名（`pypdf/_page.py:529` のような `[\w/.-]+\.\w{1,5}:` の形）のときも
/// 一部とみなす（rich トレースバックの `file.py:529 in __getitem__` を弾く。U34-1 / P-87）。
///
/// 独立したトークンだとわかっても、それだけでは供給側の HTTP ステータスとは限らない（単なる版番号や
/// 行番号のこともある）。そこで前後に「ステータスの文脈」（`HTTP` / `status` / `error` の直後、
/// `(529)` のような括弧内、`401 Unauthorized` のようにステータス番号の直後に大文字始まりの語が続く）
/// があるときだけ一致とみなす。
fn contains_code(text: &str, code: &str) -> bool {
    let is_joined = |c: char| c.is_ascii_alphanumeric() || c == '.';
    text.match_indices(code).any(|(start, _)| {
        let end = start + code.len();
        let mut before = text[..start].chars().rev();
        let mut after = text[end..].chars();
        let joined_before = match before.next() {
            Some(':') => {
                before.next().is_some_and(|c| c.is_ascii_digit())
                    || ends_with_file_path(&text[..start - 1])
            }
            Some(c) => is_joined(c),
            None => false,
        };
        let joined_after = match after.next() {
            Some(':') => after.next().is_some_and(|c| c.is_ascii_digit()),
            Some(c) => is_joined(c),
            None => false,
        };
        !joined_before && !joined_after && has_status_context(text, start, end)
    })
}

/// `prefix`（コロンの手前までの文字列）が、拡張子付きのファイルパス（`pypdf/_page.py` や
/// `cli.js` のような、末尾が `.` + 1〜5 文字の単語文字になっている非空白トークン）で終わっているか。
fn ends_with_file_path(prefix: &str) -> bool {
    let token = prefix
        .rsplit(|c: char| c.is_whitespace() || c == '(' || c == '"' || c == '\'' || c == ',')
        .next()
        .unwrap_or("");
    match token.rfind('.') {
        Some(dot_idx) => {
            let ext = &token[dot_idx + 1..];
            let base = &token[..dot_idx];
            !base.is_empty()
                && !ext.is_empty()
                && ext.len() <= 5
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
                && base
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '/' | '-' | '.'))
        }
        None => false,
    }
}

/// `text[start..end]`（数字列そのもの）の前後に、HTTP ステータスであることを示す文脈があるか。
/// - 直前（空白・`:`・`=` を除いた最後の単語）が `http` / `status` / `error`
/// - 直前が `(`（丸括弧の中の番号、例: `(529)`）
/// - 直後が空白を挟んで大文字始まりの単語（例: `401 Unauthorized`、`529 Too Many`）
fn has_status_context(text: &str, start: usize, end: usize) -> bool {
    let prefix = text[..start].trim_end_matches([' ', '\t', ':', '=']);
    if prefix.ends_with('(') {
        return true;
    }
    let last_word: String = prefix
        .rsplit(|c: char| !c.is_ascii_alphanumeric())
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(last_word.as_str(), "http" | "status" | "error") {
        return true;
    }
    let suffix = text[end..].trim_start_matches([' ', '\t']);
    let mut chars = suffix.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_uppercase())
}

/// ADR-0054 D1（Phase 67）: `context.session` で resume を頼んだ run が、そのセッションを
/// アダプタに拒否された（見つからない・失効した）ように見えるか。`claude-code` の crash 分類
/// （`stderr` 末尾）や `codex` の crash 分類に使う決定的な部分一致（大文字小文字を無視）。
///
/// Phase 113（ADR-0054 追記。実機観測 2026-09-23、Claude Code CLI）: 本番のタスク
/// 01M35X86XTK84F97QW0CN5PGMR の reviewer run（01M388BENASH3JEBWFS03KEQYT）の stderr そのまま:
/// `No conversation found with session ID: 01a0d017-e32a-4cad-b10c-0cb63869ae13`。Phase 67 時点の
/// 一覧は「実機で確認していない」想定文（`No conversation found for session …`）しか書いておらず、
/// 実際の文言（`with session ID: <uuid>`）は「no conversation found」を含む部分一致なので幸い当たって
/// いた（`RESUME_REJECTION_PATTERNS` は変わらず有効）。壊れていたのは呼び出し側の配線（この関数では
/// ない。`claude_code.rs::run_claude_code` が `result` メッセージを観測できた run ではこの関数を一切
/// 呼んでいなかった。Phase 113 D1 で直した）。この追記では、将来の文言の揺れに備えて
/// `could not resume` を加え、「session ... not found」のように間に id が挟まる形は
/// [`looks_like_resume_rejection`] 側でギャップ許容の判定も行う。
const RESUME_REJECTION_PATTERNS: &[&str] = &[
    "no conversation found",
    "session not found",
    "no session found",
    "unknown session",
    "invalid session",
    "session does not exist",
    "session has expired",
    "could not find session",
    "resume: not found",
    "could not resume",
];

/// `resume` を頼んだ run のエラー文面（`stderr` 末尾・結果メッセージ）が
/// [`RESUME_REJECTION_PATTERNS`] のどれかを含むか、または「session」の後に近接して「not found」が
/// 現れるか（`session 01a0d017-… not found` のように間に id が挟まる形。Phase 113）。
/// 呼び出し側（`resume` を頼んでいたときだけ）で使う。
pub fn looks_like_resume_rejection(text: &str) -> bool {
    let lower = text.to_lowercase();
    RESUME_REJECTION_PATTERNS.iter().any(|p| lower.contains(p))
        || contains_gap_pattern(&lower, "session", "not found", 80)
}

/// `before` の出現位置の直後、`max_gap` バイト以内に `after` が現れるか（`lower` は小文字化済み前提）。
/// `session 01a0d017-e32a-4cad-b10c-0cb63869ae13 not found` のように、`before`/`after` の間に
/// 可変長の id や語句が挟まる文面を、固定文字列の部分一致だけでは拾えないために使う（Phase 113）。
fn contains_gap_pattern(lower: &str, before: &str, after: &str, max_gap: usize) -> bool {
    let mut search_from = 0;
    while let Some(pos) = lower[search_from..].find(before) {
        let start = search_from + pos + before.len();
        let window_end = (start + max_gap).min(lower.len());
        // `String` を byte index で切ると char 境界を壊すことがあるので、境界まで縮める。
        let mut end = window_end;
        while end < lower.len() && !lower.is_char_boundary(end) {
            end += 1;
        }
        if lower[start..end.max(start)].contains(after) {
            return true;
        }
        search_from = search_from + pos + before.len();
    }
    false
}

/// Phase 98（ADR-0054 追記。実機観測 2026-09-22 00:18 UTC、codex-cli 0.155.1）:
/// `codex exec resume <id>` が「セッションが見つからない」（[`RESUME_REJECTION_PATTERNS`]）のではなく、
/// **`exec resume` の JSON-RPC メソッド自体を実装していない**ことを示す文言。実機で観測した stderr
/// そのまま: `Error: thread/resume: thread/resume failed: list_turns is not supported yet
/// (code -32601)`。`RESUME_REJECTION_PATTERNS` とは別に扱う理由: こちらは「このセッションは resume
/// できない」ではなく「このインストールの codex は resume を一切できない」ので、同じセッションで
/// 何度リトライしても直らない（celeris 側は fresh セッションへ切り替えるしかない）。
const RESUME_RPC_UNSUPPORTED_PATTERNS: &[&str] = &["thread/resume", "-32601", "resume"];

/// `resume` を頼んだ run が、イベントを一つも出さずに終わったときの crash 分類（`codex.rs::run_codex`
/// が呼ぶ）。[`RESUME_RPC_UNSUPPORTED_PATTERNS`] のどれかを含むか（大文字小文字を無視）。
pub fn looks_like_resume_rpc_failure(text: &str) -> bool {
    let lower = text.to_lowercase();
    RESUME_RPC_UNSUPPORTED_PATTERNS
        .iter()
        .any(|p| lower.contains(p))
}

/// ADR-0054 Phase 67b 追記: Claude Code CLI 2.1.278 は `--session-id`/`--resume` に渡す id が
/// **UUID**（`xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`、16 進数 32 桁 + ハイフン 4 個）でなければ拒否する
/// （`Error: Invalid session ID. Must be a valid UUID.` / `--resume requires a valid session ID or
/// session title ... is not a UUID`）。Phase 67 は celeris 側で `ulid::Ulid::new().to_string()`
/// （`01M323X6TJQSFEP0MKXABWVY78` のような ULID）を渡していたため、本番のすべての CoS 対話・部門長
/// レビュー run が失敗した（2026-09-21 13:53 UTC 観測）。
///
/// 形式だけを見る決定的な判定（`-` の位置と 16 進数であることだけを見る。version/variant ビットの
/// 厳密な検査はしない — celeris が発行する id は Phase 67b で UUID v4 だが、アダプタ（Claude Code 自身）
/// が別の版の UUID を返す可能性を将来にわたって閉じないため）。大文字・小文字は問わない
/// （Claude Code 自身が返す id の大文字小文字は未確認）。
pub fn is_valid_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
}

#[cfg(test)]
mod tests;
