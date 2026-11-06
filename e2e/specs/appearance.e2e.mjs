import { $, browser, expect } from "@wdio/globals";

const bg = (sel) => browser.execute((s) => getComputedStyle(document.querySelector(s)).backgroundColor, sel);
const until = (sel, rgb) =>
  browser.waitUntil(async () => (await bg(sel)) === rgb, { timeoutMsg: `${sel} never became ${rgb}` });

describe("appearance", () => {
  it("Dark draws the D48 tokens and Light draws D38's", async () => {
    await $("#settings-button").click();
    await $("#appearance").selectByAttribute("value", "dark");
    await until("body", "rgb(32, 32, 32)"); // window #202020
    expect(await bg("#settings-view .det")).toBe("rgb(43, 43, 43)"); // pane #2B2B2B
    await $("#appearance").selectByAttribute("value", "light");
    await until("body", "rgb(244, 245, 248)"); // window #F4F5F8
    expect(await bg("#settings-view .det")).toBe("rgb(255, 255, 255)");
  });

  it("the choice survives a restart", async () => {
    await $("#appearance").selectByAttribute("value", "dark");
    await until("body", "rgb(32, 32, 32)");
    await browser.reloadSession(); // quits and relaunches the app with the same folders
    await until("body", "rgb(32, 32, 32)");
    await $("#settings-button").click();
    await expect($("#appearance")).toHaveValue("dark");
  });
});
