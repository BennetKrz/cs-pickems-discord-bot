import { createMemoryRouter } from 'react-router'
import { Home } from './routes/Home.tsx'

/**
 * The Activity always loads at `/` inside Discord's iframe, so routing is
 * in-memory: the browser URL never changes and deep links come from launch
 * parameters instead (T08).
 */
export function createAppRouter(initialPath = '/') {
  return createMemoryRouter([{ path: '/', element: <Home /> }], {
    initialEntries: [initialPath],
  })
}
