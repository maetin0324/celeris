import { createFileRoute } from "@tanstack/react-router";
import { SkillsScreen } from "../features/knowledge/skills-screen";
import { optionalString } from "../lib/search";
export const Route = createFileRoute("/knowledge/skills")({
  validateSearch: (search: Record<string, unknown>): { create?: string; name?: string; edit?: string } => ({
    create: optionalString(search.create),
    name: optionalString(search.name),
    edit: optionalString(search.edit),
  }),
  component: SkillsScreen,
});
