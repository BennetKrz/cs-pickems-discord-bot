import type { PartScore, PickView, TeamRef } from '../api.ts'
import { partOutcomeLabel, pointsLabel, summarizePick } from './summary.ts'

function Outcome({ part }: { part: PartScore | null | undefined }) {
  if (!part) return null
  return <span className={`outcome outcome-${part.outcome}`}>{partOutcomeLabel(part)}</span>
}

/**
 * My pick, read-only (after the lock, or when the match can't be picked),
 * with the points breakdown once settlement ran.
 */
export function MyPick({
  pick,
  teamA,
  teamB,
  closedReason,
}: {
  pick: PickView | undefined
  teamA: TeamRef | null
  teamB: TeamRef | null
  /** Why there is no form, e.g. "Picks are locked." */
  closedReason: string
}) {
  const result = pick?.result ?? null
  const breakdown = result?.breakdown
  return (
    <section className="card" aria-label="Your pick">
      <div className="card-head">
        <h2>Your pick</h2>
        {result && (
          <span className="tag tag-points">{pointsLabel(result.points, result.state)}</span>
        )}
      </div>
      <p className="muted">{closedReason}</p>
      {!pick ? (
        <p>You didn&rsquo;t pick this match.</p>
      ) : (
        <ul className="breakdown">
          {summarizePick(pick, teamA, teamB).map((part) => {
            let score: PartScore | null | undefined
            let mapScore: PartScore | null | undefined
            if (part.key === 'winner') score = breakdown?.winner
            else if (part.key === 'series') score = breakdown?.series
            else {
              const n = Number(part.key.slice(3))
              const m = breakdown?.maps.find((x) => x.map_number === n)
              score = m?.winner
              mapScore = m?.score
            }
            return (
              <li key={part.key}>
                <span className="muted">{part.label}</span>
                <span>{part.value}</span>
                <span className="breakdown-points">
                  <Outcome part={score} />
                  {mapScore && (
                    <>
                      {' '}
                      <Outcome part={mapScore} />
                    </>
                  )}
                </span>
              </li>
            )
          })}
        </ul>
      )}
      {result && result.pending_parts > 0 && (
        <p className="muted">
          {result.pending_parts} {result.pending_parts === 1 ? 'part is' : 'parts are'} waiting for
          results.
        </p>
      )}
    </section>
  )
}
