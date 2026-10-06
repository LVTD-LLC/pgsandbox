import posthog from 'posthog-js';

const key = import.meta.env.PUBLIC_POSTHOG_KEY;
const production = ['pgsandbox.dev', 'www.pgsandbox.dev'].includes(window.location.hostname);
const enabled = Boolean(key && production && navigator.doNotTrack !== '1');

// Sanitize URL properties, including initial attribution. Campaign dimensions
// remain separate SDK properties; raw query strings and fragments are not sent.
function sanitize(value: unknown): unknown {
  if (typeof value === 'string' && /^https?:\/\//.test(value)) {
    try {
      const url = new URL(value);
      return `${url.origin}${url.pathname}`;
    } catch { return undefined; }
  }
  if (Array.isArray(value)) return value.map(sanitize);
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, sanitize(item)]));
  }
  return value;
}

if (enabled) {
  posthog.init(key, {
    api_host: 'https://us.i.posthog.com',
    ui_host: 'https://us.posthog.com',
    defaults: '2026-05-30',
    person_profiles: 'identified_only',
    respect_dnt: true,
    capture_pageview: true,
    capture_pageleave: true,
    autocapture: true,
    capture_dead_clicks: true,
    capture_heatmaps: true,
    capture_performance: { web_vitals: true },
    capture_exceptions: true,
    mask_all_text: true,
    mask_all_element_attributes: true,
    session_recording: { maskAllInputs: true, maskTextSelector: '*', recordCrossOriginIframes: false },
    enable_recording_console_log: false,
    before_send: (event) => {
      if (event) {
        event.properties = sanitize(event.properties) as typeof event.properties;
        event.properties.surface = 'website';
        event.properties.app = 'pgsandbox';
        event.properties.telemetrySchemaVersion = 2;
      }
      return event;
    },
  });

  document.addEventListener('click', (event) => {
    const target = event.target instanceof Element ? event.target.closest('a') : null;
    if (!(target instanceof HTMLAnchorElement)) return;
    const url = new URL(target.href, location.href);
    if (!['http:', 'https:'].includes(url.protocol)) return;
    if (url.origin !== location.origin) {
      capture('pgsandbox_outbound_link_clicked', { destination: `${url.origin}${url.pathname}` });
    } else if (url.pathname.startsWith('/docs/')) {
      capture('pgsandbox_docs_link_clicked', { destination: url.pathname });
    }
  });
}

export function capture(event: string, properties: Record<string, unknown> = {}) {
  if (enabled) posthog.capture(event, properties);
}
