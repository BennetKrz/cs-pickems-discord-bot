import type {
  MatchDetail,
  MatchList,
  MatchPicks,
  MatchRow,
  PickStats,
  PickView,
  TeamRef,
} from '../api.ts'
import { ME } from '../test-utils.ts'

/** Test data for the match views. Times are UTC on 2026-11-20. */

export const VIT: TeamRef = {
  id: 1,
  name: 'Vitality',
  acronym: 'VIT',
  logo_url: '/api/img/team/1?v=abc',
}
export const NAVI: TeamRef = { id: 2, name: 'Natus Vincere', acronym: 'NAVI', logo_url: null }

export function row(over: Partial<MatchRow> = {}): MatchRow {
  return {
    id: 10,
    event: { id: 100, name: 'IEM Test' },
    stage: { id: 7, name: 'Playoffs' },
    team_a: VIT,
    team_b: NAVI,
    format: 'bo3',
    status: 'scheduled',
    voided: false,
    scheduled_at: '2026-11-20T18:00:00.000Z',
    started_at: null,
    ended_at: null,
    lock_at: '2026-11-20T18:00:00.000Z',
    locked: false,
    pickable: true,
    maps_won_a: 0,
    maps_won_b: 0,
    winner_team_id: null,
    current_map: null,
    pick_count: 0,
    my_pick: 'none',
    my_result: null,
    ...over,
  }
}

export function list(matches: MatchRow[], over: Partial<MatchList> = {}): MatchList {
  return { state: 'upcoming', following_any: true, matches, next_cursor: null, ...over }
}

export function detail(over: Partial<MatchDetail> = {}): MatchDetail {
  const { my_pick: _p, my_result: _r, ...header } = row()
  void _p
  void _r
  return { ...header, maps: [], ...over }
}

export function emptyStats(format: 'bo1' | 'bo3' | 'bo5' = 'bo3'): PickStats {
  const n = format === 'bo1' ? 1 : format === 'bo3' ? 3 : 5
  const zero = { a: 0, b: 0, pct_a: 0, pct_b: 0 }
  return {
    total: 0,
    winner: zero,
    series:
      format === 'bo3'
        ? [
            { a: 2, b: 0, count: 0, pct: 0 },
            { a: 2, b: 1, count: 0, pct: 0 },
            { a: 1, b: 2, count: 0, pct: 0 },
            { a: 0, b: 2, count: 0, pct: 0 },
          ]
        : [],
    maps: Array.from({ length: n }, (_, i) => ({ map_number: i + 1, ...zero })),
  }
}

export function pickView(over: Partial<PickView> = {}): PickView {
  return {
    user_id: '200000000000000099',
    display_name: 'bob',
    avatar_url: '/api/img/avatar/200000000000000099?v=0',
    winner_team_id: VIT.id,
    series: null,
    maps: [],
    created_at: '2026-11-20T09:00:00.000Z',
    updated_at: '2026-11-20T09:00:00.000Z',
    edited: false,
    result: null,
    ...over,
  }
}

/** My pick (the signed-in test member). */
export function myPick(over: Partial<PickView> = {}): PickView {
  return pickView({ user_id: ME.user_id, display_name: ME.display_name, ...over })
}

export function picks(views: PickView[], over: Partial<MatchPicks> = {}): MatchPicks {
  const stats = emptyStats()
  stats.total = views.length
  return {
    match_id: 10,
    pickable: true,
    locked: false,
    lock_at: '2026-11-20T18:00:00.000Z',
    picks: views,
    stats,
    ...over,
  }
}
