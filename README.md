# Rift Showdown

A browser card table for **Riftbound**, the League of Legends TCG. Rust server, WebAssembly client.

Unofficial fan project. Riot Games does not endorse or sponsor it. Card data comes from
[Riftcodex](https://riftcodex.com/) and Riot's [card gallery](https://riftbound.leagueoflegends.com/en-us/card-gallery/);
card images and names belong to Riot Games.

## Features

- 1v1 (Bo1/Bo3), 2v2 and 3–4 player free-for-all
- Private rooms, public room list, matchmaking, spectators and chat. No accounts: players type a name when they join a lobby
- Every page has its own URL (`/decks`, `/decks/<id>`, `/room/<code>`, ...): reload, back and forward keep your place, and a room link is the invite
- Deck builder, Piltover Archive / RiftAtlas deck code import and export, sealed pools
- Riot's preconstructed decks as starters
- Solo play and goldfish, offline as an installable app
- 3D table with drag and drop, auto-pay, showdowns, scoring and unlimited rewind

## Run with Docker

```sh
docker run -p 8080:8080 -v rift-data:/data ghcr.io/joeyshi12/rift-showdown:latest
```

The image serves on port 8080 and keeps its card image cache in `/data`. It runs as
user 65532, so a bind-mounted `/data` must be writable by that user (or set `--user`).
See [`deploy/`](deploy/) for a Compose example.

Tags: `X.Y.Z`, `X.Y` and `latest` for releases, `edge` for `main`, plus 12-character commit ids.

## Configuration

| Variable | Default | |
| --- | --- | --- |
| `RIFT_BIND` | `127.0.0.1:8080` (`0.0.0.0:8080` in the image) | Listen address |
| `RIFT_DATA` | `data` | Folder with `cards.json` |
| `RIFT_DIST` | `dist` | Built client |
| `RIFT_IMG_CACHE` | `$RIFT_DATA/imgcache` | Card image cache |

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
