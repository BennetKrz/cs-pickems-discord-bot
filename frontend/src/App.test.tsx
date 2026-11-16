import { render, screen } from '@testing-library/react'
import { RouterProvider } from 'react-router'
import { describe, expect, it, vi } from 'vitest'
import { createAppRouter } from './router.tsx'

function mockFetch(response: Response | Error) {
  return vi.spyOn(globalThis, 'fetch').mockImplementation(() =>
    response instanceof Error ? Promise.reject(response) : Promise.resolve(response),
  )
}

describe('App', () => {
  it('renders the placeholder page at / and shows server health', async () => {
    const fetchSpy = mockFetch(
      Response.json({ version: '0.1.0', git_sha: 'abc1234', db: 'ok' }),
    )
    render(<RouterProvider router={createAppRouter()} />)

    expect(screen.getByRole('heading', { name: /CS2 Pick.em/ })).toBeInTheDocument()
    expect(
      await screen.findByText('Server v0.1.0 (abc1234), database ok'),
    ).toBeInTheDocument()
    expect(fetchSpy).toHaveBeenCalledWith('/healthz', expect.anything())
  })

  it('shows an error when the server is unreachable', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    mockFetch(new TypeError('network down'))
    render(<RouterProvider router={createAppRouter()} />)

    expect(await screen.findByText('Server unreachable.')).toBeInTheDocument()
  })
})
