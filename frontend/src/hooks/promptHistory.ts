// Shared prompt-history ring for the Chat and Terminal tabs.
//
// Both tabs feed the same Agent + Session, so they share one bash-style recall
// history: submit in either tab, then Up/Down in either tab walks the same list.
// Persisted to localStorage (like ~/.bash_history) so it survives reloads and
// workspace restarts. The `entries` array is a STABLE reference — callers may
// capture it once (the Terminal does) and keep seeing live updates, because we
// only ever mutate it in place (push/shift), never reassign.

const KEY = "thclaws.promptHistory.v1";
const MAX = 200;

const entries: string[] = ((): string[] => {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw) {
      const parsed = JSON.parse(raw);
      if (Array.isArray(parsed)) {
        return parsed.filter((s) => typeof s === "string").slice(-MAX);
      }
    }
  } catch {
    // ignore malformed/absent storage — start empty
  }
  return [];
})();

function persist(): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(entries));
  } catch {
    // storage full / disabled — history just won't persist this session
  }
}

/** The live, shared history (oldest first). Stable reference. */
export function promptHistory(): readonly string[] {
  return entries;
}

/**
 * Record a submitted prompt. Trims, drops empties, and collapses a run of
 * identical consecutive entries (no value in cycling through `ls ls ls`).
 */
export function recordPrompt(entry: string): void {
  const trimmed = entry.trim();
  if (trimmed.length === 0) return;
  if (entries.length > 0 && entries[entries.length - 1] === trimmed) return;
  entries.push(trimmed);
  while (entries.length > MAX) entries.shift();
  persist();
}
