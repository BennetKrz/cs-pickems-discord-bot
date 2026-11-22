import { act, fireEvent, render, screen, within } from '@testing-library/react'
import { RouterProvider } from 'react-router'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ApiClient, type MatchDetail, type MatchPicks, type PickParts } from '../api.ts'
import { AuthContext } from '../auth-context.ts'
import { resetServerClock } from '../clock.ts'
import { createAppRouter } from '../router.tsx'
import { fakeResponse, ME, mockFetch } from '../test-utils.ts'
import { detail, myPick, pickView, picks } from './test-fixtures.ts'

let match: MatchDetail
let allPicks: MatchPicks
/** The next answer to a write, if not the default. */
let writeAnswer: (() => Response) | null
let headers: Record<string, string>

function serve() {
  return mockFetch((url, init) => {
    const method = init?.method ?? 'GET'
    if (url === '/api/matches/10' && method === 'GET') return fakeResponse(200, match, headers)
    if (url === '/api/matches/10/picks') return fakeResponse(200, allPicks, headers)
    if (url === '/api/matches/10/pick') {
      if (writeAnswer) return writeAnswer()
      if (method === 'DELETE') {
        allPicks = { ...allPicks, picks: allPicks.picks.filter((p) => p.user_id !== ME.user_id) }
        return fakeResponse(204)
      }
      const body = JSON.parse(init?.body as string) as PickParts
      const saved = myPick({
        winner_team_id: body.winner_team_id ?? null,
        series: body.series ?? null,
        maps: (body.maps ?? []).map((m) => ({
          map_number: m.map_number,
          winner_team_id: m.winner_team_id,
          rounds: m.rounds ?? null,
          overtime: m.overtime ?? false,
        })),
        edited: allPicks.picks.some((p) => p.user_id === ME.user_id),
      })
      allPicks = {
        ...allPicks,
        picks: [...allPicks.picks.filter((p) => p.user_id !== ME.user_id), saved],
      }
      return fakeResponse(200, { ...saved, match_id: 10 })
    }
    return fakeResponse(304)
  })
}

function renderDetail() {
  const api = new ApiClient('token', () => {})
  return render(
    <AuthContext.Provider value={{ me: ME, api }}>
      <RouterProvider router={createAppRouter('/matches/10')} />
    </AuthContext.Provider>,
  )
}

async function pass(ms = 0) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms)
  })
}

function form() {
  return screen.getByRole('region', { name: 'Your pick' })
}

function writes(fetch: ReturnType<typeof serve>) {
  return fetch.mock.calls
    .filter(([u, init]) => String(u) === '/api/matches/10/pick' && init?.method)
    .map(([, init]) => ({
      method: init?.method,
      body: init?.body ? (JSON.parse(init.body as string) as unknown) : undefined,
    }))
}

beforeEach(() => {
  vi.useFakeTimers()
  vi.setSystemTime(Date.parse('2026-11-20T17:00:00Z'))
  resetServerClock()
  match = detail({
    maps: [
      {
        number: 1,
        map_name: 'Mirage',
        status: 'upcoming',
        rounds_a: null,
        rounds_b: null,
        winner_team_id: null,
      },
    ],
  })
  allPicks = picks([pickView()])
  writeAnswer = null
  headers = {}
})
afterEach(() => {
  vi.useRealTimers()
})

describe('match detail', () => {
  it('shows the header, maps and everyone’s picks', async () => {
    serve()
    renderDetail()
    await pass()
    expect(screen.getByText('IEM Test · Playoffs · BO3')).toBeInTheDocument()
    expect(screen.getByRole('link', { name: '← Matches' })).toHaveAttribute('href', '/matches')
    const maps = screen.getByRole('region', { name: 'Maps' })
    expect(maps).toHaveTextContent('Map 1')
    expect(maps).toHaveTextContent('Mirage')
    // 18:00 UTC is 19:00 in Berlin.
    expect(screen.getByRole('time')).toHaveTextContent(/19:00|07:00\sPM/)
    const everyone = screen.getByRole('region', { name: 'Everyone’s picks' })
    expect(everyone).toHaveTextContent('bob')
    expect(everyone).toHaveTextContent('Winner VIT')
    expect(within(form()).getByRole('timer')).toHaveTextContent('Locks in 1 h 0 min')
  })

  it('creates a pick with winner, series and a map', async () => {
    const fetch = serve()
    renderDetail()
    await pass()
    const f = form()
    expect(within(f).queryByRole('button', { name: 'Remove pick' })).not.toBeInTheDocument()
    expect(within(f).getByRole('button', { name: 'Save' })).toBeDisabled()

    fireEvent.click(within(f).getByRole('button', { name: '2:1' }))
    const vitality = within(f).getByRole('button', { name: /Vitality/ })
    expect(vitality).toHaveAttribute('aria-pressed', 'true')
    fireEvent.click(within(f).getByRole('button', { name: 'Map 1: VIT wins' }))
    fireEvent.change(within(f).getByRole('combobox', { name: 'Map 1 score' }), {
      target: { value: '9' },
    })
    fireEvent.click(within(f).getByRole('button', { name: 'Save' }))
    await pass()

    expect(writes(fetch)).toEqual([
      {
        method: 'PUT',
        body: {
          winner_team_id: 1,
          series: { a: 2, b: 1 },
          maps: [{ map_number: 1, winner_team_id: 1, rounds: { a: 13, b: 9 }, overtime: false }],
        },
      },
    ])
    expect(screen.getByRole('status')).toHaveTextContent('Pick saved.')
    // The refreshed picks bring the saved pick: the form now edits it.
    expect(within(form()).getByRole('button', { name: 'Save changes' })).toBeDisabled()
    expect(within(form()).getByRole('button', { name: 'Remove pick' })).toBeInTheDocument()
    expect(screen.getByRole('region', { name: 'Everyone’s picks' })).toHaveTextContent('ali (you)')
  })

  it('opens an existing pick pre-filled and saves changes', async () => {
    allPicks = picks([
      myPick({
        winner_team_id: 1,
        series: { a: 2, b: 0 },
        edited: true,
        updated_at: '2026-11-20T16:30:00Z',
      }),
    ])
    const fetch = serve()
    renderDetail()
    await pass()
    const f = form()
    expect(within(f).getByRole('button', { name: '2:0' })).toHaveAttribute('aria-pressed', 'true')
    // Edited marker with the local time of the last change.
    expect(screen.getByRole('region', { name: 'Everyone’s picks' })).toHaveTextContent(
      /edited .*(17:30|05:30\sPM)/,
    )

    fireEvent.click(within(f).getByRole('button', { name: /Natus Vincere/ }))
    // The 2:0 no longer fits NAVI and is cleared.
    expect(within(f).getByRole('button', { name: '2:0' })).toHaveAttribute('aria-pressed', 'false')
    fireEvent.click(within(f).getByRole('button', { name: 'Save changes' }))
    await pass()
    expect(writes(fetch)).toEqual([
      { method: 'PUT', body: { winner_team_id: 2, series: null, maps: [] } },
    ])
    expect(screen.getByRole('status')).toHaveTextContent('Changes saved.')
  })

  it('removes a pick', async () => {
    allPicks = picks([myPick({ winner_team_id: 1 })])
    const fetch = serve()
    renderDetail()
    await pass()
    fireEvent.click(within(form()).getByRole('button', { name: 'Remove pick' }))
    await pass()
    expect(writes(fetch)).toEqual([{ method: 'DELETE', body: undefined }])
    expect(within(form()).getByRole('button', { name: 'Save' })).toBeDisabled()
    expect(within(form()).queryByRole('button', { name: 'Remove pick' })).not.toBeInTheDocument()
  })

  it('shows the soft hint when map picks and the series disagree', async () => {
    match = { ...match, maps: [] }
    serve()
    renderDetail()
    await pass()
    const f = form()
    fireEvent.click(within(f).getByRole('button', { name: '2:0' }))
    fireEvent.click(within(f).getByRole('button', { name: 'Map 1: VIT wins' }))
    fireEvent.click(within(f).getByRole('button', { name: 'Map 2: NAVI wins' }))
    fireEvent.click(within(f).getByRole('button', { name: 'Map 3: VIT wins' }))
    expect(within(f).getByRole('note')).toHaveTextContent(
      'Your map picks add up to 2:1, your series pick is 2:0.',
    )
    // A hint never blocks saving.
    expect(within(f).getByRole('button', { name: 'Save' })).toBeEnabled()
  })

  it('turns read-only exactly at the lock while the view stays open', async () => {
    vi.setSystemTime(Date.parse('2026-11-20T17:59:58Z'))
    allPicks = picks([myPick({ winner_team_id: 2 }), pickView()])
    const fetch = serve()
    renderDetail()
    await pass()
    const before = fetch.mock.calls.length
    expect(within(form()).getByRole('timer')).toHaveTextContent('Locks in 2 s')

    await pass(1_999)
    expect(within(form()).getByRole('button', { name: 'Save changes' })).toBeInTheDocument()
    await pass(1)
    const mine = form()
    expect(within(mine).queryByRole('button')).not.toBeInTheDocument()
    expect(mine).toHaveTextContent('Picks are locked.')
    expect(mine).toHaveTextContent(/Winner\s*NAVI/)
    // No poll was needed, and others' picks stay visible.
    expect(fetch.mock.calls.length).toBe(before)
    expect(screen.getByRole('region', { name: 'Everyone’s picks' })).toHaveTextContent('bob')
  })

  it('locks on the server clock, also when the browser clock is behind', async () => {
    // The browser says 17:00, the server 17:59:59.
    headers = { 'X-Server-Time': '2026-11-20T17:59:59.000Z' }
    serve()
    renderDetail()
    await pass()
    expect(within(form()).getByRole('timer')).toHaveTextContent('Locks in 1 s')
    await pass(1_000)
    expect(form()).toHaveTextContent('Picks are locked.')
  })

  it('maps pick_locked to a message and goes read-only', async () => {
    const fetch = serve()
    renderDetail()
    await pass()
    writeAnswer = () => fakeResponse(409, { error: 'pick_locked', message: 'locked' })
    fireEvent.click(within(form()).getByRole('button', { name: /Vitality/ }))
    fireEvent.click(within(form()).getByRole('button', { name: 'Save' }))
    await pass()
    expect(writes(fetch)).toHaveLength(1)
    expect(form()).toHaveTextContent('Picks are locked.')
    expect(within(form()).queryByRole('button')).not.toBeInTheDocument()
  })

  it('maps invalid_pick to a message and keeps the form', async () => {
    serve()
    renderDetail()
    await pass()
    writeAnswer = () =>
      fakeResponse(422, { error: 'invalid_pick', message: '3:0 is not a valid series score' })
    fireEvent.click(within(form()).getByRole('button', { name: /Vitality/ }))
    fireEvent.click(within(form()).getByRole('button', { name: 'Save' }))
    await pass()
    expect(screen.getByRole('alert')).toHaveTextContent(
      "That pick isn't valid: 3:0 is not a valid series score",
    )
    expect(within(form()).getByRole('button', { name: 'Save' })).toBeEnabled()
  })

  it('hides the form when a poll says the match is no longer pickable', async () => {
    serve()
    renderDetail()
    await pass()
    expect(within(form()).getByRole('button', { name: 'Save' })).toBeInTheDocument()
    allPicks = { ...allPicks, pickable: false, locked: true }
    match = { ...match, status: 'live', locked: true, pickable: false }
    await pass(20_000)
    expect(form()).toHaveTextContent('Picks are locked.')
  })

  it('after settlement: my breakdown, and everyone sorted by points', async () => {
    match = {
      ...match,
      status: 'finished',
      locked: true,
      pickable: false,
      maps_won_a: 2,
      winner_team_id: 1,
      started_at: '2026-11-20T15:00:00Z',
      ended_at: '2026-11-20T16:30:00Z',
    }
    const settled = (points: number) => ({
      points,
      state: 'final' as const,
      pending_parts: 1,
      breakdown: {
        winner: { outcome: 'hit' as const, points: 3 },
        series: { outcome: 'miss' as const, points: 0 },
        maps: [
          {
            map_number: 1,
            winner: { outcome: 'hit' as const, points: 1 },
            score: { outcome: 'pending' as const, points: 0 },
          },
        ],
      },
    })
    allPicks = picks(
      [
        pickView({ display_name: 'bob', result: settled(1) }),
        myPick({
          winner_team_id: 1,
          series: { a: 2, b: 1 },
          maps: [{ map_number: 1, winner_team_id: 1, rounds: { a: 13, b: 5 }, overtime: false }],
          result: settled(4),
        }),
        pickView({ user_id: '3', display_name: 'carl', result: null }),
      ],
      { pickable: false, locked: true },
    )
    serve()
    renderDetail()
    await pass()

    const mine = form()
    expect(mine).toHaveTextContent('4 pts')
    const lines = within(mine).getAllByRole('listitem').map((li) => li.textContent)
    expect(lines).toEqual(['WinnerVIT+3', 'Series2:10', 'Map 1VIT 13:5+1 pending'])
    expect(mine).toHaveTextContent('1 part is waiting for results.')

    const order = within(screen.getByRole('region', { name: 'Everyone’s picks' }))
      .getAllByRole('listitem')
      .map((li) => li.querySelector('.pick-name')?.textContent)
    expect(order).toEqual(['ali', 'bob', 'carl'])
  })

  it('explains why a TBD match cannot be picked', async () => {
    match = { ...match, team_b: null, pickable: false }
    allPicks = picks([], { pickable: false })
    serve()
    renderDetail()
    await pass()
    expect(form()).toHaveTextContent('Picks open once both teams are known.')
    const everyone = screen.getByRole('region', { name: 'Everyone’s picks' })
    expect(everyone).toHaveTextContent('No picks yet.')
  })

  it('shows not found for an unknown match', async () => {
    mockFetch(() => fakeResponse(404, { error: 'not_found' }))
    renderDetail()
    await pass()
    expect(screen.getByRole('alert')).toHaveTextContent('Match not found')
  })
})
