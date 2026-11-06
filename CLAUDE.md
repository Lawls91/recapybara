# Meeting capture

The plan lives outside this repo, in Jack's Obsidian vault. Read it before working on a ticket:

- Design (decisions D1-D50, testing, out of scope):
  `C:\Users\alex\OneDrive - Contoso\Notebooks\my-notebook\Projects\Meeting capture\Meeting capture 2 Design.md`
- Outline (17 tickets in dependency order, each with acceptance criteria):
  `...\Projects\Meeting capture\Meeting capture 3 Outline.md`
- Research (the evidence the decisions rest on):
  `...\Projects\Meeting capture\Meeting capture 1 Research.md`
- UI mockups (visual reference only; decisions D38-D50 are the requirement):
  `...\Projects\Meeting capture\assets\mockups\`

Rules that hold for every ticket:

- **No real meeting content in the repo** (D34). Test fixtures are synthetic VTT. Real transcripts are
  used only in live checks on Jack's machine.
- **Frozen identifiers never change** (D50): Tauri identifier `com.lawls91.meetingcapture`, skill
  folder `meeting-capture`, `MEETING_CAPTURE_*` env vars, the Run-key value name. The product name is
  the single `productName` value.
- **Automation first** (Testing Decisions): every behaviour not tied to something external gets an
  automated test (`cargo test`, Vitest, WebdriverIO E2E with the fake `claude`). `(live)` criteria are
  for external facts only.
- **Local first**: `npm run check` (all suites plus the 80% coverage gate) is the verdict until CI lands in Outline ticket 18.
- **Never call the real Claude in tests or CI**; tests use the fake `claude` binary (D36).
