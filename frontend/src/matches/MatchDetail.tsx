import { useState } from 'react'
import { Link, useOutletContext, useParams } from 'react-router'
import { matchPath, matchPicksPath, type MatchDetail as Detail, type MatchPicks } from '../api.ts'
import { useLockPassed, useServerNow } from '../clock.ts'
import { POLL_IDLE_MS, POLL_LIVE_MS, usePoll } from '../poll.ts'
import type { ShellContext } from '../shell/Shell.tsx'
import { ErrorState, Loading } from '../shell/states.tsx'
import { EveryonesPicks } from './EveryonesPicks.tsx'
import { COUNTDOWN_WINDOW_MS, formatDateTime, formatLabel, teamName } from './format.ts'
import { MyPick } from './MyPick.tsx'
import { PickForm } from './PickForm.tsx'
import { TeamLogo } from './TeamLogo.tsx'

/** Opens the detail of the match in the route (`/matches/:id`). */
export function MatchDetailRoute() {
  const { id } = useParams()
  const matchId = Number(id)
  if (!Number.isInteger(matchId) || matchId <= 0) {
    return <ErrorState title="Match not found" message="This match does not exist." />
  }
  // Keyed, so another match starts with fresh state.
  return <MatchDetailView key={matchId} matchId={matchId} />
}

/** Why there is no pick form. */
function closedReason(m: Detail, lockPassed: boolean, lockedByServer: boolean): string {
  if (m.voided) return 'This match was voided: no points for anyone.'
  if (m.status === 'canceled') return 'This match was canceled.'
  if (m.locked || lockPassed || lockedByServer || m.status === 'live' || m.status === 'finished') {
    return 'Picks are locked.'
  }
  if (!m.team_a || !m.team_b) return 'Picks open once both teams are known.'
  if (m.status === 'postponed') return 'This match is postponed.'
  if (!m.lock_at) return 'Picks open once the start time is known.'
  return 'Picks are not open for this event.'
}

function MatchDetailView({ matchId }: { matchId: number }) {
  const { me } = useOutletContext<ShellContext>()
  const now = useServerNow(30_000)
  // Polls fast while the match is live or about to lock.
  const [fast, setFast] = useState(true)
  const interval = fast ? POLL_LIVE_MS : POLL_IDLE_MS
  const detail = usePoll<Detail>(matchPath(matchId), interval)
  const picks = usePoll<MatchPicks>(matchPicksPath(matchId), interval)
  const lockPassed = useLockPassed(detail.data?.lock_at ?? picks.data?.lock_at)
  // A save answered `pick_locked`: the server's clock is ahead of ours.
  const [lockedByServer, setLockedByServer] = useState(false)

  if (detail.loading || picks.loading) return <Loading label="Loading match…" />
  if (!detail.data || !picks.data) {
    const notFound = (detail.error as { status?: number } | undefined)?.status === 404
    return (
      <ErrorState
        title={notFound ? 'Match not found' : 'Match could not be loaded'}
        message={notFound ? 'This match does not exist.' : 'Check your connection and try again.'}
        onRetry={
          notFound
            ? undefined
            : () => {
                detail.refresh()
                picks.refresh()
              }
        }
      />
    )
  }
  const m = detail.data
  const lockMs = m.lock_at ? Date.parse(m.lock_at) : Number.NaN
  const shouldBeFast =
    m.status === 'live' ||
    (!m.locked && !Number.isNaN(lockMs) && lockMs - now < COUNTDOWN_WINDOW_MS)
  if (shouldBeFast !== fast) setFast(shouldBeFast)

  const mine = picks.data.picks.find((p) => p.user_id === me.user_id)
  const teams = m.team_a && m.team_b ? { a: m.team_a.id, b: m.team_b.id } : null
  const canPick = picks.data.pickable && !lockPassed && !lockedByServer && teams !== null

  return (
    <>
      <Link to="/matches" className="back">
        ← Matches
      </Link>
      <Header m={m} />
      {m.maps.length > 0 && <Maps m={m} />}
      {canPick ? (
        <PickForm
          match={m}
          teams={teams}
          saved={mine}
          onChanged={picks.refresh}
          onLocked={() => {
            setLockedByServer(true)
            detail.refresh()
            picks.refresh()
          }}
        />
      ) : (
        <MyPick
          pick={mine}
          teamA={m.team_a}
          teamB={m.team_b}
          closedReason={closedReason(m, lockPassed, lockedByServer)}
        />
      )}
      <EveryonesPicks data={picks.data} teamA={m.team_a} teamB={m.team_b} myUserId={me.user_id} />
    </>
  )
}

const STATUS_LABEL = {
  scheduled: 'Upcoming',
  live: 'Live',
  finished: 'Final',
  canceled: 'Canceled',
  postponed: 'Postponed',
} as const

function Header({ m }: { m: Detail }) {
  const when = m.started_at ?? m.scheduled_at
  const showScore = m.status === 'live' || m.status === 'finished'
  const winnerA = m.winner_team_id !== null && m.winner_team_id === m.team_a?.id
  const winnerB = m.winner_team_id !== null && m.winner_team_id === m.team_b?.id
  return (
    <section className="card match-header">
      <p className="muted">
        {m.event.name}
        {m.stage && ` · ${m.stage.name}`} · {formatLabel(m.format)}
      </p>
      <div className="versus">
        <span className={`versus-team${winnerA ? ' is-winner' : ''}`}>
          <TeamLogo team={m.team_a} size={48} />
          <span>{teamName(m.team_a)}</span>
        </span>
        <span className="versus-mid">
          {showScore ? (
            <span className="row-score versus-score">
              {m.maps_won_a}:{m.maps_won_b}
            </span>
          ) : (
            <span className="muted">vs</span>
          )}
        </span>
        <span className={`versus-team${winnerB ? ' is-winner' : ''}`}>
          <TeamLogo team={m.team_b} size={48} />
          <span>{teamName(m.team_b)}</span>
        </span>
      </div>
      <p className="match-when">
        <span className={`tag ${m.status === 'live' ? 'tag-live' : 'tag-muted'}`}>
          {m.voided ? 'Void' : STATUS_LABEL[m.status]}
        </span>{' '}
        {when ? <time dateTime={when}>{formatDateTime(when)}</time> : 'Time to be announced'}
      </p>
    </section>
  )
}

function Maps({ m }: { m: Detail }) {
  const winner = (id: number | null) =>
    id === null ? '' : id === m.team_a?.id ? teamName(m.team_a) : teamName(m.team_b)
  return (
    <section className="card" aria-label="Maps">
      <h2>Maps</h2>
      <ol className="maps">
        {m.maps.map((map) => (
          <li key={map.number} className={`map map-${map.status}`}>
            <span className="muted">Map {map.number}</span>
            <span>{map.map_name ?? 'TBA'}</span>
            <span className="map-rounds">
              {map.status === 'not_played'
                ? 'Not played'
                : map.rounds_a !== null && map.rounds_b !== null
                  ? `${map.rounds_a}:${map.rounds_b}`
                  : map.status === 'live'
                    ? 'Live'
                    : ''}
            </span>
            <span className="map-winner">{winner(map.winner_team_id)}</span>
          </li>
        ))}
      </ol>
    </section>
  )
}
