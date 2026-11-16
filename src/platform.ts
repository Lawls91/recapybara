/** The few words and keys that differ on a Mac: the page is the same on both platforms otherwise. */

/** Whether the page runs on macOS, from the webview's user agent (WKWebView's says "Macintosh"). */
export const isMac = (userAgent: string): boolean => /Macintosh|Mac OS X/.test(userAgent);

/** Each element carrying `data-mac` shows that text on a Mac; elsewhere its own (the Windows wording) stays. */
export function applyPlatformText(root: ParentNode, mac: boolean): void {
  if (!mac) return;
  root.querySelectorAll<HTMLElement>("[data-mac]").forEach((el) => {
    el.textContent = el.dataset.mac ?? el.textContent;
  });
}

/** The platform's command modifier: Cmd on a Mac, Ctrl elsewhere. */
export const commandKey = (e: Pick<KeyboardEvent, "ctrlKey" | "metaKey">, mac: boolean): boolean => (mac ? e.metaKey : e.ctrlKey);
