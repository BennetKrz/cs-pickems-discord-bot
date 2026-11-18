import { createMemoryRouter, Navigate } from 'react-router'
import { Events, Leaderboard, Matches, Me } from './routes/tabs.tsx'
import { Shell } from './shell/Shell.tsx'

/**
 * The Activity always loads at `/` inside Discord's iframe, so routing is
 * in-memory: the browser URL never changes and deep links come from launch
 * parameters instead (T08). `/` opens the Matches tab.
 */
export function createAppRouter(initialPath = '/') {
  return createMemoryRouter(
    [
      {
        path: '/',
        element: <Shell />,
        children: [
          { index: true, element: <Navigate to="/matches" replace /> },
          { path: 'matches', element: <Matches /> },
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
