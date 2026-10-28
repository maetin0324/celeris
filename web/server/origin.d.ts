import type { IncomingMessage } from "node:http";

export type OriginPolicy = {
  publicOrigins: string[];
  hosts(): string[];
  selfOrigin(req: IncomingMessage): string;
  secure(req: IncomingMessage): boolean;
};
export function parsePublicOrigins(value?: string): string[];
export function isLoopbackAddress(address: unknown): boolean;
export function createOriginPolicy(options?: { publicOrigins?: string[] }): OriginPolicy;
