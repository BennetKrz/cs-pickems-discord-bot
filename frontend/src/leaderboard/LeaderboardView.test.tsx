import { act, fireEvent, render, screen, within } from '@testing-library/react'
import { RouterProvider } from 'react-router'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ApiClient, type Leaderboard, type LeaderboardRow, type Me } from '../api.ts'
import { AuthContext } from '../auth-context.ts'
import { POLL_IDLE_MS, POLL_LIVE_MS } from '../poll.ts'
import { createAppRouter } from '../router.tsx'
import { fakeResponse, ME, mockFetch } from '../test-utils.ts'

const BOB = '200000000000000005'
const CAROL = '200000000000000006'

function lbRow(over: Partial<LeaderboardRow>): LeaderboardRow {
  return {
    rank: 1,
    previous_rank: 1,
    user_id: BOB,
    display_name: 'Bob',
    avatar_url: `/api/img/avatar/${BOB}?v=0`,
    points: 10,
    provisional_points: 0,
    provisional_picks: 0,
    settled_picks: 4,
    winner_hits: 3,
    winner_decided: 4,
    winner_accuracy: 0.75,
    series_hits: 1,
    map_hits: 2,
    ...over,
  }
}

function board(rows: LeaderboardRow[], over: Partial<Leaderboard> = {}): Leaderboard {
  return {
    event_id: null,
    events: [
      { id: 7, name: 'IEM Test' },
      { id: 8, name: 'Major' },
    ],
    match_day: '2026-11-20',
    has_previous: true,
    live: false,
    rows,
    ...over,
  }
}

/** Boards by query string (`''` for all time); tests change them. */
let boards: Record<string, Leaderboard>

function serve() {
  return mockFetch((url) => {
    const u = new URL(url, 'http://x')
    if (u.pathname === '/api/leaderboard') {
      const body = boards[u.searchParams.get('event_id') ?? '']
      return body ? fakeResponse(200, body) : fakeResponse(404, { error: 'not_found' })
    }
    return fakeResponse(304)
  })
}

function leaderboardCalls(fetch: ReturnType<typeof serve>): string[] {
  return fetch.mock.calls.map(([u]) => String(u)).filter((u) => u.startsWith('/api/leaderboard'))
}

function renderAt(path = '/leaderboard', me: Me = ME) {
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

function rows() {
  return within(screen.getByRole('list', { name: 'Leaderboard' })).getAllByRole('link')
}

beforeEach(() => {
  vi.useFakeTimers()
  vi.setSystemTime(Date.parse('2026-11-20T12:00:00Z'))
  boards = { '': board([]) }
})
afterEach(() => {
  vi.useRealTimers()
})

describe('Leaderboard tab', () => {
  it('ranks members with points, stats and rank changes', async () => {
    serve()
    boards[''] = board([
      lbRow({ rank: 1, previous_rank: 3, user_id: BOB, display_name: 'Bob' }),
      lbRow({
        rank: 2,
        previous_rank: 1,
        user_id: ME.user_id,
        display_name: 'ali',
        points: 7,
        provisional_points: 3,
        provisional_picks: 1,
        settled_picks: 1,
        winner_accuracy: null,
        series_hits: 0,
        map_hits: 1,
      }),
      lbRow({ rank: 2, previous_rank: 2, user_id: CAROL, display_name: 'Carol', points: 7 }),
      lbRow({ rank: 4, previous_rank: null, user_id: '9', display_name: 'Dan', points: 1 }),
    ])
    renderAt()
    await pass()

    const [bob, me, carol, dan] = rows()
    expect(bob).toHaveTextContent('1')
    expect(bob).toHaveTextContent('▲2')
    expect(within(bob!).getByText('Up 2 since the last match day')).toBeInTheDocument()
    expect(bob).toHaveTextContent('Bob')
    expect(bob).toHaveTextContent('10 pts')
    expect(bob).toHaveTextContent('4 picks · Winner 75% · Exact series 1 · Exact maps 2')

    // Provisional points count and are marked.
    expect(me).toHaveTextContent('7 pts')
    expect(me).toHaveTextContent('+3 live')
    expect(me).toHaveTextContent('▼1')
    expect(me).toHaveTextContent('1 pick · Winner –')
    expect(carol).toHaveTextContent('–')
    expect(carol).not.toHaveTextContent('live')
    // A shared rank shows the same number twice.
    expect(within(me!).getByText('2', { selector: '.board-rank-number' })).toBeInTheDocument()
    expect(within(carol!).getByText('2', { selector: '.board-rank-number' })).toBeInTheDocument()
    expect(dan).toHaveTextContent('new')
    expect(screen.getByText(/Arrows show changes on the last match day/)).toBeInTheDocument()
    expect(document.body).not.toHaveTextContent(/\bbet/i)
  })

  it('shows no rank changes without an earlier match day', async () => {
    serve()
    boards[''] = board([lbRow({ previous_rank: null })], { has_previous: false })
    renderAt()
    await pass()
    expect(rows()[0]).not.toHaveTextContent('new')
    expect(screen.queryByText(/Arrows show/)).not.toBeInTheDocument()
  })

  it('highlights my row and jumps to it', async () => {
    serve()
    const scroll = vi.fn()
    Element.prototype.scrollIntoView = scroll
    boards[''] = board([
      lbRow({ user_id: BOB }),
      lbRow({ rank: 2, user_id: ME.user_id, display_name: 'ali' }),
    ])
    renderAt()
    await pass()

    const [bob, me] = rows()
    expect(me).toHaveAttribute('aria-current', 'true')
    expect(me).toHaveClass('is-me')
    expect(bob).not.toHaveAttribute('aria-current')

    fireEvent.click(screen.getByRole('button', { name: 'Jump to me' }))
    expect(scroll).toHaveBeenCalledTimes(1)
    expect(scroll.mock.contexts[0]).toBe(me)
    expect(me).toHaveFocus()
  })

  it('has no jump control when I am not on the board', async () => {
    serve()
    boards[''] = board([lbRow({ user_id: BOB })])
    renderAt()
    await pass()
    expect(rows()).toHaveLength(1)
    expect(screen.queryByRole('button', { name: 'Jump to me' })).not.toBeInTheDocument()
  })

  it('opens my profile and other members’ profiles', async () => {
    serve()
    boards[''] = board([
      lbRow({ user_id: BOB, display_name: 'Bob' }),
      lbRow({ rank: 2, user_id: ME.user_id, display_name: 'ali' }),
    ])
    renderAt()
    await pass()
    const [bob, me] = rows()
    expect(me).toHaveAttribute('href', '/me')
    expect(bob).toHaveAttribute('href', `/members/${BOB}`)

    // The name shows at once, before the profile has loaded.
    fireEvent.click(bob!)
    await pass()
    expect(screen.getByRole('heading', { name: 'Bob' })).toBeInTheDocument()
    fireEvent.click(screen.getByRole('link', { name: '← Leaderboard' }))
    await pass()
    expect(rows()).toHaveLength(2)
  })

  it('opens profiles with the same event filter', async () => {
    serve()
    boards['8'] = board(
      [
        lbRow({ user_id: BOB, display_name: 'Bob' }),
        lbRow({ rank: 2, user_id: ME.user_id, display_name: 'ali' }),
      ],
      { event_id: 8 },
    )
    renderAt('/leaderboard?event=8')
    await pass()
    const [bob, me] = rows()
    expect(me).toHaveAttribute('href', '/me?event=8')
    expect(bob).toHaveAttribute('href', `/members/${BOB}?event=8`)

    // The back link returns to the filtered board.
    fireEvent.click(bob!)
    await pass()
    fireEvent.click(screen.getByRole('link', { name: '← Leaderboard' }))
    await pass()
    expect(screen.getByRole('combobox', { name: 'Show' })).toHaveValue('8')
  })

  it('filters by a followed event', async () => {
    const fetch = serve()
    boards[''] = board([lbRow({ user_id: BOB })])
    boards['8'] = board([], { event_id: 8 })
    renderAt()
    await pass()
    const select = screen.getByRole('combobox', { name: 'Show' })
    expect(within(select).getAllByRole('option').map((o) => o.textContent)).toEqual([
      'All time',
      'IEM Test',
      'Major',
    ])

    fireEvent.change(select, { target: { value: '8' } })
    await pass()
    expect(leaderboardCalls(fetch)).toContain('/api/leaderboard?event_id=8')
    expect(screen.getByRole('heading', { name: 'No points yet' })).toBeInTheDocument()
    expect(screen.getByText('Nobody has a settled pick for this event yet.')).toBeInTheDocument()

    fireEvent.change(screen.getByRole('combobox', { name: 'Show' }), { target: { value: '' } })
    await pass()
    expect(rows()).toHaveLength(1)
  })

  it('says so when nobody has points yet', async () => {
    serve()
    renderAt()
    await pass()
    expect(screen.getByRole('heading', { name: 'No points yet' })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Jump to me' })).not.toBeInTheDocument()
  })

  it('shows a settlement change within one poll interval', async () => {
    serve()
    boards[''] = board([lbRow({ user_id: BOB, points: 10 })])
    renderAt()
    await pass()
    expect(rows()[0]).toHaveTextContent('10 pts')

    // A correction re-settled the match.
    boards[''] = board([lbRow({ user_id: BOB, points: 4 })])
    await pass(POLL_IDLE_MS - 1)
    expect(rows()[0]).toHaveTextContent('10 pts')
    await pass(1)
    expect(rows()[0]).toHaveTextContent('4 pts')
  })

  it('polls every 20 s while a match is live', async () => {
    const fetch = serve()
    boards[''] = board([lbRow({ provisional_picks: 1, provisional_points: 2 })], { live: true })
    renderAt()
    await pass()
    const before = leaderboardCalls(fetch).length

    boards[''] = board([lbRow({ points: 12, provisional_picks: 1, provisional_points: 4 })], {
      live: true,
    })
    await pass(POLL_LIVE_MS)
    expect(leaderboardCalls(fetch).length).toBeGreaterThan(before)
    expect(rows()[0]).toHaveTextContent('12 pts')
    expect(rows()[0]).toHaveTextContent('+4 live')
  })

  it('offers a retry when loading fails', async () => {
    mockFetch(() => fakeResponse(500, { error: 'internal' }))
    renderAt()
    await pass()
    expect(screen.getByRole('alert')).toHaveTextContent('Leaderboard could not be loaded')
  })
})
