import { fireEvent, render, screen, within } from '@testing-library/react'
import { RouterProvider } from 'react-router'
import { beforeEach, describe, expect, it } from 'vitest'
import { ApiClient, type Me } from '../api.ts'
import { AuthContext } from '../auth-context.ts'
import { createAppRouter } from '../router.tsx'
import { fakeResponse, ME, mockFetch } from '../test-utils.ts'

function renderShell(me: Me = ME, path = '/') {
  const api = new ApiClient('token', () => {})
  return render(
    <AuthContext.Provider value={{ me, api }}>
      <RouterProvider router={createAppRouter(path)} />
    </AuthContext.Provider>,
  )
}

describe('Shell', () => {
  beforeEach(() => {
    mockFetch((url) => {
      if (url.startsWith('/api/matches?')) {
        return fakeResponse(200, {
          state: 'upcoming',
          following_any: true,
          matches: [],
          next_cursor: null,
        })
      }
      if (url === '/api/events') return fakeResponse(200, { events: [] })
      if (url.startsWith('/api/leaderboard')) {
        return fakeResponse(200, {
          event_id: null,
          events: [],
          match_day: null,
          has_previous: false,
          live: false,
          rows: [],
        })
      }
      return fakeResponse(304)
    })
  })

  it("shows the member's name and avatar", () => {
    const { container } = renderShell()
    const header = screen.getByRole('banner')
    expect(within(header).getByText('ali')).toBeInTheDocument()
    const avatar = header.querySelector('img.avatar')
    expect(avatar).toHaveAttribute('src', ME.avatar_url)
    expect(container.querySelectorAll('img[src^="http"]')).toHaveLength(0)
  })

  it('opens on Matches and switches tabs', async () => {
    renderShell()
    const nav = screen.getByRole('navigation', { name: 'Sections' })
    const tab = (name: string) => within(nav).getByRole('link', { name })

    expect(await screen.findByRole('heading', { name: 'No upcoming matches' })).toBeInTheDocument()
    expect(tab('Matches')).toHaveAttribute('aria-current', 'page')

    fireEvent.click(tab('Leaderboard'))
    expect(await screen.findByRole('heading', { name: 'No points yet' })).toBeInTheDocument()
    expect(tab('Leaderboard')).toHaveAttribute('aria-current', 'page')
    expect(tab('Matches')).not.toHaveAttribute('aria-current')

    fireEvent.click(tab('Events'))
    expect(await screen.findByRole('heading', { name: 'No events followed yet' })).toBeInTheDocument()
  })

  it('shows the profile on Me, with the admin badge only for admins', async () => {
    renderShell(ME, '/me')
    expect(await screen.findByRole('heading', { name: 'ali' })).toBeInTheDocument()
    expect(screen.getByText('@alice')).toBeInTheDocument()
    expect(screen.queryByText('Server admin')).toBeNull()
  })

  it('marks admins on Me', async () => {
    renderShell({ ...ME, is_admin: true }, '/me')
    expect(await screen.findByText('Server admin')).toBeInTheDocument()
  })

  it('falls back to an initial when the avatar fails to load', () => {
    renderShell()
    const header = screen.getByRole('banner')
    fireEvent.error(header.querySelector('img.avatar')!)
    expect(header.querySelector('img.avatar')).toBeNull()
    expect(within(header).getByText('A')).toBeInTheDocument()
  })

  it('sends unknown paths to Matches', async () => {
    renderShell(ME, '/nowhere')
    expect(await screen.findByRole('heading', { name: 'No upcoming matches' })).toBeInTheDocument()
  })
})
