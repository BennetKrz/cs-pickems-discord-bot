import { useEffect, useMemo, useRef, useState } from 'react'
import { Link } from 'react-router'
import type { FeedRef, StageMatch, StageView, TeamRef } from '../api.ts'
import { formatDateTime, teamName } from '../matches/format.ts'
import { PickTag, StatusTag } from '../matches/MatchRow.tsx'
import { TeamLogo } from '../matches/TeamLogo.tsx'
import type { BackTo } from '../nav.ts'
import {
  fitScale,
  HEADER_H,
  layoutBracket,
  NODE_H,
  NODE_W,
  slotLabel,
  type NodeBox,
} from './bracketLayout.ts'

/**
 * An elimination stage as a tree (T13): absolutely placed match nodes over
 * an SVG layer with the connector lines. It scrolls sideways by default;
 * *Fit to width* scales the whole tree into the view.
 */
export function BracketView({ stage, now, back }: { stage: StageView; now: number; back: BackTo }) {
  const layout = useMemo(() => layoutBracket(stage.matches, stage.kind), [stage])
  const byId = useMemo(() => new Map(stage.matches.map((m) => [m.id, m])), [stage])
  const [fit, setFit] = useState(false)
  const scrollRef = useRef<HTMLDivElement>(null)
  const [containerWidth, setContainerWidth] = useState(0)

  useEffect(() => {
    const el = scrollRef.current
    if (!el) return
    const measure = () => setContainerWidth(el.clientWidth)
    measure()
    // jsdom and old WebViews have no ResizeObserver; then the first
    // measurement stays.
    if (typeof ResizeObserver === 'undefined') return
    const observer = new ResizeObserver(measure)
    observer.observe(el)
    return () => observer.disconnect()
  }, [])

  const scale = fit ? fitScale(containerWidth, layout.width) : 1
  return (
    <div className="bracket">
      <label className="filter">
        <input type="checkbox" checked={fit} onChange={() => setFit((f) => !f)} />
        Fit to width
      </label>
      <div
        ref={scrollRef}
        className={`bracket-scroll${fit ? ' is-fit' : ''}`}
        data-testid="bracket-scroll"
      >
        <div
          className="bracket-frame"
          style={{ width: layout.width * scale, height: layout.height * scale }}
        >
          <div
            className="bracket-canvas"
            data-testid="bracket-canvas"
            style={{
              width: layout.width,
              height: layout.height,
              transform: scale === 1 ? undefined : `scale(${scale})`,
            }}
          >
            <svg
              className="bracket-lines"
              width={layout.width}
              height={layout.height}
              aria-hidden="true"
            >
              {layout.edges.map((e) => (
                <path key={`${e.from}-${e.to}-${e.slot}`} d={e.d} data-edge={`${e.from}-${e.to}`} />
              ))}
            </svg>
            {layout.headers.map((h) => (
              <div
                key={h.key}
                className="bracket-header"
                style={{ left: h.x, top: h.y, width: NODE_W, height: HEADER_H }}
              >
                {h.label}
              </div>
            ))}
            {stage.matches.map((m) => {
              const box = layout.nodes.get(m.id)
              if (!box) return null
              const label = (feed: FeedRef | null) => slotLabel(feed, byId, layout.codes)
              return (
                <BracketNode
                  key={m.id}
                  m={m}
                  box={box}
                  placeholderA={label(m.bracket?.feeds_a ?? null)}
                  placeholderB={label(m.bracket?.feeds_b ?? null)}
                  now={now}
                  back={back}
                />
              )
            })}
          </div>
        </div>
      </div>
    </div>
  )
}

function TeamLine({
  team,
  placeholder,
  score,
  winner,
}: {
  team: TeamRef | null
  placeholder: string
  score: number | null
  winner: boolean
}) {
  return (
    <span className={`bracket-team${winner ? ' is-winner' : ''}${team ? '' : ' is-tbd'}`}>
      {team && <TeamLogo team={team} size={18} />}
      <span className="bracket-team-name">{team ? teamName(team) : placeholder}</span>
      {score !== null && <span className="bracket-score">{score}</span>}
    </span>
  )
}

function BracketNode({
  m,
  box,
  placeholderA,
  placeholderB,
  now,
  back,
}: {
  m: StageMatch
  box: NodeBox
  placeholderA: string
  placeholderB: string
  now: number
  back: BackTo
}) {
  const played = m.status === 'live' || m.status === 'finished'
  const winner = (t: TeamRef | null) => m.winner_team_id !== null && m.winner_team_id === t?.id
  return (
    <Link
      to={`/matches/${m.id}`}
      state={{ back }}
      className={`bracket-node${m.status === 'live' ? ' is-live' : ''}`}
      data-match={m.id}
      style={{ left: box.x, top: box.y, width: NODE_W, height: NODE_H }}
    >
      <TeamLine
        team={m.team_a}
        placeholder={placeholderA}
        score={played ? m.maps_won_a : null}
        winner={winner(m.team_a)}
      />
      <TeamLine
        team={m.team_b}
        placeholder={placeholderB}
        score={played ? m.maps_won_b : null}
        winner={winner(m.team_b)}
      />
      {/* My pick first: the narrow node clips from the right. */}
      <span className="bracket-meta">
        <PickTag row={m} />
        <StatusTag row={m} now={now} />
        {m.status === 'scheduled' && !m.locked && m.scheduled_at && (
          <span className="bracket-time">{formatDateTime(m.scheduled_at)}</span>
        )}
      </span>
    </Link>
  )
}
