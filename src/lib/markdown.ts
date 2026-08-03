// Markdown rendering for chat bubbles and the preview dialog (markdown-it).
// Red line R1 (design-principles.md): rendering happens ONCE per finished
// text segment — never during streaming. The instance is shared and
// configured safe: raw HTML disabled, links linkified.
//
// Local image paths (C:\…, /home/… — e.g. a pasted-image markdown the
// Composer inserts) are rewritten through convertFileSrc so the webview's
// asset protocol can load them; real URLs pass through untouched.
import MarkdownIt from 'markdown-it';
import { convertFileSrc } from '@tauri-apps/api/core';
import { openPath, openUrl } from './tauri';

const URL_SCHEME = /^(https?|asset|data|blob|tauri):/i;

function patchImageRule(mdi: MarkdownIt): void {
  const defaultImage =
    mdi.renderer.rules.image ??
    ((tokens, idx, options, env, self) => self.renderToken(tokens, idx, options));
  mdi.renderer.rules.image = (tokens, idx, options, env, self) => {
    const t = tokens[idx];
    let src = t.attrGet('src') ?? '';
    if (src && !URL_SCHEME.test(src)) {
      // markdown-it URL-encodes the destination (space → %20) — decode
      // first so convertFileSrc doesn't double-encode the '%'.
      try {
        src = decodeURIComponent(src);
      } catch {
        /* malformed escape — keep as-is */
      }
      t.attrSet('src', convertFileSrc(src));
    }
    return defaultImage(tokens, idx, options, env, self);
  };
}

// Fenced code blocks get a wrapper with a copy button; the click is
// delegated by the bubble (v-html can't carry Vue handlers) — same
// pattern as the inline-image lightbox.
function patchCodeRule(mdi: MarkdownIt): void {
  const defaultFence =
    mdi.renderer.rules.fence ??
    ((tokens, idx, options, env, self) => self.renderToken(tokens, idx, options));
  mdi.renderer.rules.fence = (tokens, idx, options, env, self) =>
    `<div class="codeblock"><button class="codeblock__copy" type="button">复制</button>${
      defaultFence(tokens, idx, options, env, self)
    }</div>`;
}

const md = new MarkdownIt({
  html: false,
  linkify: true,
  breaks: false,
});
patchImageRule(md);
patchCodeRule(md);

// User bubbles: pasted-image markdown (四.8 fold still applies) with the
// user's own line breaks preserved — breaks: true, otherwise identical.
const mdUser = new MarkdownIt({
  html: false,
  linkify: true,
  breaks: true,
});
patchImageRule(mdUser);
patchCodeRule(mdUser);

export function renderMarkdown(text: string): string {
  return md.render(text);
}

/** User-message variant — only used when the text actually contains an
 * image embed; plain user text never goes through markdown. */
export function renderUserMarkdown(text: string): string {
  return mdUser.render(text);
}

/** User-message quote blocks: <selection>…</selection> → one elliptical
 * capsule with the body elided to a fixed length (same gold wash as the
 * composer's QuoteBar chips; the capsule reads as a single atomic unit). */
export function renderQuoteHighlight(text: string): string {
  const QUOTE_ELIDE = 24;
  const esc = (s: string): string =>
    s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  const elide = (s: string): string => {
    const t = s.replace(/\s+/g, ' ').trim();
    const chars = [...t];
    return chars.length <= QUOTE_ELIDE ? t : chars.slice(0, QUOTE_ELIDE).join('') + '…';
  };
  return text
    .split(/(<selection>[\s\S]*?<\/selection>)/g)
    .map((p) => {
      if (/^<selection>[\s\S]*?<\/selection>$/.test(p)) {
        const body = p.replace(/^<selection>/, '').replace(/<\/selection>$/, '');
        return `<span class="md-selection">${esc(elide(body))}</span>`;
      }
      return esc(p);
    })
    .join('');
}

/** Click delegation for v-html markdown bodies — a plain <a href> click
 * would make the Tauri webview navigate to the URL and REPLACE the whole
 * app, so every anchor click is intercepted here instead:
 *  - http(s)/mailto/tel → system browser (tauri-plugin-opener)
 *  - local paths (Windows C:\…, /home/…, relative) → OS default handler
 *  - `#` fragments, asset:/data:/blob: → ignored (no app navigation)
 * The callers (bubble / preview dialog) pass their own delegated click
 * events; returns true when a link was consumed. */
export function handleMdLinkClick(e: MouseEvent): boolean {
  const t = e.target as HTMLElement | null;
  const a = t?.closest?.('a');
  if (!a) return false;
  const rawHref = a.getAttribute('href') ?? '';
  const href = rawHref.trim();
  if (!href) return false;
  e.preventDefault();
  e.stopPropagation();
  if (/^(https?|mailto|tel):/i.test(href)) {
    void openUrl(href);
    return true;
  }
  if (href.startsWith('#') || /^(asset|data|blob|tauri):/i.test(href)) {
    return true; // no app navigation, nothing to open
  }
  try {
    void openPath(decodeURIComponent(href));
  } catch {
    void openPath(href);
  }
  return true;
}
