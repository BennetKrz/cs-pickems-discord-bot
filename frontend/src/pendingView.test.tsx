import { render, waitFor } from '@testing-library/react'
import { StrictMode } from 'react'
import { RouterProvider } from 'react-router'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { ApiClient } from './api.ts'
import { AuthContext } from './auth-context.ts'
import { PENDING_VIEW_PATH, PendingViewOpener } from './pendingView.tsx'
import { createAppRouter } from './router.tsx'
import { fakeResponse, header, ME, mockFetch } from './test-utils.ts'

/** Renders the app as `App` does, signed in, and records API calls. */
function renderApp(pending: Response | (() => Response)) {
  const calls: string[] = []
  const fetchMock = mockFetch((url, init) => {
    calls.push(url)
    if (url === PENDING_VIEW_PATH) {
      expect(header(init, 'Authorization')).toBe('Bearer token')
      return typeof pending === 'function' ? pending() : pending
    }
    if (url.startsWith('/api/matches?')) {
      return fakeResponse(200, {
        state: 'upcoming',
        following_any: true,
        matches: [],
        next_cursor: null,
      })
    }
    return fakeResponse(304)
  })
  const router = createAppRouter()
  const api = new ApiClient('token', () => {})
  render(
    <StrictMode>
      <AuthContext.Provider value={{ me: ME, api }}>
        <PendingViewOpener onOpen={(path) => void router.navigate(path)} />
        <RouterProvider router={router} />
      </AuthContext.Provider>
    </StrictMode>,
  )
  return { router, calls, fetchMock }
}

describe('PendingViewOpener', () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('opens the match a Pick now button asked for, asking only once', async () => {
    const { router, calls } = renderApp(fakeResponse(200, { match_id: 12 }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/matches/12'))
    expect(calls.filter((c) => c === PENDING_VIEW_PATH)).toHaveLength(1)
  })

  it('stays on Matches without a pending view', async () => {
    const { router, calls } = renderApp(fakeResponse(200, { match_id: null }))
    await waitFor(() => expect(calls).toContain(PENDING_VIEW_PATH))
    await Promise.resolve()
    expect(router.state.location.pathname).toBe('/matches')
  })

  it('ignores a failed request', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const { router } = renderApp(fakeResponse(500, { error: 'internal', message: 'db' }))
    await waitFor(() => expect(warn).toHaveBeenCalled())
    expect(router.state.location.pathname).toBe('/matches')
  })
})
