import { act, fireEvent, render, screen, within } from '@testing-library/react'
import { RouterProvider } from 'react-router'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ApiClient, type MatchList, type Me } from '../api.ts'
import { AuthContext } from '../auth-context.ts'
import { resetServerClock } from '../clock.ts'
import { POLL_LIVE_MS } from '../poll.ts'
import { createAppRouter } from '../router.tsx'
import { fakeResponse, ME, mockFetch } from '../test-utils.ts'
import { list, row } from './test-fixtures.ts'

/** Lists per segment; tests change them to simulate the server. */
let lists: Record<string, MatchList>

function serve() {
  return mockFetch((url) => {
    const u = new URL(url, 'http://x')
    if (u.pathname === '/api/matches') {
      const state = u.searchParams.get('state') ?? ''
      const cursor = u.searchParams.get('cursor')
      const body = lists[cursor ? `${state}:${cursor}` : state]
      return body ? fakeResponse(200, body) : fakeResponse(404, { error: 'not_found' })
    }
    return fakeResponse(304)
  })
}

function renderAt(path = '/matches', me: Me = ME) {
  const api = new ApiClient('token', () => {})
  return render(
    <AuthContext.Provider value={{ me, api }}>
      <RouterProvider router={createAppRouter(path)} />
    </AuthContext.Provider>,
  )
}

async function pass(ms = 0) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms)
  })
}

function tab(name: string) {
  return within(screen.getByRole('tablist', { name: 'Matches' })).getByRole('tab', { name })
}

beforeEach(() => {
  vi.useFakeTimers()
  // 13:00 in Berlin.
  vi.setSystemTime(Date.parse('2026-11-20T12:00:00Z'))
  resetServerClock()
  lists = {
    upcoming: list([]),
    live: list([], { state: 'live' }),
    finished: list([], { state: 'finished' }),
  }
})
afterEach(() => {
  vi.useRealTimers()
})

describe('Matches tab', () => {
  it('groups upcoming matches by local day and event, with status and countdown', async () => {
    lists.upcoming = list([
      row({ id: 1, pick_count: 3, my_pick: 'partial' }),
      row({
        id: 2,
        scheduled_at: '2026-11-21T09:00:00Z',
        lock_at: '2026-11-21T09:00:00Z',
        event: { id: 200, name: 'Other Cup' },
        my_pick: 'full',
        pick_count: 1,
      }),
      row({
        id: 3,
        scheduled_at: '2026-11-25T09:00:00Z',
        lock_at: '2026-11-25T09:00:00Z',
        team_b: null,
        pickable: false,
      }),
    ])
    serve()
    renderAt()
    await pass()

    const days = screen.getAllByRole('heading', { level: 2 })
    expect(days.map((h) => h.textContent)).toEqual([
      'Today',
      'Tomorrow',
      expect.not.stringMatching(/Today|Tomorrow/),
    ])
    expect(screen.getAllByRole('heading', { level: 3 }).map((h) => h.textContent)).toEqual([
      'IEM Test',
      'Other Cup',
      'IEM Test',
    ])
    const links = screen
      .getAllByRole('link')
      .filter((a) => a.getAttribute('href')?.startsWith('/matches/'))
    expect(links).toHaveLength(3)
    const first = links[0]!
    expect(first).toHaveAttribute('href', '/matches/1')
    expect(first).toHaveTextContent('Vitality')
    expect(first).toHaveTextContent('Natus Vincere')
    expect(first).toHaveTextContent('BO3 · Playoffs · 3 picks')
    expect(first).toHaveTextContent('Partial pick')
    // 18:00 UTC is 19:00 in Berlin; locks in 6 h.
    expect(first).toHaveTextContent(/19:00|07:00\sPM/)
    expect(first).toHaveTextContent('Locks in 6 h 0 min')
    expect(links[1]).toHaveTextContent('Picked')
    expect(links[1]).toHaveTextContent('Locks in 21 h 0 min')
    // More than 24 h away: no countdown. TBD team.
    expect(links[2]).not.toHaveTextContent('Locks in')
    expect(links[2]).toHaveTextContent('TBD')
    expect(links[2]).toHaveTextContent('No pick')
  })

  it('filters to matches that need my pick', async () => {
    lists.upcoming = list([
      row({ id: 1, my_pick: 'none' }),
      row({ id: 2, my_pick: 'partial' }),
      row({ id: 3, my_pick: 'none', pickable: false }),
    ])
    serve()
    renderAt()
    await pass()
    expect(screen.getAllByRole('listitem')).toHaveLength(3)
    fireEvent.click(screen.getByRole('checkbox', { name: 'Needs my pick' }))
    await pass()
    const items = screen.getAllByRole('listitem')
    expect(items).toHaveLength(1)
    expect(within(items[0]!).getByRole('link')).toHaveAttribute('href', '/matches/1')

    lists.upcoming = list([row({ id: 2, my_pick: 'full' })])
    await pass(POLL_LIVE_MS)
    expect(screen.getByRole('heading', { name: 'All caught up' })).toBeInTheDocument()
  })

  it('moves a match from Upcoming to Live to Results without a reload', async () => {
    const fetch = serve()
    lists.upcoming = list([row({ id: 5 })])
    renderAt()
    await pass()
    expect(screen.getByRole('link', { name: /Vitality/ })).toHaveAttribute('href', '/matches/5')

    // Sync reports it live: the next poll drops it from Upcoming.
    lists.upcoming = list([])
    lists.live = list(
      [
        row({
          id: 5,
          status: 'live',
          locked: true,
          pickable: false,
          started_at: '2026-11-20T12:00:00Z',
          maps_won_a: 1,
          current_map: { number: 2, map_name: 'Inferno', rounds_a: 7, rounds_b: 4 },
          my_pick: 'full',
        }),
      ],
      { state: 'live' },
    )
    await pass(POLL_LIVE_MS)
    expect(screen.getByRole('heading', { name: 'No upcoming matches' })).toBeInTheDocument()
    fireEvent.click(tab('Live'))
    await pass()
    const live = screen.getByRole('link', { name: /Vitality/ })
    expect(live).toHaveTextContent('1:0')
    expect(live).toHaveTextContent('Map 2 · 7:4')
    expect(live).toHaveTextContent('Live')

    // Live scores update within one poll (20 s).
    lists.live = list(
      [
        {
          ...lists.live.matches[0]!,
          current_map: { number: 2, map_name: 'Inferno', rounds_a: 9, rounds_b: 4 },
        },
      ],
      { state: 'live' },
    )
    await pass(POLL_LIVE_MS)
    expect(screen.getByRole('link', { name: /Vitality/ })).toHaveTextContent('Map 2 · 9:4')

    lists.live = list([], { state: 'live' })
    lists.finished = list(
      [
        row({
          id: 5,
          status: 'finished',
          locked: true,
          pickable: false,
          ended_at: '2026-11-20T13:30:00Z',
          maps_won_a: 2,
          maps_won_b: 0,
          winner_team_id: 1,
          my_pick: 'full',
          my_result: { points: 15, state: 'final', pending_parts: 0 },
        }),
      ],
      { state: 'finished' },
    )
    await pass(POLL_LIVE_MS)
    expect(screen.getByRole('heading', { name: 'No live matches' })).toBeInTheDocument()
    fireEvent.click(tab('Results'))
    await pass()
    const done = screen.getByRole('link', { name: /Vitality/ })
    expect(done).toHaveTextContent('2:0')
    expect(done).toHaveTextContent('15 pts')
    expect(done).toHaveTextContent('Final')
    // No full page load: every request went through fetch polling.
    expect(fetch.mock.calls.every(([u]) => String(u).startsWith('/api/'))).toBe(true)
  })

  it('pages results', async () => {
    lists.finished = list(
      [row({ id: 1, status: 'finished', ended_at: '2026-11-20T10:00:00Z' })],
      { state: 'finished', next_cursor: 'c1' },
    )
    lists['finished:c1'] = list(
      [
        row({
          id: 2,
          status: 'finished',
          ended_at: '2026-11-18T10:00:00Z',
          my_result: { points: 0, state: 'void', pending_parts: 0 },
        }),
      ],
      { state: 'finished' },
    )
    serve()
    renderAt()
    await pass()
    fireEvent.click(tab('Results'))
    await pass()
    expect(screen.getAllByRole('listitem')).toHaveLength(1)
    fireEvent.click(screen.getByRole('button', { name: 'Load more' }))
    await pass()
    const items = screen.getAllByRole('listitem')
    expect(items).toHaveLength(2)
    expect(items[1]).toHaveTextContent('Void')
    expect(screen.queryByRole('button', { name: 'Load more' })).not.toBeInTheDocument()
  })

  it('tells admins how to follow an event', async () => {
    lists.upcoming = list([], { following_any: false })
    serve()
    renderAt('/matches', { ...ME, is_admin: true })
    await pass()
    expect(screen.getByRole('heading', { name: 'No events followed yet' })).toBeInTheDocument()
    expect(screen.getByText(/\/event follow/)).toBeInTheDocument()
  })

  it('tells members that admins choose events', async () => {
    lists.upcoming = list([], { following_any: false })
    serve()
    renderAt()
    await pass()
    expect(screen.getByText(/Admins choose/)).toBeInTheDocument()
    expect(screen.queryByText(/\/event follow/)).not.toBeInTheDocument()
  })

  it('shows an error with a retry', async () => {
    mockFetch(() => fakeResponse(500, { error: 'internal' }))
    renderAt()
    await pass()
    expect(screen.getByRole('alert')).toHaveTextContent('Matches could not be loaded')
  })
})
