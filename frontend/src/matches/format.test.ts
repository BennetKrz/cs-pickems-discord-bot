import { describe, expect, it } from 'vitest'
import {
  dayLabel,
  formatCountdown,
  formatTime,
  groupByDayAndEvent,
  localDayKey,
} from './format.ts'
import { row } from './test-fixtures.ts'

// Tests run with TZ=Europe/Berlin (vite.config.ts): UTC+1 in November.
const NOW = Date.parse('2026-11-20T12:00:00Z')

describe('local time', () => {
  it('runs in a non-UTC zone', () => {
    expect(new Date(NOW).getTimezoneOffset()).toBe(-60)
  })

  it('shows times in the local zone', () => {
    expect(formatTime('2026-11-20T17:00:00Z')).toMatch(/^(18:00|06:00\sPM)$/)
  })

  it('puts a late UTC time on the next local day', () => {
    expect(localDayKey(Date.parse('2026-11-20T23:30:00Z'))).toBe('2026-11-21')
    expect(dayLabel(Date.parse('2026-11-20T23:30:00Z'), NOW)).toBe('Tomorrow')
    expect(dayLabel(Date.parse('2026-11-20T22:59:00Z'), NOW)).toBe('Today')
    expect(dayLabel(Date.parse('2026-11-19T10:00:00Z'), NOW)).toBe('Yesterday')
    expect(dayLabel(Date.parse('2026-11-28T10:00:00Z'), NOW)).not.toMatch(/Today|Tomorrow/)
  })
})

describe('countdown', () => {
  it('formats hours, minutes and seconds', () => {
    expect(formatCountdown((3 * 3600 + 12 * 60 + 30) * 1000)).toBe('3 h 12 min')
    expect(formatCountdown((12 * 60 + 5) * 1000)).toBe('12 min 05 s')
    expect(formatCountdown(45_500)).toBe('45 s')
    expect(formatCountdown(-5)).toBe('0 s')
    expect(formatCountdown(45_000, true)).toBe('< 1 min')
    expect(formatCountdown(12 * 60_000 + 5_000, true)).toBe('12 min')
  })
})

describe('grouping', () => {
  it('groups by local day, then by event, in the given order', () => {
    const rows = [
      row({ id: 1, scheduled_at: '2026-11-20T15:00:00Z' }),
      row({ id: 2, scheduled_at: '2026-11-20T16:00:00Z', event: { id: 200, name: 'Other' } }),
      row({ id: 3, scheduled_at: '2026-11-20T17:00:00Z' }),
      row({ id: 4, scheduled_at: '2026-11-20T23:30:00Z' }),
    ]
    const days = groupByDayAndEvent(rows, 'upcoming', NOW)
    expect(days.map((d) => d.label)).toEqual(['Today', 'Tomorrow'])
    expect(days[0]?.events.map((e) => [e.eventName, e.rows.map((r) => r.id)])).toEqual([
      ['IEM Test', [1, 3]],
      ['Other', [2]],
    ])
    expect(days[1]?.events[0]?.rows.map((r) => r.id)).toEqual([4])
  })

  it('uses the end time for results', () => {
    const r = row({
      status: 'finished',
      scheduled_at: '2026-11-19T21:00:00Z',
      ended_at: '2026-11-19T23:10:00Z',
    })
    expect(groupByDayAndEvent([r], 'finished', NOW)[0]?.label).toBe('Today')
  })
})
