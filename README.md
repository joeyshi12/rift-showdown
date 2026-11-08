# Rift Online

A browser card table for **Riftbound**, the League of Legends TCG. Rust server, WebAssembly client.

Unofficial fan project. Riot Games does not endorse or sponsor it. Card data comes from
[Riftcodex](https://riftcodex.com/) and Riot's [card gallery](https://riftbound.leagueoflegends.com/en-us/card-gallery/);
card images and names belong to Riot Games.

## Features

- 1v1 (Bo1/Bo3), 2v2 and 3–4 player free-for-all
- Private rooms, public room list, matchmaking, spectators and chat
- Deck builder, Piltover Archive / RiftAtlas deck code import and export, sealed pools
- Riot's preconstructed decks as starters
- Solo play and goldfish, offline as an installable app
- 3D table with drag and drop, auto-pay, showdowns, scoring and unlimited rewind

## Run with Docker

```sh
docker run -p 8080:8080 -v rift-data:/data ghcr.io/joeyshi12/rift-online:latest
```

The image serves on port 8080 and stores accounts and the card image cache in `/data`. It runs as
user 65532, so a bind-mounted `/data` must be writable by that user (or set `--user`).
See [`deploy/`](deploy/) for a Compose example.

Tags: `X.Y.Z`, `X.Y` and `latest` for releases, `edge` for `main`, plus 12-character commit ids.

## Configuration

| Variable | Default | |
| --- | --- | --- |
| `RIFT_BIND` | `127.0.0.1:8080` (`0.0.0.0:8080` in the image) | Listen address |
| `RIFT_PUBLIC_URL` | unset | Public base URL; needed for OAuth and reset emails, enables secure cookies when https |
| `RIFT_DATA` | `data` | Folder with `cards.json` |
| `RIFT_DIST` | `dist` | Built client |
| `RIFT_ACCOUNTS_DB` | `$RIFT_DATA/accounts.db` | SQLite account database |
| `RIFT_IMG_CACHE` | `$RIFT_DATA/imgcache` | Card image cache |
| `RIFT_SECURE_COOKIES` | from `RIFT_PUBLIC_URL` | Force `Secure` cookies on or off |
| `RIFT_SMTP_URL`, `RIFT_MAIL_FROM` | unset | Password reset email, e.g. `smtps://user:pass@host`. Unset: links go to the log |
| `RIFT_{GOOGLE,DISCORD,TWITCH}_CLIENT_ID` / `_SECRET` | unset | OAuth sign-in. Redirect URI: `<RIFT_PUBLIC_URL>/auth/<provider>/callback` |

## Build from source

Needs Rust with the `wasm32-unknown-unknown` target and `wasm-bindgen-cli` 0.2.130.

```sh
./build.sh                      # dist/ and target/release/rift-server
./target/release/rift-server    # http://127.0.0.1:8080
```

## Tests

```sh
cargo test -p rift-shared -p rift-server -p rift-client
./dev-server.sh start           # server for the browser tests on :8787
cd e2e && npm ci && npx playwright install chromium && node run.mjs
./dev-server.sh stop
```

## Card data

```sh
python3 data/fetch_cards.py     # refresh data/cards.json
python3 data/build_precons.py   # rebuild data/precons.json from data/precons/*.txt
```

## License

Code: MIT ([`LICENSE`](LICENSE)). Fonts: SIL Open Font License ([`web/fonts/OFL-Barlow.txt`](web/fonts/OFL-Barlow.txt)).
Riot Games' card names, text and art are not covered.
