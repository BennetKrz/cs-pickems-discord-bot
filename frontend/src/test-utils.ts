import { vi } from 'vitest'
import type { Me, SessionResponse } from './api.ts'

export const ME: Me = {
  user_id: '200000000000000004',
  guild_id: '100000000000000001',
  username: 'alice',
  display_name: 'ali',
  avatar_url: '/api/img/avatar/200000000000000004?v=abc123',
  is_admin: false,
}

export function session(token: string, me: Me = ME): SessionResponse {
  return { session_token: token, expires_at: '2026-11-27T09:00:00Z', access_token: 'at-1', me }
}

/**
 * A minimal fetch `Response`. Plain promises instead of body streams, so
 * tests with fake timers don't stall.
 */
export function fakeResponse(
  status: number,
  body?: unknown,
  headers: Record<string, string> = {},
): Response {
  return {
    status,
    ok: status >= 200 && status < 300,
    headers: new Headers(headers),
    json: () =>
      body === undefined ? Promise.reject(new SyntaxError('no body')) : Promise.resolve(body),
  } as unknown as Response
}

/** Answers every `fetch` with `handler(url, init)`. */
export function mockFetch(
  handler: (url: string, init: RequestInit | undefined) => Response,
) {
  return vi
    .spyOn(globalThis, 'fetch')
    .mockImplementation((input, init) => Promise.resolve(handler(String(input), init)))
}

/** A request header of a recorded `fetch` call. */
export function header(init: RequestInit | undefined, name: string): string | undefined {
  const headers = (init?.headers ?? {}) as Record<string, string>
  return headers[name]
}
