import {
  ApiError,
  INVALID_PICK,
  NOT_PICKABLE,
  PICK_LOCKED,
  type MatchFormat,
  type PickParts,
  type SavedPickParts,
  type Score,
} from '../api.ts'

/**
 * The pick form's state and rules (T10 rules, T12 UI), as pure functions.
 * The draft uses sides (`a`/`b` of the match's team order) instead of team
 * IDs; scores are always A:B.
 */

export type Side = 'a' | 'b'

export interface MapDraft {
  winner: Side | null
  /** Rounds of the loser for an exact 13:x score (0…11); `null` for none. */
  loser: number | null
  /** The winner wins in overtime; excludes `loser`. */
  overtime: boolean
}

export interface Draft {
  winner: Side | null
  series: Score | null
  /** One entry per map number 1…N; index 0 is map 1. */
  maps: MapDraft[]
}

export interface Teams {
  a: number
  b: number
}

/** Rounds a team needs to win a map in regulation. */
export const MAP_WIN_ROUNDS = 13
/** Highest loser score in regulation; 12:12 goes to overtime. */
export const MAP_MAX_LOSER_ROUNDS = 11

export function mapCount(format: MatchFormat): number {
  return format === 'bo1' ? 1 : format === 'bo3' ? 3 : 5
}

export function winsNeeded(format: MatchFormat): number {
  return Math.floor(mapCount(format) / 2) + 1
}

/** Pickable series scores, team A's wins first; none for Bo1. */
export function seriesOptions(format: MatchFormat): Score[] {
  if (format === 'bo1') return []
  const w = winsNeeded(format)
  const out: Score[] = []
  for (let l = 0; l < w; l++) out.push({ a: w, b: l })
  for (let l = w - 1; l >= 0; l--) out.push({ a: l, b: w })
  return out
}

const NO_MAP: MapDraft = { winner: null, loser: null, overtime: false }

export function emptyDraft(format: MatchFormat): Draft {
  return {
    winner: null,
    series: null,
    maps: Array.from({ length: mapCount(format) }, () => ({ ...NO_MAP })),
  }
}

function sideOf(teamId: number | null, teams: Teams): Side | null {
  if (teamId === teams.a) return 'a'
  if (teamId === teams.b) return 'b'
  return null
}

function seriesWinner(s: Score): Side {
  return s.a > s.b ? 'a' : 'b'
}

/** The form's draft for a saved pick (or an empty one). */
export function draftFromPick(
  format: MatchFormat,
  teams: Teams,
  saved: SavedPickParts | null | undefined,
): Draft {
  const draft = emptyDraft(format)
  if (!saved) return draft
  draft.winner = sideOf(saved.winner_team_id, teams)
  draft.series = saved.series ? { a: saved.series.a, b: saved.series.b } : null
  for (const m of saved.maps) {
    const winner = sideOf(m.winner_team_id, teams)
    if (!winner || m.map_number < 1 || m.map_number > draft.maps.length) continue
    const loser = m.rounds ? (winner === 'a' ? m.rounds.b : m.rounds.a) : null
    draft.maps[m.map_number - 1] = { winner, loser, overtime: m.overtime }
  }
  return draft
}

/** The `PUT` body for a draft. */
export function toParts(draft: Draft, teams: Teams): PickParts {
  const team = (s: Side) => (s === 'a' ? teams.a : teams.b)
  const maps = draft.maps.flatMap((m, i) => {
    if (!m.winner) return []
    const rounds =
      m.loser === null
        ? null
        : m.winner === 'a'
          ? { a: MAP_WIN_ROUNDS, b: m.loser }
          : { a: m.loser, b: MAP_WIN_ROUNDS }
    return [{ map_number: i + 1, winner_team_id: team(m.winner), rounds, overtime: m.overtime }]
  })
  return {
    winner_team_id: draft.winner ? team(draft.winner) : null,
    series: draft.series,
    maps,
  }
}

export function isEmptyDraft(draft: Draft): boolean {
  return !draft.winner && !draft.series && draft.maps.every((m) => !m.winner)
}

export function sameDraft(x: Draft, y: Draft): boolean {
  return JSON.stringify(x) === JSON.stringify(y)
}

/**
 * Picks (or, if already picked, clears) the winner. A series score for the
 * other team no longer fits and is cleared.
 */
export function chooseWinner(draft: Draft, side: Side): Draft {
  const winner = draft.winner === side ? null : side
  const series =
    draft.series && winner && seriesWinner(draft.series) === winner ? draft.series : null
  return { ...draft, winner, series }
}

/** Picks (or clears) a series score; picking one also sets its winner. */
export function chooseSeries(draft: Draft, score: Score): Draft {
  if (draft.series && draft.series.a === score.a && draft.series.b === score.b) {
    return { ...draft, series: null }
  }
  return { ...draft, series: { ...score }, winner: seriesWinner(score) }
}

function withMap(draft: Draft, mapNumber: number, f: (m: MapDraft) => MapDraft): Draft {
  return {
    ...draft,
    maps: draft.maps.map((m, i) => (i === mapNumber - 1 ? f(m) : m)),
  }
}

/** Picks (or clears) a map's winner. The score stays, relative to the winner. */
export function chooseMapWinner(draft: Draft, mapNumber: number, side: Side): Draft {
  return withMap(draft, mapNumber, (m) =>
    m.winner === side ? { ...NO_MAP } : { ...m, winner: side },
  )
}

/** A map score choice: the loser's rounds, `'ot'`, or `null` for none. */
export type MapScoreChoice = number | 'ot' | null

export function chooseMapScore(draft: Draft, mapNumber: number, choice: MapScoreChoice): Draft {
  return withMap(draft, mapNumber, (m) => {
    if (!m.winner) return m
    if (choice === 'ot') return { ...m, loser: null, overtime: true }
    return { ...m, loser: choice, overtime: false }
  })
}

/** The score choice of a map, for a `<select>` value. */
export function mapScoreValue(m: MapDraft): string {
  if (m.overtime) return 'ot'
  return m.loser === null ? '' : String(m.loser)
}

export function parseMapScoreValue(value: string): MapScoreChoice {
  if (value === 'ot') return 'ot'
  if (value === '') return null
  const n = Number(value)
  return Number.isInteger(n) && n >= 0 && n <= MAP_MAX_LOSER_ROUNDS ? n : null
}

/** The exact scores a map with `winner` can end in, A:B. */
export function mapScoreOptions(winner: Side): { value: string; label: string }[] {
  const out = []
  for (let l = 0; l <= MAP_MAX_LOSER_ROUNDS; l++) {
    const label = winner === 'a' ? `${MAP_WIN_ROUNDS}:${l}` : `${l}:${MAP_WIN_ROUNDS}`
    out.push({ value: String(l), label })
  }
  return out
}

/**
 * Soft hints when map picks and the series (or winner) pick disagree. They
 * are allowed to (each part scores on its own), so this never blocks a save.
 */
export function pickHints(
  format: MatchFormat,
  draft: Draft,
  names: Record<Side, string>,
): string[] {
  const tally = { a: 0, b: 0 }
  let lastPicked = 0
  draft.maps.forEach((m, i) => {
    if (m.winner) {
      tally[m.winner] += 1
      lastPicked = i + 1
    }
  })
  if (lastPicked === 0) return []
  const s = draft.series
  if (s) {
    if (tally.a > s.a || tally.b > s.b) {
      return [
        `Your map picks add up to ${tally.a}:${tally.b}, your series pick is ${s.a}:${s.b}.`,
      ]
    }
    if (lastPicked > s.a + s.b) {
      return [
        `With a ${s.a}:${s.b} series, map ${lastPicked} won't be played, but you picked it.`,
      ]
    }
    return []
  }
  if (draft.winner) {
    const other: Side = draft.winner === 'a' ? 'b' : 'a'
    if (tally[other] >= winsNeeded(format)) {
      const mine = names[draft.winner]
      return [`Your map picks have ${names[other]} winning, but your winner pick is ${mine}.`]
    }
  }
  return []
}

/** User-facing text for a failed save or delete. */
export function pickErrorMessage(error: unknown): string {
  if (error instanceof ApiError) {
    switch (error.code) {
      case PICK_LOCKED:
        return 'This match has started, so picks are locked.'
      case NOT_PICKABLE:
        return 'Picks are not open for this match.'
      case INVALID_PICK:
        return `That pick isn't valid: ${error.message}`
    }
  }
  return 'Your pick could not be saved. Please try again.'
}
