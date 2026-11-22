import { useState } from 'react'
import { Link, useParams, useSearchParams } from 'react-router'
import { eventPath, type EventDetail as Detail, type StageView } from '../api.ts'
import { useServerNow } from '../clock.ts'
import { COUNTDOWN_WINDOW_MS } from '../matches/format.ts'
import type { BackTo } from '../nav.ts'
import { POLL_IDLE_MS, POLL_LIVE_MS, usePoll } from '../poll.ts'
import { EmptyState, ErrorState, Loading } from '../shell/states.tsx'
import { BracketView } from './BracketView.tsx'
import { defaultStage, formatDateRange, STATUS_LABEL, stageKey, tierLabel } from './format.ts'
import { StageList, StandingsView } from './StandingsView.tsx'

/** Opens the event in the route (`/events/:id`). */
export function EventDetailRoute() {
  const { id } = useParams()
  const eventId = Number(id)
  if (!Number.isInteger(eventId) || eventId <= 0) {
    return <ErrorState title="Event not found" message="This event does not exist." />
  }
  // Keyed, so another event starts with fresh state.
  return <EventDetailView key={eventId} eventId={eventId} />
}

/** A match is live or locks within 24 h: poll fast. */
function needsFastPoll(d: Detail, now: number): boolean {
  return d.stages.some((s) =>
    s.matches.some((m) => {
      if (m.status === 'live') return true
      const lock = m.lock_at ? Date.parse(m.lock_at) : Number.NaN
      return !m.locked && !Number.isNaN(lock) && lock - now < COUNTDOWN_WINDOW_MS
    }),
  )
}

/**
 * One event (T13): a tab per stage. Elimination stages are a bracket tree,
 * Swiss and group stages standings with matches per round. Polls every
 * 20 s while a match is live or about to lock, else every 60 s.
 */
function EventDetailView({ eventId }: { eventId: number }) {
  const [params, setParams] = useSearchParams()
  const now = useServerNow(15_000)
  const [fast, setFast] = useState(true)
  const poll = usePoll<Detail>(eventPath(eventId), fast ? POLL_LIVE_MS : POLL_IDLE_MS)

  if (poll.loading) return <Loading label="Loading event…" />
  if (!poll.data) {
    const notFound = (poll.error as { status?: number } | undefined)?.status === 404
    return (
      <ErrorState
        title={notFound ? 'Event not found' : 'Event could not be loaded'}
        message={notFound ? 'This event does not exist.' : 'Check your connection and try again.'}
        onRetry={notFound ? undefined : poll.refresh}
      />
    )
  }
  const d = poll.data
  const shouldBeFast = needsFastPoll(d, now)
  if (shouldBeFast !== fast) setFast(shouldBeFast)

  const chosen = params.get('stage')
  const stage = d.stages.find((s) => stageKey(s) === chosen) ?? defaultStage(d.stages)
  const meta = [tierLabel(d.event.tier), formatDateRange(d.event), STATUS_LABEL[d.event.status]]
    .filter(Boolean)
    .join(' · ')

  return (
    <>
      <Link to="/events" className="back">
        ← Events
      </Link>
      <header className="event-head">
        <h1>{d.event.name}</h1>
        <p className="muted">{meta}</p>
        {!d.followed && (
          <p className="hint">
            This server does not follow this event: its matches can't be picked.
          </p>
        )}
      </header>
      {d.stages.length > 1 && (
        <div className="stage-tabs" role="tablist" aria-label="Stages">
          {d.stages.map((s) => (
            <button
              key={stageKey(s)}
              type="button"
              role="tab"
              aria-selected={s === stage}
              className="segment"
              onClick={() => setParams({ stage: stageKey(s) }, { replace: true })}
            >
              {s.name}
            </button>
          ))}
        </div>
      )}
      {stage ? (
        <Stage
          stage={stage}
          now={now}
          back={{ to: `/events/${eventId}?stage=${stageKey(stage)}`, label: d.event.name }}
        />
      ) : (
        <EmptyState title="No matches yet">The schedule shows up here once it is known.</EmptyState>
      )}
    </>
  )
}

function Stage({ stage, now, back }: { stage: StageView; now: number; back: BackTo }) {
  if (stage.matches.length === 0) {
    return (
      <EmptyState title="No matches yet">The schedule shows up here once it is known.</EmptyState>
    )
  }
  switch (stage.view) {
    case 'bracket':
      return <BracketView stage={stage} now={now} back={back} />
    case 'standings':
      return <StandingsView stage={stage} now={now} back={back} />
    case 'list':
      return <StageList stage={stage} now={now} back={back} />
  }
}
