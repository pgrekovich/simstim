// Fictional fixtures for screenshots of the real renderer. No native bridge,
// recording, cloud AI, model download or filesystem mutation is available.
//
// Dates are relative to today so the sidebar reads Today and Yesterday in
// every screenshot; the notes folder is a small design studio's week.
const apiPreview = new URLSearchParams(location.search).has("api");
// `/banner.html` renders the floating call banner; here it always shows a
// detected Zoom call.
const setupPreview = new URLSearchParams(location.search).has("setup");
const updatesPreview = new URLSearchParams(location.search).has("updates");

/** Local ISO timestamp `daysAgo` days before today at `hh:mm`, with the zone offset. */
function at(daysAgo: number, hh: number, mm: number): string {
  const d = new Date();
  d.setDate(d.getDate() - daysAgo);
  d.setHours(hh, mm, 0, 0);
  const pad = (n: number) => String(n).padStart(2, "0");
  const offset = -d.getTimezoneOffset();
  const sign = offset >= 0 ? "+" : "-";
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(hh)}:${pad(mm)}:00${sign}${pad(Math.floor(Math.abs(offset) / 60))}:${pad(Math.abs(offset) % 60)}`;
}
/** `Nov 6` for a day relative to today; negative is in the future. */
function md(daysAgo: number): string {
  const d = new Date();
  d.setDate(d.getDate() - daysAgo);
  return d.toLocaleDateString("en-US", { month: "short", day: "numeric" });
}
/** `Wed Nov 11` for a day relative to today. */
function wmd(daysAgo: number): string {
  const d = new Date();
  d.setDate(d.getDate() - daysAgo);
  return d.toLocaleDateString("en-US", { weekday: "short", month: "short", day: "numeric" }).replace(",", "");
}
/** The call file name Simstim would have written for that timestamp. */
function file(iso: string): string {
  return `${iso.slice(0, 10)} ${iso.slice(11, 19).replaceAll(":", "-")} call.md`;
}

interface Note {
  path: string;
  date: string;
  title: string;
  title_by: null;
  audio_path: null;
  duration: string;
  duration_secs: number;
  people: string[];
  notes: string;
  summary: string;
  transcript: string;
  summary_model: string;
  summary_style: null;
  summary_prompt: null;
}

function note(n: Omit<Note, "path" | "title_by" | "audio_path" | "summary_model" | "summary_style" | "summary_prompt">): Note {
  return { ...n, path: `/demo/${file(n.date)}`, title_by: null, audio_path: null, summary_model: "example", summary_style: null, summary_prompt: null };
}

const review = note({
  date: at(0, 15, 30), title: "Website design review", duration: "32m", duration_secs: 1920, people: ["Alex", "Maya", "Leo"],
  notes: "The quieter direction feels right. Give the work room to breathe.\n\n- Lead with the Fieldwork project, not the studio manifesto.\n- Keep the warm background. The pure white version feels too clinical.\n- Check the mobile crop on the ceramics photos.\n- Ask Maya for the final portrait selects.",
  summary: `The team chose a warmer, more editorial direction for the studio website. The homepage will lead with a finished project and let the work introduce the studio.

### Decisions

- **Open with Fieldwork.** Use one full-width project image, a short introduction and a clear link to the case study.
- **Keep the navigation simple.** Work, Studio and Contact. Journal can wait until there is enough material to maintain it.
- **Use the warm palette.** Off-white backgrounds, charcoal text and a single muted blue accent.
- **Show the process.** Each case study should pair finished photography with sketches and one honest explanation of the problem.

### Open loops

**Mine**

- Send the revised prototype to two clients for a quick navigation check · 00:28:12

**Waiting for**

- Maya: homepage layout, mobile crops and the portrait selects, by ${wmd(-5)} · 00:29:58
- Leo: edit the Fieldwork case study down to three short sections · 00:22:56

**Open questions**

- Project credits beside the introduction or at the end of each case study? · 00:26:27`,
  transcript: "[00:00:04] Me: I keep coming back to the quieter version. You see the work first, and then you get a sense of who we are.\n[00:01:49] Them: Same. The big studio statement is asking people to care before we have shown them anything.\n[00:03:35] Me: What if we open with Fieldwork? One image, two lines of context, and then straight into the project.\n[00:05:20] Them: That is the strongest photography we have. I would use the wide shot of the shelves, not the close-up.\n[00:07:06] Me: Yes, but we need a different crop on mobile. The object gets lost when we just center the desktop image.\n[00:08:51] Them: I can make a dedicated mobile crop. And I would keep the off-white background from the second direction.\n[00:10:37] Me: The white version is clean, but it feels a little clinical. The warmer one suits the materials better.\n[00:12:22] Them: Agreed. Charcoal for the text, then just that muted blue for links. We do not need a color for every project.\n[00:14:08] Me: Can we settle the navigation? I have Work, Studio, Journal and Contact in this version.\n[00:15:54] Them: I would leave Journal out for now. There is only one draft, and an empty section will make the site feel unfinished.\n[00:17:39] Me: Work, Studio, Contact. That is enough. The project pages can carry more of the story.\n[00:19:25] Them: On those pages, I want to keep some of the sketches. The final pictures look good, but they do not explain the choices.\n[00:21:10] Me: Right. A short brief, the approach, and the result. We should say what was difficult without turning it into a long essay.\n[00:22:56] Them: I will cut the Fieldwork copy into those three sections. Some of the current introduction repeats the brief.\n[00:24:41] Me: What about the credits? They feel a little heavy beside the opening text.\n[00:26:27] Them: Let us try them at the bottom as well. I do not think we need to decide until both versions are in the prototype.\n[00:28:12] Me: Maya, can we have the homepage and crops by Wednesday? Then I can send it to two clients to see if they can find their way around.\n[00:29:58] Them: Wednesday works. I will include the portrait selects too, so the Studio page does not hold things up.\n[00:31:44] Me: Great. We have a direction. Let us make this version feel finished before adding anything else.",
});

const kickoff = note({
  date: at(0, 11, 0), title: "Fieldwork project kickoff", duration: "24m", duration_secs: 1440, people: ["Alex", "Maya"],
  notes: "Client wants a useful identity system, not just a new logo.\n\nStart with packaging and the shop window. Everything else follows.",
  summary: `### Direction

Fieldwork is opening a small ceramics shop this autumn. The identity should feel tactile and welcoming, with enough flexibility for new makers and seasonal collections.

### Agreed scope

- Wordmark, type palette and two packaging formats.
- Shop-window lettering and a simple care card.
- A short usage guide the shop team can actually follow.

First concept review is next Tuesday. Photography and an online store are outside this phase.

### Open loops

**Mine**

- Confirm the packaging dimensions with the supplier before sketches begin · 00:19:00

**Waiting for**

- Maya: collect references from print, signage and ceramic glazes · 00:14:16
- Maya: bring two directions to the concept review on ${wmd(-4)} · 00:23:44`,
  transcript: "[00:00:04] Me: The important thing is that this works in the shop, not just in a presentation.\n[00:04:48] Them: Exactly. The owner will be packing orders herself, so the labels need to be easy to print and apply.\n[00:09:32] Me: Let us start with the two actual box sizes and the window. We can build the rest around those.\n[00:14:16] Them: I will gather references that have some texture. More paper and signage, fewer polished logo mockups.\n[00:19:00] Me: Good. I will confirm the dimensions before we commit to any layouts.\n[00:23:44] Them: Then we can bring two directions to the review next Tuesday.",
});

const onboarding = note({
  date: at(1, 16, 0), title: "A quieter onboarding", duration: "28m", duration_secs: 1680, people: ["Alex", "Nora"],
  notes: "The first useful moment is a saved note. Measure time to that moment.\n\nOptional choices can wait until someone needs them.",
  summary: `### What we learned

People understood the product once they had saved their first note. Most confusion happened before that, when setup asked them to make decisions without context.

### Changes to try

- Replace the introduction carousel with one short welcome screen.
- Use a sensible default folder and make it easy to change.
- Move advanced preferences into Settings.
- Explain optional features at the moment they become useful.

### Open loops

**Mine**

- Prepare the revised onboarding prototype without tooltips for the first pass · 00:27:44

**Waiting for**

- Nora: schedule test sessions with three new participants · 00:27:44`,
  transcript: "[00:00:04] Me: Nobody had trouble understanding the note once it existed. The friction was all before that.\n[00:05:36] Them: The welcome carousel was the worst part. People were trying to remember things they had not used yet.\n[00:11:08] Me: Could we start with a sensible folder and one clear action?\n[00:16:40] Them: Yes. We can explain the optional choices later, when they actually mean something.\n[00:22:12] Me: Let us test that with three people who have not seen the previous version.\n[00:27:44] Them: I will schedule those sessions. Please keep the prototype free of tooltips for the first pass.",
});

const coffee = note({
  date: at(1, 10, 30), title: "Coffee with Sam", duration: "18m", duration_secs: 1080, people: ["Alex", "Sam"],
  notes: "Leave one afternoon open each week. Good work needs a little unplanned space.",
  summary: `### Ideas worth keeping

- Protect one afternoon for experiments without a client deliverable.
- Share a small piece of work each week, even if it is unfinished.
- Keep a folder of interesting materials, places and conversations rather than collecting only other designers' work.

### Open loops

**Mine**

- Visit the print workshop with Sam and compare paper samples · 00:17:44`,
  transcript: "[00:00:04] Me: I have been filling every gap in the week, and then wondering why nothing new happens.\n[00:03:36] Them: Try leaving an afternoon empty. Not for admin. Actually empty.\n[00:07:08] Me: That sounds uncomfortable, which probably means I need it.\n[00:10:40] Them: You can use it to make something small. It does not have to become a project.\n[00:14:12] Me: I also want to get away from looking at the same references online.\n[00:17:44] Them: Come to the print workshop with me next week. There is a whole room of paper samples.",
});

const interview = note({
  date: at(2, 14, 0), title: "Customer interview · Northline", duration: "36m", duration_secs: 2160, people: ["Alex", "Iris"],
  notes: "“I want to find the decision, not reread the whole meeting.”\n\nSearch terms are usually a project name or a person.",
  summary: `### Main finding

Iris returns to meeting notes to recover a decision or check who owns an action. Reading the entire conversation is a fallback, not the usual workflow.

### Patterns

- Project names and people are the most common search terms.
- A short decision list is more useful than a chronological recap.
- Links back to the original wording help when the summary feels ambiguous.

### Open loops

Nothing to follow up from this call.`,
  transcript: "[00:00:04] Me: Think about the last time you opened an old meeting note. What were you trying to find?\n[00:07:12] Them: Who had agreed to send the supplier list. I remembered the conversation, but not who owned it.\n[00:14:20] Me: Did you read the whole note?\n[00:21:28] Them: No, I searched for the project name and then scanned for actions.\n[00:28:36] Me: What would have made that easier?\n[00:35:44] Them: A short list of decisions, with a way to check the exact wording if I needed to.",
});

const checkin = note({
  date: at(2, 9, 30), title: "Monday studio check-in", duration: "22m", duration_secs: 1320, people: ["Alex", "Maya", "Leo"],
  notes: "Keep Friday afternoon free for the website review.\n\nFieldwork supplier dimensions are the only external dependency this week.",
  summary: `### This week

- **Fieldwork:** confirm production constraints, then begin identity exploration.
- **Studio website:** settle the homepage direction and tighten the first case study.
- **Research:** finish the Northline interview and share a short readout.

### Capacity

Keep one project review per day. Friday afternoon stays open for the website so feedback can be handled together.

### Open loops

**Mine**

- Follow up with the supplier about the Fieldwork packaging sizes · 00:08:44

**Waiting for**

- Leo: the first case study edit before the website review · 00:21:44`,
  transcript: "[00:00:04] Me: Let us keep this short. What needs a decision this week?\n[00:04:24] Them: The website direction, and the packaging sizes for Fieldwork.\n[00:08:44] Me: I can follow up with the supplier today. Anything else blocking the concepts?\n[00:13:04] Them: No. Once we have the sizes, I can start with the actual applications.\n[00:17:24] Me: Then let us protect Friday afternoon for the website review.\n[00:21:44] Them: Works for me. I will have the first case study edit ready before then.",
});

const print = note({
  date: at(5, 13, 0), title: "Print samples & packaging", duration: "27m", duration_secs: 1620, people: ["Alex", "Maya"],
  notes: "The uncoated stock looks better in daylight. Keep the ink coverage light.",
  summary: `### Selected for prototyping

- Uncoated warm-white stock for the care cards.
- A one-color label that can be printed in small batches.
- A folded insert instead of a second printed box.

### Open loops

**Waiting for**

- Maya: test the label adhesive when the real boxes arrive · 00:26:44

**Open questions**

- Is the smallest type size readable under ordinary shop lighting? · 00:16:04`,
  transcript: "[00:00:04] Me: The coated sample looked good in the supplier photo, but it is too shiny in person.\n[00:05:24] Them: This uncoated one feels closer to the objects. The color changes nicely in daylight.\n[00:10:44] Me: Let us keep the ink coverage light. We should not hide the paper.\n[00:16:04] Them: And use a folded insert rather than printing another box?\n[00:21:24] Me: Yes. Fewer components, easier to replace if the care instructions change.\n[00:26:44] Them: I will test the adhesive when the real boxes arrive.",
});

const launch = note({
  date: at(6, 15, 0), title: "Autumn launch planning", duration: "31m", duration_secs: 1860, people: ["Alex", "Leo"],
  notes: "Launch with three complete projects. Better than six half-written ones.",
  summary: `### Launch plan

Publish the studio website with three finished case studies, a short Studio page and a direct contact link.

### Later

The journal and newsletter can follow once there is a sustainable publishing rhythm. No date was committed for either.

### Open loops

**Mine**

- Arrange two navigation checks with people outside the studio · 00:24:36

**Waiting for**

- Leo: the list of image permissions and project credits · 00:18:28`,
  transcript: "[00:00:04] Me: We have six projects in the folder, but only three have enough material to tell the story properly.\n[00:06:12] Them: Then launch with three. I would rather see something finished than a grid of placeholders.\n[00:12:20] Me: Agreed. We need to confirm the image permissions and credits first.\n[00:18:28] Them: I will make a list. Can you arrange two quick navigation checks with people outside the studio?\n[00:24:36] Me: Yes. We should test on a phone as well as the laptop.\n[00:30:44] Them: And leave the journal for later. We do not have a publishing rhythm yet.",
});

const notes = [review, kickoff, onboarding, coffee, interview, checkin, print, launch];
const config = {
  output_dir: "/demo", active_model: "parakeet-tdt-0.6b-v3-int8", onboarding_complete: true,
  theme: "light", show_in_dock: true, keep_audio: true, audio_format: "aac",
  retranscribe_model: "whisper-large-v3-turbo-q8", auto_retranscribe: true,
  delete_audio_after_retranscribe: false, enhance_provider: apiPreview ? "openrouter" : "claude", enhance_models: apiPreview ? { openrouter: "example/atlas" } : {},
  auto_enhance: true, enhance_rename_title: true, enhance_use_calendar: true,
  detect_calls: true, auto_stop_after_call: false, track_open_loops: true,
};
const sessions = notes.map((n) => ({ path: n.path, date: n.date, title: n.title, duration: n.duration, duration_secs: n.duration_secs, people: n.people }));

// Open loops lives in memory here; `?loops=empty` previews the first build.
const loopsPreview = new URLSearchParams(location.search).get("loops");
const link = (n: Note, t: string) => `[${n.title}](${encodeURIComponent(file(n.date))}#t=${t})`;
const item = (text: string, n: Note, daysAgo: number, t: string) => `- [ ] ${text} · ${md(daysAgo)} · ${link(n, t)}`;
let loopsLines = [
  "---", "simstim: open-loops", `updated: ${at(0, 16, 4)}`, "---", "",
  "## Mine", "",
  item("Send the revised prototype to two clients for a quick navigation check", review, 0, "00:28:12"),
  item("Confirm the packaging dimensions with the supplier before sketches begin", kickoff, 0, "00:19:00"),
  item("Prepare the revised onboarding prototype without tooltips for the first pass", onboarding, 1, "00:27:44"),
  item("Arrange two navigation checks with people outside the studio", launch, 6, "00:24:36"),
  "",
  "## Waiting for", "",
  item(`Maya: homepage layout, mobile crops and the portrait selects, by ${wmd(-5)}`, review, 0, "00:29:58"),
  item("Leo: edit the Fieldwork case study down to three short sections", review, 0, "00:22:56"),
  item("Nora: schedule test sessions with three new participants", onboarding, 1, "00:27:44"),
  item("Maya: test the label adhesive when the real boxes arrive", print, 5, "00:26:44"),
  "",
  "## Open questions", "",
  item("Project credits beside the introduction or at the end of each case study?", review, 0, "00:26:27"),
  "",
  "## Done", "",
  `- [x] Follow up with the supplier about the Fieldwork packaging sizes · ${md(2)} · ${link(checkin, "00:08:44")} · done ${md(0)} per ${link(kickoff, "00:09:32")}`,
  `- [x] Visit the print workshop with Sam and compare paper samples · ${md(1)} · ${link(coffee, "00:17:44")} · done ${md(0)}`,
];
function loopsOut() {
  if (loopsPreview === "empty") {
    return { exists: false, body: "", body_line: 1, updated: null, open_count: 0, running: false, unread: 0 };
  }
  let section = "";
  let open = 0;
  for (const line of loopsLines) {
    if (line.startsWith("## ")) section = line.slice(3);
    if ((section === "Mine" || section === "Waiting for") && line.startsWith("- [ ] ")) open += 1;
  }
  return { exists: true, body: loopsLines.slice(4).join("\n"), body_line: 5, updated: at(0, 16, 4), open_count: open, running: false, unread: 0 };
}

// Backend events the real app would push; `emit` fires them from here.
const listeners = new Map<string, Set<(event: { payload: unknown }) => void>>();
function emit(name: string, payload: unknown) {
  for (const callback of listeners.get(name) ?? []) callback({ payload });
}
// Scripted previews (site/demo-gif.mjs) push their own live lines through here.
(window as unknown as { __simstimDemo?: unknown }).__simstimDemo = { emit };
// A pretend recording: Record continues the open note, the first live line
// arrives right away so the toolbar shows the hearing dots, and both sides
// count as heard.
let recordingPath: string | null = null;

export async function invoke(command: string, args?: Record<string, unknown>): Promise<unknown> {
  switch (command) {
    case "start_recording": {
      // Continuing a note gets a first line at once; a fresh recording stays
      // silent so a scripted preview can push its own lines.
      const continuing = typeof args?.path === "string";
      recordingPath = continuing ? (args.path as string) : "/demo/live.md";
      if (continuing) setTimeout(() => emit("transcript-line", "[00:31:52] Them: Let us make this version feel finished before adding anything else."), 200);
      return { path: recordingPath, title: null };
    }
    case "stop_recording": {
      const path = recordingPath;
      recordingPath = null;
      return path;
    }
    case "update_active_session": return null;
    case "recording_activity": return recordingPath ? { me_silent_ms: 900, them_silent_ms: 2600, mic: "MacBook Pro Microphone" } : null;
    case "banner_current_state": return { kind: "detected", bundle_id: "us.zoom.xos", app_name: "Zoom" };
    case "banner_dismiss": return null;
    case "show_main_window": return null;
    case "get_config": return config;
    case "get_launch_at_login": return "unavailable";
    case "list_sessions": return sessions;
    case "read_session": return notes.find((n) => n.path === args?.path) ?? review;
    case "list_models": return [
      { id: config.active_model, name: "Parakeet TDT v3", size_mb: 670, downloaded: true, active: true, role: "live", technical_name: null, tagline: null, size_on_disk_bytes: 671000000, retired: false },
      { id: config.retranscribe_model, name: "Whisper large-v3 turbo", size_mb: 874, downloaded: true, active: true, role: "retranscribe", technical_name: null, tagline: null, size_on_disk_bytes: 874000000, retired: false },
    ];
    case "list_api_models": return [
      { value: "example/atlas", label: "Example: Atlas" },
      { value: "example/atlas-mini", label: "Example: Atlas Mini" },
      { value: "example/orbit", label: "Example: Orbit" },
    ];
    case "list_enhance_providers": return [
      ...(apiPreview ? [{ id: "openrouter", name: "OpenRouter API", available: true, version: "Your API key", models: [] }] : []),
      { id: "claude", name: "Claude Code", available: !setupPreview, version: setupPreview ? null : "2.1.314 (Claude Code)", models: [{ value: "sonnet", label: "Sonnet" }, { value: "haiku", label: "Haiku" }] },
      { id: "codex", name: "Codex", available: !setupPreview, version: setupPreview ? null : "codex-cli 0.166.0", models: [{ value: "gpt-6.1-sol", label: "GPT-6.1 Sol" }] },
    ];
    case "get_prompts": return { summary_instructions: "", summary_styles: [], quick_questions: [] };
    case "ai_setup_status": return null;
    case "ai_provider_status": return setupPreview ? [] : [
      { id: "claude", name: "Claude Code", installed_version: updatesPreview ? "2.1.236" : "2.1.314", latest_version: "2.1.314", update_available: updatesPreview },
      { id: "codex", name: "Codex", installed_version: updatesPreview ? "0.152.0" : "0.166.0", latest_version: "0.166.0", update_available: updatesPreview },
    ];
    case "check_ai_updates": return updatesPreview ? [
      { id: "claude", name: "Claude Code", installed_version: "2.1.236", latest_version: "2.1.314" },
      { id: "codex", name: "Codex", installed_version: "0.152.0", latest_version: "0.166.0" },
    ] : [];
    case "chat_history": return { messages: [], running: false };
    case "storage_stats": return { note_count: notes.length, notes_bytes: 24576, audio_bytes: 0, audio_secs: 0 };
    case "search_sessions": return sessions.filter(s => s.title.toLowerCase().includes(String(args?.query ?? "").toLowerCase())).map(s => ({ ...s, snippets: [], total_matches: 1 }));
    case "open_loops_read": return loopsOut();
    case "open_loops_toggle": {
      // Mirrors loops.rs: a checked item moves to the end of Done (the last
      // section here), an unchecked one back to its kind of section.
      const index = Number(args?.line) - 1;
      if (loopsLines[index] !== args?.expected) throw new Error("Open loops changed on disk; showing the current file");
      const line = loopsLines[index];
      loopsLines.splice(index, 1);
      if (args?.done) {
        loopsLines.push(`${line.replace("- [ ] ", "- [x] ")} · done ${md(0)}`);
      } else {
        const restored = line.replace("- [x] ", "- [ ] ").replace(/ · done .*$/, "");
        const action = restored.slice(6).split(" · ")[0];
        const heading = action.endsWith("?") ? "## Open questions" : /^[^:]{1,30}: /.test(action) ? "## Waiting for" : "## Mine";
        const start = loopsLines.indexOf(heading);
        const next = loopsLines.findIndex((l, i) => i > start && l.startsWith("## "));
        const items = loopsLines.slice(start + 1, next).filter((l) => l.startsWith("- ["));
        loopsLines.splice(items.length ? next - 1 : start + 2, 0, restored);
      }
      return loopsOut();
    }
    case "open_loops_save":
      loopsLines = [...loopsLines.slice(0, 4), ...String(args?.body).split("\n")];
      return loopsOut();
    default: throw new Error("This is a fictional preview. Use the native app for this action.");
  }
}
export async function listen(name: string, callback: (event: { payload: unknown }) => void) {
  if (!listeners.has(name)) listeners.set(name, new Set());
  listeners.get(name)!.add(callback);
  return () => { listeners.get(name)?.delete(callback); };
}
export async function emitTo() {}
export function getCurrentWebviewWindow() { return { label: "main" }; }
export function convertFileSrc(path: string) { return path; }
export async function ask() { return false; }
export async function open() { return null; }
export async function openPath() { throw new Error("Preview only"); }
export async function revealItemInDir() { throw new Error("Preview only"); }
