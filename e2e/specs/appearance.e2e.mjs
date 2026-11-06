import { $, browser, expect } from "@wdio/globals";

const bg = (sel) => browser.execute((s) => getComputedStyle(document.querySelector(s)).backgroundColor, sel);
const until = (sel, rgb) =>
  browser.waitUntil(async () => (await bg(sel)) === rgb, { timeoutMsg: `${sel} never became ${rgb}` });
const WINDOW = { dark: "rgb(32, 32, 32)", light: "rgb(244, 245, 248)" }; // D48 #202020, D38 #F4F5F8

describe("appearance", () => {
  beforeEach(async () => {
    if (await $("#settings-back").isDisplayed()) await $("#settings-back").click(); // start on the list
  });

  it("Dark draws the D48 tokens and Light draws D38's", async () => {
    await $("#settings-button").click();
    await $("#appearance").selectByAttribute("value", "dark");
    await until("body", WINDOW.dark);
    expect(await bg("#settings-view .det")).toBe("rgb(43, 43, 43)"); // pane #2B2B2B
    await $("#appearance").selectByAttribute("value", "light");
    await until("body", WINDOW.light);
    expect(await bg("#settings-view .det")).toBe("rgb(255, 255, 255)");
  });

  // Both choices, so one of them differs from whatever theme Windows itself is in.
  for (const choice of ["light", "dark"]) {
    it(`${choice} survives a restart`, async () => {
      await $("#settings-button").click();
      await $("#appearance").selectByAttribute("value", choice);
      await until("body", WINDOW[choice]);
      await browser.reloadSession(); // quits and relaunches the app with the same folders
      await until("body", WINDOW[choice]);
      await $("#settings-button").click();
      await expect($("#appearance")).toHaveValue(choice);
    });
  }
});
