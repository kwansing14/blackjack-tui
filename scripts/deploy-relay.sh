#!/usr/bin/env bash
# Deploy the relay: apply pending D1 migrations, publish the Worker, then check the live
# relay actually answers. Safe to re-run: migrations that already ran are skipped and
# redeploying unchanged code is a no-op, so a run that died halfway (network, auth) is
# finished by running the same command again.
#
# The schema goes first and the Worker second. Between the two steps the old code is
# running against the new schema, which is fine; the other order leaves new code running
# against a database that has not caught up, and that gap is somebody's game night.
#
# Usage:
#   npm run deploy:relay                  migrate, deploy, verify
#   ./scripts/deploy-relay.sh --dry-run   check everything and change nothing
#
# Requires: worker/node_modules (npm install), npx wrangler login.
set -euo pipefail

cd "$(dirname "$0")/../worker"

DB=blackjack-scores
DRY_RUN=false
if [[ ${1:-} == --dry-run ]]; then
  DRY_RUN=true
elif [[ -n ${1:-} ]]; then
  echo "usage: $(basename "$0") [--dry-run]" >&2
  exit 1
fi

if [[ ! -d node_modules ]]; then
  echo "error: worker/node_modules is missing; run: cd worker && npm install" >&2
  exit 1
fi

echo "==> Checking login"
if ! npx wrangler whoami >/dev/null 2>&1; then
  echo "error: wrangler is not logged in; run: cd worker && npx wrangler login" >&2
  exit 1
fi

# A dry run bundles the Worker and resolves every binding without uploading anything, so a
# config mistake (an unbound database, a typo in wrangler.jsonc) is caught before we migrate.
echo "==> Checking the Worker builds and its bindings resolve"
bindings=$(npx wrangler deploy --dry-run 2>&1) || { echo "$bindings" >&2; exit 1; }
if ! grep -q "env.DB" <<<"$bindings"; then
  echo "$bindings" >&2
  echo "error: env.DB is not bound; the relay would run games but keep no scores" >&2
  echo "       add the database to d1_databases in worker/wrangler.jsonc" >&2
  exit 1
fi
grep -E "^env\." <<<"$bindings" | sed 's/^/    /'

echo "==> Pending migrations"
pending=$(npx wrangler d1 migrations list "$DB" --remote 2>&1) || { echo "$pending" >&2; exit 1; }
if grep -q "No migrations to apply" <<<"$pending"; then
  echo "    none; the database is up to date"
  pending=""
else
  grep -oE "[0-9]{4}_[A-Za-z0-9_]+\.sql" <<<"$pending" | sort -u | sed 's/^/    /'
fi

if [[ $DRY_RUN == true ]]; then
  echo "==> Dry run, stopping here. Nothing was migrated or deployed."
  exit 0
fi

if [[ -n "$pending" ]]; then
  echo "==> Applying migrations to $DB"
  npx wrangler d1 migrations apply "$DB" --remote
else
  echo "==> Skipping migrations"
fi

# Trust the live relay, not the exit status. Behind a TLS-inspecting proxy the reply can be
# dropped after Cloudflare has already accepted the upload, so a failed deploy that actually
# landed is normal here; the check below is what decides whether this run worked.
echo "==> Deploying"
deployed=$(npx wrangler deploy 2>&1) || true
sed 's/^/    /' <<<"$deployed" | tail -6
url=$(grep -oE "https://[A-Za-z0-9.-]+\.workers\.dev" <<<"$deployed" | head -1)
if [[ -z "$url" ]]; then
  url=$(grep -oE "https://[A-Za-z0-9.-]+\.workers\.dev" ../src/net.rs | head -1)
  echo "    (no URL in the deploy output; checking $url)"
fi

echo "==> Checking $url"
body=$(curl -s --max-time 20 -w '\n%{http_code}' -X POST "$url/scores" -d '{"id":""}') || {
  echo "error: could not reach $url; if the deploy itself failed, run this again" >&2
  exit 1
}
code=$(tail -1 <<<"$body")
body=$(sed '$d' <<<"$body")

case "$code" in
  200)
    if grep -q '"week"' <<<"$body"; then
      echo "    ok: serving the weekly scoreboard, $(sed -n 's/.*"week":"\([^"]*\)".*/\1/p' <<<"$body")"
    else
      echo "error: $url answered without a week; the old Worker is still live" >&2
      echo "       the deploy did not land, run this again" >&2
      exit 1
    fi
    ;;
  501)
    echo "    warning: deployed, but the relay keeps no scores (no database bound)" >&2
    ;;
  *)
    echo "error: $url answered $code: $body" >&2
    exit 1
    ;;
esac

echo "==> Done. Players need no upgrade; an old client just shows the table with no week heading."
