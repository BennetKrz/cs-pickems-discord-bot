import type { PartScore, PickView, ResultState, SavedPickParts, TeamRef } from '../api.ts'
import { teamShort } from './format.ts'

/** One part of a pick, for read-only display. */
export interface SummaryPart {
  key: string
  label: string
  value: string
}

/**
 * A pick as short labelled parts: "Winner VIT", "Series 2:1",
 * "Map 1 VIT 13:9", "Map 2 NAVI OT".
 */
export function summarizePick(
  pick: SavedPickParts,
  teamA: TeamRef | null,
  teamB: TeamRef | null,
): SummaryPart[] {
  const name = (id: number | null) =>
    id === teamA?.id ? teamShort(teamA) : id === teamB?.id ? teamShort(teamB) : '?'
  const parts: SummaryPart[] = []
  if (pick.winner_team_id !== null) {
    parts.push({ key: 'winner', label: 'Winner', value: name(pick.winner_team_id) })
  }
  if (pick.series) {
    parts.push({ key: 'series', label: 'Series', value: `${pick.series.a}:${pick.series.b}` })
  }
  for (const m of pick.maps) {
    let value = name(m.winner_team_id)
    if (m.rounds) value += ` ${m.rounds.a}:${m.rounds.b}`
    else if (m.overtime) value += ' OT'
    parts.push({ key: `map${m.map_number}`, label: `Map ${m.map_number}`, value })
  }
  return parts
}

/** "+3", "0", "pending", "void" for one scored part. */
export function partOutcomeLabel(p: PartScore): string {
  switch (p.outcome) {
    case 'hit':
      return `+${p.points}`
    case 'miss':
      return '0'
    case 'pending':
      return 'pending'
    case 'void':
      return 'void'
  }
}

/** "12 pts", "12 pts so far", "Void". */
export function pointsLabel(points: number, state: ResultState): string {
  if (state === 'void') return 'Void'
  const pts = `${points} ${points === 1 ? 'pt' : 'pts'}`
  return state === 'provisional' ? `${pts} so far` : pts
}

/** Picks with points first (highest first, then by name), the rest as listed. */
export function sortPicks(picks: PickView[]): PickView[] {
  if (!picks.some((p) => p.result)) return picks
  return [...picks].sort((x, y) => {
    const px = x.result ? x.result.points : -1
    const py = y.result ? y.result.points : -1
    return py - px || x.display_name.localeCompare(y.display_name)
  })
}
