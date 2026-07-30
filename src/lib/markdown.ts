// Markdown rendering for chat bubbles and the preview dialog (markdown-it).
// Red line R1 (design-principles.md): rendering happens ONCE per finished
// text segment — never during streaming. The instance is shared and
// configured safe: raw HTML disabled, links linkified.
import MarkdownIt from 'markdown-it';

const md = new MarkdownIt({
  html: false,
  linkify: true,
  breaks: false,
});

export function renderMarkdown(text: string): string {
  return md.render(text);
}
