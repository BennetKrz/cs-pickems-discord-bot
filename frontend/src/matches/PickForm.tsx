import { useState } from 'react'
import { ApiError, PICK_LOCKED, type MapRow, type MatchDetail, type PickView } from '../api.ts'
import { useAuth } from '../auth-context.ts'
import { useServerNow } from '../clock.ts'
import { formatCountdown, teamName, teamShort } from './format.ts'
import {
  chooseMapScore,
  chooseMapWinner,
  chooseSeries,
  chooseWinner,
  draftFromPick,
  isEmptyDraft,
  mapScoreOptions,
  mapScoreValue,
  parseMapScoreValue,
  pickErrorMessage,
  pickHints,
  sameDraft,
  seriesOptions,
  toParts,
  type Draft,
  type Side,
} from './pickForm.ts'
import { TeamLogo } from './TeamLogo.tsx'

interface Props {
  match: MatchDetail
  /** Both teams are known: a pickable match has them. */
  teams: { a: number; b: number }
  /** My saved pick, from `GET …/picks`. */
  saved: PickView | undefined
  /** After a save or delete: refresh the picks. */
  onChanged: () => void
  /** The server said `pick_locked`. */
  onLocked: () => void
}

/**
 * The pick form (T10 rules): winner, series score (Bo3/Bo5) and map picks,
 * every part optional. An existing pick opens pre-filled with *Save
 * changes* and *Remove pick*. The parent swaps it for the read-only view at
 * the lock.
 */
export function PickForm({ match, teams, saved, onChanged, onLocked }: Props) {
  const { api } = useAuth()
  const baseline = draftFromPick(match.format, teams, saved)
  const [draft, setDraft] = useState<Draft>(baseline)
  // The saved pick the draft started from; when a poll brings a newer one
  // and nothing was changed, the form follows it.
  const [base, setBase] = useState<Draft>(baseline)
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState<{ kind: 'error' | 'ok'; text: string } | null>(null)
  const now = useServerNow(1_000)

  if (!sameDraft(base, baseline)) {
    setBase(baseline)
    if (sameDraft(draft, base)) setDraft(baseline)
  }

  const dirty = !sameDraft(draft, baseline)
  const names: Record<Side, string> = { a: teamShort(match.team_a), b: teamShort(match.team_b) }
  const hints = pickHints(match.format, draft, names)
  const lockLeft = match.lock_at ? Date.parse(match.lock_at) - now : Number.NaN

  function edit(next: Draft) {
    setDraft(next)
    setMessage(null)
  }

  async function run(action: () => Promise<unknown>, done: string) {
    setBusy(true)
    setMessage(null)
    try {
      await action()
      setMessage({ kind: 'ok', text: done })
      onChanged()
    } catch (err) {
      setMessage({ kind: 'error', text: pickErrorMessage(err) })
      if (err instanceof ApiError && err.code === PICK_LOCKED) onLocked()
    } finally {
      setBusy(false)
    }
  }

  function save() {
    if (isEmptyDraft(draft)) {
      void run(() => api.deletePick(match.id), 'Pick removed.')
    } else {
      const done = saved ? 'Changes saved.' : 'Pick saved.'
      void run(() => api.putPick(match.id, toParts(draft, teams)), done)
    }
  }

  function remove() {
    setDraft(draftFromPick(match.format, teams, null))
    void run(() => api.deletePick(match.id), 'Pick removed.')
  }

  const series = seriesOptions(match.format)
  const mapsOpen = draft.maps.some((m) => m.winner !== null)

  return (
    <section className="card pick-form" aria-label="Your pick">
      <div className="card-head">
        <h2>Your pick</h2>
        {!Number.isNaN(lockLeft) && lockLeft > 0 && (
          <span className="tag tag-warn" role="timer">
            Locks in {formatCountdown(lockLeft)}
          </span>
        )}
      </div>

      <div className="team-choice" role="group" aria-label="Winner">
        {(['a', 'b'] as const).map((side) => {
          const team = side === 'a' ? match.team_a : match.team_b
          return (
            <button
              key={side}
              type="button"
              className="choice choice-team"
              aria-pressed={draft.winner === side}
              onClick={() => edit(chooseWinner(draft, side))}
            >
              <TeamLogo team={team} size={32} />
              <span>{teamName(team)}</span>
            </button>
          )
        })}
      </div>

      {series.length > 0 && (
        <div className="chips" role="group" aria-label="Series score">
          {series.map((s) => (
            <button
              key={`${s.a}:${s.b}`}
              type="button"
              className="chip"
              aria-pressed={draft.series?.a === s.a && draft.series.b === s.b}
              onClick={() => edit(chooseSeries(draft, s))}
            >
              {s.a}:{s.b}
            </button>
          ))}
        </div>
      )}

      <details className="map-picks" open={mapsOpen}>
        <summary>Map picks</summary>
        <ol className="map-pick-list">
          {draft.maps.map((m, i) => {
            const n = i + 1
            const mapName = match.maps.find((x: MapRow) => x.number === n)?.map_name
            return (
              <li key={n} className="map-pick">
                <span className="map-pick-name">
                  Map {n}
                  {mapName && <span className="muted"> · {mapName}</span>}
                </span>
                <span className="map-pick-controls">
                  {(['a', 'b'] as const).map((side) => (
                    <button
                      key={side}
                      type="button"
                      className="chip"
                      aria-pressed={m.winner === side}
                      aria-label={`Map ${n}: ${names[side]} wins`}
                      onClick={() => edit(chooseMapWinner(draft, n, side))}
                    >
                      {names[side]}
                    </button>
                  ))}
                  <select
                    aria-label={`Map ${n} score`}
                    value={mapScoreValue(m)}
                    disabled={!m.winner}
                    onChange={(e) =>
                      edit(chooseMapScore(draft, n, parseMapScoreValue(e.target.value)))
                    }
                  >
                    <option value="">Score (optional)</option>
                    {m.winner &&
                      mapScoreOptions(m.winner).map((o) => (
                        <option key={o.value} value={o.value}>
                          {o.label}
                        </option>
                      ))}
                    <option value="ot">Overtime win</option>
                  </select>
                </span>
              </li>
            )
          })}
        </ol>
      </details>

      {hints.map((h) => (
        <p key={h} className="hint" role="note">
          {h}
        </p>
      ))}

      <div className="form-actions">
        <button
          type="button"
          className="button"
          disabled={busy || !dirty || (!saved && isEmptyDraft(draft))}
          onClick={save}
        >
          {saved ? 'Save changes' : 'Save'}
        </button>
        {saved && (
          <button type="button" className="button button-danger" disabled={busy} onClick={remove}>
            Remove pick
          </button>
        )}
      </div>
      {message && (
        <p
          className={message.kind === 'error' ? 'form-error' : 'form-ok'}
          role={message.kind === 'error' ? 'alert' : 'status'}
        >
          {message.text}
        </p>
      )}
    </section>
  )
}
