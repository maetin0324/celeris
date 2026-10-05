import type { Express, Request } from "express";

export const SESSION_COOKIE_NAME: string;
export const SESSION_MAX_AGE_SECONDS: number;
export const FAILED_LOGIN_DELAY_MS: number;
export function safeNextPath(value: unknown): string;
export function createAuth(options?: {
  passwordFile?: string;
  secretFile?: string;
  now?: () => number;
  failedDelayMs?: number;
}): {
  enabled: boolean;
  register(app: Express): void;
  authenticated(req: Request): boolean;
  sessionKey(req: Request): string | null;
  onLogout(listener: (sessionKey: string | null) => void): void;
  issue(): string;
  isValid(token: unknown): boolean;
  verifyPassword(candidate: unknown): boolean;
};
