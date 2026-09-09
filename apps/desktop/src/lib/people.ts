/**
 * People on a call, as the note's `people:` frontmatter carries them: the
 * calendar connector often hands back e-mail addresses rather than names.
 * `formatPerson` turns "m.rivera@example.com" into "M. Rivera" and
 * "anna.berg@x.io" into "Anna Berg"; a real name passes through untouched.
 */
export function formatPerson(raw: string): string {
  const trimmed = raw.trim();
  const at = trimmed.indexOf("@");
  if (at <= 0) return trimmed;
  const local = trimmed.slice(0, at).replace(/\+.*$/, "");
  const parts = local.split(/[._-]+/).filter(Boolean);
  if (parts.length === 0) return trimmed;
  return parts
    .map((part) => {
      const word = part.charAt(0).toUpperCase() + part.slice(1).toLowerCase();
      return part.length === 1 ? `${word}.` : word;
    })
    .join(" ");
}

/**
 * Mailing lists and group aliases the calendar hands back next to (or
 * instead of) the people on them. Mirrors `drop_group_addresses` in
 * enhance.rs; applied here as well so notes written before the backend
 * filter existed never say "with Internal". Keep the two lists in step.
 */
const GROUP_WORDS = new Set([
  "all", "everyone", "everybody", "team", "teams", "staff", "internal", "external",
  "dev", "devs", "developers", "engineering", "eng", "product", "design", "sales",
  "marketing", "support", "hr", "people", "ops", "admin", "office", "company", "group",
  "list", "noreply", "no-reply", "calendar", "meetings", "invites", "info", "hello",
  "founders", "leadership", "management", "board",
]);

export function isGroupAddress(raw: string): boolean {
  const trimmed = raw.trim();
  const at = trimmed.indexOf("@");
  // A real name has a space or is an address; a lone word is judged as a list name.
  if (at < 0 && trimmed.includes(" ")) return false;
  const local = (at < 0 ? trimmed : trimmed.slice(0, at)).toLowerCase();
  return (
    GROUP_WORDS.has(local) || local.endsWith("-team") || local.endsWith("-all") || local.startsWith("all-")
  );
}

/** How many names are spelled out before the rest become "and N others". */
const SHOWN = 4;

/**
 * The roster line under a note's title: the first few names, the rest
 * counted, and the whole formatted list for a tooltip. `null` for nobody.
 */
export function describePeople(people: string[]): { summary: string; full: string } | null {
  const names = people
    .filter((person) => !isGroupAddress(person))
    .map(formatPerson)
    .filter((name) => name.length > 0);
  if (names.length === 0) return null;
  const full = names.join(", ");
  if (names.length <= SHOWN) return { summary: full, full };
  const rest = names.length - SHOWN;
  return {
    summary: `${names.slice(0, SHOWN).join(", ")} and ${rest} ${rest === 1 ? "other" : "others"}`,
    full,
  };
}
