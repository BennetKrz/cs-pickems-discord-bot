import { describe, expect, it } from 'vitest'
import {
  fitScale,
  layoutBracket,
  NODE_H,
  NODE_W,
  slotLabel,
  type BracketLayout,
} from './bracketLayout.ts'
import { de16Matches, playoffMatches } from './test-fixtures.ts'

function overlaps(layout: BracketLayout): [number, number][] {
  const boxes = [...layout.nodes.values()]
  const out: [number, number][] = []
  for (let i = 0; i < boxes.length; i++) {
    for (let j = i + 1; j < boxes.length; j++) {
      const a = boxes[i]!
      const b = boxes[j]!
      const apart =
        a.x + NODE_W <= b.x || b.x + NODE_W <= a.x || a.y + NODE_H <= b.y || b.y + NODE_H <= a.y
      if (!apart) out.push([a.id, b.id])
    }
  }
  return out
}

describe('bracket layout', () => {
  it('draws the fixture playoff with winner lines only', () => {
    const layout = layoutBracket(playoffMatches(), 'double_elim')
    const edges = layout.edges.map((e) => `${e.from}->${e.to}:${e.slot}`).sort()
    expect(edges).toEqual(
      [
        '101->103:a',
        '102->103:b',
        // lb-f: loser of 103 is a label, winner of 104 a line.
        '104->105:b',
        // The grand final: from both finals.
        '103->106:a',
        '105->106:b',
      ].sort(),
    )
    expect(layout.headers.map((h) => h.label)).toEqual([
      'UB round 1',
      'Upper final',
      'LB round 1',
      'Lower final',
      'Grand final',
    ])
    expect(overlaps(layout)).toEqual([])

    const at = (id: number) => layout.nodes.get(id)!
    // The upper final is centred on its semis; the grand final sits after
    // both brackets, between the two finals.
    expect(at(103).y).toBe((at(101).y + at(102).y) / 2)
    expect(at(104).y).toBeGreaterThan(at(102).y + NODE_H)
    expect(at(106).x).toBeGreaterThan(at(105).x)
    expect(at(106).y).toBe((at(103).y + at(105).y) / 2)
    // Every line starts at a right edge and ends at a left edge.
    for (const e of layout.edges) {
      const from = at(e.from)
      const to = at(e.to)
      expect(e.d.startsWith(`M${from.x + NODE_W} `)).toBe(true)
      expect(e.d.endsWith(`H${to.x}`)).toBe(true)
    }
  })

  it('labels empty slots by their feeders', () => {
    const ms = playoffMatches()
    const byId = new Map(ms.map((m) => [m.id, m]))
    const { codes } = layoutBracket(ms, 'double_elim')
    const uf = byId.get(103)!.bracket!
    expect(slotLabel(uf.feeds_a, byId, codes)).toBe('Winner of AUR vs FJO')
    const lf = byId.get(105)!.bracket!
    // The feeder's teams are TBD: its code instead.
    expect(slotLabel(lf.feeds_a, byId, codes)).toBe('Loser of UB R2 #1')
    expect(slotLabel(lf.feeds_b, byId, codes)).toBe('Winner of LB R1 #1')
    expect(slotLabel(null, byId, codes)).toBe('TBD')
    expect(codes.get(106)).toBe('Grand final')
  })

  it('names single-elimination columns', () => {
    const ms = playoffMatches().filter((m) => m.bracket!.side === 'upper')
    const layout = layoutBracket(ms, 'single_elim')
    expect(layout.headers.map((h) => h.label)).toEqual(['Semifinals', 'Final'])
    expect(layout.codes.get(101)).toBe('R1 #1')
  })

  it('keeps a 16-team double elimination apart and fits it into 360 px', () => {
    const ms = de16Matches()
    expect(ms).toHaveLength(30)
    const layout = layoutBracket(ms, 'double_elim')
    expect(layout.nodes.size).toBe(30)
    // 6 lower rounds set the width, plus the grand final.
    expect(new Set([...layout.nodes.values()].map((n) => n.x)).size).toBe(7)
    expect(overlaps(layout)).toEqual([])
    for (const n of layout.nodes.values()) {
      expect(n.x).toBeGreaterThanOrEqual(0)
      expect(n.y).toBeGreaterThanOrEqual(0)
      expect(n.x + NODE_W).toBeLessThanOrEqual(layout.width)
      expect(n.y + NODE_H).toBeLessThanOrEqual(layout.height)
    }
    // Upper final → grand final, lower final → grand final.
    expect(layout.edges.filter((e) => e.to === 30)).toHaveLength(2)
    // Every winner feed within a side gets a line: 7 upper matches with
    // two, 7 lower drop-in matches with one, 3 lower pairings with two.
    expect(layout.edges).toHaveLength(14 + 13 + 2)

    const scale = fitScale(360, layout.width)
    expect(layout.width * scale).toBeLessThanOrEqual(360)
    expect(scale).toBeGreaterThan(0.2)
    expect(fitScale(2000, layout.width)).toBe(1)
  })
})
