# Security policy

## Report a vulnerability

Do not open a public issue for a security problem. Use the private report
form of GitHub: on the repository page, open "Security", then "Report a
vulnerability". Say what is affected, how to reproduce it and what an
attacker gains.

We answer within a week. We fix the problem in `main` and credit you in the
changelog, unless you prefer not to be named.

## Supported versions

FlowScope is in development. Only the `main` branch gets fixes.

## What to know before you expose FlowScope

The Docker Compose setup is made to get started. Before you put it on a
network you do not control:

- The console has no login of its own. It trusts the `Remote-User` headers
  set by a proxy in front of it. Put an authenticating proxy in front of
  port 8081. `docker/docker-compose-local.yml` has an example.
- Port 8080 gives access to ClickHouse, the configuration and the other
  internal services without a login. The setup binds it to the loopback
  address. Keep it private.
- The inlet accepts flows over UDP from any address. The BMP port accepts
  any peer. Filter both with a firewall so only your routers reach them.
  This matters: the outlet keeps state for each exporter and polls it
  with SNMP, sending the configured community. Someone able to send flows
  can use up memory, slow the outlet down and learn the community.
- Grafana starts with its default administrator account. Change the
  password at first login.
- Put your tokens and license keys in `.env`. Git ignores it.
