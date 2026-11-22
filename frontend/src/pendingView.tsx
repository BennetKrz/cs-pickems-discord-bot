import { useEffect, useRef } from 'react'
import type { ApiClient } from './api.ts'
import { useAuth } from './auth-context.ts'

/** `GET`: the match a bot button asked to open; taken once (T08). */
export const PENDING_VIEW_PATH = '/api/me/pending-view'

export interface PendingView {
  match_id: number | null
}

/**
 * Opens the match whose *Pick now* button on a bot post launched the
 * Activity. Discord passes no launch parameters, so the bot stores the
 * match server-side for 10 minutes and the Activity asks once per sign-in.
 * The ref keeps React's StrictMode double effect from taking the view
 * twice (the second call would find nothing).
 */
export function PendingViewOpener({ onOpen }: { onOpen: (path: string) => void }) {
  const { api } = useAuth()
  const asked = useRef<ApiClient | null>(null)
  useEffect(() => {
    if (asked.current === api) return
    asked.current = api
    api.get<PendingView>(PENDING_VIEW_PATH).then(
      (res) => {
        if (!res.notModified && res.data.match_id != null) {
          onOpen(`/matches/${res.data.match_id}`)
        }
      },
      (error: unknown) => {
        // The deep link is a convenience; the app works without it.
        console.warn('pending view failed', error)
      },
    )
  }, [api, onOpen])

  return null
}
