/** Diff of a proposed file change as computed server-side (core `file_ops`). */
export interface DiffPreview {
  added: number
  removed: number
  /** Unified-style lines: `+`, `-`, ` ` prefixed, or `@@ … @@` markers. */
  lines: string[]
  truncated: boolean
}

/** `change_preview` event / approval snapshot `preview` field. */
export interface ChangePreview {
  tool?: string
  path: string
  exists?: boolean
  /** Baseline binding: sha256 of the current content, `new-file` or `unhashed`. */
  baseline?: string
  size?: number
  newBytes?: number
  newLines?: number
  diff?: DiffPreview | null
  reason?: string | null
  backup?: boolean
  /** The server could not resolve or read the target; nothing is bound. */
  unavailable?: boolean
}

export function isChangePreview(v: unknown): v is ChangePreview {
  return !!v && typeof v === 'object' && typeof (v as ChangePreview).path === 'string'
}

export function diffLineClass(line: string): string {
  if (line.startsWith('+')) return 'diff-add'
  if (line.startsWith('-')) return 'diff-del'
  if (line.startsWith('@@')) return 'diff-hunk'
  return 'diff-ctx'
}
