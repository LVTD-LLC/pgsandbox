---
title: "Connect a Docker Container to Host PostgreSQL Safely"
excerpt: "Connect a Dockerized app to PostgreSQL running on the host without relying on localhost, broad HBA rules, or leaked sandbox credentials."
author: "PGSandbox Team"
status: "published"
publishedAt: "2026-07-18"
updatedAt: "2026-07-18T06:00:00Z"
tags: ["Postgres", "Docker", "Docker Compose", "database connections", "coding agents"]
category: "Engineering"
metaTitle: "Connect Docker to Host PostgreSQL Safely"
metaDescription: "Connect Docker to host PostgreSQL with host.docker.internal, Linux host-gateway, scoped pg_hba.conf rules, and safe sandbox credentials."
canonicalUrl: "https://pgsandbox-mcp.lvtd.dev/blog/docker-connect-host-postgres/"
heroImageUrl: ""
featured: false
sortOrder: 140
---
To connect a Docker container to PostgreSQL running on the host, use `host.docker.internal` instead of `localhost`. Docker Desktop provides that hostname. On native Linux Docker Engine, map it with `host.docker.internal:host-gateway` and verify that PostgreSQL listens beyond loopback; the name mapping alone cannot reach a server bound only to `127.0.0.1`. Then confirm that `pg_hba.conf` allows the actual client source address.

Do not treat the hostname swap as the entire fix. A resolved name proves only that the container can find an address. It does not prove that PostgreSQL is listening there, that a firewall allows the route, that HBA rules match, or that the role can connect to the intended database.

PGSandbox supports disposable database workflows with a Docker-oriented `localContainer` connection variant. The important decision is where the database client runs, not where the `docker compose` command starts.

## Docker-to-host PostgreSQL at a glance

| Database client location | Host in the PostgreSQL URL | Docker configuration | PGSandbox mode |
| --- | --- | --- | --- |
| Host-native command | `localhost` or `127.0.0.1` | None | `direct` |
| Docker Desktop container | `host.docker.internal` | Built into Docker Desktop on its supported platforms | `localContainer` |
| Native Linux bridge container | `host.docker.internal` | Add `host.docker.internal:host-gateway`; verify the PostgreSQL listener and HBA path | `localContainer` only when the loopback profile is actually reachable through that path |
| Container reaching a routable database host | The real DNS name or IP | Normal container networking | `direct` |
| Native Linux container using host networking | `localhost` | `network_mode: host`; weaker isolation and not portable | `direct` |

The table separates two ideas that are easy to mix up. `localContainer` means "a local application container reaching PostgreSQL on the host." It does not mean PostgreSQL itself runs in a container.

This guide covers **container to host PostgreSQL**. If PostgreSQL runs in another Compose service, use that service name on the shared Docker network. If the application runs on the host and PostgreSQL runs in a container, publish the database container's port and connect from the host. Those are different network directions with different addresses.

## PGSandbox quick path

Use this six-step path when the application process runs in Docker and the selected PGSandbox profile runs PostgreSQL on the same host:

1. Identify the runner: confirm that the database client runs inside the application container.
2. Call `get_connection_string` with the sandbox id and an explicit credential request:

   ```json
   {
     "databaseId": "sandbox-id",
     "includeCredentials": true
   }
   ```

3. Select `connectionStrings.localContainer` from the result. On native Linux Docker Engine, add the `host-gateway` mapping and verify that the PostgreSQL listener is reachable from the bridge.
4. Inject the returned URL unchanged into the application service at runtime. Do not reconstruct it or assume port 5432.
5. Run the application check or migration from the container, then verify database effects with bounded SQL or a schema diff.
6. Delete the sandbox when the proof is complete, or let its TTL cleanup policy remove it.

The credential-bearing result is sensitive. Prefer `run_repo_command` injection when PGSandbox can launch the required command without returning the raw URL to an intermediate workflow.

## Why localhost fails inside a Docker container

With normal bridge networking, a container has its own network namespace and loopback interface. Docker's [networking overview](https://docs.docker.com/engine/network/) explains that `127.0.0.1` inside a container refers to that container's loopback address.

That makes this URL wrong when PostgreSQL runs on the host:

```text
postgresql://sandbox_role:password@localhost:5432/sandbox_database
```

The application asks for port 5432 inside its own container. Unless another process in that same container is listening there, the connection fails.

The conventional host alias is `host.docker.internal`. [Docker Desktop documents it](https://docs.docker.com/desktop/features/networking/networking-how-tos/) as the hostname for reaching a service on the host from a container. Docker Engine documents the special [`host-gateway` value](https://docs.docker.com/reference/cli/docker/container/run/#add-host), which can map the same name on native Linux.

Use this Compose shape:

```yaml
services:
  app:
    extra_hosts:
      - "host.docker.internal:host-gateway"
    environment:
      DATABASE_URL: ${SANDBOX_DATABASE_URL}
```

Set `SANDBOX_DATABASE_URL` in the process that invokes Compose. Do not put a real credential-bearing URL in a tracked Compose file. The `extra_hosts` entry is redundant on Docker Desktop but useful when the same file also runs on native Linux Docker Engine.

The mapping solves name resolution. It does not open PostgreSQL to that address.

A credential-bearing URL has this shape:

```text
postgresql://<role>:<password>@host.docker.internal:<returned-port>/<database>
```

Use the complete URL returned by PGSandbox. Do not reconstruct it or assume port 5432.

The raw `docker run` equivalent is:

```bash
docker run --rm \
  --add-host host.docker.internal:host-gateway \
  -e DATABASE_URL="$SANDBOX_DATABASE_URL" \
  your-app-image
```

The variable should contain the real sandbox port. Do not rebuild the URL around an assumed port 5432.

## Check three boundaries in order

A Docker-to-host PostgreSQL connection crosses three independent boundaries. Diagnose them in order so a routing problem does not turn into an overly broad database rule.

### 1. Container name and route

The container must resolve `host.docker.internal` to a host-side address and route traffic to it. On native Linux, Docker's `host-gateway` normally resolves to an address on the host side of the default bridge, though a daemon administrator can override it.

Check from the same service container that will run the application:

```bash
getent hosts host.docker.internal
```

Then test the PostgreSQL port with a tool already present in the image:

```bash
nc -vz host.docker.internal <returned-port>
```

A successful hostname lookup is not a successful PostgreSQL connection. It only clears the first boundary.

### 2. PostgreSQL listener and host firewall

PostgreSQL's default [`listen_addresses`](https://www.postgresql.org/docs/current/runtime-config-connection.html) value is `localhost`. That accepts loopback TCP connections but may not accept traffic sent to a Linux bridge gateway address.

Inspect the effective settings on the host:

```sql
SHOW listen_addresses;
SHOW port;
SHOW hba_file;
```

On Linux, confirm the bound addresses with `ss -ltn`. If PostgreSQL must listen beyond loopback, add only the host interface the container actually reaches when practical:

```conf
listen_addresses = 'localhost,<reachable-host-interface>'
```

Changing `listen_addresses` requires a PostgreSQL restart. Avoid `listen_addresses = '*'` as a troubleshooting shortcut. It turns a narrow local problem into a service exposed on every available interface unless other controls happen to block it.

The host firewall is a separate check. A timeout after successful name resolution often points to routing or packet filtering. A quick `0.0.0.0/0` firewall rule is not a safe diagnostic step on a laptop, shared development server, or agent host.

### 3. HBA rule, role, and database

Once TCP reaches PostgreSQL, `pg_hba.conf` decides whether the client address, database, role, and authentication method match an allowed record. PostgreSQL's [HBA documentation](https://www.postgresql.org/docs/current/auth-pg-hba-conf.html) states that records are considered in order and the first matching record is used. If authentication fails under that record, PostgreSQL does not fall through to later records.

Scope a rule to the sandbox database, sandbox role, and actual Docker source CIDR:

```conf
host    <sandbox_database>    <sandbox_role>    <actual-docker-cidr>    scram-sha-256
```

`<actual-docker-cidr>` is deliberately a placeholder. Inspect the Compose network as a candidate instead of copying `172.17.0.0/16` from an old answer:

```bash
docker network inspect <compose-network>
```

Docker networks can use different subnets, and Desktop or NAT paths may present a different source address to PostgreSQL. Confirm the server-observed source address from a controlled connection attempt, the PostgreSQL log, or a `no pg_hba.conf entry` error before finalizing the CIDR. The destination alias `host.docker.internal` does not belong in an HBA rule merely because the client used that name.

Use `scram-sha-256` where the application driver supports it. PostgreSQL describes SCRAM as its strongest currently provided password authentication method, while MD5-encrypted passwords are deprecated. Do not use `trust` for a Docker subnet. A matching `trust` rule lets any reachable client authenticate as a permitted database role without a password.

After editing `pg_hba.conf`, reload with `SELECT pg_reload_conf()` or your normal service command. Validate the parsed rules before widening them:

```sql
SELECT line_number,
       type,
       database,
       user_name,
       address,
       auth_method,
       error
FROM pg_hba_file_rules
ORDER BY line_number;
```

Rows with a non-null `error` identify malformed or ineffective rules. Access to this view may itself require an appropriately privileged operator role.

## Use the PGSandbox connection variant that matches the client

PGSandbox derives two URL variants for a loopback sandbox profile:

- `direct` preserves the original host, such as `localhost` or `127.0.0.1`.
- `localContainer` rewrites only that loopback host to `host.docker.internal`. The scheme, role, password, port, database, and query parameters remain the same.

Use the returned variant as a unit. Do not copy the role and database name into a new URL or replace its port with 5432.

Creation tools return redacted variants for safe summaries. The [`get_connection_string` contract](/docs/mcp-tools/) also returns redacted values by default and includes credential-bearing values only when `includeCredentials: true` is requested. A non-loopback profile does not receive a `localContainer` variant because its original routable hostname is already the correct destination.

Call `run_repo_command` with the direct URL mode for a host-native migration command:

```json
{
  "repoPath": "/absolute/path/to/repo",
  "databaseId": "sandbox-id",
  "command": ["npm", "run", "migrate"],
  "connectionMode": "direct"
}
```

Call `run_repo_command` with the container variant when the database client runs inside the Compose service:

```json
{
  "repoPath": "/absolute/path/to/repo",
  "databaseId": "sandbox-id",
  "command": ["docker", "compose", "run", "--rm", "web", "python", "manage.py", "migrate"],
  "connectionMode": "localContainer",
  "databaseUrlEnvNames": ["SANDBOX_DATABASE_URL"],
  "tailLines": 80,
  "suppressDockerLifecycle": true
}
```

The [repo workflow tool contract](/docs/mcp-tools/) injects `DATABASE_URL`, `PGSANDBOX_DATABASE_URL`, libpq `PG*` variables, and validated URL aliases into the child process. Compose still needs to pass or interpolate the chosen variable into the application service, as the earlier YAML example does. Starting `docker compose` on the host does not make `localhost` correct inside the service container.

PGSandbox rejects `connectionMode: "localContainer"` for a non-loopback profile with `local_container_unavailable`; use `direct` for that profile. It also executes direct argument arrays instead of shell strings, bounds returned command output, and masks exact injected URL and password values from captured output.

Masking is a defense against accidental output, not permission to print credentials. A child process can transform, split, persist, or transmit a secret in ways exact-token masking cannot catch. Keep raw URLs out of chat, logs, PR comments, issues, screenshots, and tracked files.

## Native Linux needs one extra reachability check

On native Linux bridge networking, `extra_hosts: host.docker.internal:host-gateway` maps a name to the host bridge gateway. It does not make a PostgreSQL server bound only to `127.0.0.1` listen on that gateway.

This matters for PGSandbox's current managed-local runtime. It binds its managed PostgreSQL profile to loopback. The `localContainer` variant can therefore produce the right Docker destination name while the listener remains unreachable from a native Linux bridge container.

Do not repair that mismatch by automatically exposing the managed cluster on every interface. Choose an explicit path:

1. Run the repo command on the host with `connectionMode: "direct"` when container execution is not required.
2. Use Docker Desktop and verify the host-forwarding path from the actual service container.
3. Configure a deliberate external PGSandbox profile whose PostgreSQL listener, firewall, HBA rules, and data policy allow the required Docker network.
4. Use Linux host networking only in a controlled development environment after accepting its weaker network isolation and platform differences.

The [PGSandbox architecture](/docs/architecture/) keeps managed-local and external profiles explicit. A routable external profile is not automatically safer; it is a different network contract that needs its own narrow access rules.

## Do not confuse authentication with transport security

SCRAM authenticates the role. It does not encrypt all PostgreSQL traffic. TLS is a separate server and client configuration.

If the profile requires `sslmode=verify-full`, the hostname in the connection URL must match the server certificate. Rewriting a URL from `localhost` to `host.docker.internal` can fail certificate hostname verification when the certificate does not include that DNS name. PostgreSQL's [libpq SSL documentation](https://www.postgresql.org/docs/current/libpq-ssl.html) recommends `verify-full` in security-sensitive environments because it verifies both the certificate chain and the requested hostname.

Do not downgrade TLS verification to make a Docker alias work. Use a certificate with the intended name, use the profile's real routable name, or keep the command on the host when that preserves the verified connection contract.

## Troubleshoot by the error boundary

| Symptom | Boundary most likely failing | Next check |
| --- | --- | --- |
| `host.docker.internal` does not resolve | Container name mapping | Docker Desktop support or Linux `extra_hosts` |
| Connection refused | Listener address or port | `SHOW listen_addresses`, `SHOW port`, host sockets |
| Connection timeout | Route, firewall, VPN, or endpoint security | TCP path from the same service container |
| `no pg_hba.conf entry` | HBA database/user/source/encryption match | Actual source address, rule order, `pg_hba_file_rules` |
| Password authentication failed | Role, password, or auth-method compatibility | Sandbox credential and first matching HBA rule |
| Certificate hostname mismatch | TLS identity verification | Certificate SAN and URL hostname |
| PGSandbox `local_container_unavailable` | Profile is not loopback | Use the profile's `direct` URL |
| Database or role does not exist | URL was rebuilt or points to stale sandbox state | Use the complete current URL returned for the sandbox id |

Test from the same network namespace as the application. A host-native `psql` success does not prove the container path, and a successful `ping` does not prove TCP, HBA, or role authentication. If the image includes PostgreSQL client tools, use:

```bash
pg_isready -h host.docker.internal -p <returned-port>
```

Then run the application's real database check. A generic port probe can clear the network boundary, but only the real driver exercises URL parsing, TLS, authentication, and database selection together. When `psql` is available in the same container, a small identity check is useful:

```bash
psql "$DATABASE_URL" -c 'select current_database(), current_user;'
```

## Capture evidence after the command

An exit code of zero proves that a command completed. It does not prove which schema objects or rows changed.

After a Dockerized migration or seed command, use [`schema_digest` or schema snapshots](/blog/postgres-schema-snapshots-agent-migration-reviews/) to record the database shape. Use [`run_sql` with bounded results](/blog/postgres-run-sql-bounded-results/) for small data assertions. When the command is a migration, `validate_schema_change` can capture the before and after schema contract.

This is the final part of a safe agent workflow. Network reachability gets the command to the sandbox. Scoped role credentials limit its authority. Evidence shows what it actually did. The [database migration testing guide](/blog/database-migration-testing-agent-pr/) shows how to turn that evidence into a reviewable PR note.

## The Runner-Network-Secret-Evidence checklist

Use these four checks before letting a coding agent run a Dockerized database command:

1. **Runner:** Identify where the database client process runs. A host-started Docker CLI can launch a client inside a container.
2. **Network:** Choose `direct` for host-native clients and routable profiles. Choose `localContainer` only for a local container reaching a loopback profile through a verified host path.
3. **Secret:** Inject the task-scoped sandbox URL at runtime. Do not bake it into an image, tracked Compose file, build argument, log, or PR note.
4. **Evidence:** Treat command output as execution evidence, then inspect database effects with a bounded query, schema digest, snapshot, or schema-change validation.

This framework is more durable than memorizing a Docker hostname. It forces the workflow to account for the process boundary, network boundary, credential boundary, and proof boundary independently.

## Frequently asked questions

### What hostname should a Docker container use for PostgreSQL on the host?

Use `host.docker.internal`. Docker Desktop provides it automatically. On native Linux Docker Engine, add `host.docker.internal:host-gateway` through `extra_hosts` or `--add-host`. Then verify that PostgreSQL listens on the resolved host interface and that HBA and firewall rules allow the container's actual source address.

### Why does host.docker.internal resolve but PostgreSQL still refuses the connection?

Name resolution is only the first boundary. PostgreSQL may still listen only on loopback, the host firewall may block the port, or the first matching `pg_hba.conf` record may reject the container's source address, database, role, or encryption mode. Test routing, listener, HBA, and authentication separately.

### Should I put host.docker.internal in pg_hba.conf?

Usually no. The URL hostname is the destination the client dials. HBA address rules match the client source address PostgreSQL sees. Inspect the actual Docker network and use the narrow source CIDR or address required for that sandbox role and database.

### Does PGSandbox expose raw database URLs by default?

No. Creation tools and `get_connection_string` return redacted direct and container variants by default. Raw credential-bearing variants require an explicit `includeCredentials: true` request. Prefer `run_repo_command` runtime injection when the tool can launch the required command directly.

### Does a successful Dockerized migration prove the database change is correct?

No. Exit zero proves command completion. Capture a schema digest or before-and-after diff, run bounded data assertions, and record cleanup. Database evidence is a separate part of the workflow.

<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "HowTo",
      "name": "Connect a Docker Container to Host PostgreSQL Safely",
      "description": "Connect a Dockerized application to PostgreSQL on the host by choosing the right hostname, verifying the listener, scoping HBA access, and protecting sandbox credentials.",
      "datePublished": "2026-07-18",
      "dateModified": "2026-07-18",
      "step": [
        {"@type": "HowToStep", "position": 1, "name": "Identify where the database client runs", "text": "Confirm that the database client runs inside the application container."},
        {"@type": "HowToStep", "position": 2, "name": "Request the container URL", "text": "Call get_connection_string for the sandbox id with includeCredentials enabled."},
        {"@type": "HowToStep", "position": 3, "name": "Configure and verify the host route", "text": "Select connectionStrings.localContainer, add the Linux host-gateway mapping when needed, and verify that PostgreSQL is reachable."},
        {"@type": "HowToStep", "position": 4, "name": "Inject the returned URL", "text": "Pass the complete returned URL into the application service at runtime without reconstructing it or storing it in tracked files."},
        {"@type": "HowToStep", "position": 5, "name": "Run and verify the database command", "text": "Run the application check or migration, then verify database effects with bounded SQL or a schema diff."},
        {"@type": "HowToStep", "position": 6, "name": "Clean up the sandbox", "text": "Delete the sandbox when the proof is complete or let its TTL policy remove it."}
      ]
    },
    {
      "@type": "BreadcrumbList",
      "itemListElement": [
        {"@type": "ListItem", "position": 1, "name": "PGSandbox", "item": "https://pgsandbox-mcp.lvtd.dev/"},
        {"@type": "ListItem", "position": 2, "name": "Blog", "item": "https://pgsandbox-mcp.lvtd.dev/blog/"},
        {"@type": "ListItem", "position": 3, "name": "Connect a Docker Container to Host PostgreSQL Safely", "item": "https://pgsandbox-mcp.lvtd.dev/blog/docker-connect-host-postgres/"}
      ]
    },
    {
      "@type": "FAQPage",
      "mainEntity": [
        {"@type": "Question", "name": "What hostname should a Docker container use for PostgreSQL on the host?", "acceptedAnswer": {"@type": "Answer", "text": "Use host.docker.internal. Docker Desktop provides it automatically. On native Linux Docker Engine, map the same name to host-gateway, then verify the PostgreSQL listener, firewall, and HBA path."}},
        {"@type": "Question", "name": "Why does host.docker.internal resolve but PostgreSQL still refuse the connection?", "acceptedAnswer": {"@type": "Answer", "text": "Name resolution proves only the destination address. PostgreSQL may listen only on loopback, a firewall may block the route, or the first matching HBA rule may reject the source address, database, role, or encryption mode."}},
        {"@type": "Question", "name": "Should I put host.docker.internal in pg_hba.conf?", "acceptedAnswer": {"@type": "Answer", "text": "Usually no. The URL hostname is a destination alias, while HBA address rules match the client source address PostgreSQL sees. Use the narrow observed Docker source address or CIDR."}},
        {"@type": "Question", "name": "Does PGSandbox expose raw database URLs by default?", "acceptedAnswer": {"@type": "Answer", "text": "No. PGSandbox returns redacted direct and container variants by default. Raw credential-bearing variants require an explicit includeCredentials request."}},
        {"@type": "Question", "name": "Does a successful Dockerized migration prove the database change is correct?", "acceptedAnswer": {"@type": "Answer", "text": "No. Exit zero proves command completion. Use a schema digest or diff and bounded data assertions to verify database effects, then record cleanup."}}
      ]
    }
  ]
}
</script>
