/**
 * People on a call, as the note's `people:` frontmatter carries them: the
 * calendar connector often hands back e-mail addresses rather than names.
 * `formatPerson` turns "m.suwald@demoboost.com" into "M. Suwald" and
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

/** How many names are spelled out before the rest become "and N others". */
const SHOWN = 4;

/**
 * The roster line under a note's title: the first few names, the rest
 * counted, and the whole formatted list for a tooltip. `null` for nobody.
 */
export function describePeople(people: string[]): { summary: string; full: string } | null {
  const names = people.map(formatPerson).filter((name) => name.length > 0);
  if (names.length === 0) return null;
  const full = names.join(", ");
  if (names.length <= SHOWN) return { summary: full, full };
  const rest = names.length - SHOWN;
  return {
    summary: `${names.slice(0, SHOWN).join(", ")} and ${rest} ${rest === 1 ? "other" : "others"}`,
    full,
  };
}
