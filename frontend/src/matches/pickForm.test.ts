import { describe, expect, it } from 'vitest'
import { ApiError } from '../api.ts'
import {
  chooseMapScore,
  chooseMapWinner,
  chooseSeries,
  chooseWinner,
  draftFromPick,
  emptyDraft,
  isEmptyDraft,
  mapScoreOptions,
  parseMapScoreValue,
  pickErrorMessage,
  pickHints,
  seriesOptions,
  toParts,
  type Draft,
} from './pickForm.ts'

const TEAMS = { a: 1, b: 2 }
const NAMES = { a: 'VIT', b: 'NAVI' }

describe('series options', () => {
  it('follow the format', () => {
    expect(seriesOptions('bo1')).toEqual([])
    expect(seriesOptions('bo3').map((s) => `${s.a}:${s.b}`)).toEqual(['2:0', '2:1', '1:2', '0:2'])
    expect(seriesOptions('bo5').map((s) => `${s.a}:${s.b}`)).toEqual([
      '3:0',
      '3:1',
      '3:2',
      '2:3',
      '1:3',
      '0:3',
    ])
  })
})

describe('winner and series', () => {
  it('a series score sets its winner', () => {
    const d = chooseSeries(emptyDraft('bo3'), { a: 1, b: 2 })
    expect(d.winner).toBe('b')
    expect(d.series).toEqual({ a: 1, b: 2 })
  })

  it('changing the winner clears a series that no longer fits', () => {
    let d = chooseSeries(emptyDraft('bo3'), { a: 2, b: 1 })
    d = chooseWinner(d, 'b')
    expect(d).toMatchObject({ winner: 'b', series: null })
  })

  it('clicking the picked winner or series again clears it', () => {
    let d = chooseSeries(emptyDraft('bo3'), { a: 2, b: 0 })
    d = chooseSeries(d, { a: 2, b: 0 })
    expect(d).toMatchObject({ winner: 'a', series: null })
    d = chooseWinner(d, 'a')
    expect(d.winner).toBeNull()
    expect(isEmptyDraft(d)).toBe(true)
  })

  it('clearing the winner also clears the series', () => {
    const d = chooseWinner(chooseSeries(emptyDraft('bo3'), { a: 2, b: 0 }), 'a')
    expect(d).toMatchObject({ winner: null, series: null })
  })
})

describe('map picks', () => {
  it('keep the score relative to the winner and convert to A:B', () => {
    let d = chooseMapWinner(emptyDraft('bo3'), 2, 'b')
    d = chooseMapScore(d, 2, 9)
    expect(toParts(d, TEAMS).maps).toEqual([
      { map_number: 2, winner_team_id: 2, rounds: { a: 9, b: 13 }, overtime: false },
    ])
    d = chooseMapWinner(d, 2, 'a')
    expect(toParts(d, TEAMS).maps?.[0]?.rounds).toEqual({ a: 13, b: 9 })
  })

  it('overtime replaces an exact score, and back', () => {
    let d = chooseMapScore(chooseMapWinner(emptyDraft('bo1'), 1, 'a'), 1, 11)
    d = chooseMapScore(d, 1, 'ot')
    expect(d.maps[0]).toEqual({ winner: 'a', loser: null, overtime: true })
    d = chooseMapScore(d, 1, null)
    expect(d.maps[0]).toEqual({ winner: 'a', loser: null, overtime: false })
  })

  it('no score without a winner; clearing the winner clears the score', () => {
    let d = chooseMapScore(emptyDraft('bo3'), 1, 5)
    expect(d.maps[0]?.loser).toBeNull()
    d = chooseMapScore(chooseMapWinner(d, 1, 'a'), 1, 5)
    d = chooseMapWinner(d, 1, 'a')
    expect(d.maps[0]).toEqual({ winner: null, loser: null, overtime: false })
  })

  it('score options cover 13:0 to 13:11 from the winner', () => {
    expect(mapScoreOptions('a').map((o) => o.label)).toEqual(
      Array.from({ length: 12 }, (_, i) => `13:${i}`),
    )
    expect(mapScoreOptions('b')[11]?.label).toBe('11:13')
    expect(parseMapScoreValue('ot')).toBe('ot')
    expect(parseMapScoreValue('')).toBeNull()
    expect(parseMapScoreValue('7')).toBe(7)
    expect(parseMapScoreValue('12')).toBeNull()
  })
})

describe('saved picks', () => {
  it('round-trip through the draft', () => {
    const saved = {
      winner_team_id: 2,
      series: { a: 1, b: 2 },
      maps: [
        { map_number: 1, winner_team_id: 1, rounds: { a: 13, b: 4 }, overtime: false },
        { map_number: 3, winner_team_id: 2, rounds: null, overtime: true },
      ],
    }
    const d = draftFromPick('bo3', TEAMS, saved)
    expect(d.winner).toBe('b')
    expect(d.maps[1]?.winner).toBeNull()
    expect(toParts(d, TEAMS)).toEqual(saved)
  })

  it('an empty draft has no parts', () => {
    expect(toParts(emptyDraft('bo5'), TEAMS)).toEqual({
      winner_team_id: null,
      series: null,
      maps: [],
    })
    expect(draftFromPick('bo5', TEAMS, null).maps).toHaveLength(5)
  })
})

describe('soft hints', () => {
  const withMaps = (d: Draft, winners: ('a' | 'b')[]) =>
    winners.reduce((acc, w, i) => chooseMapWinner(acc, i + 1, w), d)

  it('map tally above the series pick', () => {
    const d = withMaps(chooseSeries(emptyDraft('bo3'), { a: 2, b: 0 }), ['a', 'b', 'a'])
    expect(pickHints('bo3', d, NAMES)).toEqual([
      'Your map picks add up to 2:1, your series pick is 2:0.',
    ])
  })

  it('a picked map the series would not reach', () => {
    let d = chooseSeries(emptyDraft('bo3'), { a: 2, b: 0 })
    d = chooseMapWinner(d, 3, 'a')
    expect(pickHints('bo3', d, NAMES)).toEqual([
      "With a 2:0 series, map 3 won't be played, but you picked it.",
    ])
  })

  it('map picks deciding the series for the other team', () => {
    const d = withMaps(chooseWinner(emptyDraft('bo3'), 'a'), ['b', 'b'])
    expect(pickHints('bo3', d, NAMES)).toEqual([
      'Your map picks have NAVI winning, but your winner pick is VIT.',
    ])
    const bo1 = withMaps(chooseWinner(emptyDraft('bo1'), 'b'), ['a'])
    expect(pickHints('bo1', bo1, NAMES)).toHaveLength(1)
  })

  it('none when they agree or are partial', () => {
    const fits = withMaps(chooseSeries(emptyDraft('bo3'), { a: 2, b: 1 }), ['a', 'b', 'a'])
    expect(pickHints('bo3', fits, NAMES)).toEqual([])
    const partial = withMaps(chooseSeries(emptyDraft('bo3'), { a: 2, b: 0 }), ['a'])
    expect(pickHints('bo3', partial, NAMES)).toEqual([])
    expect(pickHints('bo3', chooseWinner(emptyDraft('bo3'), 'a'), NAMES)).toEqual([])
  })
})

describe('error messages', () => {
  it('map the pick API codes', () => {
    expect(pickErrorMessage(new ApiError(409, 'pick_locked', 'locked'))).toMatch(/locked/)
    expect(pickErrorMessage(new ApiError(409, 'not_pickable', 'x'))).toBe(
      'Picks are not open for this match.',
    )
    expect(pickErrorMessage(new ApiError(422, 'invalid_pick', '2:0 is not allowed'))).toBe(
      "That pick isn't valid: 2:0 is not allowed",
    )
    expect(pickErrorMessage(new Error('network'))).toMatch(/could not be saved/)
  })
})
