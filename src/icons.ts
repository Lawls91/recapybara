// Icons are SVG files bundled with the app (src/icons, from Fluent UI System Icons, MIT), drawn through a CSS
// mask so they take the text colour and font size like the icon font they replace. The Segoe icon fonts only
// exist on Windows, so a glyph from them was an empty square on macOS.

/** Every icon the app can show: each has src/icons/<name>.svg and a rule in icons.css. */
export const ICONS = [
  "add", "arrow_download", "arrow_left", "arrow_swap", "arrow_sync", "book_letter", "calendar_ltr", "chat",
  "checkmark", "chevron_right", "copy", "delete", "dismiss", "document", "edit", "filter", "folder_open",
  "lock_closed", "more_horizontal", "next", "open", "people", "person", "prohibited", "search", "settings", "tag",
  "text_bullet_list", "warning",
] as const;

export type Icon = (typeof ICONS)[number];

/** The markup for an icon: decorative, so hidden from screen readers; `cls` adds classes such as "big". */
export const ic = (name: Icon, cls = "") => `<i aria-hidden="true" class="ic${cls ? ` ${cls}` : ""}" data-i="${name}"></i>`;
