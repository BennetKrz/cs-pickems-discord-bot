import { fireEvent, render, screen } from '@testing-library/react'
import { useEffect, useState } from 'react'
import { describe, expect, it, vi } from 'vitest'
import { ApiError } from './api.ts'
import { AuthGate } from './auth.tsx'
import { useAuth } from './auth-context.ts'
import type { SignIn } from './discord.ts'
import { fakeResponse, header, mockFetch, ME, session } from './test-utils.ts'

const TOKEN_A = 'a'.repeat(64)
const TOKEN_B = 'b'.repeat(64)

function Hello() {
  const { me } = useAuth()
  return <p>hello {me.display_name}</p>
}

/** Calls the API once per client and shows the outcome. */
function ApiProbe() {
  const { api } = useAuth()
  const [text, setText] = useState('waiting')
  useEffect(() => {
    api.get('/api/probe').then(
      () => setText('ok'),
      (e: unknown) => setText(e instanceof ApiError ? e.code : 'error'),
    )
  }, [api])
  return <p>{text}</p>
}

const signedIn = (token: string): SignIn => ({ kind: 'signed-in', session: session(token) })

describe('AuthGate', () => {
  it('shows only "Open this from the server" outside a server', async () => {
    render(
      <AuthGate signIn={() => Promise.resolve({ kind: 'outside' })}>
        <Hello />
      </AuthGate>,
    )
    expect(await screen.findByText('Open this from the server')).toBeInTheDocument()
    expect(document.body.textContent).toBe('Open this from the server')
  })

  it('shows a loading state, then the app', async () => {
    render(
      <AuthGate signIn={() => Promise.resolve(signedIn(TOKEN_A))}>
        <Hello />
      </AuthGate>,
    )
    expect(screen.getByRole('status')).toHaveTextContent('Signing in…')
    expect(await screen.findByText(`hello ${ME.display_name}`)).toBeInTheDocument()
  })

  it('explains a failed sign-in and retries on request', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    const signIn = vi
      .fn<() => Promise<SignIn>>()
      .mockRejectedValueOnce(new ApiError(403, 'not_a_member', 'nope'))
      .mockResolvedValueOnce(signedIn(TOKEN_A))
    render(
      <AuthGate signIn={signIn}>
        <Hello />
      </AuthGate>,
    )
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'You are not a member of this server.',
    )
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    expect(await screen.findByText(`hello ${ME.display_name}`)).toBeInTheDocument()
    expect(signIn).toHaveBeenCalledTimes(2)
  })

  it('signs in again once when the session ends', async () => {
    const signIn = vi
      .fn<() => Promise<SignIn>>()
      .mockResolvedValueOnce(signedIn(TOKEN_A))
      .mockResolvedValueOnce(signedIn(TOKEN_B))
    const fetchSpy = mockFetch((_url, init) =>
      header(init, 'Authorization') === `Bearer ${TOKEN_B}`
        ? fakeResponse(200, {})
        : fakeResponse(401, { error: 'unauthenticated' }),
    )
    render(
      <AuthGate signIn={signIn}>
        <ApiProbe />
      </AuthGate>,
    )
    expect(await screen.findByText('ok')).toBeInTheDocument()
    expect(signIn).toHaveBeenCalledTimes(2)
    expect(fetchSpy).toHaveBeenCalledTimes(2)
  })

  it('gives up when the new session ends right away too', async () => {
    const signIn = vi.fn(() => Promise.resolve(signedIn(TOKEN_A)))
    mockFetch(() => fakeResponse(401, { error: 'unauthenticated' }))
    render(
      <AuthGate signIn={signIn}>
        <ApiProbe />
      </AuthGate>,
    )
    expect(await screen.findByRole('alert')).toHaveTextContent('Your sign-in did not go through.')
    expect(signIn).toHaveBeenCalledTimes(2)
  })
})
