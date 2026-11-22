import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  ApiClient,
  ApiError,
  PICK_LOCKED,
  matchesPath,
  matchPath,
  matchPicksPath,
  type MyPick,
} from './api.ts'
import { fakeResponse, header, mockFetch } from './test-utils.ts'

const SAVED: MyPick = {
  match_id: 7,
  winner_team_id: 1,
  series: { a: 2, b: 1 },
  maps: [],
  created_at: '2026-11-20T09:00:00Z',
  updated_at: '2026-11-20T09:00:00Z',
  edited: false,
}

afterEach(() => {
  vi.restoreAllMocks()
})

describe('pick API', () => {
  it('PUTs the whole pick as JSON with the session', async () => {
    const fetch = mockFetch(() => fakeResponse(200, SAVED))
    const api = new ApiClient('tok', () => {})
    const saved = await api.putPick(7, { series: { a: 2, b: 1 } })
    expect(saved).toEqual(SAVED)
    const [url, init] = fetch.mock.calls[0]!
    expect(url).toBe('/api/matches/7/pick')
    expect(init?.method).toBe('PUT')
    expect(header(init, 'Authorization')).toBe('Bearer tok')
    expect(header(init, 'Content-Type')).toBe('application/json')
    expect(JSON.parse(init?.body as string)).toEqual({ series: { a: 2, b: 1 } })
  })

  it('resolves to null when an empty pick deleted it', async () => {
    mockFetch(() => fakeResponse(204))
    const api = new ApiClient('tok', () => {})
    await expect(api.putPick(7, {})).resolves.toBeNull()
  })

  it('DELETEs without a body', async () => {
    const fetch = mockFetch(() => fakeResponse(204))
    const api = new ApiClient('tok', () => {})
    await api.deletePick(7)
    const [url, init] = fetch.mock.calls[0]!
    expect(url).toBe('/api/matches/7/pick')
    expect(init?.method).toBe('DELETE')
    expect(init?.body).toBeUndefined()
  })

  it('surfaces pick_locked as an ApiError', async () => {
    mockFetch(() =>
      fakeResponse(409, { error: 'pick_locked', message: 'picks for this match are locked' }),
    )
    const api = new ApiClient('tok', () => {})
    const err = await api.putPick(7, { winner_team_id: 1 }).catch((e: unknown) => e)
    expect(err).toBeInstanceOf(ApiError)
    expect((err as ApiError).status).toBe(409)
    expect((err as ApiError).code).toBe(PICK_LOCKED)
  })

  it('signals a lost session', async () => {
    mockFetch(() => fakeResponse(401, { error: 'unauthenticated', message: 'sign in' }))
    const onUnauthorized = vi.fn()
    const api = new ApiClient('tok', onUnauthorized)
    await expect(api.deletePick(7)).rejects.toBeInstanceOf(ApiError)
    expect(onUnauthorized).toHaveBeenCalledOnce()
  })

  it('builds the poll path of a match', () => {
    expect(matchPicksPath(42)).toBe('/api/matches/42/picks')
  })
})

describe('match paths', () => {
  it('build the list and detail URLs', () => {
    expect(matchesPath('upcoming')).toBe('/api/matches?state=upcoming')
    expect(matchesPath('finished', { eventId: 3, cursor: '1700000000000_9' })).toBe(
      '/api/matches?state=finished&event_id=3&cursor=1700000000000_9',
    )
    expect(matchesPath('live', { cursor: null })).toBe('/api/matches?state=live')
    expect(matchPath(12)).toBe('/api/matches/12')
  })
})
