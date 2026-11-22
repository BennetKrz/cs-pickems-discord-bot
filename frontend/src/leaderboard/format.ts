import type { LeaderboardRow } from '../api.ts'

/** How a member's rank moved since the previous match day. */
export interface RankChange {
  kind: 'up' | 'down' | 'same' | 'new'
  /** Short visible text, e.g. `▲2`. */
  short: string
  /** Full text for screen readers and tooltips. */
  label: string
}

/**
 * The rank change of a row, or `null` when there is nothing to compare
 * with (no standing before the latest match day).
 */
export function rankChange(row: LeaderboardRow, hasPrevious: boolean): RankChange | null {
  if (!hasPrevious) return null
  if (row.previous_rank === null) {
    return { kind: 'new', short: 'new', label: 'New since the last match day' }
  }
  const delta = row.previous_rank - row.rank
  if (delta > 0) {
    return { kind: 'up', short: `▲${delta}`, label: `Up ${delta} since the last match day` }
  }
  if (delta < 0) {
    return {
      kind: 'down',
      short: `▼${-delta}`,
      label: `Down ${-delta} since the last match day`,
    }
  }
  return { kind: 'same', short: '–', label: 'Same rank as before the last match day' }
}

/** Winner accuracy as a whole percentage, `–` before any is decided. */
export function accuracyLabel(accuracy: number | null): string {
  return accuracy === null ? '–' : `${Math.round(accuracy * 100)}%`
}

/** `17 pts`, `1 pt`. */
export function pointsText(points: number): string {
  return `${points} ${Math.abs(points) === 1 ? 'pt' : 'pts'}`
}

/** `4 picks`, `1 pick`. */
export function picksText(n: number): string {
  return `${n} ${n === 1 ? 'pick' : 'picks'}`
}

const dayFmt = new Intl.DateTimeFormat(undefined, {
  weekday: 'short',
  day: 'numeric',
  month: 'short',
})

/**
 * A `YYYY-MM-DD` calendar date as a short label, e.g. `Fri, 20 Nov`. It is
 * a date, not an instant, so it is not converted between time zones.
 */
export function matchDayLabel(day: string): string {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(day)
  if (!m) return day
  return dayFmt.format(new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3])))
}
