import { useEffect, useState } from 'react'
import { fetchHealth, type Health } from '../api.ts'

type State =
  | { kind: 'loading' }
  | { kind: 'ok'; health: Health }
  | { kind: 'error' }

/** Placeholder start screen until the Activity shell lands (T07). */
export function Home() {
  const [state, setState] = useState<State>({ kind: 'loading' })

  useEffect(() => {
    const ctrl = new AbortController()
    fetchHealth(ctrl.signal)
      .then((health) => setState({ kind: 'ok', health }))
      .catch((err: unknown) => {
        if (!ctrl.signal.aborted) {
          console.warn(err)
          setState({ kind: 'error' })
        }
      })
    return () => ctrl.abort()
  }, [])

  return (
    <main className="home">
      <h1>CS2 Pick&rsquo;em</h1>
      <p>Pick match winners, series scores and map scores. Coming soon.</p>
      <p className="status" role="status">
        {state.kind === 'loading' && 'Connecting to the server…'}
        {state.kind === 'error' && 'Server unreachable.'}
        {state.kind === 'ok' &&
          `Server v${state.health.version} (${state.health.git_sha}), database ${state.health.db}`}
      </p>
    </main>
  )
}
