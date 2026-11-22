import { act, fireEvent, render, screen, within } from '@testing-library/react'
import { RouterProvider } from 'react-router'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ApiClient, type EventList, type Me } from '../api.ts'
import { AuthContext } from '../auth-context.ts'
import { resetServerClock } from '../clock.ts'
import { createAppRouter } from '../router.tsx'
import { fakeResponse, ME, mockFetch } from '../test-utils.ts'
import { eventDetail, summary } from './test-fixtures.ts'

let list: EventList

function serve() {
  return mockFetch((url) => {
    if (url === '/api/events') return fakeResponse(200, list)
    if (url === '/api/events/100') return fakeResponse(200, eventDetail())
    return fakeResponse(304)
  })
}

function renderAt(path = '/events', me: Me = ME) {
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

beforeEach(() => {
  vi.useFakeTimers()
  vi.setSystemTime(Date.parse('2026-11-20T12:00:00Z'))
  resetServerClock()
})
afterEach(() => {
  vi.useRealTimers()
})

describe('Events tab', () => {
  it('lists running, upcoming and finished events in the server order', async () => {
    list = {
      events: [
        summary({ live: 2 }),
        summary({ id: 101, name: 'Next Cup', status: 'upcoming', tier: 'unknown', picks: 1 }),
        summary({ id: 102, name: 'Old Major', status: 'finished', matches: 1 }),
        summary({ id: 103, name: 'Gone Open', status: 'canceled', begin_at: null, end_at: null }),
      ],
    }
    serve()
    renderAt()
    await pass()

    expect(screen.getAllByRole('heading', { level: 2 }).map((h) => h.textContent)).toEqual([
      'Running',
      'Upcoming',
      'Finished',
    ])
    const cards = screen
      .getAllByRole('link')
      .filter((a) => a.getAttribute('href')?.startsWith('/events/'))
    expect(cards.map((a) => a.getAttribute('href'))).toEqual([
      '/events/100',
      '/events/101',
      '/events/102',
      '/events/103',
    ])
    expect(cards[0]).toHaveTextContent('Fixture Masters 2026')
    expect(cards[0]).toHaveTextContent(/S-tier · (20 Nov – 23 Nov|Nov 20 – Nov 23)/)
    expect(cards[0]).toHaveTextContent('2 live')
    expect(cards[0]).toHaveTextContent('12 matches')
    expect(cards[0]).toHaveTextContent('3 picks')
    expect(cards[1]).not.toHaveTextContent('tier')
    expect(cards[1]).toHaveTextContent('1 pick')
    expect(cards[1]).not.toHaveTextContent('live')
    expect(cards[2]).toHaveTextContent('1 match')
    expect(cards[3]).toHaveTextContent('Canceled')

    fireEvent.click(cards[0]!)
    await pass()
    expect(
      screen.getByRole('heading', { level: 1, name: 'Fixture Masters 2026' }),
    ).toBeInTheDocument()
    const nav = screen.getByRole('navigation', { name: 'Sections' })
    const tab = within(nav).getByRole('link', { name: 'Events' })
    expect(tab).toHaveAttribute('aria-current', 'page')
  })

  it('points admins to /event follow when nothing is followed', async () => {
    list = { events: [] }
    serve()
    renderAt('/events', { ...ME, is_admin: true })
    await pass()
    expect(screen.getByRole('heading', { name: 'No events followed yet' })).toBeInTheDocument()
    expect(screen.getByText(/Use \/event follow in Discord/)).toBeInTheDocument()
  })
})
