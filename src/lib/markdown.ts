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
