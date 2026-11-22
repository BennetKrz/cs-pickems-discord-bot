import { type Ref, useRef, useState } from 'react'
import { Link, useOutletContext, useSearchParams } from 'react-router'
import { leaderboardPath, type Leaderboard, type LeaderboardRow } from '../api.ts'
import { type ProfileLink, profileLink } from '../members/link.ts'
import { POLL_IDLE_MS, POLL_LIVE_MS, usePoll } from '../poll.ts'
import { Avatar } from '../shell/Avatar.tsx'
import type { ShellContext } from '../shell/Shell.tsx'
import { EmptyState, ErrorState, Loading } from '../shell/states.tsx'
import {
  accuracyLabel,
  matchDayLabel,
  picksText,
  pointsText,
  rankChange,
} from './format.ts'

function eventOf(value: string | null): number | null {
  return value !== null && /^\d+$/.test(value) ? Number(value) : null
}

/** The Leaderboard tab's route, with its filter. */
function leaderboardRoute(eventId: number | null): string {
  return eventId === null ? '/leaderboard' : `/leaderboard?event=${eventId}`
}

/**
 * The Leaderboard tab (T14): every member with a settled pick, ranked by
 * points, then exact hits, then winner accuracy. Points from live matches
 * count and are marked. Filter: all time or one followed event. Polls every
 * 60 s, every 20 s while a match is live.
 */
export function LeaderboardView() {
  const { me } = useOutletContext<ShellContext>()
  const [params, setParams] = useSearchParams()
  const eventId = eventOf(params.get('event'))
  // Faster polling while points can move; adjusted from the latest answer.
  const [fast, setFast] = useState(false)
  const poll = usePoll<Leaderboard>(leaderboardPath(eventId), fast ? POLL_LIVE_MS : POLL_IDLE_MS)
  const live = poll.data?.live ?? false
  if (live !== fast) setFast(live)
  const myRow = useRef<HTMLAnchorElement>(null)

  if (poll.loading) return <Loading label="Loading leaderboard…" />
  if (!poll.data) {
    return (
      <ErrorState
        title="Leaderboard could not be loaded"
        message="Check your connection and try again."
        onRetry={poll.refresh}
      />
    )
  }
  const board = poll.data
  const mine = board.rows.find((r) => r.user_id === me.user_id)

  function jumpToMe() {
    const el = myRow.current
    if (!el) return
    el.scrollIntoView({ block: 'center', behavior: 'smooth' })
    el.focus({ preventScroll: true })
  }

  return (
    <>
      <div className="board-controls">
        <label className="board-filter">
          <span>Show</span>
          <select
            value={eventId === null ? '' : String(eventId)}
            onChange={(e) => setParams(e.target.value ? { event: e.target.value } : {})}
          >
            <option value="">All time</option>
            {board.events.map((ev) => (
              <option key={ev.id} value={String(ev.id)}>
                {ev.name}
              </option>
            ))}
            {eventId !== null && !board.events.some((ev) => ev.id === eventId) && (
              <option value={String(eventId)}>Unfollowed event</option>
            )}
          </select>
        </label>
        {mine && (
          <button type="button" className="button button-quiet" onClick={jumpToMe}>
            Jump to me
          </button>
        )}
      </div>
      {board.rows.length === 0 ? (
        <EmptyState title="No points yet">
          {eventId === null
            ? 'The leaderboard fills up once the first picked matches have finished.'
            : 'Nobody has a settled pick for this event yet.'}
        </EmptyState>
      ) : (
        <>
          {board.has_previous && board.match_day && (
            <p className="board-note">
              Arrows show changes on the last match day ({matchDayLabel(board.match_day)}).
            </p>
          )}
          <ol className="board" aria-label="Leaderboard">
            {board.rows.map((row) => (
              <BoardRow
                key={row.user_id}
                row={row}
                hasPrevious={board.has_previous}
                isMe={row.user_id === me.user_id}
                linkRef={row.user_id === me.user_id ? myRow : undefined}
                link={profileLink(
                  row,
                  me.user_id,
                  { to: leaderboardRoute(eventId), label: 'Leaderboard' },
                  eventId,
                )}
              />
            ))}
          </ol>
        </>
      )}
    </>
  )
}

function BoardRow({
  row,
  hasPrevious,
  isMe,
  linkRef,
  link,
}: {
  row: LeaderboardRow
  hasPrevious: boolean
  isMe: boolean
  linkRef?: Ref<HTMLAnchorElement>
  /** The member's profile, with the same event filter. */
  link: ProfileLink
}) {
  const change = rankChange(row, hasPrevious)
  return (
    <li>
      <Link
        ref={linkRef}
        to={link.to}
        state={link.state}
        className={`board-row${isMe ? ' is-me' : ''}`}
        aria-current={isMe ? 'true' : undefined}
      >
        <span className="board-rank">
          <span className="board-rank-number">{row.rank}</span>
          {change && (
            <span className={`rank-change rank-${change.kind}`} title={change.label}>
              <span aria-hidden="true">{change.short}</span>
              <span className="sr-only">{change.label}</span>
            </span>
          )}
        </span>
        <Avatar src={row.avatar_url} name={row.display_name} size={36} />
        <span className="board-who">
          <span className="board-name">
            {row.display_name}
            {isMe && <span className="sr-only"> (you)</span>}
          </span>
          <span className="board-stats">
            {picksText(row.settled_picks)} · Winner {accuracyLabel(row.winner_accuracy)} · Exact
            series {row.series_hits} · Exact maps {row.map_hits}
          </span>
        </span>
        <span className="board-points">
          <span className="board-points-total">{pointsText(row.points)}</span>
          {row.provisional_picks > 0 && (
            <span className="tag tag-live" title="Points from live matches can still change">
              +{row.provisional_points} live
            </span>
          )}
        </span>
      </Link>
    </li>
  )
}
