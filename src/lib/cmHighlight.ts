// Syntax highlighting for the file-preview text editor (CodeMirror).
// Separate from markdown `--md-*` reading colors: those style rendered
// markdown in chat bubbles / the preview's markdown pane; this file only
// colours source tokens in the text editor.
import type { Extension } from '@codemirror/state';
import { tags as t } from '@lezer/highlight';
import { createTheme, dracula } from 'thememirror';

/** Dark-ink tokens on a light pane (pure theme). Contrast is the point:
 * thememirror's bundled light themes wash comments/strings out on white. */
const previewLight = createTheme({
  variant: 'light',
  settings: {
    background: 'transparent',
    foreground: '#1f242c',
    caret: '#1f242c',
    selection: '#2f6fd038',
    lineHighlight: '#0f172a0d',
    gutterBackground: 'transparent',
    gutterForeground: '#5b6573',
  },
  styles: [
    { tag: t.comment, color: '#5b6573' },
    { tag: [t.string, t.special(t.brace), t.regexp], color: '#0a3069' },
    { tag: [t.number, t.bool, t.null], color: '#0550ae' },
    { tag: [t.keyword, t.operator], color: '#b42318' },
    { tag: [t.definitionKeyword, t.modifier], color: '#b42318', fontWeight: 'bold' },
    { tag: t.typeName, color: '#116329' },
    { tag: [t.className, t.definition(t.typeName)], color: '#116329' },
    { tag: [t.tagName, t.self], color: '#116329', fontWeight: 'bold' },
    { tag: t.attributeName, color: '#0550ae' },
    { tag: [t.function(t.variableName), t.definition(t.propertyName)], color: '#6639ba' },
    { tag: t.variableName, color: '#1f242c' },
    { tag: t.propertyName, color: '#1f242c' },
    { tag: t.angleBracket, color: '#5b6573' },
    { tag: t.punctuation, color: '#1f242c' },
    { tag: t.meta, color: '#5b6573' },
    { tag: t.link, color: '#0969da' },
    { tag: t.heading, color: '#0d1117', fontWeight: 'bold' },
    { tag: t.strong, fontWeight: 'bold' },
    { tag: t.emphasis, fontStyle: 'italic' },
  ],
});

/** war → Dracula (dark pane); pure → high-contrast light tokens. */
export function previewSyntax(plain: boolean): Extension {
  return plain ? previewLight : dracula;
}
