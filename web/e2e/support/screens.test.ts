import { expect, it } from "vitest";
import { screens, v3Screens } from "./screens";

it("selects marked screens and picks up a newly marked screen", () => {
  expect(v3Screens().map((screen) => screen.path)).toEqual(["/inbox", "/tasks"]);
  expect(
    v3Screens(screens.map((screen) => (screen.path === "/projects" ? { ...screen, v3: true } : screen))).map(
      (screen) => screen.path,
    ),
  ).toEqual(["/inbox", "/projects", "/tasks"]);
});

it("keeps every screen path unique", () => {
  expect(new Set(screens.map((screen) => screen.path)).size).toBe(screens.length);
});
