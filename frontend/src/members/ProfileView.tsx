import { type ReactNode, useState } from 'react'
import { Link, useLocation, useSearchParams } from 'react-router'
import {
  type Profile,
  type ProfilePick,
  type ProfilePicks,
  type ProfileSegment,
  profilePath,
  profilePicksPath,
} from '../api.ts'
import { useAuth } from '../auth-context.ts'
import { useServerNow } from '../clock.ts'
import { pointsText } from '../leaderboard/format.ts'
import { formatDateTime, formatLabel, teamName } from '../matches/format.ts'
import { StatusTag } from '../matches/MatchRow.tsx'
import { partOutcomeLabel, pointsLabel } from '../matches/summary.ts'
import { TeamLogo } from '../matches/TeamLogo.tsx'
import type { BackTo } from '../nav.ts'
import { POLL_IDLE_MS, POLL_LIVE_MS, usePoll } from '../poll.ts'
import { Avatar } from '../shell/Avatar.tsx'
import { EmptyState, ErrorState, Loading } from '../shell/states.tsx'
import {
  accuracyText,
  eventOf,
  isVoid,
  rankText,
  SEGMENTS,
  scoredParts,
  segmentOf,
} from './format.ts'

/** Who is shown before the profile has loaded. */
export interface ProfileFallback {
  name: string
  avatarUrl: string
}

/**
 * A member's profile (T15): rank, points, picks and accuracy per part, then
 * their Open, Live and Settled picks, all time or for one event. The same
 * numbers as the leaderboard for the same filter. Used for the Me tab and
 * for every other member; picks are public.
 *
 * Polls every 60 s, every 20 s while a picked match is live.
 */
export function ProfileView({
  userId,
  fallback,
  children,
}: {
  userId: string
  /** Name and avatar to show while loading. */
  fallback?: ProfileFallback
  /** Extra lines under the name (handle, badges). */
  children?: ReactNode
}) {
  const [params, setParams] = useSearchParams()
  const location = useLocation()
  const eventId = eventOf(params.get('event'))
  const [fast, setFast] = useState(false)
  const interval = fast ? POLL_LIVE_MS : POLL_IDLE_MS
  const poll = usePoll<Profile>(profilePath(userId, eventId), interval)
  const live = poll.data?.live ?? false
  if (live !== fast) setFast(live)
  const p = poll.data

  const name = p?.display_name ?? fallback?.name
  const avatar = p?.avatar_url ?? fallback?.avatarUrl

  function setParam(key: string, value: string | null) {
    const next = new URLSearchParams(params)
    if (value === null) next.delete(key)
    else next.set(key, value)
    // Keep the router state: it holds the back link and the fallback.
    setParams(next, { replace: true, state: location.state as unknown })
  }

  return (
    <>
      {name && (
        <section className="profile">
          {avatar && <Avatar src={avatar} name={name} size={64} />}
          <div>
            <h1 className="profile-name">{name}</h1>
            {children}
          </div>
        </section>
      )}
      {poll.loading ? (
        <Loading label="Loading profile…" />
      ) : !p ? (
        <ErrorState
          title="Profile could not be loaded"
          message="Check your connection and try again."
          onRetry={poll.refresh}
        />
      ) : (
        <>
          <Stats profile={p} />
          <div className="board-controls">
            <label className="board-filter">
              <span>Show</span>
              <select
                value={eventId === null ? '' : String(eventId)}
                onChange={(e) => setParam('event', e.target.value || null)}
              >
                <option value="">All time</option>
                {p.events.map((ev) => (
                  <option key={ev.id} value={String(ev.id)}>
                    {ev.name}
                  </option>
                ))}
                {eventId !== null && !p.events.some((ev) => ev.id === eventId) && (
                  <option value={String(eventId)}>Unfollowed event</option>
                )}
              </select>
            </label>
          </div>
          <Lists
            profile={p}
            eventId={eventId}
            segment={segmentOf(params.get('list'), p.picks)}
            onSelect={(s) => setParam('list', s)}
            interval={interval}
          />
        </>
      )}
    </>
  )
}

function Stats({ profile: p }: { profile: Profile }) {
  const tiles: { key: keyof Profile['accuracy']; label: string }[] = [
    { key: 'winner', label: 'Winner' },
    { key: 'series', label: 'Exact series' },
    { key: 'map_winner', label: 'Map winner' },
    { key: 'map_score', label: 'Exact map' },
  ]
  return (
    <>
      <dl className="profile-stats">
        <div className="stat">
          <dt>Rank</dt>
          <dd className="stat-value">{rankText(p.rank)}</dd>
        </div>
        <div className="stat">
          <dt>Points</dt>
          <dd className="stat-value">
            {pointsText(p.points)}
            {p.provisional_picks > 0 && (
              <span className="tag tag-live" title="Points from live matches can still change">
                +{p.provisional_points} live
              </span>
            )}
          </dd>
        </div>
        <div className="stat">
          <dt>Picks</dt>
          <dd className="stat-value">
            {p.picks.total}
            <span className="stat-sub">{p.settled_picks} settled</span>
          </dd>
        </div>
      </dl>
      <section aria-label="Accuracy">
        <dl className="accuracy-grid">
          {tiles.map((t) => {
            const a = accuracyText(p.accuracy[t.key])
            return (
              <div key={t.key} className="stat">
                <dt>{t.label}</dt>
                <dd className="stat-value">
                  {a.pct}
                  <span className="stat-sub">{a.ratio}</span>
                </dd>
              </div>
            )
          })}
        </dl>
      </section>
    </>
  )
}

function Lists({
  profile: p,
  eventId,
  segment,
  onSelect,
  interval,
}: {
  profile: Profile
  eventId: number | null
  segment: ProfileSegment
  onSelect: (s: ProfileSegment) => void
  interval: number
}) {
  return (
    <>
      <div className="segments" role="tablist" aria-label="Picks">
        {SEGMENTS.map((s) => (
          <button
            key={s.key}
            type="button"
            role="tab"
            aria-selected={s.key === segment}
            className="segment"
            onClick={() => onSelect(s.key)}
          >
            {s.label} <span className="segment-count">{p.picks[s.key]}</span>
          </button>
        ))}
      </div>
      {/* Keyed, so a new list or filter starts with its own polling and pages. */}
      <PickList
        key={`${segment}:${eventId ?? ''}`}
        userId={p.user_id}
        isMe={p.is_me}
        segment={segment}
        eventId={eventId}
        interval={interval}
      />
    </>
  )
}

const EMPTY: Record<ProfileSegment, { title: string; mine: string; theirs: string }> = {
  open: {
    title: 'No open picks',
    mine: 'Pick an upcoming match on the Matches tab.',
    theirs: 'No picks on matches that haven’t started.',
  },
  live: {
    title: 'No live picks',
    mine: 'None of your picked matches is being played.',
    theirs: 'None of their picked matches is being played.',
  },
  settled: {
    title: 'No settled picks',
    mine: 'Your points show up here once your picked matches are over.',
    theirs: 'Their points show up here once their picked matches are over.',
  },
}

function PickList({
  userId,
  isMe,
  segment,
  eventId,
  interval,
}: {
  userId: string
  isMe: boolean
  segment: ProfileSegment
  eventId: number | null
  interval: number
}) {
  const { api } = useAuth()
  const location = useLocation()
  const poll = usePoll<ProfilePicks>(profilePicksPath(userId, segment, { eventId }), interval)
  const now = useServerNow(15_000)
  const [more, setMore] = useState<{ picks: ProfilePick[]; cursor: string | null } | null>(null)
  const [loadingMore, setLoadingMore] = useState(false)
  const [moreError, setMoreError] = useState(false)

  if (poll.loading) return <Loading label="Loading picks…" />
  if (!poll.data) {
    return (
      <ErrorState
        title="Picks could not be loaded"
        message="Check your connection and try again."
        onRetry={poll.refresh}
      />
    )
  }
  const list = poll.data
  let picks = more ? [...list.picks, ...more.picks] : list.picks
  // A pick already on the first page wins over its copy from a later page.
  picks = picks.filter((x, i) => picks.findIndex((y) => y.match.id === x.match.id) === i)
  const cursor = more ? more.cursor : list.next_cursor

  async function loadMore() {
    if (!cursor) return
    setLoadingMore(true)
    setMoreError(false)
    try {
      const res = await api.get<ProfilePicks>(
        profilePicksPath(userId, segment, { eventId, cursor }),
      )
      if (!res.notModified) {
        const page = res.data
        setMore((m) => ({ picks: [...(m?.picks ?? []), ...page.picks], cursor: page.next_cursor }))
      }
    } catch {
      setMoreError(true)
    } finally {
      setLoadingMore(false)
    }
  }

  if (picks.length === 0) {
    const e = EMPTY[segment]
    return <EmptyState title={e.title}>{isMe ? e.mine : e.theirs}</EmptyState>
  }
  const segmentLabel = SEGMENTS.find((s) => s.key === segment)?.label
  // The match detail leads back to this list, with its filter.
  const back: BackTo = {
    to: `${location.pathname}${location.search}`,
    label: isMe ? 'My picks' : 'Profile',
  }
  return (
    <>
      <ul className="history" aria-label={`${segmentLabel} picks`}>
        {picks.map((p) => (
          <HistoryItem key={p.match.id} item={p} isMe={isMe} now={now} back={back} />
        ))}
      </ul>
      {cursor && (
        <div className="more">
          <button
            type="button"
            className="button button-quiet"
            onClick={() => void loadMore()}
            disabled={loadingMore}
          >
            {loadingMore ? 'Loading…' : 'Load more'}
          </button>
          {moreError && <p className="form-error">Could not load more picks.</p>}
        </div>
      )}
    </>
  )
}

/**
 * The pick's tag: points, void, or "Edit pick" on my own pick while it can
 * change. A locked match says so in its status tag.
 */
function ItemTag({ item, isMe }: { item: ProfilePick; isMe: boolean }) {
  if (isVoid(item)) return <span className="tag tag-muted">Void</span>
  if (item.result) {
    return (
      <span className="tag tag-points">{pointsLabel(item.result.points, item.result.state)}</span>
    )
  }
  if (isMe && item.match.pickable) return <span className="tag tag-pick-full">Edit pick</span>
  return null
}

function HistoryItem({
  item,
  isMe,
  now,
  back,
}: {
  item: ProfilePick
  isMe: boolean
  now: number
  back: BackTo
}) {
  const m = item.match
  const scored = item.result !== null && !isVoid(item)
  const parts = scoredParts(item.pick, item.result?.breakdown ?? null, m.team_a, m.team_b)
  const showScore = m.status === 'live' || m.status === 'finished'
  const when = m.started_at ?? m.scheduled_at
  return (
    <li className={`history-item${isVoid(item) ? ' is-void' : ''}`}>
      <Link to={`/matches/${m.id}`} state={{ back }} className="history-link">
        <span className="history-teams">
          <TeamLogo team={m.team_a} size={20} />
          <span className="history-team">{teamName(m.team_a)}</span>
          <span className="history-score">
            {showScore ? `${m.maps_won_a}:${m.maps_won_b}` : 'vs'}
          </span>
          <span className="history-team">{teamName(m.team_b)}</span>
          <TeamLogo team={m.team_b} size={20} />
        </span>
        <span className="history-meta">
          <span className="muted">
            {m.event.name} · {formatLabel(m.format)}
            {when && ` · ${formatDateTime(when)}`}
          </span>
          <span className="row-tags">
            {!isVoid(item) && <StatusTag row={m} now={now} />}
            <ItemTag item={item} isMe={isMe} />
          </span>
        </span>
      </Link>
      <ul className="pick-parts history-parts" aria-label="Pick">
        {parts.map((part) => (
          <li key={part.key} className="part">
            <span className="muted">{part.label}</span> {part.value}
            {scored && part.score && (
              <span className={`outcome outcome-${part.score.outcome}`}>
                {partOutcomeLabel(part.score)}
              </span>
            )}
          </li>
        ))}
      </ul>
    </li>
  )
}
