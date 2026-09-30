import { expect, it } from "vitest";
import { screens, v3Screens } from "./screens";

it("selects marked screens and picks up a newly marked screen", () => {
  expect(v3Screens().map((screen) => screen.path)).toEqual(
    screens.filter((screen) => "v3" in screen && screen.v3).map((screen) => screen.path),
  );
  expect(
    v3Screens(screens.map((screen) => (screen.path === "/projects" ? { ...screen, v3: true } : screen))).map(
      (screen) => screen.path,
    ),
  ).toEqual(
    screens
      .filter((screen) => ("v3" in screen && screen.v3) || screen.path === "/projects")
      .map((screen) => screen.path),
  );
});

it("keeps every screen path unique", () => {
  expect(new Set(screens.map((screen) => screen.path)).size).toBe(screens.length);
});
