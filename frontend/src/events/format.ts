import type { EventInfo, EventStatus, EventSummary, StageMatch, StageView } from '../api.ts'

/** Display helpers for events; dates in the viewer's time zone. */

const dateFmt = new Intl.DateTimeFormat(undefined, { day: 'numeric', month: 'short' })

/** "20 Nov – 23 Nov", "from 20 Nov", or `null` without dates. */
export function formatDateRange(e: Pick<EventInfo, 'begin_at' | 'end_at'>): string | null {
  const begin = e.begin_at ? dateFmt.format(new Date(e.begin_at)) : null
  const end = e.end_at ? dateFmt.format(new Date(e.end_at)) : null
  if (begin && end) return begin === end ? begin : `${begin} – ${end}`
  if (begin) return `from ${begin}`
  if (end) return `until ${end}`
  return null
}

export const STATUS_LABEL: Record<EventStatus, string> = {
  running: 'Running',
  upcoming: 'Upcoming',
  finished: 'Finished',
  canceled: 'Canceled',
}

/** "S-tier"; `null` when unknown. */
export function tierLabel(tier: EventInfo['tier']): string | null {
  return tier === 'unknown' ? null : `${tier.toUpperCase()}-tier`
}

export interface EventGroup {
  key: 'running' | 'upcoming' | 'finished'
  label: string
  events: EventSummary[]
}

/** The list's sections, in the server's order; canceled counts as finished. */
export function groupEvents(events: EventSummary[]): EventGroup[] {
  const groups: EventGroup[] = [
    { key: 'running', label: 'Running', events: [] },
    { key: 'upcoming', label: 'Upcoming', events: [] },
    { key: 'finished', label: 'Finished', events: [] },
  ]
  for (const e of events) {
    const key = e.status === 'canceled' ? 'finished' : e.status
    groups.find((g) => g.key === key)!.events.push(e)
  }
  return groups.filter((g) => g.events.length > 0)
}

const OPEN = new Set(['scheduled', 'postponed'])

/**
 * The stage to open first: one with a live match, else the first with a
 * match still to play, else the last.
 */
export function defaultStage(stages: StageView[]): StageView | undefined {
  return (
    stages.find((s) => s.matches.some((m) => m.status === 'live')) ??
    stages.find((s) => s.matches.some((m) => OPEN.has(m.status))) ??
    stages.at(-1)
  )
}

/** The key of a stage in the `stage` search parameter. */
export function stageKey(s: StageView): string {
  return s.id === null ? 'none' : String(s.id)
}

/** Matches grouped by round, ascending; matches without one last. */
export function byRound(matches: StageMatch[]): { round: number | null; matches: StageMatch[] }[] {
  const rounds = new Map<number | null, StageMatch[]>()
  for (const m of matches) {
    const list = rounds.get(m.round) ?? []
    list.push(m)
    rounds.set(m.round, list)
  }
  return [...rounds.entries()]
    .sort(([a], [b]) => (a ?? Infinity) - (b ?? Infinity))
    .map(([round, ms]) => ({ round, matches: ms }))
}

/** "+3", "0", "−2". */
export function signed(n: number): string {
  return n > 0 ? `+${n}` : n < 0 ? `−${-n}` : '0'
}
