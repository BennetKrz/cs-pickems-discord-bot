import { describe, expect, it } from 'vitest'
import type { LeaderboardRow } from '../api.ts'
import { accuracyLabel, matchDayLabel, picksText, pointsText, rankChange } from './format.ts'

function row(rank: number, previous_rank: number | null): LeaderboardRow {
  return {
    rank,
    previous_rank,
    user_id: '1',
    display_name: 'x',
    avatar_url: '',
    points: 0,
    provisional_points: 0,
    provisional_picks: 0,
    settled_picks: 1,
    winner_hits: 0,
    winner_decided: 0,
    winner_accuracy: null,
    series_hits: 0,
    map_hits: 0,
  }
}

describe('rankChange', () => {
  it('compares with the rank before the last match day', () => {
    expect(rankChange(row(1, 3), true)).toEqual({
      kind: 'up',
      short: '▲2',
      label: 'Up 2 since the last match day',
    })
    expect(rankChange(row(4, 1), true)).toMatchObject({ kind: 'down', short: '▼3' })
    expect(rankChange(row(2, 2), true)).toMatchObject({ kind: 'same', short: '–' })
    expect(rankChange(row(5, null), true)).toMatchObject({ kind: 'new', short: 'new' })
  })

  it('has nothing to show without an earlier match day', () => {
    expect(rankChange(row(1, null), false)).toBeNull()
    expect(rankChange(row(1, 3), false)).toBeNull()
  })
})

describe('labels', () => {
  it('formats accuracy, points and picks', () => {
    expect(accuracyLabel(null)).toBe('–')
    expect(accuracyLabel(0)).toBe('0%')
    expect(accuracyLabel(2 / 3)).toBe('67%')
    expect(accuracyLabel(1)).toBe('100%')
    expect(pointsText(1)).toBe('1 pt')
    expect(pointsText(0)).toBe('0 pts')
    expect(pointsText(17)).toBe('17 pts')
    expect(picksText(1)).toBe('1 pick')
    expect(picksText(4)).toBe('4 picks')
  })

  it('shows the match day as the calendar date it is', () => {
    // No time zone shift: 20 Nov stays 20 Nov.
    expect(matchDayLabel('2026-11-20')).toMatch(/20/)
    expect(matchDayLabel('2026-11-20')).toMatch(/Nov/)
    expect(matchDayLabel('garbage')).toBe('garbage')
  })
})
