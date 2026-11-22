import { useState } from 'react'
import { useOutletContext, useSearchParams } from 'react-router'
import { matchesPath, type MatchList, type MatchRow as Row, type MatchSegment } from '../api.ts'
import { useAuth } from '../auth-context.ts'
import { useServerNow } from '../clock.ts'
import { POLL_IDLE_MS, POLL_LIVE_MS, usePoll } from '../poll.ts'
import type { ShellContext } from '../shell/Shell.tsx'
import { EmptyState, ErrorState, Loading } from '../shell/states.tsx'
import { groupByDayAndEvent } from './format.ts'
import { MatchRow } from './MatchRow.tsx'

const SEGMENTS: { key: MatchSegment; label: string }[] = [
  { key: 'upcoming', label: 'Upcoming' },
  { key: 'live', label: 'Live' },
  { key: 'finished', label: 'Results' },
]

function segmentOf(value: string | null): MatchSegment {
  return value === 'live' || value === 'finished' ? value : 'upcoming'
}

/**
 * The Matches tab (T12): Upcoming (next 14 days), Live and Results (last
 * 30 days, paged), grouped by local day and event. Upcoming and Live poll
 * every 20 s, so a match moves between them without a reload.
 */
export function MatchesView() {
  const [params, setParams] = useSearchParams()
  const segment = segmentOf(params.get('state'))
  const needsPick = segment === 'upcoming' && params.get('needs') === '1'

  const select = (next: MatchSegment) => setParams(next === 'upcoming' ? {} : { state: next })
  const toggleNeeds = () => setParams(needsPick ? {} : { needs: '1' })

  return (
    <>
      <div className="segments" role="tablist" aria-label="Matches">
        {SEGMENTS.map((s) => (
          <button
            key={s.key}
            type="button"
            role="tab"
            aria-selected={s.key === segment}
            className="segment"
            onClick={() => select(s.key)}
          >
            {s.label}
          </button>
        ))}
      </div>
      {segment === 'upcoming' && (
        <label className="filter">
          <input type="checkbox" checked={needsPick} onChange={toggleNeeds} />
          Needs my pick
        </label>
      )}
      {/* Keyed, so a new segment starts with its own polling and pages. */}
      <SegmentList key={segment} segment={segment} needsPick={needsPick} />
    </>
  )
}

function SegmentList({ segment, needsPick }: { segment: MatchSegment; needsPick: boolean }) {
  const { me } = useOutletContext<ShellContext>()
  const { api } = useAuth()
  const poll = usePoll<MatchList>(
    matchesPath(segment),
    segment === 'finished' ? POLL_IDLE_MS : POLL_LIVE_MS,
  )
  const now = useServerNow(15_000)
  // Later pages of Results, loaded on demand.
  const [more, setMore] = useState<{ rows: Row[]; cursor: string | null } | null>(null)
  const [loadingMore, setLoadingMore] = useState(false)
  const [moreError, setMoreError] = useState(false)

  if (poll.loading) return <Loading label="Loading matches…" />
  if (!poll.data) {
    return (
      <ErrorState
        title="Matches could not be loaded"
        message="Check your connection and try again."
        onRetry={poll.refresh}
      />
    )
  }
  const list = poll.data
  if (!list.following_any) {
    return (
      <EmptyState title="No events followed yet">
        {me.is_admin
          ? 'Use /event follow in Discord to choose the tournaments members can pick.'
          : 'Admins choose which tournaments are open for picks.'}
      </EmptyState>
    )
  }

  let rows = more ? [...list.matches, ...more.rows] : list.matches
  // A row already on the first page wins over its copy from a later page.
  rows = rows.filter((r, i) => rows.findIndex((x) => x.id === r.id) === i)
  if (needsPick) rows = rows.filter((r) => r.pickable && r.my_pick === 'none')
  const cursor = more ? more.cursor : list.next_cursor

  async function loadMore() {
    if (!cursor) return
    setLoadingMore(true)
    setMoreError(false)
    try {
      const res = await api.get<MatchList>(matchesPath(segment, { cursor }))
      if (!res.notModified) {
        const page = res.data
        setMore((m) => ({ rows: [...(m?.rows ?? []), ...page.matches], cursor: page.next_cursor }))
      }
    } catch {
      setMoreError(true)
    } finally {
      setLoadingMore(false)
    }
  }

  if (rows.length === 0) {
    return <SegmentEmpty segment={segment} needsPick={needsPick} />
  }
  return (
    <>
      {groupByDayAndEvent(rows, segment, now).map((day) => (
        <section key={day.key} className="day">
          <h2 className="day-label">{day.label}</h2>
          {day.events.map((ev) => (
            <div key={ev.eventId} className="event-group">
              <h3 className="event-label">{ev.eventName}</h3>
              <ul className="match-list">
                {ev.rows.map((r) => (
                  <MatchRow key={r.id} row={r} now={now} />
                ))}
              </ul>
            </div>
          ))}
        </section>
      ))}
      {segment === 'finished' && cursor && (
        <div className="more">
          <button
            type="button"
            className="button button-quiet"
            onClick={() => void loadMore()}
            disabled={loadingMore}
          >
            {loadingMore ? 'Loading…' : 'Load more'}
          </button>
          {moreError && <p className="form-error">Could not load more results.</p>}
        </div>
      )}
    </>
  )
}

function SegmentEmpty({ segment, needsPick }: { segment: MatchSegment; needsPick: boolean }) {
  if (needsPick) {
    return <EmptyState title="All caught up">You have picked every open match.</EmptyState>
  }
  switch (segment) {
    case 'upcoming':
      return (
        <EmptyState title="No upcoming matches">
          Nothing is scheduled for the next 14 days.
        </EmptyState>
      )
    case 'live':
      return (
        <EmptyState title="No live matches">Matches show up here while they are played.</EmptyState>
      )
    case 'finished':
      return (
        <EmptyState title="No results yet">
          Finished matches of the last 30 days show up here.
        </EmptyState>
      )
  }
}
