# Test server scripts

Build, run and seed the Mattermost server on the Ubuntu VM that MatterLite tests against
(`https://ubuntu` and `http://ubuntu:8065`). Accounts, data, the expected starting state, live-event recipes and
plugin deploys are in [`docs/MatterLite-TestServer.md`](../../docs/MatterLite-TestServer.md).

| Script | Does |
|---|---|
| `setup.sh` | Installs Go, PostgreSQL and a pinned Node; fetches the Mattermost commit; builds the server, `mmctl` and the web UI; installs and starts the `mattermost-dev` service; patches the config; sets up nginx with HTTPS/WSS on :443 and a test CA (`~/mm-test-ca`). Idempotent; `REBUILD=1` forces the builds |
| `seed.sh` | Creates the test users, teams, channels and posts; writes `~/mm-test-server/credentials.env` on the VM. A no-op if already seeded |
| `reset.sh` | Wipes the database, uploaded files and plugins (config kept). Run `seed.sh` after it |

All run on the VM; pipe them over SSH from the repository root:

```sh
ssh andrew@ubuntu 'bash -s' < script/test-server/setup.sh
ssh andrew@ubuntu 'bash -s' < script/test-server/seed.sh
```
