/**
 * Where a "back" link should lead. Views that open a match pass it as
 * router state (`<Link state={{ back }}>`), so the match detail can return
 * to the bracket instead of the Matches tab (T13).
 */
export interface BackTo {
  to: string
  label: string
}

/** The `back` entry of a router location's state, if it has a valid one. */
export function backFrom(state: unknown): BackTo | null {
  if (typeof state !== 'object' || state === null || !('back' in state)) return null
  const back = (state as { back: unknown }).back
  if (typeof back !== 'object' || back === null) return null
  const { to, label } = back as Record<string, unknown>
  return typeof to === 'string' && typeof label === 'string' ? { to, label } : null
}
