// Synthetic 35-minute transcript of exactly 60,000 chars (D34: no real meeting content).
import fs from "node:fs";
const TARGET = 60000;
const topics = ["retry rule", "filter chain", "hotfix branch", "release notes", "policy editor", "test matrix", "audit log"];
const verbs = ["second review", "quick spike", "written plan", "owner", "demo", "smoke test"];
const t = (s) => {
  const ms = Math.round(s * 1000);
  const p = (n, w = 2) => String(n).padStart(w, "0");
  return `${p(Math.floor(ms / 3600000))}:${p(Math.floor(ms / 60000) % 60)}:${p(Math.floor(ms / 1000) % 60)}.${p(ms % 1000, 3)}`;
};
const speakers = ["Speaker A", "Speaker B", "Speaker C"];
let out = "WEBVTT\n\n1\n00:00:03.720 --> 00:00:13.960\n<v Speaker A>so we agreed the approach of adding a simple retry limit field</v>\n";
const cues = [];
for (let i = 2; ; i++) {
  const start = 14.5 + (i - 2) * 4;
  const sp = speakers[Math.floor((i - 2) / 3) % 3];
  const text = `point ${i} on the ${topics[i % topics.length]} needs a ${verbs[i % verbs.length]} before release`;
  const block = `\n${i}\n${t(start)} --> ${t(start + 3.8)}\n<v ${sp}>${text}</v>\n`;
  if (out.length + block.length > TARGET - 40) break;
  out += block;
  cues.push([i, t(start), t(start + 3.8), sp, text]);
}
// Pad the last cue's text to the exact length.
const [i, s, e, sp] = cues.at(-1);
const head = `\n${i}\n${s} --> ${e}\n<v ${sp}>`;
const lastText = cues.at(-1)[4];
out = out.slice(0, out.lastIndexOf(head));
const fill = TARGET - out.length - head.length - "</v>\n".length;
let text = lastText + " and that closes the review";
while (text.length < fill) text += " ok";
text = text.slice(0, fill);
out += `${head}${text}</v>\n`;
fs.writeFileSync(process.argv[2], out);
console.log(out.length, "chars, last cue", s, e);
for (const c of cues) if (c[1] <= "00:31:12" && c[2] >= "00:31:12") console.log("31:12 ->", c);
for (const c of cues) if (c[1] <= "00:20:00" && c[2] >= "00:20:00") console.log("20:00 ->", c);
