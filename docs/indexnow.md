# Website IndexNow notifications

The `Deploy site` workflow submits the canonical sitemap URLs to
`https://api.indexnow.org/indexnow` after a successful CapRover deployment.
It first waits for the deployed Git SHA, ownership key, and sitemap to match
the build. Deployments are serialized to avoid overlapping rollouts.

The public ownership file is `site/public/indexnow-key.txt`, served at
`https://pgsandbox.dev/indexnow-key.txt`. This is an IndexNow verification key,
not an account/API credential; no additional secret is needed. Keep it stable.

The small static site submits its complete current sitemap on each deployment
(in batches of at most 10,000 URLs), not on a recurring polling schedule.
The script refuses empty sitemaps, non-production origins, query strings,
fragments, and redirected verification endpoints. CI validates build output
without sending any notifications.

For a deliberate retry after a submission failure, with the matching build:

```sh
python3 scripts/submit_indexnow.py --dry-run
python3 scripts/submit_indexnow.py --revision "$(git rev-parse HEAD)"
```

Alternatively rerun the `Deploy site` workflow. A failed IndexNow step does not
roll back the already-deployed site; investigate HTTP 403/422 errors before
retrying and respect HTTP 429 rate limits. HTTP 200 means received; HTTP 202
means received with ownership validation pending. Neither proves indexing.
URLs removed from the sitemap are not submitted by this full-current-sitemap
implementation; submit removed URLs separately when retiring pages.

Protocol: https://www.indexnow.org/documentation
