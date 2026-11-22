import { act, fireEvent, render, screen, within } from '@testing-library/react'
import { RouterProvider } from 'react-router'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ApiClient, type EventDetail } from '../api.ts'
import { AuthContext } from '../auth-context.ts'
import { resetServerClock } from '../clock.ts'
import { detail, picks } from '../matches/test-fixtures.ts'
import { POLL_LIVE_MS } from '../poll.ts'
import { createAppRouter } from '../router.tsx'
import { fakeResponse, ME, mockFetch } from '../test-utils.ts'
import { layoutBracket } from './bracketLayout.ts'
import { AUR, CIN, eventDetail, playoffMatches, playoffStage } from './test-fixtures.ts'

/** The event the server answers with; tests change it between polls. */
let event: EventDetail

function serve() {
  return mockFetch((url) => {
    if (url === '/api/events/100') return fakeResponse(200, event)
    if (url === '/api/events/999') return fakeResponse(404, { error: 'not_found' })
    if (url === '/api/matches/101') return fakeResponse(200, detail({ id: 101 }))
    if (url === '/api/matches/101/picks') return fakeResponse(200, picks([], { match_id: 101 }))
    return fakeResponse(304)
  })
}

function renderAt(path = '/events/100') {
  const api = new ApiClient('token', () => {})
  return render(
    <AuthContext.Provider value={{ me: ME, api }}>
      <RouterProvider router={createAppRouter(path)} />
    </AuthContext.Provider>,
  )
}

async function pass(ms = 0) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms)
  })
}

function node(id: number): HTMLElement {
  const el = document.querySelector<HTMLElement>(`[data-match="${id}"]`)
  if (!el) throw new Error(`no node ${id}`)
  return el
}

function stageTab(name: string) {
  return within(screen.getByRole('tablist', { name: 'Stages' })).getByRole('tab', { name })
}

beforeEach(() => {
  vi.useFakeTimers()
  vi.setSystemTime(Date.parse('2026-11-20T12:00:00Z'))
  resetServerClock()
  event = eventDetail()
})
afterEach(() => {
  vi.useRealTimers()
  vi.restoreAllMocks()
})

describe('Event view', () => {
  it('opens on the stage with matches to play and draws the bracket', async () => {
    serve()
    const { container } = renderAt()
    await pass()

    expect(screen.getByRole('heading', { level: 1, name: 'Fixture Masters 2026' })).toBeVisible()
    expect(screen.getByText(/S-tier · .* · Running/)).toBeInTheDocument()
    expect(stageTab('Playoffs')).toHaveAttribute('aria-selected', 'true')
    expect(stageTab('Swiss Stage')).toHaveAttribute('aria-selected', 'false')

    for (const label of ['UB round 1', 'Upper final', 'LB round 1', 'Lower final', 'Grand final']) {
      expect(screen.getByText(label)).toBeInTheDocument()
    }
    // Six nodes, five winner lines; loser drops are labels.
    expect(container.querySelectorAll('[data-match]')).toHaveLength(6)
    const edges = [...container.querySelectorAll('path[data-edge]')].map((p) =>
      p.getAttribute('data-edge'),
    )
    expect(edges.sort()).toEqual(['101-103', '102-103', '103-106', '104-105', '105-106'])

    // Finished semis: teams, score, winner, my points.
    expect(node(101)).toHaveTextContent('Aurora Wolves')
    expect(node(101)).toHaveTextContent('Fjord Five')
    expect(node(101).querySelector('.is-winner')).toHaveTextContent('Aurora Wolves2')
    expect(node(102)).toHaveTextContent('7 pts')
    // Empty slots name where the team comes from.
    expect(node(103)).toHaveTextContent('Winner of AUR vs FJO')
    expect(node(103)).toHaveTextContent('Winner of GLA vs CIN')
    expect(node(104)).toHaveTextContent('Loser of AUR vs FJO')
    expect(node(105)).toHaveTextContent('Loser of UB R2 #1')
    expect(node(105)).toHaveTextContent('Winner of LB R1 #1')
    expect(node(106)).toHaveTextContent('Winner of UB R2 #1')
    // My pick marker and the lock countdown (18:00, 6 h away).
    expect(node(103)).toHaveTextContent('No pick')
    expect(node(103)).toHaveTextContent('Locks in 6 h 0 min')
  })

  it('fills a TBD slot on the next poll once its feeder ends', async () => {
    serve()
    renderAt()
    await pass()
    expect(node(103)).toHaveTextContent('Winner of AUR vs FJO')

    // The server now reports the upper final's teams.
    const ms = playoffMatches().map((m) =>
      m.id === 103 ? { ...m, team_a: AUR, team_b: CIN, pickable: true } : m,
    )
    event = eventDetail({ stages: [event.stages[0]!, playoffStage(ms)] })
    // A match locks within 24 h: the view polls every 20 s.
    await pass(POLL_LIVE_MS)
    expect(node(103)).toHaveTextContent('Aurora Wolves')
    expect(node(103)).toHaveTextContent('Cinder Esports')
    expect(node(103)).not.toHaveTextContent('Winner of')
    // The lower final's feeder now has teams too.
    expect(node(105)).toHaveTextContent('Loser of AUR vs CIN')
  })

  it('opens a match from the bracket and comes back to it', async () => {
    serve()
    renderAt()
    await pass()
    expect(node(101)).toHaveAttribute('href', '/matches/101')
    fireEvent.click(node(101))
    await pass()
    const back = screen.getByRole('link', { name: '← Fixture Masters 2026' })
    expect(back).toHaveAttribute('href', '/events/100?stage=2')
    fireEvent.click(back)
    await pass()
    expect(stageTab('Playoffs')).toHaveAttribute('aria-selected', 'true')
  })

  it('scrolls sideways and fits the tree to the width on request', async () => {
    vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockReturnValue(360)
    serve()
    renderAt()
    await pass()
    const scroll = screen.getByTestId('bracket-scroll')
    const canvas = screen.getByTestId('bracket-canvas')
    expect(scroll).not.toHaveClass('is-fit')
    expect(canvas.style.transform).toBe('')

    fireEvent.click(screen.getByRole('checkbox', { name: 'Fit to width' }))
    const { width } = layoutBracket(event.stages[1]!.matches, 'double_elim')
    expect(scroll).toHaveClass('is-fit')
    const scale = Number(/scale\((.+)\)/.exec(canvas.style.transform)?.[1])
    expect(scale).toBeCloseTo(360 / width, 5)
    // The frame shrinks with it, so nothing overflows.
    expect(parseFloat((canvas.parentElement as HTMLElement).style.width)).toBeCloseTo(360)
  })

  it('shows Swiss standings and the matches per round', async () => {
    serve()
    renderAt()
    await pass()
    fireEvent.click(stageTab('Swiss Stage'))
    expect(stageTab('Swiss Stage')).toHaveAttribute('aria-selected', 'true')

    const table = screen.getByRole('table')
    const rows = within(table)
      .getAllByRole('row')
      .slice(1)
      .map((r) =>
        within(r)
          .getAllByRole('cell')
          .map((c) => c.textContent)
          // The team name, without the logo's fallback acronym.
          .concat(
            within(r).getByRole('rowheader').querySelector('span > span:last-child')!.textContent,
          ),
      )
    // Position, W–L, maps, rounds, then the team.
    expect(rows).toEqual([
      ['1', '2–0', '+3', '+13', 'Cinder Esports'],
      ['2', '2–0', '+2', '+11', 'Aurora Wolves'],
      ['3', '1–1', '0', '−2', 'Glacier Union'],
      ['4', '0–0', '0', '0', 'Dune Raiders'],
      ['5', '0–0', '0', '0', 'Echo Kings'],
      ['6', '0–1', '−1', '−2', 'Fjord Five'],
      ['7', '0–1', '−1', '−5', 'Basalt Gaming'],
      ['8', '0–2', '−3', '−15', 'Halcyon Club'],
    ])
    expect(screen.getAllByRole('heading', { level: 3 }).map((h) => h.textContent)).toEqual([
      'Round 1',
      'Round 2',
    ])
    const links = screen
      .getAllByRole('link')
      .filter((a) => a.getAttribute('href')?.startsWith('/matches/'))
    expect(links.map((a) => a.getAttribute('href'))).toEqual([
      '/matches/1',
      '/matches/2',
      '/matches/3',
      '/matches/4',
      '/matches/5',
      '/matches/6',
    ])
    expect(links[3]).toHaveTextContent('Canceled')
  })

  it('says when the server does not follow the event, and handles a missing one', async () => {
    event = eventDetail({ followed: false })
    serve()
    renderAt()
    await pass()
    expect(screen.getByText(/does not follow this event/)).toBeInTheDocument()

    renderAt('/events/999')
    await pass()
    expect(screen.getByRole('heading', { name: 'Event not found' })).toBeInTheDocument()
  })
})
