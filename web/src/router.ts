// Routing with TanStack Router over the URL's hash (`#/runs/123?tab=steps`),
// so the server only ever serves index.html. App reads the route as path
// segments and a query; pages navigate with navigate('/path').

import { createElement } from 'react';
import { createHashHistory, createRootRoute, createRoute, createRouter, useRouterState } from '@tanstack/react-router';
import { App } from './App';

/** The current route, split into path segments and query. */
export interface Route {
  parts: string[];
  query: URLSearchParams;
}

// The whole dashboard is the root route's component (read when it renders,
// so the import cycle with App is fine).
const rootRoute = createRootRoute({ component: () => createElement(App) });
// One route for every path: App picks the page from the segments.
const anyRoute = createRoute({ getParentRoute: () => rootRoute, path: '$' });
const indexRoute = createRoute({ getParentRoute: () => rootRoute, path: '/' });

export const router = createRouter({
  routeTree: rootRoute.addChildren([indexRoute, anyRoute]),
  history: createHashHistory(),
  // Keep the query as written (`?q=a+b`), not as JSON.
  parseSearch: (s) => Object.fromEntries(new URLSearchParams(s)),
  stringifySearch: (o) => {
    const qs = new URLSearchParams(o as Record<string, string>).toString();
    return qs ? `?${qs}` : '';
  },
});

declare module '@tanstack/react-router' {
  interface Register {
    router: typeof router;
  }
}

export function useRoute(): Route {
  const location = useRouterState({ select: (s) => s.location });
  return {
    parts: location.pathname.split('/').filter(Boolean),
    query: new URLSearchParams(location.searchStr),
  };
}

/** Go to `/path?query` (a leading `#` is fine). */
export function navigate(to: string) {
  const [path, qs] = to.replace(/^#/, '').split('?');
  void router.navigate({
    to: path.startsWith('/') ? path : `/${path}`,
    search: Object.fromEntries(new URLSearchParams(qs ?? '')),
  } as never);
}
