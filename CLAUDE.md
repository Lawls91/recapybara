# Recapybara

Rules for anyone changing this repo, people and coding agents alike:

- **No real meeting content in the repo.** Test fixtures are synthetic. Real transcripts are used only in
  checks run by hand on a real machine, and never committed.
- **Fixed identifiers never change**: the Tauri identifier `com.lawls91.recapybara`, the skill folder
  `recapybara`, the `RECAPYBARA_*` env vars, the Run-key value name `Recapybara`, the exe `recapybara.exe` and
  the MSI UpgradeCode (installer/wix/Package.wxs). Renaming them breaks upgrades, toasts or uninstall. The
  product name is the single `productName` value.
- **Automation first**: every behaviour not tied to something external gets an automated test (`cargo test`,
  Vitest, WebdriverIO E2E with the fake `claude`). Only external facts are checked by hand.
- **Local first**: `npm run check` (all suites plus the 80% coverage gate) is the local verdict. CI
  (`.github/workflows/ci.yml`) runs it on every PR and push to main, with E2E in its own job (de-elevated, see
  docs/development.md, CI).
- **Never call the real Claude in tests or CI**; tests use the fake `claude` binary.
