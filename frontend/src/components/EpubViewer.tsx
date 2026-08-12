import { useEffect, useRef, useState } from "react";
import ePub, { type Book, type Rendition } from "epubjs";

// EPUB preview for the Files tab. Browsers/webviews can't render an
// EPUB natively (it's a zipped bundle of XHTML), so we fetch the bytes
// off the /file-asset route and hand them to epub.js, which unzips +
// renders client-side. `flow: scrolled-doc` gives a scroll-per-chapter
// reading pane (matching the .md / .pdf preview feel); Prev/Next + the
// arrow keys step between spine sections.
type Props = {
  // Same-origin /file-asset URL for the .epub on disk.
  url: string;
  theme: "light" | "dark";
};

function applyTheme(rendition: Rendition, theme: "light" | "dark") {
  const dark = theme === "dark";
  // `transparent` lets the pane's own background show through so the
  // reader matches the surrounding app chrome in both themes.
  rendition.themes.default({
    body: {
      color: dark ? "#e6e6e6 !important" : "#1a1a1a !important",
      background: "transparent !important",
      "line-height": "1.6",
      padding: "0 1rem !important",
    },
    a: { color: dark ? "#7aa2f7 !important" : "#2563eb !important" },
  });
}

export function EpubViewer({ url, theme }: Props) {
  const hostRef = useRef<HTMLDivElement>(null);
  const renditionRef = useRef<Rendition | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [label, setLabel] = useState("");

  useEffect(() => {
    let cancelled = false;
    let book: Book | null = null;
    setError(null);
    setLoading(true);
    setLabel("");
    (async () => {
      try {
        const res = await fetch(url);
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const buf = await res.arrayBuffer();
        if (cancelled || !hostRef.current) return;
        book = ePub(buf);
        const rendition = book.renderTo(hostRef.current, {
          width: "100%",
          height: "100%",
          flow: "scrolled-doc",
          spread: "none",
        });
        renditionRef.current = rendition;
        applyTheme(rendition, theme);
        rendition.on("relocated", (loc: { start?: { href?: string } }) => {
          if (cancelled || !book) return;
          const href = loc?.start?.href;
          const item = href ? book.navigation?.get(href) : undefined;
          setLabel(item?.label?.trim() ?? "");
        });
        await rendition.display();
        if (!cancelled) setLoading(false);
      } catch (e) {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : String(e));
          setLoading(false);
        }
      }
    })();
    return () => {
      cancelled = true;
      renditionRef.current = null;
      try {
        book?.destroy();
      } catch {
        /* book may not have finished opening — nothing to tear down */
      }
    };
    // Re-open only when the file changes; theme is handled separately.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [url]);

  // Re-theme on a light/dark swap without reloading the book.
  useEffect(() => {
    if (renditionRef.current) applyTheme(renditionRef.current, theme);
  }, [theme]);

  // Arrow keys step between chapters, like a reader.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "ArrowRight") void renditionRef.current?.next();
      else if (e.key === "ArrowLeft") void renditionRef.current?.prev();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div className="flex-1 min-h-0 flex flex-col">
      <div
        ref={hostRef}
        className="flex-1 min-h-0 rounded border overflow-hidden"
        style={{ borderColor: "var(--border)", background: "var(--bg-primary)" }}
      />
      <div
        className="flex items-center justify-between gap-2 px-2 py-1 text-[11px] font-mono shrink-0"
        style={{ color: "var(--text-secondary)" }}
      >
        <button
          onClick={() => void renditionRef.current?.prev()}
          className="px-2 py-0.5 rounded hover:bg-white/10 shrink-0"
          title="Previous chapter (←)"
        >
          ‹ Prev
        </button>
        <span className="truncate flex-1 text-center" title={label}>
          {error ? `EPUB error: ${error}` : loading ? "Loading EPUB…" : label}
        </span>
        <button
          onClick={() => void renditionRef.current?.next()}
          className="px-2 py-0.5 rounded hover:bg-white/10 shrink-0"
          title="Next chapter (→)"
        >
          Next ›
        </button>
      </div>
    </div>
  );
}
