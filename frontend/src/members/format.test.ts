import { describe, expect, it } from 'vitest'
import type { ProfilePick } from '../api.ts'
import { NAVI, row, VIT } from '../matches/test-fixtures.ts'
import { accuracyText, eventOf, isVoid, rankText, scoredParts, segmentOf } from './format.ts'

describe('scoredParts', () => {
  const pick = {
    winner_team_id: VIT.id,
    series: { a: 2, b: 1 },
    maps: [
      { map_number: 1, winner_team_id: VIT.id, rounds: { a: 13, b: 9 }, overtime: false },
      { map_number: 2, winner_team_id: NAVI.id, rounds: null, overtime: true },
      { map_number: 3, winner_team_id: VIT.id, rounds: null, overtime: false },
    ],
  }

  it('lists every part, map winners and map scores apart', () => {
    expect(scoredParts(pick, null, VIT, NAVI).map((p) => [p.label, p.value, p.score])).toEqual([
      ['Winner', 'VIT', null],
      ['Series', '2:1', null],
      ['Map 1', 'VIT', null],
      ['Map 1 score', '13:9', null],
      ['Map 2', 'NAVI', null],
      ['Map 2 score', 'OT', null],
      ['Map 3', 'VIT', null],
    ])
  })

  it('takes each part’s outcome from the breakdown', () => {
    const hit = { outcome: 'hit' as const, points: 3 }
    const miss = { outcome: 'miss' as const, points: 0 }
    const parts = scoredParts(
      pick,
      {
        winner: hit,
        series: miss,
        maps: [
          { map_number: 1, winner: hit, score: miss },
          { map_number: 2, winner: { outcome: 'void', points: 0 }, score: null },
        ],
      },
      VIT,
      NAVI,
    )
    expect(parts.map((p) => p.score?.outcome ?? null)).toEqual([
      'hit',
      'miss',
      'hit',
      'miss',
      'void',
      null,
      // Map 3 is missing from the breakdown.
      null,
    ])
  })

  it('names an unknown team with a question mark', () => {
    const parts = scoredParts({ winner_team_id: 99, series: null, maps: [] }, null, null, null)
    expect(parts[0]!.value).toBe('?')
  })
})

describe('isVoid', () => {
  const { my_pick: _p, my_result: _r, ...match } = row()
  void _p
  void _r
  const item = (over: Partial<ProfilePick> = {}): ProfilePick => ({
    match,
    pick: { winner_team_id: 1, series: null, maps: [] },
    updated_at: '2026-11-20T09:00:00Z',
    result: null,
    ...over,
  })
  const breakdown = { winner: null, series: null, maps: [] }
  const result = { points: 0, pending_parts: 0, breakdown }

  it('is void for a void result, a voided match or a canceled one', () => {
    expect(isVoid(item())).toBe(false)
    expect(isVoid(item({ result: { ...result, state: 'final' } }))).toBe(false)
    expect(isVoid(item({ result: { ...result, state: 'void' } }))).toBe(true)
    expect(isVoid(item({ match: { ...match, voided: true } }))).toBe(true)
    expect(isVoid(item({ match: { ...match, status: 'canceled' } }))).toBe(true)
  })
})

describe('labels', () => {
  it('shows accuracy as a percentage and a ratio', () => {
    expect(accuracyText({ hits: 3, decided: 4, rate: 0.75 })).toEqual({ pct: '75%', ratio: '3/4' })
    expect(accuracyText({ hits: 1, decided: 3, rate: 1 / 3 })).toEqual({ pct: '33%', ratio: '1/3' })
    expect(accuracyText({ hits: 0, decided: 0, rate: null })).toEqual({ pct: '–', ratio: '0/0' })
  })

  it('shows a rank or a dash', () => {
    expect(rankText(1)).toBe('#1')
    expect(rankText(null)).toBe('–')
  })
})

describe('URL parameters', () => {
  const counts = (open: number, live: number, settled: number) => ({ open, live, settled })

  it('takes the list from the URL, else the first with picks', () => {
    expect(segmentOf('settled', counts(1, 1, 1))).toBe('settled')
    expect(segmentOf(null, counts(1, 1, 1))).toBe('live')
    expect(segmentOf(null, counts(1, 0, 1))).toBe('open')
    expect(segmentOf('bogus', counts(0, 0, 1))).toBe('settled')
    expect(segmentOf(null, counts(0, 0, 0))).toBe('open')
  })

  it('reads a numeric event ID', () => {
    expect(eventOf('8')).toBe(8)
    expect(eventOf(null)).toBeNull()
    expect(eventOf('x8')).toBeNull()
  })
})
