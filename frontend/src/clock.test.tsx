import { act, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ApiClient } from './api.ts'
import {
  noteServerTime,
  resetServerClock,
  serverNow,
  useLockPassed,
  useServerNow,
} from './clock.ts'
import { fakeResponse, mockFetch } from './test-utils.ts'

const T = Date.parse('2026-11-20T17:00:00Z')

beforeEach(() => {
  vi.useFakeTimers()
  vi.setSystemTime(T)
  resetServerClock()
})
afterEach(() => {
  vi.useRealTimers()
  resetServerClock()
})

describe('server clock', () => {
  it('follows the X-Server-Time header of API responses', async () => {
    expect(serverNow()).toBe(T)
    mockFetch(() => fakeResponse(304, undefined, { 'X-Server-Time': '2026-11-20T19:00:00.000Z' }))
    await new ApiClient('t', () => {}).get('/api/matches?state=live', { etag: '"x"' })
    expect(serverNow()).toBe(Date.parse('2026-11-20T19:00:00Z'))
    vi.advanceTimersByTime(5_000)
    expect(serverNow()).toBe(Date.parse('2026-11-20T19:00:05Z'))
  })

  it('ignores missing, invalid and tiny changes', () => {
    noteServerTime(null)
    noteServerTime('not a time')
    noteServerTime('2026-11-20T17:00:00.400Z')
    expect(serverNow()).toBe(T)
  })

  it('useServerNow ticks and follows the offset', () => {
    const { result } = renderHook(() => useServerNow(1_000))
    expect(result.current).toBe(T)
    act(() => vi.advanceTimersByTime(1_000))
    expect(result.current).toBe(T + 1_000)
    act(() => noteServerTime('2026-11-20T18:00:01.000Z'))
    expect(result.current).toBe(Date.parse('2026-11-20T18:00:01Z'))
  })
})

describe('useLockPassed', () => {
  it('flips exactly at the lock without a poll', () => {
    const { result } = renderHook(() => useLockPassed('2026-11-20T17:00:10.000Z'))
    expect(result.current).toBe(false)
    act(() => vi.advanceTimersByTime(9_999))
    expect(result.current).toBe(false)
    act(() => vi.advanceTimersByTime(1))
    expect(result.current).toBe(true)
  })

  it('uses the server clock', () => {
    const { result } = renderHook(() => useLockPassed('2026-11-20T18:00:00.000Z'))
    expect(result.current).toBe(false)
    // The server is an hour ahead: the lock is 1 s away.
    act(() => noteServerTime('2026-11-20T17:59:59.000Z'))
    expect(result.current).toBe(false)
    act(() => vi.advanceTimersByTime(1_000))
    expect(result.current).toBe(true)
  })

  it('is true for a past lock and false without one', () => {
    expect(renderHook(() => useLockPassed('2026-11-20T16:00:00Z')).result.current).toBe(true)
    expect(renderHook(() => useLockPassed(null)).result.current).toBe(false)
  })

  it('follows a lock that moves', () => {
    const { result, rerender } = renderHook(({ at }) => useLockPassed(at), {
      initialProps: { at: '2026-11-20T18:00:00Z' },
    })
    act(() => vi.advanceTimersByTime(60_000))
    expect(result.current).toBe(false)
    // Moved into the past: the timer re-checks at once.
    rerender({ at: '2026-11-20T17:00:30Z' })
    act(() => vi.advanceTimersByTime(0))
    expect(result.current).toBe(true)
  })
})
