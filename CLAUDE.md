# Meeting capture

The plan lives outside this repo, in the owner's Obsidian vault; its paths are in CLAUDE.local.md (not committed). Read it before working on a ticket.

Rules that hold for every ticket:

- **No real meeting content in the repo** (D34). Test fixtures are synthetic VTT. Real transcripts are
  used only in live checks on Jack's machine.
- **Frozen identifiers never change** (D50): Tauri identifier `com.lawls91.meetingcapture`, skill
  folder `meeting-capture`, `MEETING_CAPTURE_*` env vars, the Run-key value name. The product name is
  the single `productName` value.
- **Automation first** (Testing Decisions): every behaviour not tied to something external gets an
  automated test (`cargo test`, Vitest, WebdriverIO E2E with the fake `claude`). `(live)` criteria are
  for external facts only.
- **Local first**: `npm run check` (all suites plus the 80% coverage gate) is the local verdict; CI (`.github/workflows/ci.yml`, Outline ticket 18) runs it on every PR and push to main, with E2E in its own job (de-elevated, see README CI).
- **Never call the real Claude in tests or CI**; tests use the fake `claude` binary (D36).
