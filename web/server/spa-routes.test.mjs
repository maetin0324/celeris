import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { isSpaRoute, spaRoutePatterns } from "./spa-routes.js";

test("spa route patterns match the declared route tree", () => {
  const tree = readFileSync(new URL("../routeTree.gen.ts", import.meta.url), "utf8");
  const block = /export interface FileRoutesByFullPath \{([^}]*)\}/.exec(tree)?.[1] ?? "";
  const declared = [...block.matchAll(/'([^']+)'/g)].map((m) => (m[1] === "/" ? "/" : m[1].replace(/\/$/, "")));
  assert.deepEqual([...declared].sort(), [...spaRoutePatterns].sort());
  assert.equal(spaRoutePatterns.length + 1, 37, "36 declared screens + the 404");
});

test("isSpaRoute accepts declared paths and rejects the rest", () => {
  for (const ok of ["/", "/tasks", "/tasks/", "/tasks/T1", "/tasks/T1/runs/R1", "/projects/p/docs/maintenance"])
    assert.equal(isSpaRoute(ok), true, ok);
  for (const ng of ["/no-such-page", "/tasks/T1/unknown", "/tasks/T1/runs", "/org/a/b", "/tasks/.."])
    assert.equal(isSpaRoute(ng), false, ng);
});
