import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { libraryLabel, type Settings } from "./settings";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

let current: Settings;

async function load() {
  current = await invoke<Settings>("get_settings");
  $("library-path").textContent = libraryLabel(current.libraryPath);
  $<HTMLSelectElement>("summary-model").value = current.summaryModel;
  $<HTMLInputElement>("start-at-logon").checked = current.startAtLogon;
}

async function save(change: Partial<Settings>) {
  await invoke("save_settings", { settings: { ...current, ...change } });
}

function show(view: "list" | "settings") {
  $("list-view").hidden = view !== "list";
  $("settings-view").hidden = view !== "settings";
}

$("settings-button").onclick = () => show("settings");
$("settings-back").onclick = () => show("list");
$("change-folder").onclick = async () => {
  const picked = await invoke<string | null>("pick_library_folder");
  if (picked) await save({ libraryPath: picked });
};
$<HTMLSelectElement>("summary-model").onchange = (e) =>
  save({ summaryModel: (e.target as HTMLSelectElement).value });
$<HTMLInputElement>("start-at-logon").onchange = (e) =>
  save({ startAtLogon: (e.target as HTMLInputElement).checked });

listen("settings-changed", load);
listen("open-settings", () => show("settings"));
load();
