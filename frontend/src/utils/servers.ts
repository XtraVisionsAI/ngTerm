import type { TagType } from '@/utils/audit'

/**
 * Environment marker derived from server tags. `env:<name>` is the explicit
 * form; a few bare words are recognised for convenience. Anything else is a
 * plain tag.
 */
export interface EnvBadge {
  label: string
  type: TagType
}

const ENV_ALIASES: Record<string, { label: string; type: TagType }> = {
  prod: { label: 'PROD', type: 'error' },
  production: { label: 'PROD', type: 'error' },
  staging: { label: 'STAGING', type: 'warning' },
  stage: { label: 'STAGING', type: 'warning' },
  uat: { label: 'UAT', type: 'warning' },
  test: { label: 'TEST', type: 'info' },
  qa: { label: 'QA', type: 'info' },
  dev: { label: 'DEV', type: 'success' },
  development: { label: 'DEV', type: 'success' }
}

export function envBadge(tags: string[] | undefined): EnvBadge | null {
  for (const raw of tags ?? []) {
    const t = raw.trim().toLowerCase()
    const name = t.startsWith('env:') ? t.slice(4) : t
    const known = ENV_ALIASES[name]
    if (known) return known
    if (t.startsWith('env:') && name) return { label: name.toUpperCase(), type: 'default' }
  }
  return null
}

/** Tags other than the environment marker. */
export function plainTags(tags: string[] | undefined): string[] {
  return (tags ?? []).filter((t) => {
    const l = t.trim().toLowerCase()
    return !l.startsWith('env:') && !ENV_ALIASES[l]
  })
}

/** Case-insensitive match against alias, host, user, group and tags. */
export function matchesServer(
  s: { alias: string; host: string; username: string; groupName: string; tags: string[] },
  query: string
): boolean {
  const q = query.trim().toLowerCase()
  if (!q) return true
  return (
    s.alias.toLowerCase().includes(q) ||
    s.host.toLowerCase().includes(q) ||
    s.username.toLowerCase().includes(q) ||
    s.groupName.toLowerCase().includes(q) ||
    s.tags.some((t) => t.toLowerCase().includes(q))
  )
}
