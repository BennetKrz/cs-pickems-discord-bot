import { useCallback, useEffect, useRef, useState } from 'react'
import { useAuth } from './auth-context.ts'

/** Poll interval of views that show live matches. */
export const POLL_LIVE_MS = 20_000
/** Poll interval of every other view. */
export const POLL_IDLE_MS = 60_000

export interface Poll<T> {
  /** The latest data; kept while a later poll fails. */
  data: T | undefined
  /** The error of the latest poll, if it failed. */
  error: unknown
  /** No data yet and no error yet. */
  loading: boolean
  /** Fetches now and restarts the interval. */
  refresh: () => void
}

interface PollState<T> {
  data: T | undefined
  error: unknown
}

/**
 * Polls `path` every `intervalMs` with ETag revalidation: an unchanged
 * resource costs a 304 without a body. Polling pauses while the document is
 * hidden and fetches at once when it is visible again.
 *
 * With `initial`, the first fetch waits one interval, e.g. for data that
 * came with the sign-in.
 */
export function usePoll<T>(path: string, intervalMs: number, initial?: T): Poll<T> {
  const { api } = useAuth()
  const [state, setState] = useState<PollState<T>>({ data: initial, error: undefined })
  const [generation, setGeneration] = useState(0)
  const etag = useRef<{ path: string; value: string | null }>({ path, value: null })
  const waitFirst = useRef(initial !== undefined)

  useEffect(() => {
    let active = true
    let ctrl: AbortController | undefined
    let timer: ReturnType<typeof setInterval> | undefined

    async function load() {
      ctrl?.abort()
      const mine = new AbortController()
      ctrl = mine
      const tag = etag.current.path === path ? etag.current.value : null
      try {
        const res = await api.get<T>(path, { etag: tag, signal: mine.signal })
        if (!active || mine.signal.aborted) return
        if (res.notModified) {
          setState((s) => (s.error === undefined ? s : { data: s.data, error: undefined }))
        } else {
          etag.current = { path, value: res.etag }
          setState({ data: res.data, error: undefined })
        }
      } catch (error) {
        if (!active || mine.signal.aborted) return
        setState((s) => ({ data: s.data, error }))
      }
    }

    function start() {
      timer ??= setInterval(() => void load(), intervalMs)
    }
    function stop() {
      clearInterval(timer)
      timer = undefined
      ctrl?.abort()
    }
    function onVisibility() {
      if (document.hidden) {
        stop()
      } else {
        void load()
        start()
      }
    }

    if (!document.hidden) {
      if (waitFirst.current) waitFirst.current = false
      else void load()
      start()
    }
    document.addEventListener('visibilitychange', onVisibility)
    return () => {
      active = false
      stop()
      document.removeEventListener('visibilitychange', onVisibility)
    }
  }, [api, path, intervalMs, generation])

  const refresh = useCallback(() => setGeneration((g) => g + 1), [])
  return {
    data: state.data,
    error: state.error,
    loading: state.data === undefined && state.error === undefined,
    refresh,
  }
}
