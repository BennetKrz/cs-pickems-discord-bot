import type {
  Accuracy,
  Breakdown,
  PartScore,
  ProfilePick,
  ProfileSegment,
  SavedPickParts,
  TeamRef,
} from '../api.ts'
import { teamShort } from '../matches/format.ts'

/** One part of a pick with its outcome, once scored. */
export interface ScoredPart {
  key: string
  label: string
  value: string
  /** `null` before settlement, or when the breakdown lacks the part. */
  score: PartScore | null
}

/**
 * A pick as its scored parts: winner, series, and per map the winner and,
 * if picked, the exact score (or overtime). Each map part is scored on its
 * own, so they are listed apart.
 */
export function scoredParts(
  pick: SavedPickParts,
  breakdown: Breakdown | null,
  teamA: TeamRef | null,
  teamB: TeamRef | null,
): ScoredPart[] {
  const name = (id: number | null) =>
    id === teamA?.id ? teamShort(teamA) : id === teamB?.id ? teamShort(teamB) : '?'
  const parts: ScoredPart[] = []
  if (pick.winner_team_id !== null) {
    parts.push({
      key: 'winner',
      label: 'Winner',
      value: name(pick.winner_team_id),
      score: breakdown?.winner ?? null,
    })
  }
  if (pick.series) {
    parts.push({
      key: 'series',
      label: 'Series',
      value: `${pick.series.a}:${pick.series.b}`,
      score: breakdown?.series ?? null,
    })
  }
  for (const m of pick.maps) {
    const scored = breakdown?.maps.find((x) => x.map_number === m.map_number)
    parts.push({
      key: `map${m.map_number}`,
      label: `Map ${m.map_number}`,
      value: name(m.winner_team_id),
      score: scored?.winner ?? null,
    })
    const exact = m.rounds ? `${m.rounds.a}:${m.rounds.b}` : m.overtime ? 'OT' : null
    if (exact) {
      parts.push({
        key: `map${m.map_number}score`,
        label: `Map ${m.map_number} score`,
        value: exact,
        score: scored?.score ?? null,
      })
    }
  }
  return parts
}

/** A pick counts for nothing: its result is void, or the match was voided or canceled. */
export function isVoid(p: ProfilePick): boolean {
  return p.result?.state === 'void' || p.match.voided || p.match.status === 'canceled'
}

/** `75%` with `3/4`, or `–` before any part is decided. */
export function accuracyText(a: Accuracy): { pct: string; ratio: string } {
  return {
    pct: a.rate === null ? '–' : `${Math.round(a.rate * 100)}%`,
    ratio: `${a.hits}/${a.decided}`,
  }
}

/** `#3`, or `–` without a rank. */
export function rankText(rank: number | null): string {
  return rank === null ? '–' : `#${rank}`
}

export const SEGMENTS: { key: ProfileSegment; label: string }[] = [
  { key: 'open', label: 'Open' },
  { key: 'live', label: 'Live' },
  { key: 'settled', label: 'Settled' },
]

/** A list from the URL, else the first that has picks (live first). */
export function segmentOf(
  value: string | null,
  counts: { open: number; live: number; settled: number },
): ProfileSegment {
  if (value === 'open' || value === 'live' || value === 'settled') return value
  if (counts.live > 0) return 'live'
  if (counts.open > 0) return 'open'
  return counts.settled > 0 ? 'settled' : 'open'
}

/** An event ID from the URL. */
export function eventOf(value: string | null): number | null {
  return value !== null && /^\d+$/.test(value) ? Number(value) : null
}
