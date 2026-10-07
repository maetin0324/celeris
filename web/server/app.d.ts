import type { Express } from "express";

export type Bind = { host: string; port: number };
export function parseBind(value?: string): Bind;
export function isLoopback(host: string): boolean;
export function readPasswordFile(file?: string): string | null;
export function validateConfig(config: { bind: Bind; passwordFile?: string }): void;
export function createApp(options?: {
  bind?: Bind;
  passwordFile?: string;
  allowedHosts?: string;
  distDir?: string;
  release?: string;
  secretFile?: string;
  failedLoginDelayMs?: number;
  daemonUrl?: string;
  daemonTokenFile?: string;
  relayTimeoutMs?: number;
  liveUpstream?: string;
  attestationKeyFile?: string;
  ownerSocket?: string;
  probe?: boolean;
  now?: () => number;
  log?: (entry: { path: string; status: number; ms: number }) => void;
  registerRoutes?: (app: Express) => void;
}): Express;
