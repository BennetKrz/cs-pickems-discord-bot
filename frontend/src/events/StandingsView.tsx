import type { StageView } from '../api.ts'
import { MatchRow } from '../matches/MatchRow.tsx'
import { TeamLogo } from '../matches/TeamLogo.tsx'
import type { BackTo } from '../nav.ts'
import { byRound, signed } from './format.ts'

interface Props {
  stage: StageView
  now: number
  /** Where the match detail's back link returns to. */
  back: BackTo
}

/**
 * A Swiss or group stage (T13): the standings (wins–losses, map and round
 * difference), then the matches of each round.
 */
export function StandingsView({ stage, now, back }: Props) {
  const rows = stage.standings ?? []
  return (
    <>
      {rows.length > 0 && (
        <table className="standings">
          <caption className="sr-only">Standings of {stage.name}</caption>
          <thead>
            <tr>
              <th scope="col">#</th>
              <th scope="col">Team</th>
              <th scope="col">W–L</th>
              <th scope="col">
                <abbr title="Map difference">Maps</abbr>
              </th>
              <th scope="col">
                <abbr title="Round difference">Rounds</abbr>
              </th>
            </tr>
          </thead>
          <tbody>
            {rows.map((r, i) => (
              <tr key={r.team.id}>
                <td className="standings-pos">{i + 1}</td>
                <th scope="row" className="standings-team">
                  <span className="standings-team-inner">
                    <TeamLogo team={r.team} size={20} />
                    <span>{r.team.name}</span>
                  </span>
                </th>
                <td>
                  {r.wins}–{r.losses}
                </td>
                <td>{signed(r.map_diff)}</td>
                <td>{signed(r.round_diff)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {byRound(stage.matches).map((r) => (
        <section key={r.round ?? 'none'} className="event-group">
          <h3 className="event-label">{r.round === null ? 'Other matches' : `Round ${r.round}`}</h3>
          <ul className="match-list">
            {r.matches.map((m) => (
              <MatchRow key={m.id} row={m} now={now} back={back} />
            ))}
          </ul>
        </section>
      ))}
    </>
  )
}

/** A stage without structure: its matches by start time. */
export function StageList({ stage, now, back }: Props) {
  return (
    <ul className="match-list">
      {stage.matches.map((m) => (
        <MatchRow key={m.id} row={m} now={now} back={back} />
      ))}
    </ul>
  )
}
