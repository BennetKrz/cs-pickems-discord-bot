/**
 * HTTP access to the backend. Inside Discord, relative paths go through
 * Discord's proxy to the backend; in `pnpm dev`, Vite proxies `/api`.
 */

import { noteServerTime, SERVER_TIME_HEADER } from './clock.ts'

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

// ---- picks (T10) ------------------------------------------------------------
// Scores are relative to the match's team A/B. Team IDs are internal IDs.

export interface Score {
  a: number
  b: number
}

export interface MapPickParts {
  /** 1…N for a BoN. */
  map_number: number
  winner_team_id: number
  /** Exact regulation score: 13:0 to 13:11 for either side. */
  rounds?: Score | null
  /** The winner wins in overtime; excludes `rounds`. */
  overtime?: boolean
}

/**
 * The three parts of a pick, every one optional. `PUT` replaces the whole
 * pick: a part left out is removed, and a pick without parts is deleted.
 */
export interface PickParts {
  winner_team_id?: number | null
  /** Bo3/Bo5 only; sets the winner. */
  series?: Score | null
  maps?: MapPickParts[]
}

/** A pick as the API returns it: every part present, maps sorted. */
export interface SavedPickParts {
  winner_team_id: number | null
  series: Score | null
  maps: Required<MapPickParts>[]
}

/** The caller's pick: answer of `PUT …/pick`, entries of `GET /api/me/picks`. */
export interface MyPick extends SavedPickParts {
  match_id: number
  created_at: string
  /** Time of the last change. */
  updated_at: string
  /** Saved more than once. */
  edited: boolean
}

/** One member's pick, as everyone sees it. */
export interface PickView extends SavedPickParts {
  user_id: string
  display_name: string
  avatar_url: string
  created_at: string
  updated_at: string
  edited: boolean
  /** The settled points (T11); `null` until settlement ran. */
  result: PickResult | null
}

// ---- points (T11) -----------------------------------------------------------

/** `provisional` while live, `final` once over, `void` for no points. */
export type ResultState = 'provisional' | 'final' | 'void'

export interface PartScore {
  outcome: 'hit' | 'miss' | 'pending' | 'void'
  points: number
}

/** Every scored part of a pick; a part the pick doesn't have is `null` or absent. */
export interface Breakdown {
  winner: PartScore | null
  series: PartScore | null
  maps: { map_number: number; winner: PartScore; score: PartScore | null }[]
}

export interface PickResult {
  points: number
  state: ResultState
  /** Parts still waiting for a result or for data. */
  pending_parts: number
  breakdown: Breakdown
}

/** Picks split between team A and team B; percentages add up to 100. */
export interface Split {
  a: number
  b: number
  pct_a: number
  pct_b: number
}

export interface PickStats {
  total: number
  winner: Split
  /** Every allowed series score of the format, also unpicked ones; empty for Bo1. */
  series: (Score & { count: number; pct: number })[]
  /** One entry per map number 1…N. */
  maps: (Split & { map_number: number })[]
}

/** Answer of `GET /api/matches/{id}/picks`. */
export interface MatchPicks {
  match_id: number
  /** The caller may create, edit or delete their pick now. */
  pickable: boolean
  /** Picks are read-only: hide the edit controls. */
  locked: boolean
  lock_at: string | null
  picks: PickView[]
  stats: PickStats
}

export interface MyPicks {
  picks: MyPick[]
}

/** Path of every pick of a match, for `usePoll`. */
export function matchPicksPath(matchId: number): string {
  return `/api/matches/${matchId}/picks`
}

/** Path of the caller's picks, for `usePoll`. */
export const MY_PICKS_PATH = '/api/me/picks'

// ---- matches (T12) ----------------------------------------------------------
// All times are RFC 3339 UTC; the UI shows them in the viewer's time zone.

export type MatchFormat = 'bo1' | 'bo3' | 'bo5'
export type MatchStatus = 'scheduled' | 'live' | 'finished' | 'canceled' | 'postponed'
export type MapStatus = 'upcoming' | 'live' | 'finished' | 'not_played'
/** A segment of the match list. */
export type MatchSegment = 'upcoming' | 'live' | 'finished'

export interface Named {
  id: number
  name: string
}

export interface TeamRef {
  id: number
  name: string
  acronym: string | null
  /** Relative URL through the image proxy; `null` without a logo. */
  logo_url: string | null
}

/** What a list row and the detail header share. */
export interface MatchHeader {
  id: number
  event: Named
  stage: Named | null
  /** `null` while TBD. */
  team_a: TeamRef | null
  team_b: TeamRef | null
  format: MatchFormat
  status: MatchStatus
  /** Voided by an admin: no points for anyone. */
  voided: boolean
  scheduled_at: string | null
  started_at: string | null
  ended_at: string | null
  lock_at: string | null
  locked: boolean
  pickable: boolean
  maps_won_a: number
  maps_won_b: number
  winner_team_id: number | null
  /** The map being played now. */
  current_map: {
    number: number
    map_name: string | null
    rounds_a: number | null
    rounds_b: number | null
  } | null
  /** Picks in this server. */
  pick_count: number
}

export interface MatchRow extends MatchHeader {
  my_pick: 'none' | 'partial' | 'full'
  my_result: { points: number; state: ResultState; pending_parts: number } | null
}

/** Answer of `GET /api/matches`. */
export interface MatchList {
  state: MatchSegment
  /** The server follows at least one event. */
  following_any: boolean
  matches: MatchRow[]
  /** For the next page; `null` on the last. */
  next_cursor: string | null
}

export interface MapRow {
  number: number
  /** `null` until the veto is known. */
  map_name: string | null
  status: MapStatus
  rounds_a: number | null
  rounds_b: number | null
  winner_team_id: number | null
}

/** Answer of `GET /api/matches/{id}`. */
export interface MatchDetail extends MatchHeader {
  maps: MapRow[]
}

/** Path of one segment of the match list. */
export function matchesPath(
  state: MatchSegment,
  opts: { eventId?: number; cursor?: string | null } = {},
): string {
  const q = new URLSearchParams({ state })
  if (opts.eventId !== undefined) q.set('event_id', String(opts.eventId))
  if (opts.cursor) q.set('cursor', opts.cursor)
  return `/api/matches?${q.toString()}`
}

/** Path of one match. */
export function matchPath(matchId: number): string {
  return `/api/matches/${matchId}`
}

// ---- leaderboard (T14) ------------------------------------------------------

/** One member on the leaderboard. */
export interface LeaderboardRow {
  /** 1-based; equal members share a rank and the next one skips (1, 1, 3). */
  rank: number
  /** The rank before the latest match day; `null` if the member had none. */
  previous_rank: number | null
  user_id: string
  display_name: string
  avatar_url: string
  /** All points, provisional ones included. */
  points: number
  /** The part of `points` from live matches. */
  provisional_points: number
  /** Picks on live matches; their points can still move. */
  provisional_picks: number
  /** Picks with a result that is not void. */
  settled_picks: number
  winner_hits: number
  winner_decided: number
  /** 0–1; `null` before any winner pick is decided. */
  winner_accuracy: number | null
  series_hits: number
  map_hits: number
}

/** Answer of `GET /api/leaderboard`. */
export interface Leaderboard {
  /** The event filter; `null` for all time. */
  event_id: number | null
  /** The events the server follows, for the filter. */
  events: Named[]
  /** The latest match day (`YYYY-MM-DD`, server time zone). */
  match_day: string | null
  /** Rank changes are meaningful: someone ranked before `match_day`. */
  has_previous: boolean
  /** A match is live: points can still move. */
  live: boolean
  rows: LeaderboardRow[]
}

/** Path of the leaderboard, all time or for one event. */
export function leaderboardPath(eventId: number | null = null): string {
  return eventId === null ? '/api/leaderboard' : `/api/leaderboard?event_id=${eventId}`
}

// ---- events (T13) -----------------------------------------------------------

export type EventStatus = 'upcoming' | 'running' | 'finished' | 'canceled'
export type EventTier = 's' | 'a' | 'b' | 'c' | 'd' | 'unknown'
export type StageKind = 'swiss' | 'groups' | 'single_elim' | 'double_elim' | 'other'
export type BracketSide = 'upper' | 'lower' | 'final'
export type FeedOutcome = 'winner' | 'loser'

export interface EventInfo {
  id: number
  name: string
  tier: EventTier
  status: EventStatus
  begin_at: string | null
  end_at: string | null
}

/** One followed event in `GET /api/events`. */
export interface EventSummary extends EventInfo {
  /** Matches of any state. */
  matches: number
  /** Matches being played now. */
  live: number
  /** Picks this server made on the event. */
  picks: number
}

/** Answer of `GET /api/events`: running first, then upcoming, then finished. */
export interface EventList {
  events: EventSummary[]
}

/** A team slot fed by the winner or loser of another match. */
export interface FeedRef {
  match_id: number
  outcome: FeedOutcome
}

/** Where a match sits in an elimination tree. */
export interface BracketPos {
  side: BracketSide
  /** Column within the side, from 0. */
  round: number
  /** Row within the column, from 0. */
  slot: number
  /** Derived from the feeds rather than stored by the source. */
  derived: boolean
  feeds_a: FeedRef | null
  feeds_b: FeedRef | null
}

/** A match of a stage: a list row plus its place in the stage. */
export interface StageMatch extends MatchRow {
  /** Set in a `bracket` stage. */
  bracket: BracketPos | null
  /** The round, from 1, in a `standings` stage. */
  round: number | null
}

export interface StandingRow {
  team: TeamRef
  wins: number
  losses: number
  map_diff: number
  round_diff: number
}

/** How a stage is shown. */
export type StageViewKind = 'bracket' | 'standings' | 'list'

export interface StageView {
  /** `null` for matches without a stage. */
  id: number | null
  name: string
  kind: StageKind | null
  view: StageViewKind
  /** Set in a `standings` stage, best first. */
  standings: StandingRow[] | null
  /** By start time (unscheduled last). */
  matches: StageMatch[]
}

/** Answer of `GET /api/events/{id}`. */
export interface EventDetail {
  event: EventInfo
  /** This server follows the event: its matches can be picked. */
  followed: boolean
  stages: StageView[]
}

export const EVENTS_PATH = '/api/events'

/** Path of one event. */
export function eventPath(eventId: number): string {
  return `/api/events/${eventId}`
}

/** Error codes of the pick API. */
export const PICK_LOCKED = 'pick_locked'
export const NOT_PICKABLE = 'not_pickable'
export const INVALID_PICK = 'invalid_pick'

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
    noteServerTime(res.headers.get(SERVER_TIME_HEADER))
    if (res.status === 304) return { notModified: true }
    if (!res.ok) {
      const err = await toApiError(res)
      if (res.status === 401) this.onUnauthorized()
      throw err
    }
    return { notModified: false, data: (await res.json()) as T, etag: res.headers.get('ETag') }
  }

  /** A write with the session; resolves to the JSON body, or `null` for 204. */
  private async send<T>(method: 'PUT' | 'DELETE', path: string, body?: unknown): Promise<T | null> {
    const headers: Record<string, string> = { Authorization: `Bearer ${this.token}` }
    const init: RequestInit = { method, headers, cache: 'no-store' }
    if (body !== undefined) {
      headers['Content-Type'] = 'application/json'
      init.body = JSON.stringify(body)
    }
    const res = await fetch(path, init)
    noteServerTime(res.headers.get(SERVER_TIME_HEADER))
    if (!res.ok) {
      const err = await toApiError(res)
      if (res.status === 401) this.onUnauthorized()
      throw err
    }
    return res.status === 204 ? null : ((await res.json()) as T)
  }

  /**
   * Creates or replaces the caller's pick. Resolves to the saved pick, or
   * `null` if `parts` was empty and the pick got deleted. Throws an
   * `ApiError` with `PICK_LOCKED`, `NOT_PICKABLE` or `INVALID_PICK`.
   */
  putPick(matchId: number, parts: PickParts): Promise<MyPick | null> {
    return this.send<MyPick>('PUT', `/api/matches/${matchId}/pick`, parts)
  }

  /** Deletes the caller's pick (no error if there is none). */
  async deletePick(matchId: number): Promise<void> {
    await this.send('DELETE', `/api/matches/${matchId}/pick`)
  }
}
