/**
 * Client-side redaction for text the user is about to send to an AI agent as
 * context. The patterns mirror the server's audit redaction
 * (`audit_events::redact`) so what the user sees masked here matches what the
 * audit trail masks. This is a safety net for common shapes (bearer tokens,
 * `password=`, `sk-` keys, AWS access keys, credentials in URLs); it does not
 * claim to find every secret. The user reviews the result before sending.
 */

const RULES: Array<{ re: RegExp; to: string }> = [
  // `(?!\[REDACTED\])` keeps already-masked values from matching again, so
  // `looksSensitive` goes quiet once the text has been redacted.
  { re: /(authorization:\s*bearer\s+)(?!\[REDACTED\])\S+/gi, to: '$1[REDACTED]' },
  { re: /((?:password|passwd|pwd|token|secret|api[_-]?key)\s*[=:]\s*)(?!\[REDACTED\])\S+/gi, to: '$1[REDACTED]' },
  { re: /(--?(?:password|token|api-key|secret)[= ])(?!\[REDACTED\])\S+/gi, to: '$1[REDACTED]' },
  { re: /\bsk-[A-Za-z0-9_-]{8,}/g, to: '[REDACTED]' },
  { re: /\bAKIA[0-9A-Z]{16}\b/g, to: '[REDACTED]' },
  { re: /(https?:\/\/[^/\s:]+:)(?!\[REDACTED\]@)[^@\s]+@/gi, to: '$1[REDACTED]@' },
  // Private key blocks: the whole body between the markers.
  {
    re: /(-----BEGIN [A-Z ]*PRIVATE KEY-----)(?!\s*\[REDACTED\]\s*-----END)[\s\S]*?(-----END [A-Z ]*PRIVATE KEY-----)/g,
    to: '$1\n[REDACTED]\n$2'
  }
]

export interface RedactResult {
  text: string
  /** Number of replacements made. */
  count: number
}

export function redactText(input: string): RedactResult {
  let text = input
  let count = 0
  for (const { re, to } of RULES) {
    text = text.replace(re, (...args) => {
      count++
      // Rebuild the replacement with capture groups, like String.replace does.
      const groups = args.slice(1, -2) as string[]
      return to.replace(/\$(\d)/g, (_, n) => groups[Number(n) - 1] ?? '')
    })
  }
  return { text, count }
}

/** Whether the text still contains something the rules would mask. */
export function looksSensitive(input: string): boolean {
  return RULES.some(({ re }) => {
    re.lastIndex = 0
    const hit = re.test(input)
    re.lastIndex = 0
    return hit
  })
}
