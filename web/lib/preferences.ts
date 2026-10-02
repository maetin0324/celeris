// 表示の好みの保存（P2-06・H4・ADR-0081 D2）。localStorage に保存するのは {timeZone, theme} だけ。
// token・API 応答・本文・ログ・下書きは入れない。読むときに検証し、未知の項目と不正な値は捨てる。

export const PREFERENCES_KEY = "celeris.web.display";
export const THEMES = ["system", "light", "dark"] as const;
export type Theme = (typeof THEMES)[number];
export type Preferences = { timeZone?: string; theme?: Theme };

export function isValidTimeZone(value: unknown): value is string {
  if (typeof value !== "string" || value === "" || value.length > 64) return false;
  try {
    new Intl.DateTimeFormat("en", { timeZone: value });
    return true;
  } catch {
    return false;
  }
}

export function sanitizePreferences(raw: unknown): Preferences {
  const out: Preferences = {};
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) return out;
  const record = raw as Record<string, unknown>;
  if (isValidTimeZone(record.timeZone)) out.timeZone = record.timeZone;
  if (typeof record.theme === "string" && (THEMES as readonly string[]).includes(record.theme))
    out.theme = record.theme as Theme;
  return out;
}

function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

export function readPreferences(): Preferences {
  const s = storage();
  if (!s) return {};
  try {
    const text = s.getItem(PREFERENCES_KEY);
    return text === null ? {} : sanitizePreferences(JSON.parse(text));
  } catch {
    return {};
  }
}

export function writePreferences(next: Preferences): Preferences {
  const clean = sanitizePreferences(next);
  const s = storage();
  if (s) {
    try {
      if (Object.keys(clean).length === 0) s.removeItem(PREFERENCES_KEY);
      else s.setItem(PREFERENCES_KEY, JSON.stringify(clean));
    } catch {
      // 保存できなくても表示は続ける。
    }
  }
  return clean;
}
