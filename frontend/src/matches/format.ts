import type { MatchFormat, MatchHeader, MatchSegment, TeamRef } from '../api.ts'

/**
 * Display helpers for matches. The API sends UTC; everything here formats
 * in the viewer's time zone and locale (`Intl` with the default locale).
 */

const timeFmt = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' })
const dayFmt = new Intl.DateTimeFormat(undefined, {
  weekday: 'short',
  day: 'numeric',
  month: 'short',
})
const dateTimeFmt = new Intl.DateTimeFormat(undefined, {
  weekday: 'short',
  day: 'numeric',
  month: 'short',
  hour: '2-digit',
  minute: '2-digit',
})

/** Local time of day, e.g. "18:00" or "06:00 PM". */
export function formatTime(iso: string): string {
  return timeFmt.format(new Date(iso))
}

/** Local weekday, date and time. */
export function formatDateTime(iso: string): string {
  return dateTimeFmt.format(new Date(iso))
}

/** The local calendar day of `ms`, as `YYYY-MM-DD`. */
export function localDayKey(ms: number): string {
  const d = new Date(ms)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`
}

/** "Today", "Tomorrow", "Yesterday", or the local weekday and date. */
export function dayLabel(ms: number, nowMs: number): string {
  const day = localDayKey(ms)
  const offset = (days: number) => {
    const d = new Date(nowMs)
    d.setDate(d.getDate() + days)
    return localDayKey(d.getTime())
  }
  if (day === offset(0)) return 'Today'
  if (day === offset(1)) return 'Tomorrow'
  if (day === offset(-1)) return 'Yesterday'
  return dayFmt.format(new Date(ms))
}

/**
 * Time left until a lock: "3 h 12 min", "12 min 05 s", "45 s". With
 * `coarse`, nothing below a minute: "< 1 min".
 */
export function formatCountdown(ms: number, coarse = false): string {
  const total = Math.max(0, Math.floor(ms / 1000))
  const h = Math.floor(total / 3600)
  const m = Math.floor((total % 3600) / 60)
  const s = total % 60
  if (h > 0) return `${h} h ${m} min`
  if (coarse) return m > 0 ? `${m} min` : '< 1 min'
  if (m > 0) return `${m} min ${String(s).padStart(2, '0')} s`
  return `${s} s`
}

/** Show a lock countdown when the lock is less than this far away. */
export const COUNTDOWN_WINDOW_MS = 24 * 3600 * 1000

/** "BO3". */
export function formatLabel(format: MatchFormat): string {
  return format.toUpperCase()
}

/** Short team name: acronym, else name; "TBD" while unknown. */
export function teamShort(t: TeamRef | null): string {
  return t ? (t.acronym ?? t.name) : 'TBD'
}

export function teamName(t: TeamRef | null): string {
  return t ? t.name : 'TBD'
}

/** The time a row is grouped and sorted by, per segment (as the API sorts). */
export function rowTime(m: MatchHeader, segment: MatchSegment): string | null {
  switch (segment) {
    case 'upcoming':
      return m.scheduled_at
    case 'live':
      return m.started_at ?? m.scheduled_at
    case 'finished':
      return m.ended_at ?? m.started_at ?? m.scheduled_at
  }
}

export interface EventGroup<T> {
  eventId: number
  eventName: string
  rows: T[]
}

export interface DayGroup<T> {
  key: string
  label: string
  events: EventGroup<T>[]
}

/**
 * Groups rows by local day, then by event, keeping the order the rows come
 * in (the API's order). Rows without a time go into a "No date" group.
 */
export function groupByDayAndEvent<T extends MatchHeader>(
  rows: T[],
  segment: MatchSegment,
  nowMs: number,
): DayGroup<T>[] {
  const days: DayGroup<T>[] = []
  for (const row of rows) {
    const iso = rowTime(row, segment)
    const ms = iso ? Date.parse(iso) : Number.NaN
    const key = Number.isNaN(ms) ? 'none' : localDayKey(ms)
    let day = days.find((d) => d.key === key)
    if (!day) {
      day = { key, label: Number.isNaN(ms) ? 'No date' : dayLabel(ms, nowMs), events: [] }
      days.push(day)
    }
    let ev = day.events.find((e) => e.eventId === row.event.id)
    if (!ev) {
      ev = { eventId: row.event.id, eventName: row.event.name, rows: [] }
      day.events.push(ev)
    }
    ev.rows.push(row)
  }
  return days
}
