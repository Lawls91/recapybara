import { describe, expect, it } from "vitest";
import { applyPlatformText, commandKey, isMac } from "./platform";

describe("platform", () => {
  it("knows a Mac from its webview's user agent", () => {
    expect(isMac("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)")).toBe(true);
    expect(isMac("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0 Edg/140.0")).toBe(false);
  });

  it("swaps in the Mac wording only on a Mac", () => {
    // A stand-in root: one element marked data-mac, one not (it is never selected).
    const root = () => {
      const marked = { dataset: { mac: "the Trash" } as DOMStringMap, textContent: "the Recycle Bin" };
      return { marked, node: { querySelectorAll: () => [marked] } as unknown as ParentNode };
    };
    const win = root();
    applyPlatformText(win.node, false);
    expect(win.marked.textContent).toBe("the Recycle Bin");
    const mac = root();
    applyPlatformText(mac.node, true);
    expect(mac.marked.textContent).toBe("the Trash");
  });

  it("takes Cmd as the command key on a Mac and Ctrl elsewhere", () => {
    expect(commandKey({ ctrlKey: false, metaKey: true }, true)).toBe(true);
    expect(commandKey({ ctrlKey: true, metaKey: false }, true)).toBe(false);
    expect(commandKey({ ctrlKey: true, metaKey: false }, false)).toBe(true);
    expect(commandKey({ ctrlKey: false, metaKey: true }, false)).toBe(false);
  });
});
