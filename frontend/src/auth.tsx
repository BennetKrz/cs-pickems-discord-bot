import { useEffect, useRef, useState, type ReactNode } from 'react'
import { ApiClient, ApiError, type Me } from './api.ts'
import { AuthContext } from './auth-context.ts'
import { signIn as defaultSignIn, type SignIn } from './discord.ts'
import { ErrorState, Loading } from './shell/states.tsx'

type State =
  | { status: 'loading' }
  | { status: 'outside' }
  | { status: 'error'; error: unknown }
  | { status: 'signed-in'; me: Me; api: ApiClient }

/** A second 401 within this time after an automatic re-sign-in is shown as an error. */
const REAUTH_COOLDOWN_MS = 60_000

/** User-facing text for a failed sign-in. */
function signInMessage(error: unknown): string {
  if (error instanceof ApiError) {
    switch (error.code) {
      case 'not_a_member':
        return 'You are not a member of this server.'
      case 'wrong_guild':
        return "This Pick'em belongs to another server."
      case 'discord_unavailable':
        return 'Discord cannot be reached right now.'
      case 'invalid_code':
      case 'unauthenticated':
        return 'Your sign-in did not go through.'
    }
  }
  return 'Something went wrong while signing in.'
}

/**
 * Signs the member in and renders `children` only once that worked. Inside
 * a DM, or outside Discord in production, it shows only "Open this from the
 * server". When the session ends (401), it signs in again in the background;
 * a second 401 right after shows the error state.
 */
export function AuthGate({
  children,
  signIn = defaultSignIn,
}: {
  children: ReactNode
  signIn?: () => Promise<SignIn>
}) {
  const [state, setState] = useState<State>({ status: 'loading' })
  const [attempt, setAttempt] = useState(0)
  const lastReauth = useRef(Number.NEGATIVE_INFINITY)

  useEffect(() => {
    let current = true
    signIn().then(
      (result) => {
        if (!current) return
        if (result.kind === 'outside') {
          setState({ status: 'outside' })
          return
        }
        const api = new ApiClient(result.session.session_token, () => {
          if (!current) return
          const now = Date.now()
          if (now - lastReauth.current < REAUTH_COOLDOWN_MS) {
            setState({
              status: 'error',
              error: new ApiError(401, 'unauthenticated', 'the session ended again'),
            })
            return
          }
          lastReauth.current = now
          setAttempt((n) => n + 1)
        })
        setState({ status: 'signed-in', me: result.session.me, api })
      },
      (error: unknown) => {
        if (!current) return
        console.warn('sign-in failed', error)
        setState({ status: 'error', error })
      },
    )
    return () => {
      current = false
    }
  }, [signIn, attempt])

  switch (state.status) {
    case 'loading':
      return (
        <main className="center">
          <Loading label="Signing in…" />
        </main>
      )
    case 'outside':
      return (
        <main className="center">
          <p className="outside">Open this from the server</p>
        </main>
      )
    case 'error':
      return (
        <main className="center">
          <ErrorState
            title="Could not sign in"
            message={signInMessage(state.error)}
            onRetry={() => {
              setState({ status: 'loading' })
              setAttempt((n) => n + 1)
            }}
          />
        </main>
      )
    case 'signed-in':
      return (
        <AuthContext.Provider value={{ me: state.me, api: state.api }}>
          {children}
        </AuthContext.Provider>
      )
  }
}
