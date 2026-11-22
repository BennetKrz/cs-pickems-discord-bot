import { act, fireEvent, render, screen, within } from '@testing-library/react'
import { RouterProvider } from 'react-router'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  ApiClient,
  type Leaderboard,
  type Me,
  type Profile,
  type ProfilePick,
  type ProfilePicks,
} from '../api.ts'
import { AuthContext } from '../auth-context.ts'
import { detail, NAVI, pickView, picks as matchPicks, row, VIT } from '../matches/test-fixtures.ts'
import { POLL_IDLE_MS, POLL_LIVE_MS } from '../poll.ts'
import { createAppRouter } from '../router.tsx'
import { fakeResponse, ME, mockFetch } from '../test-utils.ts'

const BOB = '200000000000000005'

function profile(over: Partial<Profile> = {}): Profile {
  const acc = (hits: number, decided: number) => ({
    hits,
    decided,
    rate: decided === 0 ? null : hits / decided,
  })
  return {
    user_id: BOB,
    display_name: 'Bob',
    avatar_url: `/api/img/avatar/${BOB}?v=0`,
    is_me: false,
    event_id: null,
    events: [
      { id: 7, name: 'IEM Test' },
      { id: 8, name: 'Major' },
    ],
    rank: 2,
    previous_rank: 2,
    points: 12,
    provisional_points: 3,
    provisional_picks: 1,
    settled_picks: 4,
    accuracy: {
      winner: acc(3, 4),
      series: acc(1, 3),
      map_winner: acc(2, 2),
      map_score: acc(0, 0),
    },
    picks: { open: 1, live: 0, settled: 2, total: 3 },
    live: false,
    ...over,
  }
}

function header(over: Parameters<typeof row>[0] = {}): ProfilePick['match'] {
  const { my_pick: _p, my_result: _r, ...h } = row(over)
  void _p
  void _r
  return h
}

function item(over: Partial<ProfilePick> = {}): ProfilePick {
  return {
    match: header(),
    pick: { winner_team_id: VIT.id, series: null, maps: [] },
    updated_at: '2026-11-20T09:00:00Z',
    result: null,
    ...over,
  }
}

function page(items: ProfilePick[], over: Partial<ProfilePicks> = {}): ProfilePicks {
  return { state: 'open', picks: items, next_cursor: null, ...over }
}

/** Answers by path and query; tests change them. */
let profiles: Record<string, Profile>
let lists: Record<string, ProfilePicks>
let failProfile: boolean

function serve() {
  return mockFetch((url) => {
    const u = new URL(url, 'http://x')
    const m = /^\/api\/users\/(\d+)\/(profile|picks)$/.exec(u.pathname)
    if (m) {
      const [, user, kind] = m
      const ev = u.searchParams.get('event_id') ?? ''
      if (kind === 'profile') {
        if (failProfile) return fakeResponse(500, { error: 'db' })
        const body = profiles[`${user}:${ev}`]
        return body ? fakeResponse(200, body) : fakeResponse(404, { error: 'not_found' })
      }
      const state = u.searchParams.get('state')
      const key = `${user}:${state}:${ev}:${u.searchParams.get('cursor') ?? ''}`
      const body = lists[key]
      return body ? fakeResponse(200, body) : fakeResponse(200, page([]))
    }
    if (u.pathname === '/api/leaderboard') {
      const lb: Leaderboard = {
        event_id: null,
        events: [],
        match_day: null,
        has_previous: false,
        live: false,
        rows: [
          {
            rank: 1,
            previous_rank: null,
            user_id: BOB,
            display_name: 'Bob',
            avatar_url: `/api/img/avatar/${BOB}?v=0`,
            points: 12,
            provisional_points: 3,
            provisional_picks: 1,
            settled_picks: 4,
            winner_hits: 3,
            winner_decided: 4,
            winner_accuracy: 0.75,
            series_hits: 1,
            map_hits: 0,
          },
        ],
      }
      return fakeResponse(200, lb)
    }
    if (u.pathname.startsWith('/api/matches?') || u.pathname === '/api/matches') {
      const empty = { state: 'upcoming', following_any: true, matches: [], next_cursor: null }
      return fakeResponse(200, empty)
    }
    if (u.pathname === '/api/matches/10') return fakeResponse(200, detail())
    if (u.pathname === '/api/matches/10/picks') {
      return fakeResponse(
        200,
        matchPicks([pickView({ user_id: BOB, display_name: 'Bob' })], { match_id: 10 }),
      )
    }
    return fakeResponse(304)
  })
}

function calls(fetch: ReturnType<typeof serve>, prefix: string): string[] {
  return fetch.mock.calls.map(([u]) => String(u)).filter((u) => u.startsWith(prefix))
}

function renderAt(path: string, me: Me = ME) {
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

function stat(label: string): HTMLElement {
  return screen.getByText(label, { selector: 'dt' }).parentElement!
}

/** The picked matches of a list (not the parts inside each). */
function historyItems(name: string): HTMLElement[] {
  const list = screen.getByRole('list', { name: `${name} picks` })
  return Array.from(list.children) as HTMLElement[]
}

beforeEach(() => {
  vi.useFakeTimers()
  vi.setSystemTime(Date.parse('2026-11-20T12:00:00Z'))
  failProfile = false
  profiles = { [`${BOB}:`]: profile() }
  lists = {}
})
afterEach(() => {
  vi.useRealTimers()
})

describe('Profile', () => {
  it('shows rank, points, picks and accuracy per part', async () => {
    serve()
    renderAt(`/members/${BOB}`)
    await pass()
    expect(screen.getByRole('heading', { name: 'Bob' })).toBeInTheDocument()
    expect(stat('Rank')).toHaveTextContent('#2')
    expect(stat('Points')).toHaveTextContent('12 pts')
    expect(stat('Points')).toHaveTextContent('+3 live')
    expect(stat('Picks')).toHaveTextContent('3')
    expect(stat('Picks')).toHaveTextContent('4 settled')
    const acc = within(screen.getByRole('region', { name: 'Accuracy' }))
    const tile = (label: string) => acc.getByText(label, { selector: 'dt' }).parentElement!
    expect(tile('Winner')).toHaveTextContent('75%3/4')
    expect(tile('Exact series')).toHaveTextContent('33%1/3')
    expect(tile('Map winner')).toHaveTextContent('100%2/2')
    expect(tile('Exact map')).toHaveTextContent('–0/0')
  })

  it('shows an unranked member without picks', async () => {
    serve()
    profiles[`${BOB}:`] = profile({
      rank: null,
      points: 0,
      provisional_picks: 0,
      settled_picks: 0,
      picks: { open: 0, live: 0, settled: 0, total: 0 },
    })
    renderAt(`/members/${BOB}`)
    await pass()
    expect(stat('Rank')).toHaveTextContent('–')
    expect(stat('Points')).not.toHaveTextContent('live')
    expect(screen.getByRole('heading', { name: 'No open picks' })).toBeInTheDocument()
    expect(screen.getByText('No picks on matches that haven’t started.')).toBeInTheDocument()
  })

  it('lists open, live and settled picks with points per part', async () => {
    serve()
    profiles[`${BOB}:`] = profile({ picks: { open: 1, live: 1, settled: 2, total: 4 }, live: true })
    lists[`${BOB}:live::`] = page(
      [
        item({
          match: header({ id: 11, status: 'live', locked: true, pickable: false, maps_won_a: 1 }),
          pick: {
            winner_team_id: VIT.id,
            series: null,
            maps: [
              { map_number: 1, winner_team_id: VIT.id, rounds: { a: 13, b: 7 }, overtime: false },
            ],
          },
          result: {
            points: 3,
            state: 'provisional',
            pending_parts: 1,
            breakdown: {
              winner: { outcome: 'pending', points: 0 },
              series: null,
              maps: [
                {
                  map_number: 1,
                  winner: { outcome: 'hit', points: 1 },
                  score: { outcome: 'hit', points: 2 },
                },
              ],
            },
          },
        }),
      ],
      { state: 'live' },
    )
    lists[`${BOB}:settled::`] = page(
      [
        item({
          match: header({ id: 12, status: 'finished', locked: true, maps_won_a: 2 }),
          pick: { winner_team_id: VIT.id, series: { a: 2, b: 1 }, maps: [] },
          result: {
            points: 3,
            state: 'final',
            pending_parts: 0,
            breakdown: {
              winner: { outcome: 'hit', points: 3 },
              series: { outcome: 'miss', points: 0 },
              maps: [],
            },
          },
        }),
        item({
          match: header({ id: 13, status: 'finished', voided: true, locked: true }),
          pick: { winner_team_id: NAVI.id, series: null, maps: [] },
          result: {
            points: 0,
            state: 'void',
            pending_parts: 0,
            breakdown: { winner: { outcome: 'void', points: 0 }, series: null, maps: [] },
          },
        }),
      ],
      { state: 'settled' },
    )
    renderAt(`/members/${BOB}`)
    await pass()

    // A live pick opens the Live list first; the tabs carry counts.
    expect(screen.getByRole('tab', { name: 'Live 1' })).toHaveAttribute('aria-selected', 'true')
    expect(screen.getByRole('tab', { name: 'Open 1' })).toBeInTheDocument()
    const [live] = historyItems('Live')
    expect(live).toHaveTextContent('3 pts so far')
    expect(live).toHaveTextContent('Map 1 VIT+1')
    expect(live).toHaveTextContent('Map 1 score 13:7+2')
    expect(live).toHaveTextContent('Winner VITpending')
    expect(within(live!).getByRole('link')).toHaveAttribute('href', '/matches/11')

    fireEvent.click(screen.getByRole('tab', { name: 'Settled 2' }))
    await pass()
    const [won, voided] = historyItems('Settled')
    expect(won).toHaveTextContent('3 pts')
    expect(won).toHaveTextContent('Winner VIT+3')
    expect(won).toHaveTextContent('Series 2:10')
    // Void picks are listed and marked.
    expect(voided).toHaveClass('is-void')
    expect(within(voided!).getByText('Void')).toBeInTheDocument()
    // The whole pick is void; its parts carry no outcome of their own.
    expect(voided).toHaveTextContent(/Winner NAVI$/)
  })

  it('offers to edit my own open picks, and only while pickable', async () => {
    serve()
    profiles[`${ME.user_id}:`] = profile({
      user_id: ME.user_id,
      display_name: 'ali',
      is_me: true,
      picks: { open: 2, live: 0, settled: 0, total: 2 },
    })
    const open = page([
      item({ match: header({ id: 20 }) }),
      item({ match: header({ id: 21, locked: true, pickable: false }) }),
    ])
    lists[`${ME.user_id}:open::`] = open
    lists[`${BOB}:open::`] = open
    const mine = renderAt('/me')
    await pass()
    // The Me tab keeps the handle.
    expect(screen.getByText('@alice')).toBeInTheDocument()
    let [editable, locked] = historyItems('Open')
    expect(within(editable!).getByText('Edit pick')).toBeInTheDocument()
    expect(within(editable!).getByRole('link')).toHaveAttribute('href', '/matches/20')
    expect(within(locked!).queryByText('Edit pick')).toBeNull()
    expect(within(locked!).getByText('Locked')).toBeInTheDocument()

    // Someone else's open picks: read-only.
    mine.unmount()
    renderAt(`/members/${BOB}`)
    await pass()
    ;[editable, locked] = historyItems('Open')
    expect(within(editable!).queryByText('Edit pick')).toBeNull()
    expect(within(locked!).getByText('Locked')).toBeInTheDocument()
  })

  it('filters by event', async () => {
    const fetch = serve()
    const picks = { open: 0, live: 0, settled: 1, total: 1 }
    profiles[`${BOB}:8`] = profile({ event_id: 8, points: 5, picks })
    renderAt(`/members/${BOB}`)
    await pass()
    const select = screen.getByRole('combobox', { name: 'Show' })
    expect(within(select).getAllByRole('option').map((o) => o.textContent)).toEqual([
      'All time',
      'IEM Test',
      'Major',
    ])
    fireEvent.change(select, { target: { value: '8' } })
    await pass()
    expect(calls(fetch, `/api/users/${BOB}/profile`)).toContain(
      `/api/users/${BOB}/profile?event_id=8`,
    )
    expect(calls(fetch, `/api/users/${BOB}/picks`)).toContain(
      `/api/users/${BOB}/picks?state=settled&event_id=8`,
    )
    expect(stat('Points')).toHaveTextContent('5 pts')
  })

  it('opens with the filter from the URL, also for an unfollowed event', async () => {
    serve()
    profiles[`${BOB}:99`] = profile({ event_id: 99 })
    renderAt(`/members/${BOB}?event=99`)
    await pass()
    expect(screen.getByRole('combobox', { name: 'Show' })).toHaveValue('99')
    expect(screen.getByRole('option', { name: 'Unfollowed event' })).toBeInTheDocument()
  })

  it('loads more settled picks', async () => {
    serve()
    profiles[`${BOB}:`] = profile({ picks: { open: 0, live: 0, settled: 3, total: 3 } })
    lists[`${BOB}:settled::`] = page(
      [
        item({ match: header({ id: 30, status: 'finished' }) }),
        item({ match: header({ id: 31, status: 'finished' }) }),
      ],
      { state: 'settled', next_cursor: 'c1' },
    )
    lists[`${BOB}:settled::c1`] = page([item({ match: header({ id: 32, status: 'finished' }) })], {
      state: 'settled',
    })
    renderAt(`/members/${BOB}`)
    await pass()
    expect(historyItems('Settled')).toHaveLength(2)
    fireEvent.click(screen.getByRole('button', { name: 'Load more' }))
    await pass()
    expect(historyItems('Settled')).toHaveLength(3)
    expect(screen.queryByRole('button', { name: 'Load more' })).toBeNull()
  })

  it('polls every 20 s while a picked match is live', async () => {
    const fetch = serve()
    renderAt(`/members/${BOB}`)
    await pass()
    const n = () => calls(fetch, `/api/users/${BOB}/profile`).length
    const before = n()
    await pass(POLL_LIVE_MS)
    expect(n()).toBe(before)
    await pass(POLL_IDLE_MS - POLL_LIVE_MS)
    expect(n()).toBe(before + 1)

    profiles[`${BOB}:`] = profile({ live: true })
    await pass(POLL_IDLE_MS)
    const live = n()
    await pass(POLL_LIVE_MS)
    expect(n()).toBe(live + 1)
  })

  it('offers a retry when loading fails', async () => {
    serve()
    failProfile = true
    renderAt(`/members/${BOB}`)
    await pass()
    expect(screen.getByRole('heading', { name: 'Profile could not be loaded' })).toBeInTheDocument()
    failProfile = false
    fireEvent.click(screen.getByRole('button', { name: /try again/i }))
    await pass()
    expect(stat('Rank')).toHaveTextContent('#2')
  })

  it('sends my own member page to the Me tab', async () => {
    serve()
    profiles[`${ME.user_id}:`] = profile({ user_id: ME.user_id, display_name: 'ali', is_me: true })
    renderAt(`/members/${ME.user_id}`)
    await pass()
    expect(screen.getByText('@alice')).toBeInTheDocument()
  })
})

describe('Reaching a profile', () => {
  // AC 3: two taps from anywhere: the Leaderboard tab, then the member.
  it('takes two taps from the leaderboard', async () => {
    serve()
    renderAt('/matches')
    await pass()
    fireEvent.click(screen.getByRole('link', { name: 'Leaderboard' }))
    await pass()
    fireEvent.click(within(screen.getByRole('list', { name: 'Leaderboard' })).getByRole('link'))
    await pass()
    expect(screen.getByRole('heading', { name: 'Bob' })).toBeInTheDocument()
    expect(stat('Rank')).toHaveTextContent('#2')
    expect(screen.getByRole('link', { name: '← Leaderboard' })).toBeInTheDocument()
  })

  it('opens from a pick list and leads back to the match', async () => {
    serve()
    renderAt('/matches/10')
    await pass()
    fireEvent.click(screen.getByRole('link', { name: 'Bob' }))
    await pass()
    expect(screen.getByRole('heading', { name: 'Bob' })).toBeInTheDocument()
    expect(stat('Points')).toHaveTextContent('12 pts')
    fireEvent.click(screen.getByRole('link', { name: '← Match' }))
    await pass()
    expect(screen.getByRole('region', { name: 'Everyone’s picks' })).toBeInTheDocument()
  })
})
