import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { fakeResponse, mockFetch, session } from './test-utils.ts'

/** What the fake SDK saw, in order. */
const sdk = vi.hoisted(() => ({
  guildId: '100000000000000001' as string | null,
  log: [] as string[],
}))

vi.mock('@discord/embedded-app-sdk', () => ({
  DiscordSDK: class {
    readonly guildId = sdk.guildId
    constructor(clientId: string) {
      sdk.log.push(`new ${clientId}`)
    }
    ready() {
      sdk.log.push('ready')
      return Promise.resolve()
    }
    commands = {
      authorize: (args: unknown) => {
        sdk.log.push(`authorize ${JSON.stringify(args)}`)
        return Promise.resolve({ code: 'the-code' })
      },
      authenticate: (args: { access_token: string }) => {
        sdk.log.push(`authenticate ${args.access_token}`)
        return Promise.resolve({})
      },
    }
  },
}))

/** A fresh copy of the module, so its SDK singleton starts empty. */
async function load() {
  vi.resetModules()
  return import('./discord.ts')
}

/** Backend answers; every request is logged into `sdk.log` too. */
function backend(token: Response = fakeResponse(200, session('a'.repeat(64)))) {
  return mockFetch((url, init) => {
    sdk.log.push(`${init?.method ?? 'GET'} ${url}${init?.body ? ` ${String(init.body)}` : ''}`)
    if (url === '/api/auth/config') return fakeResponse(200, { client_id: '42' })
    if (url === '/api/auth/token') return token
    if (url === '/api/dev/session') return fakeResponse(200, session('d'.repeat(64)))
    return fakeResponse(404, { error: 'not_found' })
  })
}

describe('signing in through Discord', () => {
  beforeEach(() => {
    sdk.guildId = '100000000000000001'
    sdk.log = []
  })
  afterEach(() => vi.unstubAllEnvs())

  it('authorizes, lets the backend verify the guild, then authenticates', async () => {
    backend()
    const { signInWithDiscord } = await load()
    const result = await signInWithDiscord()

    expect(result).toEqual({ kind: 'signed-in', session: session('a'.repeat(64)) })
    expect(sdk.log).toEqual([
      'GET /api/auth/config',
      'new 42',
      'ready',
      `authorize ${JSON.stringify({
        client_id: '42',
        response_type: 'code',
        state: '',
        prompt: 'none',
        scope: ['identify'],
      })}`,
      `POST /api/auth/token ${JSON.stringify({ code: 'the-code', guild_id: '100000000000000001' })}`,
      'authenticate at-1',
    ])
  })

  it('stops outside a server without asking for consent', async () => {
    sdk.guildId = null
    backend()
    const { signInWithDiscord } = await load()
    expect(await signInWithDiscord()).toEqual({ kind: 'outside' })
    expect(sdk.log.some((l) => l.startsWith('authorize') || l.includes('/api/auth/token'))).toBe(
      false,
    )
  })

  it('passes backend rejections on as ApiError', async () => {
    backend(fakeResponse(403, { error: 'not_a_member', message: 'nope' }))
    const { signInWithDiscord } = await load()
    // The same module graph as the fresh discord.ts.
    const { ApiError } = await import('./api.ts')
    const err = await signInWithDiscord().catch((e: unknown) => e)
    expect(err).toBeInstanceOf(ApiError)
    expect(err).toMatchObject({ status: 403, code: 'not_a_member' })
    expect(sdk.log.some((l) => l.startsWith('authenticate'))).toBe(false)
  })

  it('knows when it runs inside Discord', async () => {
    const { inDiscord } = await load()
    expect(inDiscord('?frame_id=abc&instance_id=1&platform=desktop')).toBe(true)
    expect(inDiscord('')).toBe(false)
    expect(inDiscord('?instance_id=1')).toBe(false)
  })

  it('uses the dev session in a plain browser under pnpm dev, once for concurrent calls', async () => {
    vi.stubEnv('VITE_DEV_USER_ID', '300000000000000001')
    const fetchSpy = backend()
    const { signIn } = await load()
    const [a, b] = await Promise.all([signIn(), signIn()])
    expect(a).toEqual({ kind: 'signed-in', session: session('d'.repeat(64)) })
    expect(b).toBe(a)
    expect(fetchSpy).toHaveBeenCalledTimes(1)
    expect(sdk.log).toEqual([
      `POST /api/dev/session ${JSON.stringify({ user_id: '300000000000000001' })}`,
    ])
  })
})
