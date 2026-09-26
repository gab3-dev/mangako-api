#!/usr/bin/env bash
set -euo pipefail

: "${API_BASE_URL:=http://localhost:3000}"
: "${API_READ_TOKEN:?set API_READ_TOKEN}"
: "${API_WRITE_TOKEN:?set API_WRITE_TOKEN}"

slug="e2e-crud-${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-0}-${RANDOM}"
write_auth="Authorization: Bearer ${API_WRITE_TOKEN}"
read_auth="Authorization: Bearer ${API_READ_TOKEN}"

created=$(curl --fail --silent --show-error \
  --header "$write_auth" \
  --header 'Content-Type: application/json' \
  --data "{\"slug\":\"${slug}\",\"primaryTitle\":\"E2E CRUD Manga\",\"originalLanguage\":\"en\"}" \
  "${API_BASE_URL}/mangas")
manga_id=$(jq --exit-status --raw-output '.id' <<<"$created")

curl --fail --silent --show-error \
  --header "$read_auth" \
  "${API_BASE_URL}/mangas/${manga_id}" \
  | jq --exit-status '.primaryTitle == "E2E CRUD Manga"' >/dev/null

updated=$(curl --fail --silent --show-error \
  --request PATCH \
  --header "$write_auth" \
  --header 'Content-Type: application/json' \
  --data '{"primaryTitle":"Updated E2E CRUD Manga","contentRating":"safe"}' \
  "${API_BASE_URL}/mangas/${manga_id}")
jq --exit-status '.primaryTitle == "Updated E2E CRUD Manga" and .contentRating == "safe"' <<<"$updated" >/dev/null

volume=$(curl --fail --silent --show-error \
  --header "$write_auth" \
  --header 'Content-Type: application/json' \
  --data '{"fileName":"e2e-volume-1.jpg","sourceUrl":"https://example.com/e2e-volume-1.jpg","volume":"1","locale":"en"}' \
  "${API_BASE_URL}/mangas/${manga_id}/volumes")
volume_id=$(jq --exit-status --raw-output '.id' <<<"$volume")

curl --fail --silent --show-error \
  --header "$read_auth" \
  "${API_BASE_URL}/mangas/${manga_id}/volumes?locale=en" \
  | jq --exit-status --arg volume_id "$volume_id" 'length == 1 and .[0].id == $volume_id' >/dev/null

updated_volume=$(curl --fail --silent --show-error \
  --request PATCH \
  --header "$write_auth" \
  --header 'Content-Type: application/json' \
  --data '{"sourceUrl":"https://example.com/e2e-volume-1-updated.jpg","volume":"1.5"}' \
  "${API_BASE_URL}/mangas/${manga_id}/volumes/${volume_id}")
jq --exit-status '.sourceUrl == "https://example.com/e2e-volume-1-updated.jpg" and .volumeKey == "1.5" and .isSpecialEdition == true' <<<"$updated_volume" >/dev/null

test "$(curl --silent --show-error --output /dev/null --write-out '%{http_code}' \
  --request DELETE --header "$write_auth" \
  "${API_BASE_URL}/mangas/${manga_id}/volumes/${volume_id}")" = "204"

curl --fail --silent --show-error \
  --header "$read_auth" \
  "${API_BASE_URL}/mangas/${manga_id}/volumes?locale=en" \
  | jq --exit-status 'length == 0' >/dev/null

test "$(curl --silent --show-error --output /dev/null --write-out '%{http_code}' \
  --request DELETE --header "$write_auth" \
  "${API_BASE_URL}/mangas/${manga_id}")" = "204"

test "$(curl --silent --show-error --output /dev/null --write-out '%{http_code}' \
  --header "$read_auth" \
  "${API_BASE_URL}/mangas/${slug}")" = "404"
