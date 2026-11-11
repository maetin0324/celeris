//! The standing context of a CoS chat run (ADR 2026-11-11-cos-standing-context D1/D3): the role of
//! the thread, the delegated grants, the user pages, CoS's own memory page and the KB entry.
//! Pure: the same context gives the same bytes. Each section has a limit in characters; what does
//! not fit is dropped oldest first and replaced by one line saying how many were left out and where
//! to read them.

use crate::protocol::{CosChatContext, CosStandingContext, CosStandingPage, CosThreadRole};

/// D1 limits in characters, headings included. Their sum is [`TOTAL_MAX`].
pub const ROLE_MAX: usize = 600;
pub const GRANTS_MAX: usize = 3600;
pub const USER_MAX: usize = 3000;
pub const USER_PAGE_MAX: usize = 600;
pub const MEMORY_MAX: usize = 2000;
pub const KB_MAX: usize = 800;
/// The whole standing context except the counting line of D3.
pub const TOTAL_MAX: usize = ROLE_MAX + GRANTS_MAX + USER_MAX + MEMORY_MAX + KB_MAX;
/// The person's words quoted per grant.
const EXCERPT_MAX: usize = 200;
const NOTE_MAX: usize = 200;
/// Room kept in a section for its "ほか N 件" line.
const OMITTED_LINE_ROOM: usize = 120;

fn chars(s: &str) -> usize {
    s.chars().count()
}

/// The first `max` characters of `s` (one line), with `…` when cut.
fn clip(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if chars(&flat) <= max {
        return flat;
    }
    let mut out: String = flat.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The sections and what the counting line reports about them.
struct Rendered {
    text: String,
    items: usize,
    omitted: usize,
}

fn role_section(standing: &CosStandingContext, inbox_duty: bool) -> Rendered {
    let mut text = String::from("### 役割 (identity)\n");
    text.push_str(match standing.role {
        CosThreadRole::General => {
            "あなたは統括 CoS（人のただ 1 つの窓口）。案件の件には自分で答えず、人の原文を案件 CoS へ取り次ぐ。\
             人が期限や task を限らずに任せた指示（「今後は」「基本は」「いつも」）は 1 task・24 時間の取り次ぎに\
             縮めず、`authority.grant_propose`（根拠は人の発言の id）で人に 1 度確かめ、受け入れられたら下の grant になる。\n"
        }
        CosThreadRole::Project => {
            "あなたはこの案件の CoS。案件の件を決めて進める。下の grant が含む件は人に回さず、grant の id を根拠に\
             答える（`authority.check` で確かめる）。grant に無い件と人が決めると書いた件だけを統括 CoS へ上げる。\n"
        }
        CosThreadRole::Review => {
            "あなたは定期見直しの CoS（人の画面に出ない裏の thread）。変化の報告は `chat.review_report` だけで出す。\
             下の grant が含む件は人に回さない。\n"
        }
    });
    if inbox_duty {
        text.push_str(
            "受信箱の一次対応: 件ごとに先に下の grant を引き、含まれる件は grant の id を根拠に答える。\n",
        );
    }
    let text = clip_block(text, ROLE_MAX);
    Rendered {
        text,
        items: 1,
        omitted: 0,
    }
}

/// Keeps a multi-line block within `max` characters (cut at a character boundary, `…` at the end).
fn clip_block(text: String, max: usize) -> String {
    if chars(&text) <= max {
        return text;
    }
    let mut out: String = text.chars().take(max.saturating_sub(2)).collect();
    out.push_str("…\n");
    out
}

fn grants_section(standing: &CosStandingContext) -> Rendered {
    let mut text = String::from(
        "### 任された範囲 (authority grants, newest first)\n\
         人が受け入れた委任。正本は authority ledger（`GET /api/v1/projects/<id>/authority`）。範囲に入る件は人に回さず、\
         答えに grant の id と人の発言の出典を書く。\n",
    );
    if standing.grants.is_empty() {
        text.push_str("- （生きている grant は無い。任された範囲の件も人に確かめる）\n");
        return Rendered {
            text,
            items: 0,
            omitted: 0,
        };
    }
    let mut shown = 0;
    for g in &standing.grants {
        let expires = g.expires_at.as_deref().unwrap_or("なし（取り消すまで）");
        let remaining = g
            .budget_remaining
            .map_or_else(|| "制限なし".to_owned(), |n| format!("{n} 回"));
        let excerpt = g
            .source_excerpt
            .as_deref()
            .map(|e| format!("「{}」", clip(e, EXCERPT_MAX)))
            .unwrap_or_default();
        let line = format!(
            "- grant `{}`（案件 {}、範囲 {}:{}、分類 {}、期限 {expires}、残り {remaining}）根拠 {}{excerpt}: {}\n",
            g.id,
            g.project_id,
            g.scope_kind,
            g.scope_ref,
            g.action_class,
            g.source,
            clip(&g.note, NOTE_MAX),
        );
        if chars(&text) + chars(&line) + OMITTED_LINE_ROOM > GRANTS_MAX {
            break;
        }
        text.push_str(&line);
        shown += 1;
    }
    let omitted = standing.grants.len() - shown;
    if omitted > 0 {
        text.push_str(&format!(
            "- ほか {omitted} 件（古いもの。`GET /api/v1/projects/<id>/authority` で読む）\n"
        ));
    }
    Rendered {
        text,
        items: shown,
        omitted,
    }
}

fn user_section(standing: &CosStandingContext) -> Rendered {
    let mut text = String::from("### 利用者 (KB `user/`)\n");
    if standing.user_pages.is_empty() {
        text.push_str("- （KB に `user/` の page が無い）\n");
        return Rendered {
            text,
            items: 0,
            omitted: 0,
        };
    }
    let mut shown = 0;
    for page in &standing.user_pages {
        let block = format!(
            "- {} (`{}`): {}\n",
            page.title,
            page.path,
            clip(&page.body, USER_PAGE_MAX)
        );
        if chars(&text) + chars(&block) + OMITTED_LINE_ROOM > USER_MAX {
            break;
        }
        text.push_str(&block);
        shown += 1;
    }
    let omitted = standing.user_pages.len() - shown;
    if omitted > 0 {
        text.push_str(&format!(
            "- ほか {omitted} page（`GET /api/v1/knowledge/page?path=user/<name>.md` で読む）\n"
        ));
    }
    Rendered {
        text,
        items: shown,
        omitted,
    }
}

/// The tail of `cos/memory.md` (newest notes are appended at the end).
fn memory_section(memory: Option<&CosStandingPage>) -> Rendered {
    let mut text = String::from(
        "### CoS の覚え書き (KB `cos/memory.md`)\n\
         権限でない覚え書き（慣習・好み・道具の癖）。足すときは KB 候補にする（`POST /api/v1/knowledge/inbox`、scope `cos`。\
         curation か人の受け入れで入る）。権限は grant にする。\n",
    );
    let Some(page) = memory else {
        text.push_str("- （まだ無い）\n");
        return Rendered {
            text,
            items: 0,
            omitted: 0,
        };
    };
    let room = MEMORY_MAX.saturating_sub(chars(&text) + OMITTED_LINE_ROOM);
    let body = page.body.trim();
    let total = chars(body);
    let mut omitted = 0;
    if total > room {
        omitted = total - room;
        let tail: String = body.chars().skip(omitted).collect();
        text.push_str(&format!(
            "（古い {omitted} 字を省略。全文は `GET /api/v1/knowledge/page?path={}`）\n{tail}\n",
            page.path
        ));
    } else {
        text.push_str(body);
        text.push('\n');
    }
    Rendered {
        text,
        items: 1,
        omitted: usize::from(omitted > 0),
    }
}

fn kb_section(standing: &CosStandingContext) -> Rendered {
    let mut text = String::from(
        "### KB の入口 (knowledge, search when needed)\n\
         KB の索引はここに入れない。判断に要る知識（手順・過去の決定・環境）は答える前に検索して読む:\n\
         `GET /api/v1/knowledge/tree?q=<語>&scope=<scope>` で探し、`GET /api/v1/knowledge/page?path=<path>` で読む。\n",
    );
    let scopes = if standing.knowledge_scopes.is_empty() {
        "（cos の profile に knowledge の mount が無い）".to_owned()
    } else {
        standing
            .knowledge_scopes
            .iter()
            .map(|s| format!("`{s}`"))
            .collect::<Vec<_>>()
            .join("・")
    };
    text.push_str(&format!("- 検索してよい scope: {scopes}\n"));
    let text = clip_block(text, KB_MAX);
    Rendered {
        text,
        items: standing.knowledge_scopes.len(),
        omitted: 0,
    }
}

/// D1/D3: the standing context section of the run specific part.
pub fn standing_section(chat: &CosChatContext) -> String {
    let standing = &chat.standing;
    let role = role_section(standing, !chat.inbox_items.is_empty());
    let grants = grants_section(standing);
    let user = user_section(standing);
    let memory = memory_section(standing.memory.as_ref());
    let kb = kb_section(standing);
    let omitted = grants.omitted + user.omitted + memory.omitted;
    let mut out = String::from("## 常設の文脈 (standing context)\n");
    out.push_str(&format!(
        "<!-- celeris:cos-standing v1 role={} grants={}/{} user={}/{} memory={}/{} kb={}/{} omitted={omitted} -->\n",
        standing.role.as_str(),
        grants.items,
        chars(&grants.text),
        user.items,
        chars(&user.text),
        memory.items,
        chars(&memory.text),
        kb.items,
        chars(&kb.text),
    ));
    for section in [role, grants, user, memory, kb] {
        out.push_str(&section.text);
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests;
