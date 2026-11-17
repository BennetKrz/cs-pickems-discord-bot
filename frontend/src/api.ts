/**
 * HTTP access to the backend. Inside Discord, relative paths go through
 * Discord's proxy to the backend; in `pnpm dev`, Vite proxies `/api`.
 */

/** The signed-in member, as `GET /api/me` returns it. */
export interface Me {
  user_id: string
  guild_id: string
  username: string
  /** Server nickname, else global display name, else username. */
  display_name: string
  /** Relative URL through the backend's image proxy. */
  avatar_url: string
  is_admin: boolean
}

/** Answer of `POST /api/auth/token` and `POST /api/dev/session`. */
export interface SessionResponse {
  session_token: string
  expires_at: string
  /** Discord access token for `commands.authenticate`; absent for dev sessions. */
  access_token?: string
  me: Me
}

/** A failed API call: HTTP status plus the `error` code of the JSON body. */
export class ApiError extends Error {
  readonly status: number
  readonly code: string

  constructor(status: number, code: string, message: string) {
    super(message)
    this.name = 'ApiError'
    this.status = status
    this.code = code
  }
}

async function toApiError(res: Response): Promise<ApiError> {
  let code = 'http_error'
  let message = `HTTP ${res.status}`
  try {
    const body = (await res.json()) as { error?: unknown; message?: unknown }
    if (typeof body.error === 'string') code = body.error
    if (typeof body.message === 'string') message = body.message
  } catch {
    // Not JSON: keep the generic code.
  }
  return new ApiError(res.status, code, message)
}

/** GET without a session, e.g. `/api/auth/config`. */
export async function getPublic<T>(path: string, signal?: AbortSignal): Promise<T> {
  const res = await fetch(path, { signal, cache: 'no-store' })
  if (!res.ok) throw await toApiError(res)
  return (await res.json()) as T
}

/** POST JSON without a session, for signing in. */
export async function postPublic<T>(path: string, body: unknown): Promise<T> {
  const res = await fetch(path, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
    cache: 'no-store',
  })
  if (!res.ok) throw await toApiError(res)
  return (await res.json()) as T
}

/** Result of a conditional GET. */
export type Fetched<T> =
  | { notModified: true }
  | { notModified: false; data: T; etag: string | null }

/**
 * Calls the API with the session. The token travels as
 * `Authorization: Bearer`, because the Activity runs in a third-party iframe
 * where cookies are unreliable. A 401 means the session ended;
 * `onUnauthorized` lets the app sign in again.
 */
export class ApiClient {
  private readonly token: string
  private readonly onUnauthorized: () => void

  constructor(token: string, onUnauthorized: () => void) {
    this.token = token
    this.onUnauthorized = onUnauthorized
  }

  /**
   * GET with ETag revalidation: pass the last `etag` to get `notModified`
   * instead of the same body again.
   */
  async get<T>(
    path: string,
    opts: { etag?: string | null; signal?: AbortSignal } = {},
  ): Promise<Fetched<T>> {
    const headers: Record<string, string> = { Authorization: `Bearer ${this.token}` }
    if (opts.etag) headers['If-None-Match'] = opts.etag
    // `no-store` keeps the browser cache out: the app does the revalidation.
    const res = await fetch(path, { headers, signal: opts.signal, cache: 'no-store' })
    if (res.status === 304) return { notModified: true }
    if (!res.ok) {
      const err = await toApiError(res)
      if (res.status === 401) this.onUnauthorized()
      throw err
    }
    return { notModified: false, data: (await res.json()) as T, etag: res.headers.get('ETag') }
  }
}
