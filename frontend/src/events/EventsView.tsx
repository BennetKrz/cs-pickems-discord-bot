import { Link, useOutletContext } from 'react-router'
import { EVENTS_PATH, type EventList, type EventSummary } from '../api.ts'
import { POLL_IDLE_MS, usePoll } from '../poll.ts'
import type { ShellContext } from '../shell/Shell.tsx'
import { EmptyState, ErrorState, Loading } from '../shell/states.tsx'
import { formatDateRange, groupEvents, tierLabel } from './format.ts'

/**
 * The Events tab (T13): the events this server follows, running first,
 * then upcoming, then finished. Each opens the event with its stages.
 */
export function EventsView() {
  const { me } = useOutletContext<ShellContext>()
  const poll = usePoll<EventList>(EVENTS_PATH, POLL_IDLE_MS)

  if (poll.loading) return <Loading label="Loading events…" />
  if (!poll.data) {
    return (
      <ErrorState
        title="Events could not be loaded"
        message="Check your connection and try again."
        onRetry={poll.refresh}
      />
    )
  }
  if (poll.data.events.length === 0) {
    return (
      <EmptyState title="No events followed yet">
        {me.is_admin
          ? 'Use /event follow in Discord to choose the tournaments members can pick.'
          : 'Admins choose which tournaments are open for picks.'}
      </EmptyState>
    )
  }
  return (
    <>
      {groupEvents(poll.data.events).map((g) => (
        <section key={g.key} className="day">
          <h2 className="day-label">{g.label}</h2>
          <ul className="event-list">
            {g.events.map((e) => (
              <EventCard key={e.id} e={e} />
            ))}
          </ul>
        </section>
      ))}
    </>
  )
}

function EventCard({ e }: { e: EventSummary }) {
  const dates = formatDateRange(e)
  const tier = tierLabel(e.tier)
  return (
    <li>
      <Link to={`/events/${e.id}`} className="event-card">
        <span className="event-card-name">{e.name}</span>
        <span className="event-card-meta">
          {[tier, dates].filter(Boolean).join(' · ')}
          {e.status === 'canceled' && ' · Canceled'}
        </span>
        <span className="row-tags">
          {e.live > 0 && <span className="tag tag-live">{e.live} live</span>}
          <span className="tag">
            {e.matches} {e.matches === 1 ? 'match' : 'matches'}
          </span>
          <span className="tag">
            {e.picks} {e.picks === 1 ? 'pick' : 'picks'}
          </span>
        </span>
      </Link>
    </li>
  )
}
