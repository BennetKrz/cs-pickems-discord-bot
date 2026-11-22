import { Link } from 'react-router'
import type { MatchRow as Row } from '../api.ts'
import {
  COUNTDOWN_WINDOW_MS,
  formatCountdown,
  formatLabel,
  formatTime,
  teamName,
} from './format.ts'
import { pointsLabel } from './summary.ts'
import { TeamLogo } from './TeamLogo.tsx'

const MY_PICK_LABEL = { none: 'No pick', partial: 'Partial pick', full: 'Picked' } as const

/** What the middle column shows: the start time, or the score. */
function Center({ row }: { row: Row }) {
  if (row.status === 'live') {
    const map = row.current_map
    return (
      <span className="row-center">
        <span className="row-score">
          {row.maps_won_a}:{row.maps_won_b}
        </span>
        {map && (
          <span className="row-sub">
            Map {map.number}
            {map.rounds_a !== null && map.rounds_b !== null && ` · ${map.rounds_a}:${map.rounds_b}`}
          </span>
        )}
      </span>
    )
  }
  if (row.status === 'finished') {
    return (
      <span className="row-center">
        <span className="row-score">
          {row.maps_won_a}:{row.maps_won_b}
        </span>
      </span>
    )
  }
  return (
    <span className="row-center">
      <span className="row-time">{row.scheduled_at ? formatTime(row.scheduled_at) : 'TBA'}</span>
    </span>
  )
}

/** The status line under the teams: live badge, lock countdown, or state. */
function Status({ row, now }: { row: Row; now: number }) {
  if (row.voided) return <span className="tag tag-muted">Void</span>
  switch (row.status) {
    case 'live':
      return <span className="tag tag-live">Live</span>
    case 'canceled':
      return <span className="tag tag-muted">Canceled</span>
    case 'postponed':
      return <span className="tag tag-muted">Postponed</span>
    case 'finished':
      return <span className="tag tag-muted">Final</span>
    case 'scheduled': {
      if (row.locked) return <span className="tag tag-muted">Locked</span>
      const lockAt = row.lock_at ? Date.parse(row.lock_at) : Number.NaN
      const left = lockAt - now
      if (!Number.isNaN(left) && left > 0 && left < COUNTDOWN_WINDOW_MS) {
        return <span className="tag tag-warn">Locks in {formatCountdown(left, true)}</span>
      }
      return null
    }
  }
}

/** One match in the list; opens the match detail. */
export function MatchRow({ row, now }: { row: Row; now: number }) {
  const winnerA = row.winner_team_id !== null && row.winner_team_id === row.team_a?.id
  const winnerB = row.winner_team_id !== null && row.winner_team_id === row.team_b?.id
  const done = row.status === 'finished' || row.status === 'canceled'
  return (
    <li>
      <Link to={`/matches/${row.id}`} className="match-row">
        <span className={`row-team row-team-a${winnerA ? ' is-winner' : ''}`}>
          <span className="row-team-name">{teamName(row.team_a)}</span>
          <TeamLogo team={row.team_a} />
        </span>
        <Center row={row} />
        <span className={`row-team row-team-b${winnerB ? ' is-winner' : ''}`}>
          <TeamLogo team={row.team_b} />
          <span className="row-team-name">{teamName(row.team_b)}</span>
        </span>
        <span className="row-meta">
          <span>
            {formatLabel(row.format)}
            {row.stage && ` · ${row.stage.name}`}
            {` · ${row.pick_count} ${row.pick_count === 1 ? 'pick' : 'picks'}`}
          </span>
          <span className="row-tags">
            <Status row={row} now={now} />
            {done && row.my_result ? (
              <span className="tag tag-points">
                {pointsLabel(row.my_result.points, row.my_result.state)}
              </span>
            ) : (
              <span className={`tag tag-pick tag-pick-${row.my_pick}`}>
                {MY_PICK_LABEL[row.my_pick]}
              </span>
            )}
          </span>
        </span>
      </Link>
    </li>
  )
}
