---
tasks: [01M4Q6GQV76NM3EGZ3P6EN378J]
---
# assign-drop progress

- Added schema migration 0069 to drop `model_role_assignments` and `model_role_scopes`; historical assignment events remain supported and readable.
- Removed model role assignment types, storage APIs, daemon application, and model catalog `assigned_tiers`.
- Confirmed routing shadow records use task/run correlation with source/model candidates and do not include a provider ID.
- Regenerated `docs/api/v1/api-v1.schema.json` and confirmed it matches the generated API schema.
- Web and GUI generated types and model assignment UI cleanup are left to the dependent `gen-types` WorkUnit; their premature changes caused the previous scope check failure and were removed from this unit's diff.
- Added a store test that reads a historical `model_role_assignment_changed` event after migration 0069 removes the assignment tables.
