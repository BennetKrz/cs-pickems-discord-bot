import { createMemoryRouter, Navigate } from 'react-router'
import { MatchDetailRoute } from './matches/MatchDetail.tsx'
import { MatchesView } from './matches/MatchesView.tsx'
import { Events, Leaderboard, Me } from './routes/tabs.tsx'
import { Shell } from './shell/Shell.tsx'

/**
 * The Activity always loads at `/` inside Discord's iframe, so routing is
 * in-memory: the browser URL never changes. Deep links from bot buttons
 * arrive as a pending view after sign-in (T08, `PendingViewOpener`), since
 * Discord passes no launch parameters. `/` opens the Matches tab.
 */
export function createAppRouter(initialPath = '/') {
  return createMemoryRouter(
    [
      {
        path: '/',
        element: <Shell />,
        children: [
          { index: true, element: <Navigate to="/matches" replace /> },
          { path: 'matches', element: <MatchesView /> },
          { path: 'matches/:id', element: <MatchDetailRoute /> },
          { path: 'events', element: <Events /> },
          { path: 'leaderboard', element: <Leaderboard /> },
          { path: 'me', element: <Me /> },
          { path: '*', element: <Navigate to="/matches" replace /> },
        ],
      },
    ],
    { initialEntries: [initialPath] },
  )
}
