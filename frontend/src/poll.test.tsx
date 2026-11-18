import { act, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ApiClient } from './api.ts'
import { AuthContext } from './auth-context.ts'
import { POLL_IDLE_MS, POLL_LIVE_MS, usePoll } from './poll.ts'
import { fakeResponse, header, ME, mockFetch } from './test-utils.ts'

function Probe({ interval, initial }: { interval: number; initial?: { n: number } }) {
  const { data, error, loading } = usePoll<{ n: number }>('/api/thing', interval, initial)
  if (loading) return <p>loading</p>
  return <p>{`n=${data?.n ?? '-'}${error === undefined ? '' : ' (error)'}`}</p>
}

function renderProbe(interval: number, initial?: { n: number }) {
  const api = new ApiClient('token', () => {})
  return render(
    <AuthContext.Provider value={{ me: ME, api }}>
      <Probe interval={interval} initial={initial} />
    </AuthContext.Provider>,
  )
}

/** Answers fetches from a queue; the last answer repeats. */
function queue(...answers: Response[]) {
  return mockFetch(() => (answers.length > 1 ? answers.shift()! : answers[0]!))
}

let hidden = false

function setHidden(value: boolean) {
  hidden = value
  document.dispatchEvent(new Event('visibilitychange'))
}

/** Lets `ms` pass on the fake clock and settles the resulting fetches. */
async function pass(ms = 0) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms)
  })
}

describe('usePoll', () => {
  beforeEach(() => {
    hidden = false
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => hidden })
    vi.useFakeTimers()
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  it('revalidates with the ETag on every interval', async () => {
    const fetchSpy = queue(
      fakeResponse(200, { n: 1 }, { ETag: '"a"' }),
      fakeResponse(304),
      fakeResponse(200, { n: 2 }, { ETag: '"b"' }),
      fakeResponse(304),
    )
    renderProbe(POLL_LIVE_MS)
    await pass()
    expect(screen.getByText('n=1')).toBeInTheDocument()
    expect(fetchSpy).toHaveBeenCalledTimes(1)
    expect(header(fetchSpy.mock.calls[0]?.[1], 'If-None-Match')).toBeUndefined()
    expect(header(fetchSpy.mock.calls[0]?.[1], 'Authorization')).toBe('Bearer token')

    await pass(POLL_LIVE_MS - 1)
    expect(fetchSpy).toHaveBeenCalledTimes(1)
    await pass(1)
    expect(fetchSpy).toHaveBeenCalledTimes(2)
    expect(header(fetchSpy.mock.calls[1]?.[1], 'If-None-Match')).toBe('"a"')
    expect(screen.getByText('n=1')).toBeInTheDocument()

    await pass(POLL_LIVE_MS)
    expect(screen.getByText('n=2')).toBeInTheDocument()
    await pass(POLL_LIVE_MS)
    expect(header(fetchSpy.mock.calls[3]?.[1], 'If-None-Match')).toBe('"b"')
  })

  it('pauses while the document is hidden', async () => {
    const fetchSpy = queue(fakeResponse(200, { n: 1 }, { ETag: '"a"' }), fakeResponse(304))
    renderProbe(POLL_IDLE_MS)
    await pass()
    expect(fetchSpy).toHaveBeenCalledTimes(1)

    act(() => setHidden(true))
    await pass(5 * POLL_IDLE_MS)
    expect(fetchSpy).toHaveBeenCalledTimes(1)

    // Visible again: fetch at once, then on the interval.
    act(() => setHidden(false))
    await pass()
    expect(fetchSpy).toHaveBeenCalledTimes(2)
    await pass(POLL_IDLE_MS)
    expect(fetchSpy).toHaveBeenCalledTimes(3)
  })

  it('keeps the data when a poll fails', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    queue(
      fakeResponse(200, { n: 1 }, { ETag: '"a"' }),
      fakeResponse(503, { error: 'unavailable' }),
      fakeResponse(304),
    )
    renderProbe(POLL_IDLE_MS)
    await pass()
    await pass(POLL_IDLE_MS)
    expect(screen.getByText('n=1 (error)')).toBeInTheDocument()
    await pass(POLL_IDLE_MS)
    expect(screen.getByText('n=1')).toBeInTheDocument()
  })

  it('with initial data, waits one interval before the first fetch', async () => {
    const fetchSpy = queue(fakeResponse(200, { n: 2 }, { ETag: '"b"' }))
    renderProbe(POLL_IDLE_MS, { n: 1 })
    await pass()
    expect(screen.getByText('n=1')).toBeInTheDocument()
    expect(fetchSpy).not.toHaveBeenCalled()
    await pass(POLL_IDLE_MS)
    expect(screen.getByText('n=2')).toBeInTheDocument()
  })
})
