// Shows gai's web push notifications and opens the dashboard on a click.

self.addEventListener('push', (event) => {
  let data = {};
  try {
    data = event.data ? event.data.json() : {};
  } catch {
    data = { title: 'gai', body: event.data ? event.data.text() : '' };
  }
  event.waitUntil(
    self.registration.showNotification(data.title || 'gai', {
      body: data.body || '',
      tag: data.tag || undefined,
      data: { url: data.url || '/' },
      requireInteraction: data.kind === 'approval',
    }),
  );
});

self.addEventListener('notificationclick', (event) => {
  event.notification.close();
  const raw = (event.notification.data && event.notification.data.url) || '/';
  const url = raw.startsWith('#') ? `/${raw}` : raw;
  event.waitUntil(
    self.clients.matchAll({ type: 'window', includeUncontrolled: true }).then((wins) => {
      for (const w of wins) {
        if (new URL(w.url).origin === self.location.origin) {
          w.focus();
          return w.navigate(new URL(url, self.location.origin).href);
        }
      }
      return self.clients.openWindow(url);
    }),
  );
});
