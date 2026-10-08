# Mobile tab bar implementation progress

- Changed the third mobile tab to `/board` (ボード) with the `kanban` icon; `/tasks` remains in `mobileOtherItems` by `navItems` order and activates その他.
- Updated tabbar E2E expectations for the new tab and verified the task entry in the その他 sheet has no badge.
- Diagnosed the prior E2E failure: the unread dot is intentionally hidden while an その他 route is active. The badge test had opened `/tasks`, which now belongs to その他. It now opens `/board` to test the dot in the inactive state.
- Existing first-screen checks cover `/board` at 360px and 1440px, including the 360px column strip.
- Check: `corepack pnpm@12.6.0 -C web e2e:all e2e/shell/ e2e/chat/ e2e/work/first-screen` — passed (83 tests).
