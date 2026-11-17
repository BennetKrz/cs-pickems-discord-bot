import { DiscordSDK } from '@discord/embedded-app-sdk'
import { getPublic, postPublic, type SessionResponse } from './api.ts'

/** Outcome of signing in. */
export type SignIn =
  | { kind: 'signed-in'; session: SessionResponse }
  /** Not inside a Discord server: a DM, a group DM, or a plain browser. */
  | { kind: 'outside' }

/** Dev sign-in user when `VITE_DEV_USER_ID` is unset. */
export const DEFAULT_DEV_USER_ID = '200000000000000001'

/**
 * Whether the page runs inside Discord's Activity iframe. Discord adds the
 * `frame_id` query parameter; the SDK refuses to start without it.
 */
export function inDiscord(search: string = window.location.search): boolean {
  return new URLSearchParams(search).has('frame_id')
}

// One SDK per page: its handshake with the Discord client happens once.
let sdk: DiscordSDK | null = null

/**
 * Signs in through the Embedded App SDK: `ready`, then `authorize` (scope
 * `identify`) for a code, which the backend exchanges and verifies together
 * with the guild. The backend returns a session and the Discord access
 * token, which `authenticate` needs.
 */
export async function signInWithDiscord(): Promise<SignIn> {
  const { client_id } = await getPublic<{ client_id: string }>('/api/auth/config')
  sdk ??= new DiscordSDK(client_id)
  await sdk.ready()
  // The SDK reports no guild in DMs and group DMs.
  const guildId = sdk.guildId
  if (!guildId) return { kind: 'outside' }

  const { code } = await sdk.commands.authorize({
    client_id,
    response_type: 'code',
    state: '',
    prompt: 'none',
    scope: ['identify'],
  })
  // The guild ID is only a claim; the backend checks it.
  const session = await postPublic<SessionResponse>('/api/auth/token', {
    code,
    guild_id: guildId,
  })
  if (session.access_token) {
    await sdk.commands.authenticate({ access_token: session.access_token })
  }
  return { kind: 'signed-in', session }
}

/**
 * Dev builds only: signs in without Discord through `POST /api/dev/session`,
 * which exists only in backend dev builds (`cargo run --features dev`).
 */
export async function devSignIn(userId: string): Promise<SignIn> {
  const session = await postPublic<SessionResponse>('/api/dev/session', { user_id: userId })
  return { kind: 'signed-in', session }
}

let pending: Promise<SignIn> | null = null

/**
 * Signs in the way that fits where the page runs: through Discord inside
 * the Activity, through the dev session under `pnpm dev`, else not at all.
 * Concurrent calls share one attempt (React's StrictMode mounts twice).
 */
export function signIn(): Promise<SignIn> {
  pending ??= (async (): Promise<SignIn> => {
    if (inDiscord()) return signInWithDiscord()
    if (import.meta.env.DEV) {
      return devSignIn(import.meta.env.VITE_DEV_USER_ID || DEFAULT_DEV_USER_ID)
    }
    return { kind: 'outside' }
  })().finally(() => {
    pending = null
  })
  return pending
}
