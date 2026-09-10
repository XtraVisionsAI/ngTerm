import DOMPurify from 'dompurify'
import { Marked } from 'marked'

const marked = new Marked({
  breaks: true,
  gfm: true
})

// Agent output is untrusted (it can echo remote file contents or LLM text),
// so everything rendered through v-html goes through DOMPurify first.
const purifier = DOMPurify()

purifier.setConfig({
  USE_PROFILES: { html: true },
  ALLOWED_URI_REGEXP: /^(?:https?|mailto):/i,
  FORBID_TAGS: ['style', 'iframe', 'object', 'embed', 'form', 'input', 'button', 'textarea', 'select', 'svg', 'math'],
  FORBID_ATTR: ['style', 'srcset']
})

purifier.addHook('afterSanitizeAttributes', (node) => {
  if (node.tagName === 'A' && node.hasAttribute('href')) {
    node.setAttribute('target', '_blank')
    node.setAttribute('rel', 'noopener noreferrer')
  }
})

export function sanitizeHtml(html: string): string {
  return purifier.sanitize(html)
}

export function renderMarkdown(text: string): string {
  return sanitizeHtml(marked.parse(text) as string)
}
