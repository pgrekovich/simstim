// Fictional fixtures for screenshots of the real renderer. No native bridge,
// recording, cloud AI, model download or filesystem mutation is available.
const setupPreview = new URLSearchParams(location.search).has("setup");
const date = "2026-09-09T10:00:00+02:00";
const path = "/demo/2026-09-09 10-00-00 call.md";
const note = {
  path, date, title: "Beta planning", title_by: null, audio_path: null,
  notes: "Keep the first release focused on recording and readable files.\nCheck the two audio channels separately before inviting testers.",
  transcript: "[00:00:04] Me: Let's start with five testers on different Macs.\n[00:00:09] Them: I'll prepare a short sample call for them.\n[00:00:16] Me: Great. I'll send the installation checklist once the candidate passes review.\n[00:00:24] Them: Should we add a PDF export before the beta?\n[00:00:30] Me: The note is already a file. People can give it to the AI tools they use.\n[00:00:38] Them: Right. Let's focus on reliable recording and readable transcripts.",
  summary: "### Decisions\n\n- Start with five testers using different Apple Silicon Macs.\n- Keep PDF creation and sharing in the AI tools people already use.\n- Collect installation and transcription feedback before a wider launch.\n\n### Next steps\n\n- **Sam:** prepare a short sample call for the testers.\n- **Alex:** send the installation checklist after the candidate passes review.",
  summary_model: "example", summary_style: null, people: ["Alex", "Sam"],
};
const config = {
  output_dir: "/demo", active_model: "parakeet-tdt-0.6b-v3-int8", onboarding_complete: true,
  theme: "light", show_in_dock: true, keep_audio: true, audio_format: "aac",
  retranscribe_model: "whisper-large-v3-turbo-q8", auto_retranscribe: true,
  delete_audio_after_retranscribe: false, enhance_provider: "claude", enhance_models: {},
  auto_enhance: true, enhance_rename_title: true, enhance_use_calendar: true,
  detect_calls: true, auto_stop_after_call: false,
};
const sessions = [
  { path, date, title: note.title, duration: "12m", duration_secs: 720, people: note.people },
  { path: "/demo/design.md", date: "2026-09-08T14:00:00+02:00", title: "Design review", duration: "24m", duration_secs: 1440, people: [] },
  { path: "/demo/weekly.md", date: "2026-09-07T09:30:00+02:00", title: "Weekly catch-up", duration: "18m", duration_secs: 1080, people: [] },
];
export async function invoke(command: string, args?: Record<string, unknown>): Promise<unknown> {
  switch (command) {
    case "get_config": return config;
    case "list_sessions": return sessions;
    case "read_session": return { ...note, title: sessions.find(s => s.path === args?.path)?.title ?? note.title };
    case "list_models": return [
      { id: config.active_model, name: "Parakeet TDT v3", size_mb: 670, downloaded: true, active: true, role: "live", technical_name: null, tagline: null, size_on_disk_bytes: 671000000, retired: false },
      { id: config.retranscribe_model, name: "Whisper large-v3 turbo", size_mb: 874, downloaded: true, active: true, role: "retranscribe", technical_name: null, tagline: null, size_on_disk_bytes: 874000000, retired: false },
    ];
    case "list_enhance_providers": return [
      { id: "claude", name: "Claude Code", available: !setupPreview, version: setupPreview ? null : "Preview" },
      { id: "codex", name: "Codex", available: !setupPreview, version: setupPreview ? null : "Preview" },
    ];
    case "get_prompts": return { summary_instructions: "", summary_styles: [], quick_questions: [] };
    case "ai_setup_status": return null;
    case "recording_activity": return null;
    case "chat_history": return { messages: [], running: false };
    case "storage_stats": return { note_count: 3, notes_bytes: 8192, audio_bytes: 0, audio_secs: 0 };
    case "search_sessions": return sessions.filter(s => s.title.toLowerCase().includes(String(args?.query ?? "").toLowerCase())).map(s => ({ ...s, snippets: [], total_matches: 1 }));
    default: throw new Error("This is a fictional preview. Use the native app for this action.");
  }
}
export async function listen() { return () => {}; }
export async function emitTo() {}
export function getCurrentWebviewWindow() { return { label: "main" }; }
export function convertFileSrc(path: string) { return path; }
export async function ask() { return false; }
export async function open() { return null; }
export async function openPath() { throw new Error("Preview only"); }
export async function revealItemInDir() { throw new Error("Preview only"); }
