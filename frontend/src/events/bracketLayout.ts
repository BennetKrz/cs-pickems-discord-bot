import type { BracketSide, FeedRef, StageKind, StageMatch } from '../api.ts'
import { teamShort } from '../matches/format.ts'

/**
 * Geometry of an elimination tree (T13), pure so it can be tested without
 * a browser. The server sends each match's side, round (column) and slot
 * (row); see `docs/adr/0017-event-view-and-brackets.md`.
 *
 * - The upper bracket is on top, the lower bracket below it, and the
 *   grand final in the column after both, between the two finals.
 * - A first-column node sits at its slot; a later one is centred on its
 *   winner feeders from the same side. A column never overlaps itself:
 *   nodes are pushed down to keep a gap.
 * - Lines run only along winner feeds within a side and into the final.
 *   Loser feeds (drops into the lower bracket) are labelled in the slot
 *   instead: "Loser of …".
 */

export const NODE_W = 184
export const NODE_H = 76
export const COL_GAP = 36
export const ROW_GAP = 14
export const HEADER_H = 28
export const SECTION_GAP = 28

export interface NodeBox {
  id: number
  side: BracketSide
  x: number
  y: number
}

export interface Edge {
  from: number
  to: number
  /** The team slot of `to` the line ends at. */
  slot: 'a' | 'b'
  /** SVG path. */
  d: string
}

export interface Header {
  key: string
  x: number
  y: number
  label: string
}

export interface BracketLayout {
  width: number
  height: number
  nodes: Map<number, NodeBox>
  edges: Edge[]
  headers: Header[]
  /** Short codes for empty slots: "UB R1 #2", "LB R3 #1", "Grand final". */
  codes: Map<number, string>
}

type Placed = StageMatch & { bracket: NonNullable<StageMatch['bracket']> }

const SIDES: BracketSide[] = ['upper', 'lower', 'final']

function feedsOf(m: Placed): { feed: FeedRef; slot: 'a' | 'b' }[] {
  const out: { feed: FeedRef; slot: 'a' | 'b' }[] = []
  if (m.bracket.feeds_a) out.push({ feed: m.bracket.feeds_a, slot: 'a' })
  if (m.bracket.feeds_b) out.push({ feed: m.bracket.feeds_b, slot: 'b' })
  return out
}

/** Labels of the columns of one side. */
function columnLabels(side: BracketSide, count: number, kind: StageKind | null, hasLower: boolean) {
  return Array.from({ length: count }, (_, i) => {
    const fromEnd = count - 1 - i
    if (side === 'final') return count === 1 ? 'Grand final' : `Grand final ${i + 1}`
    if (side === 'upper' && !hasLower && kind !== 'double_elim') {
      if (fromEnd === 0) return 'Final'
      if (fromEnd === 1) return 'Semifinals'
      if (fromEnd === 2) return 'Quarterfinals'
      return `Round ${i + 1}`
    }
    if (fromEnd === 0 && count > 1) return side === 'upper' ? 'Upper final' : 'Lower final'
    if (!hasLower) return `Round ${i + 1}`
    return `${side === 'upper' ? 'UB' : 'LB'} round ${i + 1}`
  })
}

/** Lays out the matches of a bracket stage; matches without a position are left out. */
export function layoutBracket(matches: StageMatch[], kind: StageKind | null = null): BracketLayout {
  const placed = matches.filter((m): m is Placed => m.bracket !== null)
  const hasLower = placed.some((m) => m.bracket.side === 'lower')
  const pitch = NODE_W + COL_GAP

  // Columns per side, as the sorted distinct rounds (rounds may skip).
  const columns = new Map<BracketSide, number[]>()
  for (const side of SIDES) {
    const onSide = placed.filter((m) => m.bracket.side === side)
    const rounds = [...new Set(onSide.map((m) => m.bracket.round))]
    columns.set(side, rounds.sort((a, b) => a - b))
  }
  const colOf = (m: Placed) => columns.get(m.bracket.side)!.indexOf(m.bracket.round)
  const bracketCols = Math.max(columns.get('upper')!.length, columns.get('lower')!.length)

  const nodes = new Map<number, NodeBox>()
  const headers: Header[] = []
  const codes = new Map<number, string>()

  /** Places one side's columns from `top`; returns its bottom edge. */
  const placeSide = (side: BracketSide, top: number, xOffset: number): number => {
    const rounds = columns.get(side)!
    const labels = columnLabels(side, rounds.length, kind, hasLower)
    let bottom = top
    rounds.forEach((round, col) => {
      const x = xOffset + col * pitch
      headers.push({ key: `${side}-${round}`, x, y: top, label: labels[col]! })
      const column = placed
        .filter((m) => m.bracket.side === side && m.bracket.round === round)
        .sort((a, b) => a.bracket.slot - b.bracket.slot)
      let prevBottom = -Infinity
      for (const m of column) {
        const feederYs = feedsOf(m)
          .filter(({ feed }) => feed.outcome === 'winner')
          .map(({ feed }) => nodes.get(feed.match_id))
          .filter((n): n is NodeBox => n !== undefined && (side === 'final' || n.side === side))
          .map((n) => n.y)
        const desired =
          feederYs.length > 0
            ? feederYs.reduce((s, y) => s + y, 0) / feederYs.length
            : top + HEADER_H + m.bracket.slot * (NODE_H + ROW_GAP)
        const y = Math.max(desired, top + HEADER_H, prevBottom + ROW_GAP)
        nodes.set(m.id, { id: m.id, side, x, y })
        prevBottom = y + NODE_H
        bottom = Math.max(bottom, prevBottom)
      }
    })
    return bottom
  }

  const upperBottom = placeSide('upper', 0, 0)
  let bottom = upperBottom
  if (hasLower) {
    const lowerTop = columns.get('upper')!.length > 0 ? upperBottom + SECTION_GAP : 0
    bottom = placeSide('lower', lowerTop, 0)
  }
  bottom = Math.max(bottom, placeSide('final', 0, bracketCols * pitch))

  // Codes for "Winner of …" while a feeder's teams are TBD.
  for (const m of placed) {
    const col = colOf(m) + 1
    const slot = m.bracket.slot + 1
    const code =
      m.bracket.side === 'final'
        ? columns.get('final')!.length > 1
          ? `Grand final ${col}`
          : 'Grand final'
        : `${hasLower ? (m.bracket.side === 'upper' ? 'UB ' : 'LB ') : ''}R${col} #${slot}`
    codes.set(m.id, code)
  }

  const edges: Edge[] = []
  for (const m of placed) {
    const to = nodes.get(m.id)!
    for (const { feed, slot } of feedsOf(m)) {
      if (feed.outcome !== 'winner') continue
      const from = nodes.get(feed.match_id)
      if (!from || (from.side !== to.side && to.side !== 'final')) continue
      const x1 = from.x + NODE_W
      const y1 = from.y + NODE_H / 2
      const x2 = to.x
      const y2 = to.y + (slot === 'a' ? NODE_H * 0.35 : NODE_H * 0.65)
      const xm = x2 - COL_GAP / 2
      edges.push({ from: from.id, to: m.id, slot, d: `M${x1} ${y1}H${xm}V${y2}H${x2}` })
    }
  }

  const finalCols = columns.get('final')!.length
  const totalCols = bracketCols + finalCols
  const width = totalCols > 0 ? totalCols * pitch - COL_GAP : 0
  return { width, height: bottom, nodes, edges, headers, codes }
}

/** Text for an empty team slot: "Winner of AUR vs FJO", "Loser of UB R1 #2". */
export function slotLabel(
  feed: FeedRef | null,
  byId: Map<number, StageMatch>,
  codes: Map<number, string>,
): string {
  if (!feed) return 'TBD'
  const from = byId.get(feed.match_id)
  const verb = feed.outcome === 'winner' ? 'Winner' : 'Loser'
  if (from?.team_a && from.team_b) {
    return `${verb} of ${teamShort(from.team_a)} vs ${teamShort(from.team_b)}`
  }
  const code = codes.get(feed.match_id)
  return code ? `${verb} of ${code}` : 'TBD'
}

/** The scale that fits `contentWidth` into `containerWidth`, never above 1. */
export function fitScale(containerWidth: number, contentWidth: number): number {
  if (containerWidth <= 0 || contentWidth <= 0) return 1
  return Math.min(1, containerWidth / contentWidth)
}
