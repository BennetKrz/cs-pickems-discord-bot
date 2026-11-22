import { Link } from 'react-router'
import type { MatchPicks, TeamRef } from '../api.ts'
import { profileLink } from '../members/link.ts'
import { Avatar } from '../shell/Avatar.tsx'
import { formatDateTime, teamShort } from './format.ts'
import { pointsLabel, sortPicks, summarizePick } from './summary.ts'

/** A bar split between team A and team B. */
function SplitBar({
  label,
  a,
  b,
  pctA,
  pctB,
  teamA,
  teamB,
}: {
  label: string
  a: number
  b: number
  pctA: number
  pctB: number
  teamA: TeamRef | null
  teamB: TeamRef | null
}) {
  const empty = a + b === 0
  return (
    <div
      className="split"
      aria-label={`${label}: ${teamShort(teamA)} ${pctA}%, ${teamShort(teamB)} ${pctB}%`}
    >
      <div className="split-labels">
        <span>
          {teamShort(teamA)} {pctA}%
        </span>
        <span className="muted">{label}</span>
        <span>
          {pctB}% {teamShort(teamB)}
        </span>
      </div>
      <div className={`split-bar${empty ? ' is-empty' : ''}`} aria-hidden="true">
        <span className="split-a" style={{ width: `${pctA}%` }} />
        <span className="split-b" style={{ width: `${pctB}%` }} />
      </div>
    </div>
  )
}

/**
 * Everyone's picks (T12): the winner split, the series distribution and
 * the per-map split, then every member's pick. Visible before the lock too.
 */
export function EveryonesPicks({
  data,
  teamA,
  teamB,
  myUserId,
}: {
  data: MatchPicks
  teamA: TeamRef | null
  teamB: TeamRef | null
  myUserId: string
}) {
  const { stats } = data
  const seriesTotal = stats.series.reduce((n, s) => n + s.count, 0)
  // A profile opened from here leads back to this match.
  const back = { to: `/matches/${data.match_id}`, label: 'Match' }
  return (
    <section className="card" aria-label="Everyone’s picks">
      <div className="card-head">
        <h2>Everyone&rsquo;s picks</h2>
        <span className="muted">
          {stats.total} {stats.total === 1 ? 'pick' : 'picks'}
        </span>
      </div>
      {stats.total === 0 ? (
        <p className="muted">No picks yet.</p>
      ) : (
        <>
          <SplitBar
            label="Winner"
            a={stats.winner.a}
            b={stats.winner.b}
            pctA={stats.winner.pct_a}
            pctB={stats.winner.pct_b}
            teamA={teamA}
            teamB={teamB}
          />
          {seriesTotal > 0 && (
            <div className="series-dist" aria-label="Series scores">
              {stats.series.map((s) => (
                <div key={`${s.a}:${s.b}`} className="series-bucket">
                  <span className="series-score">
                    {s.a}:{s.b}
                  </span>
                  <span className="series-bar" aria-hidden="true">
                    <span style={{ width: `${s.pct}%` }} />
                  </span>
                  <span className="series-pct">
                    {s.pct}% ({s.count})
                  </span>
                </div>
              ))}
            </div>
          )}
          {stats.maps
            .filter((m) => m.a + m.b > 0)
            .map((m) => (
              <SplitBar
                key={m.map_number}
                label={`Map ${m.map_number}`}
                a={m.a}
                b={m.b}
                pctA={m.pct_a}
                pctB={m.pct_b}
                teamA={teamA}
                teamB={teamB}
              />
            ))}
          <ul className="pick-list">
            {sortPicks(data.picks).map((p) => {
              const link = profileLink(p, myUserId, back)
              const isMe = p.user_id === myUserId
              return (
                <li key={p.user_id} className={`pick-item${isMe ? ' is-me' : ''}`}>
                  <Link to={link.to} state={link.state} tabIndex={-1} aria-hidden="true">
                    <Avatar src={p.avatar_url} name={p.display_name} size={32} />
                  </Link>
                  <div className="pick-body">
                    <div className="pick-who">
                      <Link to={link.to} state={link.state} className="pick-name">
                        {p.display_name}
                      </Link>
                      {isMe && <span className="muted"> (you)</span>}
                      {p.edited && (
                        <span className="muted pick-edited">
                          {' '}
                          · edited {formatDateTime(p.updated_at)}
                        </span>
                      )}
                    </div>
                    <div className="pick-parts">
                      {summarizePick(p, teamA, teamB).map((part) => (
                        <span key={part.key} className="part">
                          <span className="muted">{part.label}</span> {part.value}
                        </span>
                      ))}
                    </div>
                  </div>
                  {p.result && (
                    <span className="tag tag-points">
                      {pointsLabel(p.result.points, p.result.state)}
                    </span>
                  )}
                </li>
              )
            })}
          </ul>
        </>
      )}
    </section>
  )
}
