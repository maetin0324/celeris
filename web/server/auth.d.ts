import type { Express, Request } from "express";
import type { OriginPolicy } from "./origin.js";

export const SESSION_COOKIE_NAME: string;
export const SESSION_MAX_AGE_SECONDS: number;
export const FAILED_LOGIN_DELAY_MS: number;
export function safeNextPath(value: unknown): string;
export function createAuth(options?: {
  passwordFile?: string;
  secretFile?: string;
  now?: () => number;
  origins?: OriginPolicy;
  failedDelayMs?: number;
}): {
  enabled: boolean;
  register(app: Express): void;
  authenticated(req: Request): boolean;
  sessionKey(req: Request): string | null;
  sessionExpiresAt(req: Request): number | null;
  onLogout(listener: (sessionKey: string | null) => void): void;
  issue(): string;
  isValid(token: unknown): boolean;
  verifyPassword(candidate: unknown): boolean;
};
