import type {
  BracketSide,
  EventDetail,
  EventSummary,
  FeedRef,
  StageMatch,
  StageView,
  TeamRef,
} from '../api.ts'
import { row } from '../matches/test-fixtures.ts'

/** Test data for the event views, shaped like the fixture scenario. */

function team(id: number, name: string, acronym: string): TeamRef {
  return { id, name, acronym, logo_url: null }
}

export const AUR = team(1, 'Aurora Wolves', 'AUR')
export const BAS = team(2, 'Basalt Gaming', 'BAS')
export const CIN = team(3, 'Cinder Esports', 'CIN')
export const DUN = team(4, 'Dune Raiders', 'DUN')
export const ECK = team(5, 'Echo Kings', 'ECK')
export const FJO = team(6, 'Fjord Five', 'FJO')
export const GLA = team(7, 'Glacier Union', 'GLA')
export const HAL = team(8, 'Halcyon Club', 'HAL')

export const w = (match_id: number): FeedRef => ({ match_id, outcome: 'winner' })
export const l = (match_id: number): FeedRef => ({ match_id, outcome: 'loser' })

/** A match of a bracket stage. */
export function bm(
  id: number,
  side: BracketSide,
  round: number,
  slot: number,
  feeds: [FeedRef | null, FeedRef | null] = [null, null],
  over: Partial<StageMatch> = {},
): StageMatch {
  return {
    ...row({
      id,
      event: { id: 100, name: 'Fixture Masters 2026' },
      stage: { id: 2, name: 'Playoffs' },
      team_a: null,
      team_b: null,
    }),
    bracket: { side, round, slot, derived: false, feeds_a: feeds[0], feeds_b: feeds[1] },
    round: null,
    ...over,
  }
}

/**
 * The fixture's double-elimination playoff: 101/102 upper semis, 103 upper
 * final, 104 lower round 1, 105 lower final, 106 grand final. The semis
 * are done; the rest is TBD.
 */
export function playoffMatches(): StageMatch[] {
  const done = { status: 'finished' as const, locked: true, pickable: false }
  return [
    bm(101, 'upper', 0, 0, [null, null], {
      ...done,
      team_a: AUR,
      team_b: FJO,
      maps_won_a: 2,
      winner_team_id: AUR.id,
    }),
    bm(102, 'upper', 0, 1, [null, null], {
      ...done,
      team_a: GLA,
      team_b: CIN,
      maps_won_a: 1,
      maps_won_b: 2,
      winner_team_id: CIN.id,
      my_pick: 'full',
      my_result: { points: 7, state: 'final', pending_parts: 0 },
    }),
    bm(103, 'upper', 1, 0, [w(101), w(102)], { pickable: false }),
    bm(104, 'lower', 0, 0, [l(101), l(102)], { pickable: false }),
    bm(105, 'lower', 1, 0, [l(103), w(104)], { pickable: false }),
    bm(106, 'final', 2, 0, [w(103), w(105)], { format: 'bo5', pickable: false }),
  ]
}

function swissMatch(
  id: number,
  round: number,
  a: TeamRef,
  b: TeamRef,
  over: Partial<StageMatch> = {},
) {
  return {
    ...row({
      id,
      event: { id: 100, name: 'Fixture Masters 2026' },
      stage: { id: 1, name: 'Swiss Stage' },
      team_a: a,
      team_b: b,
      format: 'bo1',
      status: 'finished',
      locked: true,
      pickable: false,
    }),
    bracket: null,
    round,
    ...over,
  } satisfies StageMatch
}

export function swissStage(): StageView {
  const rec = (t: TeamRef, wins: number, losses: number, map_diff: number, round_diff: number) => ({
    team: t,
    wins,
    losses,
    map_diff,
    round_diff,
  })
  return {
    id: 1,
    name: 'Swiss Stage',
    kind: 'swiss',
    view: 'standings',
    standings: [
      rec(CIN, 2, 0, 3, 13),
      rec(AUR, 2, 0, 2, 11),
      rec(GLA, 1, 1, 0, -2),
      rec(DUN, 0, 0, 0, 0),
      rec(ECK, 0, 0, 0, 0),
      rec(FJO, 0, 1, -1, -2),
      rec(BAS, 0, 1, -1, -5),
      rec(HAL, 0, 2, -3, -15),
    ],
    matches: [
      swissMatch(1, 1, AUR, HAL, { maps_won_a: 1, winner_team_id: AUR.id }),
      swissMatch(2, 1, BAS, GLA, { maps_won_b: 1, winner_team_id: GLA.id }),
      swissMatch(3, 1, CIN, FJO, { maps_won_a: 1, winner_team_id: CIN.id }),
      swissMatch(4, 1, DUN, ECK, { status: 'canceled' }),
      swissMatch(5, 2, AUR, GLA, {
        format: 'bo3',
        maps_won_a: 2,
        maps_won_b: 1,
        winner_team_id: AUR.id,
      }),
      swissMatch(6, 2, CIN, HAL, { format: 'bo3', maps_won_a: 2, winner_team_id: CIN.id }),
    ],
  }
}

export function playoffStage(matches = playoffMatches()): StageView {
  return {
    id: 2,
    name: 'Playoffs',
    kind: 'double_elim',
    view: 'bracket',
    standings: null,
    matches,
  }
}

export function eventDetail(over: Partial<EventDetail> = {}): EventDetail {
  return {
    event: {
      id: 100,
      name: 'Fixture Masters 2026',
      tier: 's',
      status: 'running',
      begin_at: '2026-11-20T10:00:00.000Z',
      end_at: null,
    },
    followed: true,
    stages: [swissStage(), playoffStage()],
    ...over,
  }
}

export function summary(over: Partial<EventSummary> = {}): EventSummary {
  return {
    id: 100,
    name: 'Fixture Masters 2026',
    tier: 's',
    status: 'running',
    begin_at: '2026-11-20T10:00:00.000Z',
    end_at: '2026-11-23T20:00:00.000Z',
    matches: 12,
    live: 0,
    picks: 3,
    ...over,
  }
}

/**
 * A 16-team double elimination: 4 upper rounds (8, 4, 2, 1), 6 lower
 * rounds (4, 4, 2, 2, 1, 1) and the grand final, with the feeds a source
 * would send. IDs from 1.
 */
export function de16Matches(): StageMatch[] {
  const out: StageMatch[] = []
  let id = 1
  type Feeds = [FeedRef, FeedRef] | null
  const add = (side: BracketSide, round: number, slot: number, feeds: Feeds) => {
    out.push(bm(id, side, round, slot, feeds ?? [null, null]))
    return id++
  }
  const ub: number[][] = [[], [], [], []]
  for (let s = 0; s < 8; s++) ub[0]!.push(add('upper', 0, s, null))
  for (let r = 1; r < 4; r++) {
    const prev = ub[r - 1]!
    for (let s = 0; s < prev.length / 2; s++) {
      ub[r]!.push(add('upper', r, s, [w(prev[2 * s]!), w(prev[2 * s + 1]!)]))
    }
  }
  const lb: number[][] = []
  // Round 1: losers of upper round 1.
  lb.push([0, 1, 2, 3].map((s) => add('lower', 0, s, [l(ub[0]![2 * s]!), l(ub[0]![2 * s + 1]!)])))
  // Drop-in rounds take the upper losers; the others pair lower winners.
  const plan: ('drop' | 'pair')[] = ['drop', 'pair', 'drop', 'pair', 'drop']
  let ubRound = 1
  plan.forEach((step, i) => {
    const prev = lb[i]!
    const round = i + 1
    if (step === 'drop') {
      const losers = ub[ubRound++]!
      lb.push(prev.map((m, s) => add('lower', round, s, [w(m), l(losers[losers.length - 1 - s]!)])))
    } else {
      lb.push(
        Array.from({ length: prev.length / 2 }, (_, s) =>
          add('lower', round, s, [w(prev[2 * s]!), w(prev[2 * s + 1]!)]),
        ),
      )
    }
  })
  add('final', 0, 0, [w(ub[3]![0]!), w(lb[5]![0]!)])
  return out
}
