import { useEffect, useState, useSyncExternalStore } from 'react'

/**
 * The server's clock as seen from the browser (T12). Every API response
 * carries `X-Server-Time`; the offset to `Date.now()` lets countdowns and
 * the pick lock run on the server's time: the real clock in production
 * (correcting a skewed client), the fake clock in fixture mode. Without
 * the header the offset stays 0, so the client clock is used.
 */

export const SERVER_TIME_HEADER = 'X-Server-Time'

/** Offset changes smaller than this are ignored: network latency, not skew. */
const MIN_CHANGE_MS = 1_000

let offsetMs = 0
const listeners = new Set<() => void>()

/** Records the server time of a response received at `receivedAt`. */
export function noteServerTime(value: string | null, receivedAt = Date.now()): void {
  if (!value) return
  const server = Date.parse(value)
  if (Number.isNaN(server)) return
  const next = server - receivedAt
  if (Math.abs(next - offsetMs) < MIN_CHANGE_MS) return
  offsetMs = next
  for (const l of listeners) l()
}

/** "Now" on the server's clock, in ms since the epoch. */
export function serverNow(): number {
  return Date.now() + offsetMs
}

/** For tests: forget the offset. */
export function resetServerClock(): void {
  offsetMs = 0
  for (const l of listeners) l()
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

function getOffset(): number {
  return offsetMs
}

/** The server's "now", re-rendered every `intervalMs` and on offset changes. */
export function useServerNow(intervalMs: number): number {
  const offset = useSyncExternalStore(subscribe, getOffset)
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), intervalMs)
    return () => clearInterval(timer)
  }, [intervalMs])
  return now + offset
}

/** `setTimeout` can't wait longer than this (about 24.8 days). */
const MAX_TIMEOUT_MS = 2 ** 31 - 1

/**
 * Whether `lockAt` (RFC 3339) has passed on the server's clock. Flips at
 * the exact moment with a timer, so an open view locks without waiting for
 * a poll.
 */
export function useLockPassed(lockAt: string | null | undefined): boolean {
  const offset = useSyncExternalStore(subscribe, getOffset)
  const lockMs = lockAt ? Date.parse(lockAt) : Number.NaN
  // The client time of the last check; the timer moves it to the lock.
  const [checkedAt, setCheckedAt] = useState(() => Date.now())
  const passed = !Number.isNaN(lockMs) && checkedAt + offset >= lockMs

  useEffect(() => {
    if (Number.isNaN(lockMs) || passed) return
    // Also when the wait is already over (lock moved into the past): the
    // timer re-checks at once.
    const wait = Math.min(lockMs - (Date.now() + offset), MAX_TIMEOUT_MS)
    const timer = setTimeout(() => setCheckedAt(Date.now()), Math.max(0, wait))
    return () => clearTimeout(timer)
  }, [lockMs, offset, passed, checkedAt])

  return passed
}
